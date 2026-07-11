//! rustfs-operator: a Kubernetes operator managing RustFS buckets, IAM users
//! and policies through the RustFS admin and S3 APIs.

pub mod connection;
pub mod crd;
pub mod error;
pub mod provider;
pub mod reconcile;

pub use error::{Error, Result};

/// Install the process-level rustls crypto provider.
///
/// The dependency tree compiles rustls with both `ring` (via kube) and
/// `aws-lc-rs` (via the AWS SDK), so rustls cannot pick a default on its
/// own. Must be called once before any TLS connection is made.
pub fn install_crypto_provider() {
    let _ = rustls::crypto::ring::default_provider().install_default();
}
