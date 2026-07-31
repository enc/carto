//! carto's error type and the exit-code contract (spec §9.2).
//!
//! Hand-rolled rather than `thiserror`: the type is small and closed (four
//! kinds, no derive-macro combinatorics needed), and a minimal dependency
//! graph is the point of INV-1. See docs/adr/0001-hand-rolled-error-atomicfile.md.

use std::fmt;

/// What kind of failure this is, which in turn determines the process exit
/// code (spec §9.2):
///
/// | ErrorKind          | Exit |
/// |---------------------|------|
/// | (success)            | 0    |
/// | `UserError`          | 1    |
/// | `DataError`          | 2    |
/// | `InvariantRefusal`   | 3    |
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ErrorKind {
    /// Bad flag, bad path, malformed CLI invocation.
    UserError,
    /// Input file failed to parse past an acceptable threshold (e.g.
    /// unreadable tf-json, corrupt graph.json).
    DataError,
    /// An invariant (INV-1..INV-8) refused the operation. Always a hard
    /// error, never a warning.
    InvariantRefusal,
}

impl ErrorKind {
    /// The process exit code for this kind, per spec §9.2.
    pub fn exit_code(self) -> u8 {
        match self {
            ErrorKind::UserError => 1,
            ErrorKind::DataError => 2,
            ErrorKind::InvariantRefusal => 3,
        }
    }
}

/// carto's error type. Carries a kind (for the exit-code contract), a
/// human-readable message, and an optional underlying cause.
///
/// The message is assumed to be operator-authored (flag names, path
/// diagnostics) or a static description — never raw repository content.
/// Repository-derived text must go through [`crate::taint::TaintedString`]
/// before it can appear anywhere, including here (INV-5).
#[derive(Debug)]
pub struct Error {
    kind: ErrorKind,
    msg: String,
    source: Option<Box<dyn std::error::Error + Send + Sync + 'static>>,
}

impl Error {
    pub fn new(kind: ErrorKind, msg: impl Into<String>) -> Self {
        Error {
            kind,
            msg: msg.into(),
            source: None,
        }
    }

    pub fn with_source(
        kind: ErrorKind,
        msg: impl Into<String>,
        source: impl std::error::Error + Send + Sync + 'static,
    ) -> Self {
        Error {
            kind,
            msg: msg.into(),
            source: Some(Box::new(source)),
        }
    }

    pub fn user(msg: impl Into<String>) -> Self {
        Error::new(ErrorKind::UserError, msg)
    }

    pub fn data(msg: impl Into<String>) -> Self {
        Error::new(ErrorKind::DataError, msg)
    }

    pub fn invariant_refusal(msg: impl Into<String>) -> Self {
        Error::new(ErrorKind::InvariantRefusal, msg)
    }

    pub fn kind(&self) -> ErrorKind {
        self.kind
    }

    /// The process exit code this error maps to, per spec §9.2.
    pub fn exit_code(&self) -> u8 {
        self.kind.exit_code()
    }
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.msg)?;
        if let Some(source) = &self.source {
            write!(f, ": {source}")?;
        }
        Ok(())
    }
}

impl std::error::Error for Error {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        self.source
            .as_deref()
            .map(|e| e as &(dyn std::error::Error + 'static))
    }
}

impl From<std::io::Error> for Error {
    fn from(err: std::io::Error) -> Self {
        Error::with_source(ErrorKind::DataError, "I/O error", err)
    }
}

pub type Result<T> = std::result::Result<T, Error>;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn exit_codes_match_spec_9_2() {
        assert_eq!(ErrorKind::UserError.exit_code(), 1);
        assert_eq!(ErrorKind::DataError.exit_code(), 2);
        assert_eq!(ErrorKind::InvariantRefusal.exit_code(), 3);
    }

    #[test]
    fn display_includes_source() {
        let io_err = std::io::Error::new(std::io::ErrorKind::NotFound, "nope");
        let err = Error::with_source(ErrorKind::DataError, "failed to read", io_err);
        assert!(err.to_string().contains("failed to read"));
        assert!(err.to_string().contains("nope"));
    }

    #[test]
    fn io_error_converts_to_data_error() {
        let io_err = std::io::Error::new(std::io::ErrorKind::PermissionDenied, "denied");
        let err: Error = io_err.into();
        assert_eq!(err.kind(), ErrorKind::DataError);
    }
}
