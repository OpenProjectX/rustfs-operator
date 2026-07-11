//! End-to-end test: real k3s cluster + real RustFS server, controllers
//! running in-process against the k3s API.
//!
//! Run with: `cargo test --features e2e --test e2e_k3s`

#![cfg(feature = "e2e")]

mod common;

use std::time::Duration;

use k8s_openapi::api::core::v1::Secret;
use k8s_openapi::apiextensions_apiserver::pkg::apis::apiextensions::v1::CustomResourceDefinition;
use kube::api::{DeleteParams, PostParams};
use kube::config::{KubeConfigOptions, Kubeconfig};
use kube::{Api, Client, CustomResourceExt};
use serde_json::json;
use testcontainers::runners::AsyncRunner;
use testcontainers::ImageExt;
use testcontainers_modules::k3s::{K3s, KUBE_SECURE_PORT};

use rustfs_operator::crd::{
    Bucket, BucketSpec, ConnectionRef, DeletionPolicy, Policy, PolicySpec, SecretKeyRef, User,
    UserSpec,
};
use rustfs_operator::provider::RustFs;
use rustfs_operator::reconcile;

const K3S_TAG: &str = "v1.34.9-k3s1";
const NS: &str = "default";

async fn eventually<F, Fut>(what: &str, timeout_secs: u64, check: F)
where
    F: Fn() -> Fut,
    Fut: std::future::Future<Output = bool>,
{
    let deadline = std::time::Instant::now() + Duration::from_secs(timeout_secs);
    loop {
        if check().await {
            return;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "timed out after {timeout_secs}s waiting for: {what}"
        );
        tokio::time::sleep(Duration::from_millis(500)).await;
    }
}

async fn k3s_client(conf_dir: &std::path::Path, port: u16, yaml: &str) -> Client {
    let _ = conf_dir; // kubeconfig already read from the mount
    let mut kubeconfig = Kubeconfig::from_yaml(yaml).expect("invalid kubeconfig from k3s");
    for named_cluster in &mut kubeconfig.clusters {
        if let Some(cluster) = &mut named_cluster.cluster {
            cluster.server = Some(format!("https://127.0.0.1:{port}"));
        }
    }
    let config = kube::Config::from_custom_kubeconfig(kubeconfig, &KubeConfigOptions::default())
        .await
        .expect("failed to build kube config");
    Client::try_from(config).expect("failed to build kube client")
}

fn connection_secret(name: &str, endpoint: &str) -> Secret {
    let mut secret = Secret::default();
    secret.metadata.name = Some(name.into());
    secret.string_data = Some(
        [
            ("endpoint".to_string(), endpoint.to_string()),
            ("accessKey".to_string(), common::ADMIN_KEY.to_string()),
            ("secretKey".to_string(), common::ADMIN_SECRET.to_string()),
        ]
        .into(),
    );
    secret
}

