//! Resolve connection Secrets into RustFS clients.

use k8s_openapi::api::core::v1::Secret;
use kube::{Api, Client};

use crate::crd::{ConnectionRef, SecretKeyRef};
use crate::error::{Error, Result};
use crate::provider::{ConnectionInfo, RustFsProvider};

fn secret_value(secret: &Secret, key: &str) -> Option<String> {
    secret
        .data
        .as_ref()
        .and_then(|d| d.get(key))
        .and_then(|v| String::from_utf8(v.0.clone()).ok())
}

fn required(secret: &Secret, key: &str, secret_name: &str) -> Result<String> {
    secret_value(secret, key)
        .filter(|v| !v.is_empty())
        .ok_or_else(|| Error::Connection(format!("secret '{secret_name}' is missing key '{key}'")))
}

/// Read the connection Secret referenced by a CR and connect to RustFS.
pub async fn provider_for(
    client: &Client,
    namespace: &str,
    conn: &ConnectionRef,
) -> Result<RustFsProvider> {
    let secrets: Api<Secret> = Api::namespaced(client.clone(), namespace);
    let secret = secrets.get(&conn.secret_ref).await.map_err(|e| {
        Error::Connection(format!(
            "cannot read connection secret '{}': {e}",
            conn.secret_ref
        ))
    })?;

    let info = ConnectionInfo {
        endpoint: required(&secret, "endpoint", &conn.secret_ref)?,
        access_key: required(&secret, "accessKey", &conn.secret_ref)?,
        secret_key: required(&secret, "secretKey", &conn.secret_ref)?,
        region: secret_value(&secret, "region").unwrap_or_else(|| "us-east-1".into()),
        insecure: secret_value(&secret, "insecure").as_deref() == Some("true"),
    };
    RustFsProvider::connect(info).await
}

/// Read a single key from a Secret (e.g. a managed user's secret key).
pub async fn secret_key_value(
    client: &Client,
    namespace: &str,
    sref: &SecretKeyRef,
) -> Result<String> {
    let secrets: Api<Secret> = Api::namespaced(client.clone(), namespace);
    let secret = secrets
        .get(&sref.name)
        .await
        .map_err(|e| Error::Connection(format!("cannot read secret '{}': {e}", sref.name)))?;
    required(&secret, &sref.key, &sref.name)
}
