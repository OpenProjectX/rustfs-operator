//! IAM policy reconciliation.

use std::sync::Arc;

use kube::runtime::controller::Action;
use kube::runtime::finalizer::{finalizer, Event};
use kube::{Api, ResourceExt};

use super::{namespace_of, patch_status, Context, FINALIZER, REQUEUE_OK};
use crate::connection::provider_for;
use crate::crd::{DeletionPolicy, Policy, PolicySpec, ResourceStatus};
use crate::error::{Error, Result};
use crate::provider::RustFs;

/// Compare a policy document returned by RustFS with the desired one.
///
/// The server wraps the stored document in metadata (`policy_name`,
/// `create_date`, `policy: {...}`) and normalizes it: empty `Sid`, `ID` and
/// `Condition` fields are added and string arrays may be reordered. Compare
/// semantically instead of byte-for-byte, otherwise every reconcile would
/// rewrite the policy.
pub fn documents_equivalent(server: &serde_json::Value, desired: &serde_json::Value) -> bool {
    let unwrapped = match server.get("policy") {
        Some(inner @ serde_json::Value::Object(_)) => inner,
        _ => server,
    };
    canonicalize(unwrapped) == canonicalize(desired)
}

fn canonicalize(value: &serde_json::Value) -> serde_json::Value {
    use serde_json::Value;
    match value {
        Value::Object(map) => {
            let mut out = serde_json::Map::new();
            for (key, val) in map {
                let canon = canonicalize(val);
                let empty = canon.is_null()
                    || matches!(&canon, Value::String(s) if s.is_empty())
                    || matches!(&canon, Value::Object(m) if m.is_empty())
                    || matches!(&canon, Value::Array(a) if a.is_empty());
                if !empty {
                    out.insert(key.clone(), canon);
                }
            }
            Value::Object(out)
        }
        Value::Array(arr) => {
            let mut items: Vec<Value> = arr.iter().map(canonicalize).collect();
            items.sort_by_key(|item| item.to_string());
            Value::Array(items)
        }
        other => other.clone(),
    }
}

/// Make the RustFS policy match the spec document.
pub async fn ensure_policy(fs: &dyn RustFs, name: &str, spec: &PolicySpec) -> Result<()> {
    let in_sync = match fs.get_policy(name).await? {
        Some(existing) => existing
            .parse_document()
            .map(|doc| documents_equivalent(&doc, &spec.document))
            .unwrap_or(false),
        None => false,
    };
    if !in_sync {
        let document = serde_json::to_string(&spec.document)
            .map_err(|e| Error::Spec(format!("policy document is not valid JSON: {e}")))?;
        fs.put_policy(name, &document).await?;
    }
    Ok(())
}

pub async fn cleanup_policy(fs: &dyn RustFs, name: &str, spec: &PolicySpec) -> Result<()> {
    match spec.deletion_policy {
        DeletionPolicy::Delete => fs.delete_policy(name).await,
        DeletionPolicy::Retain => Ok(()),
    }
}

pub async fn reconcile(obj: Arc<Policy>, ctx: Arc<Context>) -> Result<Action> {
    let ns = namespace_of(obj.as_ref())?;
    let api: Api<Policy> = Api::namespaced(ctx.client.clone(), &ns);
    finalizer(&api, FINALIZER, obj, |event| async {
        match event {
            Event::Apply(obj) => apply(obj, &ctx).await,
            Event::Cleanup(obj) => cleanup(obj, &ctx).await,
        }
    })
    .await
    .map_err(|e| Error::Finalizer(e.to_string()))
}

async fn apply(obj: Arc<Policy>, ctx: &Context) -> Result<Action> {
    let ns = namespace_of(obj.as_ref())?;
    let api: Api<Policy> = Api::namespaced(ctx.client.clone(), &ns);

    let result = async {
        let fs = provider_for(&ctx.client, &ns, &obj.spec.connection).await?;
        ensure_policy(&fs, obj.policy_name(), &obj.spec).await
    }
    .await;

    let status = match &result {
        Ok(()) => ResourceStatus::ready(obj.metadata.generation),
        Err(e) => ResourceStatus::error(obj.metadata.generation, e),
    };
    patch_status(&api, &obj.name_any(), &status).await;
    result.map(|()| Action::requeue(REQUEUE_OK))
}

