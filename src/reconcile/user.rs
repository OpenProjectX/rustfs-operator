//! IAM user reconciliation.

use std::collections::BTreeSet;
use std::sync::Arc;

use kube::runtime::controller::Action;
use kube::runtime::finalizer::{finalizer, Event};
use kube::{Api, ResourceExt};
use rc_core::admin::UserStatus;

use super::{namespace_of, patch_status, Context, FINALIZER, REQUEUE_OK};
use crate::connection::{provider_for, secret_key_value};
use crate::crd::{DeletionPolicy, ResourceStatus, User, UserSpec};
use crate::error::{Error, Result};
use crate::provider::RustFs;

/// Make the RustFS user match the spec. `secret_key` is only used when the
/// user does not exist yet; RustFS does not expose existing secret keys.
pub async fn ensure_user(
    fs: &dyn RustFs,
    access_key: &str,
    secret_key: &str,
    spec: &UserSpec,
) -> Result<()> {
    let existing = fs.get_user(access_key).await?;
    let (enabled_now, attached) = match &existing {
        Some(u) => (u.status == UserStatus::Enabled, u.policies()),
        None => {
            fs.create_user(access_key, secret_key).await?;
            (true, Vec::new())
        }
    };

    if enabled_now != spec.enabled {
        fs.set_user_status(access_key, spec.enabled).await?;
    }

    let desired: BTreeSet<&str> = spec.policies.iter().map(String::as_str).collect();
    let attached: BTreeSet<&str> = attached.iter().map(String::as_str).collect();
    if desired != attached {
        // RustFS's set-policy endpoint replaces the whole attachment set.
        let policies: Vec<String> = desired.iter().map(|s| s.to_string()).collect();
        fs.set_user_policies(access_key, &policies).await?;
    }
    Ok(())
}

pub async fn cleanup_user(fs: &dyn RustFs, access_key: &str, spec: &UserSpec) -> Result<()> {
    match spec.deletion_policy {
        DeletionPolicy::Delete => fs.delete_user(access_key).await,
        DeletionPolicy::Retain => Ok(()),
    }
}

pub async fn reconcile(obj: Arc<User>, ctx: Arc<Context>) -> Result<Action> {
    let ns = namespace_of(obj.as_ref())?;
    let api: Api<User> = Api::namespaced(ctx.client.clone(), &ns);
    finalizer(&api, FINALIZER, obj, |event| async {
        match event {
            Event::Apply(obj) => apply(obj, &ctx).await,
            Event::Cleanup(obj) => cleanup(obj, &ctx).await,
        }
    })
    .await
    .map_err(|e| Error::Finalizer(e.to_string()))
}

async fn apply(obj: Arc<User>, ctx: &Context) -> Result<Action> {
    let ns = namespace_of(obj.as_ref())?;
    let api: Api<User> = Api::namespaced(ctx.client.clone(), &ns);

    let result = async {
        let secret_key = secret_key_value(&ctx.client, &ns, &obj.spec.secret_key_ref).await?;
        let fs = provider_for(&ctx.client, &ns, &obj.spec.connection).await?;
        ensure_user(&fs, obj.access_key(), &secret_key, &obj.spec).await
    }
    .await;

    let status = match &result {
        Ok(()) => ResourceStatus::ready(obj.metadata.generation),
        Err(e) => ResourceStatus::error(obj.metadata.generation, e),
    };
    patch_status(&api, &obj.name_any(), &status).await;
    result.map(|()| Action::requeue(REQUEUE_OK))
}

async fn cleanup(obj: Arc<User>, ctx: &Context) -> Result<Action> {
    if obj.spec.deletion_policy == DeletionPolicy::Retain {
        return Ok(Action::await_change());
    }
    let ns = namespace_of(obj.as_ref())?;
    let fs = provider_for(&ctx.client, &ns, &obj.spec.connection).await?;
    cleanup_user(&fs, obj.access_key(), &obj.spec).await?;
    Ok(Action::await_change())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::crd::{ConnectionRef, SecretKeyRef};
    use crate::provider::MockRustFs;
    use rc_core::admin::User as RfUser;

    fn spec(policies: &[&str], enabled: bool) -> UserSpec {
        UserSpec {
            connection: ConnectionRef {
                secret_ref: "conn".into(),
            },
            access_key: None,
            secret_key_ref: SecretKeyRef {
                name: "creds".into(),
                key: "secretKey".into(),
            },
            policies: policies.iter().map(|s| s.to_string()).collect(),
            enabled,
            deletion_policy: DeletionPolicy::default(),
        }
    }

    fn remote_user(policies: Option<&str>, status: UserStatus) -> RfUser {
        let mut u = RfUser::new("alice");
        u.status = status;
        u.policy_name = policies.map(str::to_string);
        u
    }

    #[tokio::test]
    async fn creates_missing_user_and_attaches_policies() {
        let mut fs = MockRustFs::new();
        fs.expect_get_user().return_once(|_| Ok(None));
        fs.expect_create_user()
            .withf(|ak, sk| ak == "alice" && sk == "hunter2hunter2")
            .return_once(|_, _| Ok(()));
        fs.expect_set_user_policies()
            .withf(|u, p| u == "alice" && p == ["readwrite".to_string()])
            .return_once(|_, _| Ok(()));

        ensure_user(&fs, "alice", "hunter2hunter2", &spec(&["readwrite"], true))
            .await
            .unwrap();
    }

    #[tokio::test]
    async fn replaces_policy_set_on_drift() {
        let mut fs = MockRustFs::new();
        fs.expect_get_user().return_once(|_| {
            Ok(Some(remote_user(
                Some("stale,readwrite"),
                UserStatus::Enabled,
            )))
        });
        // one replace-all call with the desired set (sorted)
        fs.expect_set_user_policies()
            .withf(|u, p| u == "alice" && p == ["fresh".to_string(), "readwrite".to_string()])
            .return_once(|_, _| Ok(()));

        ensure_user(&fs, "alice", "unused", &spec(&["readwrite", "fresh"], true))
            .await
            .unwrap();
    }

    #[tokio::test]
    async fn disables_user_when_spec_says_so() {
        let mut fs = MockRustFs::new();
        fs.expect_get_user()
            .return_once(|_| Ok(Some(remote_user(None, UserStatus::Enabled))));
        fs.expect_set_user_status()
            .withf(|u, enabled| u == "alice" && !enabled)
            .return_once(|_, _| Ok(()));

        ensure_user(&fs, "alice", "unused", &spec(&[], false))
            .await
            .unwrap();
    }

    #[tokio::test]
    async fn in_sync_user_is_untouched() {
        let mut fs = MockRustFs::new();
        fs.expect_get_user()
            .return_once(|_| Ok(Some(remote_user(Some("readwrite"), UserStatus::Enabled))));

        ensure_user(&fs, "alice", "unused", &spec(&["readwrite"], true))
            .await
            .unwrap();
    }
}
