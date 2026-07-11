//! Bucket reconciliation.

use std::sync::Arc;

use kube::runtime::controller::Action;
use kube::runtime::finalizer::{finalizer, Event};
use kube::{Api, ResourceExt};

use super::{namespace_of, patch_status, Context, FINALIZER, REQUEUE_OK};
use crate::connection::provider_for;
use crate::crd::{Bucket, BucketSpec, DeletionPolicy, ResourceStatus};
use crate::error::{Error, Result};
use crate::provider::RustFs;

/// Make the RustFS bucket match the spec. Pure logic, unit-testable.
pub async fn ensure_bucket(fs: &dyn RustFs, name: &str, spec: &BucketSpec) -> Result<()> {
    if !fs.bucket_exists(name).await? {
        fs.create_bucket(name).await?;
    }
    if let Some(versioning) = spec.versioning {
        let current = fs.get_versioning(name).await?.unwrap_or(false);
        if current != versioning {
            fs.set_versioning(name, versioning).await?;
        }
    }
    if let Some(quota) = spec.quota_bytes
        && fs.get_bucket_quota(name).await? != Some(quota)
    {
        fs.set_bucket_quota(name, quota).await?;
    }
    Ok(())
}

/// Remove the bucket if the deletion policy asks for it.
pub async fn cleanup_bucket(fs: &dyn RustFs, name: &str, spec: &BucketSpec) -> Result<()> {
    match spec.deletion_policy {
        DeletionPolicy::Delete => fs.delete_bucket(name).await,
        DeletionPolicy::Retain => Ok(()),
    }
}

pub async fn reconcile(obj: Arc<Bucket>, ctx: Arc<Context>) -> Result<Action> {
    let ns = namespace_of(obj.as_ref())?;
    let api: Api<Bucket> = Api::namespaced(ctx.client.clone(), &ns);
    finalizer(&api, FINALIZER, obj, |event| async {
        match event {
            Event::Apply(obj) => apply(obj, &ctx).await,
            Event::Cleanup(obj) => cleanup(obj, &ctx).await,
        }
    })
    .await
    .map_err(|e| Error::Finalizer(e.to_string()))
}

async fn apply(obj: Arc<Bucket>, ctx: &Context) -> Result<Action> {
    let ns = namespace_of(obj.as_ref())?;
    let api: Api<Bucket> = Api::namespaced(ctx.client.clone(), &ns);

    let result = async {
        let fs = provider_for(&ctx.client, &ns, &obj.spec.connection).await?;
        ensure_bucket(&fs, obj.bucket_name(), &obj.spec).await
    }
    .await;

    let status = match &result {
        Ok(()) => ResourceStatus::ready(obj.metadata.generation),
        Err(e) => ResourceStatus::error(obj.metadata.generation, e),
    };
    patch_status(&api, &obj.name_any(), &status).await;
    result.map(|()| Action::requeue(REQUEUE_OK))
}

async fn cleanup(obj: Arc<Bucket>, ctx: &Context) -> Result<Action> {
    if obj.spec.deletion_policy == DeletionPolicy::Retain {
        return Ok(Action::await_change());
    }
    let ns = namespace_of(obj.as_ref())?;
    let fs = provider_for(&ctx.client, &ns, &obj.spec.connection).await?;
    cleanup_bucket(&fs, obj.bucket_name(), &obj.spec).await?;
    Ok(Action::await_change())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::crd::ConnectionRef;
    use crate::provider::MockRustFs;

    fn spec(versioning: Option<bool>, quota_bytes: Option<u64>) -> BucketSpec {
        BucketSpec {
            connection: ConnectionRef {
                secret_ref: "conn".into(),
            },
            bucket_name: None,
            versioning,
            quota_bytes,
            deletion_policy: DeletionPolicy::default(),
        }
    }

    #[tokio::test]
    async fn creates_missing_bucket() {
        let mut fs = MockRustFs::new();
        fs.expect_bucket_exists()
            .withf(|b| b == "demo")
            .return_once(|_| Ok(false));
        fs.expect_create_bucket()
            .withf(|b| b == "demo")
            .return_once(|_| Ok(()));

        ensure_bucket(&fs, "demo", &spec(None, None)).await.unwrap();
    }

    #[tokio::test]
    async fn existing_bucket_untouched_when_spec_matches() {
        let mut fs = MockRustFs::new();
        fs.expect_bucket_exists().return_once(|_| Ok(true));
        fs.expect_get_versioning().return_once(|_| Ok(Some(true)));
        fs.expect_get_bucket_quota()
            .return_once(|_| Ok(Some(1024)));
        // no create/set expectations: any call would panic

        ensure_bucket(&fs, "demo", &spec(Some(true), Some(1024)))
            .await
            .unwrap();
    }

    #[tokio::test]
    async fn corrects_versioning_and_quota_drift() {
        let mut fs = MockRustFs::new();
        fs.expect_bucket_exists().return_once(|_| Ok(true));
        fs.expect_get_versioning().return_once(|_| Ok(None));
        fs.expect_set_versioning()
            .withf(|b, v| b == "demo" && *v)
            .return_once(|_, _| Ok(()));
        fs.expect_get_bucket_quota().return_once(|_| Ok(Some(5)));
        fs.expect_set_bucket_quota()
            .withf(|b, q| b == "demo" && *q == 2048)
            .return_once(|_, _| Ok(()));

        ensure_bucket(&fs, "demo", &spec(Some(true), Some(2048)))
            .await
            .unwrap();
    }

    #[tokio::test]
    async fn retain_policy_skips_remote_delete() {
        let fs = MockRustFs::new(); // any call panics
        let mut s = spec(None, None);
        s.deletion_policy = DeletionPolicy::Retain;
        cleanup_bucket(&fs, "demo", &s).await.unwrap();
    }

    #[tokio::test]
    async fn delete_policy_removes_bucket() {
        let mut fs = MockRustFs::new();
        fs.expect_delete_bucket()
            .withf(|b| b == "demo")
            .return_once(|_| Ok(()));
        cleanup_bucket(&fs, "demo", &spec(None, None)).await.unwrap();
    }
}
