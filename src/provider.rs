//! Narrow interface over the RustFS management plane.
//!
//! Reconcilers depend on this trait instead of `rc-core`'s full `AdminApi` /
//! `ObjectStore` traits so unit tests can mock exactly the surface they use.

use async_trait::async_trait;
use rc_core::admin::{AdminApi, Policy, PolicyEntity, User, UserStatus};
use rc_core::traits::ObjectStore;
use rc_core::{Alias, Error as RcError};
use rc_s3::{AdminClient, S3Client};

use crate::error::Result;

/// Connection parameters resolved from the connection Secret.
#[derive(Clone, Debug)]
pub struct ConnectionInfo {
    pub endpoint: String,
    pub access_key: String,
    pub secret_key: String,
    pub region: String,
    pub insecure: bool,
}

impl ConnectionInfo {
    fn into_alias(self) -> Alias {
        Alias {
            name: "rustfs-operator".into(),
            endpoint: self.endpoint,
            access_key: self.access_key,
            secret_key: self.secret_key,
            anonymous: false,
            client_cert: None,
            client_key: None,
            region: self.region,
            signature: "v4".into(),
            bucket_lookup: "auto".into(),
            insecure: self.insecure,
            ca_bundle: None,
            retry: None,
            timeout: None,
        }
    }
}

/// Operations the reconcilers need against a RustFS server.
#[cfg_attr(test, mockall::automock)]
#[async_trait]
pub trait RustFs: Send + Sync {
    // Buckets
    async fn bucket_exists(&self, bucket: &str) -> Result<bool>;
    async fn create_bucket(&self, bucket: &str) -> Result<()>;
    async fn delete_bucket(&self, bucket: &str) -> Result<()>;
    async fn get_versioning(&self, bucket: &str) -> Result<Option<bool>>;
    async fn set_versioning(&self, bucket: &str, enabled: bool) -> Result<()>;
    async fn get_bucket_quota(&self, bucket: &str) -> Result<Option<u64>>;
    async fn set_bucket_quota(&self, bucket: &str, quota: u64) -> Result<()>;

    // Users
    async fn get_user(&self, access_key: &str) -> Result<Option<User>>;
    async fn create_user(&self, access_key: &str, secret_key: &str) -> Result<()>;
    async fn delete_user(&self, access_key: &str) -> Result<()>;
    async fn set_user_status(&self, access_key: &str, enabled: bool) -> Result<()>;
    /// Replace the user's full policy attachment set. RustFS's
    /// `/set-user-or-group-policy` endpoint has replace semantics; there is
    /// no separate attach/detach.
    async fn set_user_policies(&self, user: &str, policies: &[String]) -> Result<()>;

    // Policies
    async fn get_policy(&self, name: &str) -> Result<Option<Policy>>;
    async fn put_policy(&self, name: &str, document: &str) -> Result<()>;
    async fn delete_policy(&self, name: &str) -> Result<()>;
}

/// Real implementation backed by `rc-s3`.
pub struct RustFsProvider {
    s3: S3Client,
    admin: AdminClient,
}

impl RustFsProvider {
    pub async fn connect(info: ConnectionInfo) -> Result<Self> {
        let alias = info.into_alias();
        let admin = AdminClient::new(&alias)?;
        let s3 = S3Client::new(alias).await?;
        Ok(Self { s3, admin })
    }
}

/// Treat "does not exist" responses as absence rather than failure.
fn absent(err: RcError) -> Result<()> {
    match &err {
        RcError::NotFound(_) => Ok(()),
        // The beta server does not consistently return typed not-found
        // errors for every admin route; fall back to message sniffing.
        other => {
            let msg = other.to_string().to_ascii_lowercase();
            if msg.contains("not found") || msg.contains("nosuch") || msg.contains("does not exist")
            {
                Ok(())
            } else {
                Err(err.into())
            }
        }
    }
}

fn optional<T>(res: rc_core::Result<T>) -> Result<Option<T>> {
    match res {
        Ok(v) => Ok(Some(v)),
        Err(e) => absent(e).map(|()| None),
    }
}

#[async_trait]
impl RustFs for RustFsProvider {
    async fn bucket_exists(&self, bucket: &str) -> Result<bool> {
        Ok(self.s3.bucket_exists(bucket).await?)
    }

    async fn create_bucket(&self, bucket: &str) -> Result<()> {
        Ok(self.s3.create_bucket(bucket).await?)
    }

    async fn delete_bucket(&self, bucket: &str) -> Result<()> {
        match self.s3.delete_bucket(bucket).await {
            Ok(()) => Ok(()),
            Err(e) => absent(e),
        }
    }

    async fn get_versioning(&self, bucket: &str) -> Result<Option<bool>> {
        Ok(self.s3.get_versioning(bucket).await?)
    }

    async fn set_versioning(&self, bucket: &str, enabled: bool) -> Result<()> {
        Ok(self.s3.set_versioning(bucket, enabled).await?)
    }

    async fn get_bucket_quota(&self, bucket: &str) -> Result<Option<u64>> {
        match optional(self.admin.get_bucket_quota(bucket).await)? {
            Some(q) => Ok(q.quota.filter(|q| *q > 0)),
            None => Ok(None),
        }
    }

    async fn set_bucket_quota(&self, bucket: &str, quota: u64) -> Result<()> {
        self.admin.set_bucket_quota(bucket, quota).await?;
        Ok(())
    }

    async fn get_user(&self, access_key: &str) -> Result<Option<User>> {
        optional(self.admin.get_user(access_key).await)
    }

    async fn create_user(&self, access_key: &str, secret_key: &str) -> Result<()> {
        self.admin.create_user(access_key, secret_key).await?;
        Ok(())
    }

    async fn delete_user(&self, access_key: &str) -> Result<()> {
        match self.admin.delete_user(access_key).await {
            Ok(()) => Ok(()),
            Err(e) => absent(e),
        }
    }

    async fn set_user_status(&self, access_key: &str, enabled: bool) -> Result<()> {
        let status = if enabled {
            UserStatus::Enabled
        } else {
            UserStatus::Disabled
        };
        Ok(self.admin.set_user_status(access_key, status).await?)
    }

    async fn set_user_policies(&self, user: &str, policies: &[String]) -> Result<()> {
        Ok(self
            .admin
            .attach_policy(policies, PolicyEntity::User, user)
            .await?)
    }

    async fn get_policy(&self, name: &str) -> Result<Option<Policy>> {
        optional(self.admin.get_policy(name).await)
    }

    async fn put_policy(&self, name: &str, document: &str) -> Result<()> {
        Ok(self.admin.create_policy(name, document).await?)
    }

    async fn delete_policy(&self, name: &str) -> Result<()> {
        match self.admin.delete_policy(name).await {
            Ok(()) => Ok(()),
            Err(e) => absent(e),
        }
    }
}
