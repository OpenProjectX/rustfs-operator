//! Custom resource definitions: Bucket, User, Policy (group `rustfs.com/v1alpha1`).
//!
//! Every resource points at a connection Secret in its own namespace holding
//! the RustFS endpoint and admin credentials:
//!
//! ```yaml
//! stringData:
//!   endpoint: http://rustfs.storage.svc:9000
//!   accessKey: rustfsadmin
//!   secretKey: rustfsadmin
//!   # optional:
//!   region: us-east-1
//!   insecure: "true"
//! ```

use kube::CustomResource;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// Reference to the connection Secret (same namespace as the resource).
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct ConnectionRef {
    /// Name of the Secret holding `endpoint`, `accessKey` and `secretKey`.
    pub secret_ref: String,
}

/// What happens to the remote resource when the CR is deleted.
#[derive(Clone, Copy, Debug, Default, Serialize, Deserialize, JsonSchema, PartialEq)]
pub enum DeletionPolicy {
    /// Delete the resource in RustFS.
    #[default]
    Delete,
    /// Keep the resource in RustFS, only remove the CR.
    Retain,
}

/// Shared status for all RustFS resources.
#[derive(Clone, Debug, Default, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct ResourceStatus {
    /// True once the remote resource matches the spec.
    #[serde(default)]
    pub ready: bool,
    /// Human-readable state or last error.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub message: Option<String>,
    /// Generation last acted upon.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub observed_generation: Option<i64>,
}

impl ResourceStatus {
    pub fn ready(generation: Option<i64>) -> Self {
        Self {
            ready: true,
            message: Some("reconciled".into()),
            observed_generation: generation,
        }
    }

    pub fn error(generation: Option<i64>, message: impl std::fmt::Display) -> Self {
        Self {
            ready: false,
            message: Some(message.to_string()),
            observed_generation: generation,
        }
    }
}

/// An S3 bucket in RustFS.
#[derive(CustomResource, Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[kube(
    group = "rustfs.com",
    version = "v1alpha1",
    kind = "Bucket",
    namespaced,
    status = "ResourceStatus",
    shortname = "rfb",
    printcolumn = r#"{"name":"Ready","type":"boolean","jsonPath":".status.ready"}"#,
    printcolumn = r#"{"name":"Message","type":"string","jsonPath":".status.message"}"#
)]
#[serde(rename_all = "camelCase")]
pub struct BucketSpec {
    pub connection: ConnectionRef,
    /// Bucket name in RustFS; defaults to the CR name.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub bucket_name: Option<String>,
    /// Desired versioning state; unset means unmanaged.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub versioning: Option<bool>,
    /// Hard quota in bytes; unset means unmanaged.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub quota_bytes: Option<u64>,
    #[serde(default)]
    pub deletion_policy: DeletionPolicy,
}

impl Bucket {
    /// Effective bucket name in RustFS.
    pub fn bucket_name(&self) -> &str {
        self.spec
            .bucket_name
            .as_deref()
            .unwrap_or_else(|| self.metadata.name.as_deref().unwrap_or_default())
    }
}

/// Reference to a key inside a Secret in the same namespace.
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct SecretKeyRef {
    /// Secret name.
    pub name: String,
    /// Key within the Secret; defaults to `secretKey`.
    #[serde(default = "default_secret_key_key")]
    pub key: String,
}

fn default_secret_key_key() -> String {
    "secretKey".into()
}

fn default_true() -> bool {
    true
}

/// An IAM user in RustFS.
#[derive(CustomResource, Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[kube(
    group = "rustfs.com",
    version = "v1alpha1",
    kind = "User",
    namespaced,
    status = "ResourceStatus",
    shortname = "rfu",
    printcolumn = r#"{"name":"Ready","type":"boolean","jsonPath":".status.ready"}"#,
    printcolumn = r#"{"name":"Message","type":"string","jsonPath":".status.message"}"#
)]
#[serde(rename_all = "camelCase")]
pub struct UserSpec {
    pub connection: ConnectionRef,
    /// Access key (username) in RustFS; defaults to the CR name.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub access_key: Option<String>,
    /// Secret holding the user's secret key (used at creation time).
    pub secret_key_ref: SecretKeyRef,
    /// Policies attached to the user; managed declaratively (extra
    /// attachments are detached).
    #[serde(default)]
    pub policies: Vec<String>,
    /// Whether the user is enabled.
    #[serde(default = "default_true")]
    pub enabled: bool,
    #[serde(default)]
    pub deletion_policy: DeletionPolicy,
}

impl User {
    /// Effective access key in RustFS.
    pub fn access_key(&self) -> &str {
        self.spec
            .access_key
            .as_deref()
            .unwrap_or_else(|| self.metadata.name.as_deref().unwrap_or_default())
    }
}

/// An IAM policy in RustFS.
#[derive(CustomResource, Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[kube(
    group = "rustfs.com",
    version = "v1alpha1",
    kind = "Policy",
    namespaced,
    status = "ResourceStatus",
    shortname = "rfp",
    printcolumn = r#"{"name":"Ready","type":"boolean","jsonPath":".status.ready"}"#,
    printcolumn = r#"{"name":"Message","type":"string","jsonPath":".status.message"}"#
)]
#[serde(rename_all = "camelCase")]
pub struct PolicySpec {
    pub connection: ConnectionRef,
    /// Policy name in RustFS; defaults to the CR name.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub policy_name: Option<String>,
    /// IAM policy document, written inline as YAML/JSON.
    #[schemars(schema_with = "policy_document_schema")]
    pub document: serde_json::Value,
    #[serde(default)]
    pub deletion_policy: DeletionPolicy,
}

/// Arbitrary JSON object; Kubernetes requires an explicit type plus
/// `x-kubernetes-preserve-unknown-fields` for free-form fields.
fn policy_document_schema(_: &mut schemars::SchemaGenerator) -> schemars::Schema {
    schemars::json_schema!({
        "type": "object",
        "x-kubernetes-preserve-unknown-fields": true
    })
}

impl Policy {
    /// Effective policy name in RustFS.
    pub fn policy_name(&self) -> &str {
        self.spec
            .policy_name
            .as_deref()
            .unwrap_or_else(|| self.metadata.name.as_deref().unwrap_or_default())
    }
}
