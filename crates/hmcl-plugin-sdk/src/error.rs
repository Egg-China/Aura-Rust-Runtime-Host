use crate::abi::HmclStatus;
use std::fmt::{Display, Formatter};

/// Stable, redacted error categories carried by Bridge Value v1.
#[repr(u8)]
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum ErrorCode {
    /// An argument or locally constructed value is invalid.
    InvalidArgument = 0,
    /// A foreign result is malformed or violates the wire contract.
    InvalidResult = 1,
    /// The plugin lacks permission for the requested operation.
    PermissionDenied = 2,
    /// The requested resource does not exist.
    NotFound = 3,
    /// A required host service is not available.
    Unavailable = 4,
    /// A host-managed handle is no longer live.
    StaleHandle = 5,
    /// An asynchronous operation was cancelled.
    Cancelled = 6,
    /// A callback was completed more than once or otherwise failed.
    CallbackFailed = 7,
    /// An implementation error occurred without exposing private diagnostics.
    Internal = 8,
}

impl ErrorCode {
    pub(crate) const fn from_wire(value: u8) -> Option<Self> {
        match value {
            0 => Some(Self::InvalidArgument),
            1 => Some(Self::InvalidResult),
            2 => Some(Self::PermissionDenied),
            3 => Some(Self::NotFound),
            4 => Some(Self::Unavailable),
            5 => Some(Self::StaleHandle),
            6 => Some(Self::Cancelled),
            7 => Some(Self::CallbackFailed),
            8 => Some(Self::Internal),
            _ => None,
        }
    }
}

/// A deliberately redacted SDK error containing only a stable category.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Error {
    code: ErrorCode,
}

impl Error {
    /// Creates a redacted error in `code`.
    #[must_use]
    pub const fn new(code: ErrorCode) -> Self {
        Self { code }
    }

    /// Returns the stable category without exposing implementation diagnostics.
    #[must_use]
    pub const fn code(&self) -> ErrorCode {
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
