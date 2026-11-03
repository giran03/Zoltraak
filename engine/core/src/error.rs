use std::path::PathBuf;

pub type Result<T> = std::result::Result<T, Error>;

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("network error: {0}")]
    Http(#[from] reqwest::Error),

    #[error("i/o error on {path}: {source}")]
    Io {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },

    #[error("database error: {0}")]
    Db(#[from] sqlx::Error),

    /// The server's validator changed between the original request and a
    /// resume attempt, so the partial file on disk no longer matches the
    /// remote file. Resuming would silently corrupt it.
    #[error("remote file changed since download started (validator mismatch)")]
    StaleResume,

    #[error("server returned {status} for {url}")]
    UnexpectedStatus { status: u16, url: String },

    #[error("server ignored byte ranges; download must restart as a single stream")]
    RangeUnsupported,

    #[error("server returned an invalid byte range; download must restart as a single stream")]
    InvalidRange,

    #[error("download response ended before all expected bytes arrived")]
    IncompleteResponse,

    #[error("download server stopped responding")]
    NetworkTimeout,

    /// A remote API refused the request and said why. Kept apart from
    /// [`Error::UnexpectedStatus`] so the service's own wording — "missing
    /// permissions", "unknown channel" — reaches the user instead of a code
    /// they would have to go and look up.
    #[error("{service} refused the request ({status}): {message}")]
    ApiRejected {
        service: &'static str,
        status: u16,
        message: String,
    },

    #[error("checksum mismatch: expected {expected}, got {actual}")]
    ChecksumMismatch { expected: String, actual: String },

    #[error("download {0} not found")]
    NotFound(crate::model::DownloadId),

    #[error("{url} is already queued for this destination")]
    Duplicate { url: String },

    /// Stored data that cannot be interpreted — a bad enum value, an
    /// unparseable timestamp, a failed migration.
    #[error("corrupt queue data: {0}")]
    Corrupt(String),

    #[error("insufficient disk space: need {needed} bytes, {available} available")]
    InsufficientSpace { needed: u64, available: u64 },

    /// A caller-supplied value that is well-formed JSON/HTTP but not
    /// something the engine can act on — an empty category pattern, a regex
    /// that fails to compile.
    #[error("{0}")]
    InvalidInput(String),
}

impl Error {
    /// Whether a failed attempt is worth retrying.
    ///
    /// Validator mismatches and checksum failures are deliberately terminal:
    /// retrying cannot fix either, and doing so would just spin.
    pub fn is_retryable(&self) -> bool {
        match self {
            Error::Http(e) => {
                e.is_timeout() || e.is_connect() || e.is_request() || e.is_body() || e.is_decode()
            }
            Error::IncompleteResponse | Error::NetworkTimeout => true,
            Error::RangeUnsupported | Error::InvalidRange => false,
            Error::UnexpectedStatus { status, .. } => {
                // 5xx and 429 are transient; other 4xx are not.
                *status >= 500 || *status == 429
            }
            // Same rule: a rejection we were given a reason for is still only
            // worth retrying when the reason is "busy", not "no".
            Error::ApiRejected { status, .. } => *status >= 500 || *status == 429,
            Error::Io { .. } => false,
            Error::Db(_) => false,
            Error::StaleResume => false,
            Error::ChecksumMismatch { .. } => false,
            Error::NotFound(_) => false,
            Error::Duplicate { .. } => false,
            Error::Corrupt(_) => false,
            Error::InsufficientSpace { .. } => false,
            Error::InvalidInput(_) => false,
        }
    }
}
