use thiserror::Error;

/// Unified error type for the migration engine.
#[derive(Debug, Error)]
pub enum MigratorError {
    #[error("hermes environment not found under {0}")]
    HermesNotFound(String),

    #[error("missing required file {0}")]
    MissingFile(String),

    #[error("path does not exist: {0}")]
    PathNotFound(String),

    #[error("insufficient disk space: need {need} bytes, have {have} bytes")]
    InsufficientDiskSpace { need: u64, have: u64 },

    #[error("unsupported platform: {0}")]
    UnsupportedPlatform(String),

    #[error("architecture mismatch: package built for {package}, this machine is {host}")]
    ArchMismatch { package: String, host: String },

    #[error("unsupported hermes layout version {0}")]
    UnsupportedLayoutVersion(u32),

    #[error("checksum mismatch for {0}")]
    ChecksumMismatch(String),

    #[error("integrity check failed: {0}")]
    IntegrityFailed(String),

    #[error("secret material error: {0}")]
    Secrets(String),

    #[error("permission denied: {0}")]
    PermissionDenied(String),

    #[error("invalid migration package: {0}")]
    InvalidPackage(String),

    #[error("io error: {0}")]
    Io(#[from] std::io::Error),

    #[error("json error: {0}")]
    Json(#[from] serde_json::Error),

    #[error("other: {0}")]
    Other(String),
}

impl From<zip::result::ZipError> for MigratorError {
    fn from(e: zip::result::ZipError) -> Self {
        MigratorError::InvalidPackage(e.to_string())
    }
}
