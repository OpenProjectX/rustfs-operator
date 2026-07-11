//! Integration tests against a real RustFS container (no Kubernetes).
//!
//! Run with: `cargo test --features integration --test integration_rustfs`

#![cfg(feature = "integration")]

mod common;

use rustfs_operator::provider::RustFs;
use serde_json::json;

#[tokio::test]
async fn provider_manages_buckets_policies_and_users() {
    common::setup_test_env();
    let (_container, endpoint) = common::start_rustfs().await;
    let fs = common::connect_when_ready(&endpoint).await;

    // --- buckets ---
    assert!(!fs.bucket_exists("it-bucket").await.unwrap());
    fs.create_bucket("it-bucket").await.unwrap();
    assert!(fs.bucket_exists("it-bucket").await.unwrap());

    fs.set_versioning("it-bucket", true).await.unwrap();
    assert_eq!(fs.get_versioning("it-bucket").await.unwrap(), Some(true));

    fs.set_bucket_quota("it-bucket", 10 * 1024 * 1024)
        .await
        .unwrap();
    assert_eq!(
        fs.get_bucket_quota("it-bucket").await.unwrap(),
        Some(10 * 1024 * 1024)
    );

    // --- policies ---
    let document = json!({
        "Version": "2012-10-17",
        "Statement": [{
            "Effect": "Allow",
            "Action": ["s3:GetObject", "s3:PutObject"],
            "Resource": ["arn:aws:s3:::it-bucket/*"]
        }]
    });
    assert!(fs.get_policy("it-policy").await.unwrap().is_none());
    fs.put_policy("it-policy", &document.to_string())
        .await
        .unwrap();
    let fetched = fs.get_policy("it-policy").await.unwrap().unwrap();
    assert!(
        rustfs_operator::reconcile::policy::documents_equivalent(
            &fetched.parse_document().unwrap(),
            &document
        ),
        "stored policy should be semantically equal to the submitted one"
    );

    // --- users ---
    assert!(fs.get_user("it-user").await.unwrap().is_none());
    fs.create_user("it-user", "it-secret-key-123")
        .await
        .unwrap();
    let user = fs.get_user("it-user").await.unwrap().unwrap();
    assert_eq!(user.access_key, "it-user");

    fs.set_user_policies("it-user", &["it-policy".into()])
        .await
        .unwrap();
    let user = fs.get_user("it-user").await.unwrap().unwrap();
    assert!(user.policies().contains(&"it-policy".to_string()));

    fs.set_user_status("it-user", false).await.unwrap();

    // replace semantics: setting a different set drops the old attachment
    let document2 = json!({
        "Version": "2012-10-17",
        "Statement": [{"Effect": "Allow", "Action": ["s3:ListBucket"], "Resource": ["arn:aws:s3:::it-bucket"]}]
    });
    fs.put_policy("it-policy-2", &document2.to_string())
        .await
        .unwrap();
    fs.set_user_policies("it-user", &["it-policy-2".into()])
        .await
        .unwrap();
    let user = fs.get_user("it-user").await.unwrap().unwrap();
    assert_eq!(user.policies(), vec!["it-policy-2".to_string()]);

    // --- cleanup, exercising delete paths ---
    fs.delete_user("it-user").await.unwrap();
    assert!(fs.get_user("it-user").await.unwrap().is_none());
    fs.delete_policy("it-policy").await.unwrap();
    assert!(fs.get_policy("it-policy").await.unwrap().is_none());
    fs.delete_policy("it-policy-2").await.unwrap();
    fs.delete_bucket("it-bucket").await.unwrap();
    assert!(!fs.bucket_exists("it-bucket").await.unwrap());

    // deletes are tolerant of already-absent resources
    fs.delete_user("it-user").await.unwrap();
    fs.delete_policy("it-policy").await.unwrap();
    fs.delete_bucket("it-bucket").await.unwrap();
}
