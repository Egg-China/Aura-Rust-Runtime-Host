use crate::abi::HmclStatus;
use std::fmt::{Display, Formatter};

/// Stable, redacted error categories carried by Bridge Value v1.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum BridgeErrorKind {
    /// An argument or locally constructed value is invalid.
    InvalidArgument = 0,
    /// A foreign result is malformed or violates the wire contract.
    InvalidResult = 1,
    /// The plugin lacks permission for the requested operation.
    PermissionDenied = 2,
    /// A host-managed handle is no longer live.
    StaleHandle,
    /// The expected and actual canonical handle types differ.
    TypeMismatch,
    /// An asynchronous operation was cancelled.
    Cancelled,
    /// A callback was completed more than once or otherwise failed.
    CallbackFailed,
    /// A required host service or bounded SDK resource is unavailable.
    Unavailable,
    /// An implementation error occurred without exposing private diagnostics.
    Internal,
}

impl BridgeErrorKind {
    /// Returns the stable lower-case code shared with the Java Bridge contract.
    #[must_use]
    pub const fn wire_code(self) -> &'static str {
        match self {
            Self::InvalidArgument => "invalid-argument",
            Self::InvalidResult => "invalid-result",
            Self::PermissionDenied => "permission-denied",
            Self::StaleHandle => "stale-handle",
            Self::TypeMismatch => "type-mismatch",
            Self::Cancelled => "cancelled",
            Self::CallbackFailed => "callback-failed",
            Self::Unavailable => "unavailable",
            Self::Internal => "internal",
        }
    }

    pub(crate) fn from_wire(value: &str) -> Option<Self> {
        match value {
            "invalid-argument" => Some(Self::InvalidArgument),
            "invalid-result" => Some(Self::InvalidResult),
            "permission-denied" => Some(Self::PermissionDenied),
            "stale-handle" => Some(Self::StaleHandle),
            "type-mismatch" => Some(Self::TypeMismatch),
            "cancelled" => Some(Self::Cancelled),
            "callback-failed" => Some(Self::CallbackFailed),
            "unavailable" => Some(Self::Unavailable),
            "internal" => Some(Self::Internal),
            _ => None,
        }
    }
}

/// Backward-compatible SDK name for [`BridgeErrorKind`].
pub type ErrorCode = BridgeErrorKind;

/// A deliberately redacted SDK error containing only a stable category.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Error {
    code: BridgeErrorKind,
}

impl Error {
    /// Creates a redacted error in `code`.
    #[must_use]
    pub const fn new(code: BridgeErrorKind) -> Self {
        Self { code }
    }

    /// Returns the stable category without exposing implementation diagnostics.
    #[must_use]
    pub const fn code(&self) -> BridgeErrorKind {
        self.code
    }

    pub(crate) const fn from_host_status(status: HmclStatus) -> Self {
        let code = match status.into_raw() {
            value if value == HmclStatus::InvalidArgument.into_raw() => ErrorCode::InvalidArgument,
            value if value == HmclStatus::UnsupportedAbi.into_raw() => ErrorCode::Unavailable,
            value if value == HmclStatus::BufferTooSmall.into_raw() => ErrorCode::InvalidResult,
            value if value == HmclStatus::HostError.into_raw() => ErrorCode::Unavailable,
            value if value == HmclStatus::PluginError.into_raw() => ErrorCode::Internal,
            _ => ErrorCode::Internal,
        };
        Self::new(code)
    }
}

impl Display for Error {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> std::fmt::Result {
        write!(formatter, "HMCL plugin SDK error: {:?}", self.code)
    }
}

impl std::error::Error for Error {}
