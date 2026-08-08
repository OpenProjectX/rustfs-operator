use thiserror::Error;

pub type Result<T, E = Error> = std::result::Result<T, E>;

#[derive(Debug, Error)]
pub enum Error {
    #[error("RustFS API error: {0}")]
    RustFs(#[from] rc_core::Error),

    #[error("Kubernetes API error: {0}")]
    Kube(#[from] kube::Error),

    #[error("invalid connection secret: {0}")]
    Connection(String),

    #[error("invalid spec: {0}")]
    Spec(String),

    #[error("finalizer error: {0}")]
    Finalizer(String),
}

impl Error {
    /// Whether this error can only be resolved by editing the resource (or
    /// the Secret it points at) rather than by retrying against the server.
    pub fn is_config_error(&self) -> bool {
        matches!(self, Error::Spec(_) | Error::Connection(_))
    }
}