async fn cleanup(obj: Arc<Policy>, ctx: &Context) -> Result<Action> {
    if obj.spec.deletion_policy == DeletionPolicy::Retain {
        return Ok(Action::await_change());
    }
    let ns = namespace_of(obj.as_ref())?;
    let fs = provider_for(&ctx.client, &ns, &obj.spec.connection).await?;
    cleanup_policy(&fs, obj.policy_name(), &obj.spec).await?;
    Ok(Action::await_change())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::crd::ConnectionRef;
    use crate::provider::MockRustFs;
    use rc_core::admin::Policy as RfPolicy;
    use serde_json::json;

    fn spec(document: serde_json::Value) -> PolicySpec {
        PolicySpec {
            connection: ConnectionRef {
                secret_ref: "conn".into(),
            },
            policy_name: None,
            document,
            deletion_policy: DeletionPolicy::default(),
        }
    }

    fn doc() -> serde_json::Value {
        json!({
            "Version": "2012-10-17",
            "Statement": [{"Effect": "Allow", "Action": ["s3:GetObject"], "Resource": ["arn:aws:s3:::demo/*"]}]
        })
    }

    #[tokio::test]
    async fn creates_missing_policy() {
        let mut fs = MockRustFs::new();
        fs.expect_get_policy().return_once(|_| Ok(None));
        fs.expect_put_policy()
            .withf(|name, body| {
                name == "read-demo"
                    && serde_json::from_str::<serde_json::Value>(body).unwrap() == doc()
            })
            .return_once(|_, _| Ok(()));

        ensure_policy(&fs, "read-demo", &spec(doc())).await.unwrap();
    }

    #[tokio::test]
    async fn matching_document_is_untouched() {
        let mut fs = MockRustFs::new();
        fs.expect_get_policy().return_once(|_| {
            Ok(Some(RfPolicy::new(
                "read-demo",
                serde_json::to_string(&doc()).unwrap(),
            )))
        });

        ensure_policy(&fs, "read-demo", &spec(doc())).await.unwrap();
    }

    #[tokio::test]
    async fn server_normalized_document_counts_as_in_sync() {
        // Shape actually returned by rustfs/rustfs:1.0.0-beta.8: wrapped in
        // metadata, empty Sid/ID/Condition added, Action array reordered.
        let server = json!({
            "policy_name": "read-demo",
            "create_date": "2026-07-11 13:34:10 +00:00:00",
            "update_date": "2026-07-11 13:34:10 +00:00:00",
            "policy": {
                "ID": "",
                "Version": "2012-10-17",
                "Statement": [{
                    "Sid": "",
                    "Effect": "Allow",
                    "Condition": {},
                    "Action": ["s3:PutObject", "s3:GetObject"],
                    "Resource": ["arn:aws:s3:::demo/*"]
                }]
            }
        });
        let desired = json!({
            "Version": "2012-10-17",
            "Statement": [{
                "Effect": "Allow",
                "Action": ["s3:GetObject", "s3:PutObject"],
                "Resource": ["arn:aws:s3:::demo/*"]
            }]
        });
        assert!(documents_equivalent(&server, &desired));

        let mut fs = MockRustFs::new();
        fs.expect_get_policy().return_once(move |_| {
            Ok(Some(RfPolicy::new(
                "read-demo",
                serde_json::to_string(&server).unwrap(),
            )))
        });
        // no put_policy expectation: rewriting would panic
        ensure_policy(&fs, "read-demo", &spec(desired)).await.unwrap();
    }

    #[tokio::test]
    async fn changed_document_is_rewritten() {
        let mut fs = MockRustFs::new();
        fs.expect_get_policy().return_once(|_| {
            Ok(Some(RfPolicy::new("read-demo", r#"{"Version":"old"}"#)))
        });
        fs.expect_put_policy().return_once(|_, _| Ok(()));

        ensure_policy(&fs, "read-demo", &spec(doc())).await.unwrap();
    }
}
