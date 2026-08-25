use thiserror::Error;

pub type Result<T> = std::result::Result<T, BifrostError>;

#[derive(Debug, Error)]
pub enum BifrostError {
    #[error("invalid remote path: {0}")]
    InvalidPath(String),
    #[error("unsupported capability: {0}")]
    UnsupportedCapability(String),
    #[error("configuration error: {0}")]
    Configuration(String),
    #[error("internal error: {0}")]
    Internal(String),
}
