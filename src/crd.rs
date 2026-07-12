//! Custom resource definitions: Bucket, User, Policy and ClusterConnection
//! (group `rustfs.com/v1alpha1`).
//!
//! Every namespaced resource points at a RustFS server in one of two ways:
//!
//! - `connection.secretRef`: a Secret in the resource's own namespace holding
//!   `endpoint`, `accessKey` and `secretKey` (self-service; the namespace owns
//!   its credentials).
//! - `connection.clusterRef`: the name of a cluster-scoped
//!   [`ClusterConnection`], whose admin credentials Secret lives only in the
//!   operator's namespace (centrally managed).

use kube::CustomResource;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// Reference to a RustFS server. Exactly one of `secretRef` / `clusterRef`
/// must be set (validated at reconcile time).
#[derive(Clone, Debug, Default, Serialize, Deserialize, JsonSchema, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct ConnectionRef {
    /// Name of a Secret in the resource's namespace holding `endpoint`,
    /// `accessKey` and `secretKey` (optional: `region`, `insecure`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub secret_ref: Option<String>,
    /// Name of a cluster-scoped ClusterConnection.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cluster_ref: Option<String>,
}

impl ConnectionRef {
    /// Connection via a Secret in the resource's own namespace.
    pub fn local(secret: impl Into<String>) -> Self {
        Self {
            secret_ref: Some(secret.into()),
            cluster_ref: None,
        }
    }

    /// Connection via a cluster-scoped ClusterConnection.
    pub fn cluster(name: impl Into<String>) -> Self {
        Self {
            secret_ref: None,
            cluster_ref: Some(name.into()),
        }
    }
}

/// A centrally managed connection to a RustFS server. Cluster-scoped; the
/// referenced credentials Secret lives in the operator's own namespace, so
/// application namespaces never see admin credentials.
#[derive(CustomResource, Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[kube(
    group = "rustfs.com",
    version = "v1alpha1",
    kind = "ClusterConnection",
    shortname = "rfcc",
    printcolumn = r#"{"name":"Endpoint","type":"string","jsonPath":".spec.endpoint"}"#
)]
#[serde(rename_all = "camelCase")]
pub struct ClusterConnectionSpec {
    /// RustFS endpoint URL, e.g. `http://rustfs.storage.svc:9000`.
    pub endpoint: String,
    /// Name of the Secret in the operator's namespace holding `accessKey`
    /// and `secretKey`.
    pub credentials_secret_ref: String,
    /// AWS region; defaults to `us-east-1`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub region: Option<String>,
    /// Skip TLS verification.
    #[serde(default)]
    pub insecure: bool,
    /// Namespaces whose resources may use this connection.
    /// Absent means all namespaces are allowed.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub allowed_namespaces: Option<Vec<String>>,
}

impl ClusterConnectionSpec {
    /// Whether resources in `namespace` may use this connection.
    pub fn allows_namespace(&self, namespace: &str) -> bool {
        match &self.allowed_namespaces {
            None => true,
            Some(allowed) => allowed.iter().any(|n| n == namespace),
        }
    }
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

#[cfg(test)]
mod tests {
    use super::*;

    fn cc_spec(allowed: Option<Vec<&str>>) -> ClusterConnectionSpec {
        ClusterConnectionSpec {
            endpoint: "http://rustfs:9000".into(),
            credentials_secret_ref: "rustfs-admin".into(),
            region: None,
            insecure: false,
            allowed_namespaces: allowed.map(|v| v.iter().map(|s| s.to_string()).collect()),
        }
    }

    #[test]
    fn absent_allowed_namespaces_allows_all() {
        assert!(cc_spec(None).allows_namespace("anything"));
    }

    #[test]
    fn allowed_namespaces_is_an_exact_allowlist() {
        let spec = cc_spec(Some(vec!["team-a", "team-b"]));
        assert!(spec.allows_namespace("team-a"));
        assert!(!spec.allows_namespace("team-c"));
        // empty list denies everything
        assert!(!cc_spec(Some(vec![])).allows_namespace("team-a"));
    }

    #[test]
    fn connection_ref_yaml_forms_deserialize() {
        let local: ConnectionRef = serde_yaml::from_str("secretRef: conn").unwrap();
        assert_eq!(local, ConnectionRef::local("conn"));
        let cluster: ConnectionRef = serde_yaml::from_str("clusterRef: prod").unwrap();
        assert_eq!(cluster, ConnectionRef::cluster("prod"));
    }
}
