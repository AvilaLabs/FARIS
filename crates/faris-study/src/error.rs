use thiserror::Error;

#[derive(Debug, Error)]
pub enum StudyError {
    /// The caller's input is unusable (missing file, bad argument, I/O failure).
    #[error("{0}")]
    Input(String),
    #[error("{context}: {source}")]
    Io {
        context: String,
        #[source]
        source: std::io::Error,
    },
    /// The file is not a valid study file, or fails verification.
    #[error("{0}")]
    Corrupt(String),
    /// The caller asked for the work to stop before it finished.
    #[error("cancelled")]
    Cancelled,
    #[error(
        "unsupported study-file version \"{0}\"; this FARIS reads major version 1 (format \"faris-study/1\")"
    )]
    UnsupportedVersion(String),
    #[error(
        "unsupported blob encoding \"{encoding}\" for blob {blob}; refusing to skip data this reader cannot interpret"
    )]
    UnsupportedEncoding { blob: String, encoding: String },
    #[error("blob {blob} does not match its recorded SHA-256 (found {found})")]
    HashMismatch { blob: String, found: String },
    #[error("blob {blob} has the wrong size: recorded {recorded} bytes, found {found}")]
    SizeMismatch {
        blob: String,
        recorded: u64,
        found: String,
    },
}

impl StudyError {
    pub(crate) fn io(context: impl Into<String>, source: std::io::Error) -> Self {
        Self::Io {
            context: context.into(),
            source,
        }
    }

    pub(crate) fn corrupt(message: impl Into<String>) -> Self {
        Self::Corrupt(message.into())
    }

    /// True when the file was read but failed verification (as opposed to
    /// being unreadable or badly specified by the caller).
    pub fn is_verification_failure(&self) -> bool {
        !matches!(self, Self::Input(_) | Self::Io { .. } | Self::Cancelled)
    }
}

impl From<zip::result::ZipError> for StudyError {
    fn from(error: zip::result::ZipError) -> Self {
        match error {
            zip::result::ZipError::Io(source) => Self::io("study file archive", source),
            other => Self::Corrupt(format!("not a valid study file container: {other}")),
        }
    }
}

impl From<serde_json::Error> for StudyError {
    fn from(error: serde_json::Error) -> Self {
        Self::Corrupt(format!("manifest is not valid: {error}"))
    }
}