#[tokio::test]
async fn operator_reconciles_crs_against_rustfs() {
    common::setup_test_env();

    // --- infrastructure: RustFS + k3s side by side ---
    let (_rustfs, endpoint) = common::start_rustfs().await;
    let fs = common::connect_when_ready(&endpoint).await;

    let conf_dir = tempfile::tempdir().expect("tempdir");
    let k3s = K3s::default()
        .with_conf_mount(conf_dir.path())
        .with_tag(K3S_TAG)
        .with_privileged(true)
        .with_userns_mode("host")
        .with_startup_timeout(Duration::from_secs(180))
        .start()
        .await
        .expect("failed to start k3s container");
    let kube_port = k3s
        .get_host_port_ipv4(KUBE_SECURE_PORT)
        .await
        .expect("k3s port not mapped");
    let kube_yaml = k3s.image().read_kube_config().expect("read kubeconfig");
    let client = k3s_client(conf_dir.path(), kube_port, &kube_yaml).await;

    // --- install CRDs and wait until they are served ---
    let crds: Api<CustomResourceDefinition> = Api::all(client.clone());
    for crd in [Bucket::crd(), User::crd(), Policy::crd()] {
        crds.create(&PostParams::default(), &crd)
            .await
            .expect("failed to create CRD");
    }
    let buckets: Api<Bucket> = Api::namespaced(client.clone(), NS);
    let users: Api<User> = Api::namespaced(client.clone(), NS);
    let policies: Api<Policy> = Api::namespaced(client.clone(), NS);
    eventually("CRDs are served", 60, || async {
        buckets.list(&Default::default()).await.is_ok()
            && users.list(&Default::default()).await.is_ok()
            && policies.list(&Default::default()).await.is_ok()
    })
    .await;

    // --- run the operator in-process ---
    let controller = tokio::spawn(reconcile::run_all(client.clone()));

    // --- connection + user credentials secrets ---
    let secrets: Api<Secret> = Api::namespaced(client.clone(), NS);
    secrets
        .create(&PostParams::default(), &connection_secret("rustfs-conn", &endpoint))
        .await
        .expect("create connection secret");
    let mut user_creds = Secret::default();
    user_creds.metadata.name = Some("e2e-user-creds".into());
    user_creds.string_data = Some([("secretKey".to_string(), "e2e-secret-key-123".to_string())].into());
    secrets
        .create(&PostParams::default(), &user_creds)
        .await
        .expect("create user creds secret");

    let conn = ConnectionRef {
        secret_ref: "rustfs-conn".into(),
    };

    // --- apply CRs: Policy, User (attached), Bucket ---
    policies
        .create(
            &PostParams::default(),
            &Policy::new(
                "e2e-policy",
                PolicySpec {
                    connection: conn.clone(),
                    policy_name: None,
                    document: json!({
                        "Version": "2012-10-17",
                        "Statement": [{
                            "Effect": "Allow",
                            "Action": ["s3:GetObject"],
                            "Resource": ["arn:aws:s3:::e2e-bucket/*"]
                        }]
                    }),
                    deletion_policy: DeletionPolicy::Delete,
                },
            ),
        )
        .await
        .expect("create Policy CR");

    users
        .create(
            &PostParams::default(),
            &User::new(
                "e2e-user",
                UserSpec {
                    connection: conn.clone(),
                    access_key: None,
                    secret_key_ref: SecretKeyRef {
                        name: "e2e-user-creds".into(),
                        key: "secretKey".into(),
                    },
                    policies: vec!["e2e-policy".into()],
                    enabled: true,
                    deletion_policy: DeletionPolicy::Delete,
                },
            ),
        )
        .await
        .expect("create User CR");

    buckets
        .create(
            &PostParams::default(),
            &Bucket::new(
                "e2e-bucket",
                BucketSpec {
                    connection: conn.clone(),
                    bucket_name: None,
                    versioning: Some(true),
                    quota_bytes: Some(10 * 1024 * 1024),
                    deletion_policy: DeletionPolicy::Delete,
                },
            ),
        )
        .await
        .expect("create Bucket CR");

    // --- observe convergence in RustFS ---
    eventually("bucket exists in RustFS", 120, || async {
        fs.bucket_exists("e2e-bucket").await.unwrap_or(false)
    })
    .await;
    eventually("bucket versioning enabled", 60, || async {
        fs.get_versioning("e2e-bucket").await.ok().flatten() == Some(true)
    })
    .await;
    eventually("bucket quota set", 60, || async {
        fs.get_bucket_quota("e2e-bucket").await.ok().flatten() == Some(10 * 1024 * 1024)
    })
    .await;
    eventually("policy exists in RustFS", 60, || async {
        fs.get_policy("e2e-policy").await.ok().flatten().is_some()
    })
    .await;
    eventually("user exists with policy attached", 60, || async {
        match fs.get_user("e2e-user").await {
            Ok(Some(u)) => u.policies().contains(&"e2e-policy".to_string()),
            _ => false,
        }
    })
    .await;

    // --- CR statuses report ready ---
    eventually("Bucket CR is ready", 60, || async {
        buckets
            .get("e2e-bucket")
            .await
            .ok()
            .and_then(|b| b.status)
            .map(|s| s.ready)
            .unwrap_or(false)
    })
    .await;
    eventually("User CR is ready", 60, || async {
        users
            .get("e2e-user")
            .await
            .ok()
            .and_then(|u| u.status)
            .map(|s| s.ready)
            .unwrap_or(false)
    })
    .await;
    eventually("Policy CR is ready", 60, || async {
        policies
            .get("e2e-policy")
            .await
            .ok()
            .and_then(|p| p.status)
            .map(|s| s.ready)
            .unwrap_or(false)
    })
    .await;

    // --- deletion: finalizers clean up the remote resources ---
    buckets
        .delete("e2e-bucket", &DeleteParams::default())
        .await
        .expect("delete Bucket CR");
    eventually("bucket removed from RustFS", 120, || async {
        !fs.bucket_exists("e2e-bucket").await.unwrap_or(true)
    })
    .await;

    users
        .delete("e2e-user", &DeleteParams::default())
        .await
        .expect("delete User CR");
    eventually("user removed from RustFS", 60, || async {
        matches!(fs.get_user("e2e-user").await, Ok(None))
    })
    .await;

    policies
        .delete("e2e-policy", &DeleteParams::default())
        .await
        .expect("delete Policy CR");
    eventually("policy removed from RustFS", 60, || async {
        matches!(fs.get_policy("e2e-policy").await, Ok(None))
    })
    .await;

    controller.abort();
}
