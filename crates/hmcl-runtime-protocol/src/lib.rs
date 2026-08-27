#![deny(missing_docs)]

//! Process protocol shared by isolated Rust Runtime Host endpoints.

use hmcl_plugin_sdk::Value;
use std::fmt::{Display, Formatter};
use std::io::{self, Read, Write};

/// Frozen isolated process protocol generation.
pub const PROTOCOL_VERSION: i64 = 1;

/// Maximum accepted encoded Bridge Value frame body.
pub const MAX_FRAME_BYTES: u32 = 16 * 1024 * 1024;

/// One validated isolated Host protocol message.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Message {
    /// Positive direction-scoped request identifier.
    request_id: u64,

    /// Kind-specific message body.
    body: MessageBody,
}

impl Message {
    /// Creates one validated message.
    pub fn new(request_id: u64, body: MessageBody) -> Result<Self, ProtocolError> {
        if request_id == 0 || request_id > i64::MAX as u64 {
            return Err(invalid("request ID must be a positive signed-64 integer"));
        }
        if request_id.is_multiple_of(2) != body.uses_even_request_id() {
            return Err(invalid(
                "request ID belongs to the other protocol direction",
            ));
        }
        body.validate()?;
        Ok(Self { request_id, body })
    }

    /// Returns the direction-scoped request identifier.
    #[must_use]
    pub const fn request_id(&self) -> u64 {
        self.request_id
    }

    /// Returns the kind-specific body.
    #[must_use]
    pub const fn body(&self) -> &MessageBody {
        &self.body
    }

    /// Encodes one complete message as canonical Bridge Value v1 bytes.
    pub fn to_wire(&self) -> Result<Vec<u8>, ProtocolError> {
        let (kind, payload) = self.body.to_value()?;
        Value::Map(vec![
            ("protocolVersion".into(), Value::Integer(PROTOCOL_VERSION)),
            (
                "requestId".into(),
                Value::Integer(
                    i64::try_from(self.request_id).map_err(|_| invalid("request ID overflow"))?,
                ),
            ),
            ("kind".into(), Value::String(kind.into())),
            ("payload".into(), payload),
        ])
        .to_wire()
        .map_err(|_| invalid("message cannot be encoded as Bridge Value v1"))
    }

    /// Decodes one complete canonical Bridge Value v1 message.
    pub fn from_wire(input: &[u8]) -> Result<Self, ProtocolError> {
        let envelope = exact_map(
            Value::from_wire(input).map_err(|_| invalid("invalid Bridge Value v1 message"))?,
            &["protocolVersion", "requestId", "kind", "payload"],
        )?;
        let version = integer(field(&envelope, "protocolVersion")?)?;
        if version != PROTOCOL_VERSION {
            return Err(invalid("unsupported protocol version"));
        }
        let request_id = positive_id(field(&envelope, "requestId")?)?;
        let kind = string(field(&envelope, "kind")?)?.to_owned();
        let body = MessageBody::from_value(&kind, field(&envelope, "payload")?.clone())?;
        Self::new(request_id, body)
    }
}

/// Kind-specific protocol message body.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum MessageBody {
    /// Negotiates the frozen protocol generation before payload data is accepted.
    Hello,

    /// Loads one verified payload library without enabling it.
    Load {
        /// Canonical absolute extracted package root.
        package_root: String,
        /// Canonical payload entrypoint beneath the package root.
        entrypoint: String,
        /// Positive Host-generated plugin identifier.
        plugin_id: u64,
        /// Positive Host-generated Bridge session identifier.
        session: u64,
    },

    /// Enables the loaded payload.
    Enable,

    /// Invokes one operation on the enabled payload.
    Invoke {
        /// Canonical payload operation.
        operation: String,
        /// Opaque canonical Bridge Value v1 input.
        input: Vec<u8>,
        /// Nonnegative payload-local callback identifier.
        callback_id: u64,
    },

    /// Disables the enabled payload.
    Disable,

    /// Shuts down and unloads the payload.
    Shutdown,

    /// Confirms one successful lifecycle command.
    Ok,

    /// Returns opaque invocation output.
    Result {
        /// Opaque canonical Bridge Value v1 output.
        output: Vec<u8>,
    },

    /// Reports one child-side command failure.
    Error {
        /// Stable lower-case kebab error code.
        code: String,
        /// Bounded diagnostic message.
        message: String,
    },

    /// Invokes one launcher Bridge operation from the child.
    BridgeInvoke {
        /// Canonical launcher Bridge operation.
        operation: String,
        /// Opaque canonical Bridge Value v1 input.
        input: Vec<u8>,
    },

    /// Retains one generation-safe launcher object handle.
    RetainHandle {
        /// Positive launcher object identifier.
        object_id: u64,
        /// Positive handle generation.
        generation: u64,
    },

    /// Releases one generation-safe launcher object handle.
    ReleaseHandle {
        /// Positive launcher object identifier.
        object_id: u64,
        /// Positive handle generation.
        generation: u64,
    },

    /// Returns one successful parent-side Bridge callback result.
    CallbackResult {
        /// Opaque canonical Bridge Value v1 output.
        output: Vec<u8>,
    },

    /// Reports one redacted parent-side Bridge callback failure.
    CallbackError {
        /// Stable lower-case kebab error code.
        code: String,
    },
}

impl MessageBody {
    /// Returns whether this kind belongs to the child callback ID space.
    const fn uses_even_request_id(&self) -> bool {
        matches!(
            self,
            Self::BridgeInvoke { .. }
                | Self::RetainHandle { .. }
                | Self::ReleaseHandle { .. }
                | Self::CallbackResult { .. }
                | Self::CallbackError { .. }
        )
    }

    /// Validates kind-specific scalar constraints before encoding or dispatch.
    fn validate(&self) -> Result<(), ProtocolError> {
        match self {
            Self::Hello
            | Self::Enable
            | Self::Disable
            | Self::Shutdown
            | Self::Ok
            | Self::Result { .. }
            | Self::CallbackResult { .. } => Ok(()),
            Self::Load {
                package_root,
                entrypoint,
                plugin_id,
                session,
            } => {
                require_text(package_root, "package root must not be blank")?;
                require_text(entrypoint, "entrypoint must not be blank")?;
                require_positive(*plugin_id, "plugin ID must be positive")?;
                require_positive(*session, "session ID must be positive")
            }
            Self::Invoke {
                operation,
                callback_id,
                ..
            } => {
                require_text(operation, "operation must not be blank")?;
                require_nonnegative(*callback_id, "callback ID exceeds signed-64 range")
            }
            Self::Error { code, message } => {
                require_code(code)?;
                require_text(message, "error message must not be blank")?;
                if message.len() > 4096 {
                    return Err(invalid("error message exceeds 4096 UTF-8 bytes"));
                }
                Ok(())
            }
            Self::BridgeInvoke { operation, .. } => {
                require_text(operation, "Bridge operation must not be blank")
            }
            Self::RetainHandle {
                object_id,
                generation,
            }
            | Self::ReleaseHandle {
                object_id,
                generation,
            } => {
                require_positive(*object_id, "object ID must be positive")?;
                require_positive(*generation, "handle generation must be positive")
            }
            Self::CallbackError { code } => require_code(code),
        }
    }

    /// Encodes one body into its canonical kind and exact payload map.
    fn to_value(&self) -> Result<(&'static str, Value), ProtocolError> {
        self.validate()?;
        let encoded = match self {
            Self::Hello => ("hello", map(Vec::new())),
            Self::Load {
                package_root,
                entrypoint,
                plugin_id,
                session,
            } => (
                "load",
                map(vec![
                    ("packageRoot", Value::String(package_root.clone())),
                    ("entrypoint", Value::String(entrypoint.clone())),
                    ("pluginId", id_value(*plugin_id)?),
                    ("session", id_value(*session)?),
                ]),
            ),
            Self::Enable => ("enable", map(Vec::new())),
            Self::Invoke {
                operation,
                input,
                callback_id,
            } => (
                "invoke",
                map(vec![
                    ("operation", Value::String(operation.clone())),
                    ("input", Value::Bytes(input.clone())),
                    ("callbackId", id_value(*callback_id)?),
                ]),
            ),
            Self::Disable => ("disable", map(Vec::new())),
            Self::Shutdown => ("shutdown", map(Vec::new())),
            Self::Ok => ("ok", map(Vec::new())),
            Self::Result { output } => (
                "result",
                map(vec![("output", Value::Bytes(output.clone()))]),
            ),
            Self::Error { code, message } => (
                "error",
                map(vec![
                    ("code", Value::String(code.clone())),
                    ("message", Value::String(message.clone())),
                ]),
            ),
            Self::BridgeInvoke { operation, input } => (
                "bridge-invoke",
                map(vec![
                    ("operation", Value::String(operation.clone())),
                    ("input", Value::Bytes(input.clone())),
                ]),
            ),
            Self::RetainHandle {
                object_id,
                generation,
            } => ("retain-handle", handle_payload(*object_id, *generation)?),
            Self::ReleaseHandle {
                object_id,
                generation,
            } => ("release-handle", handle_payload(*object_id, *generation)?),
            Self::CallbackResult { output } => (
                "callback-result",
                map(vec![("output", Value::Bytes(output.clone()))]),
            ),
            Self::CallbackError { code } => (
                "callback-error",
                map(vec![("code", Value::String(code.clone()))]),
            ),
        };
        Ok(encoded)
    }

    /// Decodes one exact kind-specific payload.
    fn from_value(kind: &str, payload: Value) -> Result<Self, ProtocolError> {
        let body = match kind {
            "hello" => {
                exact_map(payload, &[])?;
                Self::Hello
            }
            "load" => {
                let fields = exact_map(
                    payload,
                    &["packageRoot", "entrypoint", "pluginId", "session"],
                )?;
                Self::Load {
                    package_root: owned_string(field(&fields, "packageRoot")?)?,
                    entrypoint: owned_string(field(&fields, "entrypoint")?)?,
                    plugin_id: positive_id(field(&fields, "pluginId")?)?,
                    session: positive_id(field(&fields, "session")?)?,
                }
            }
            "enable" => {
                exact_map(payload, &[])?;
                Self::Enable
            }
            "invoke" => {
                let fields = exact_map(payload, &["operation", "input", "callbackId"])?;
                Self::Invoke {
                    operation: owned_string(field(&fields, "operation")?)?,
                    input: bytes(field(&fields, "input")?)?,
                    callback_id: nonnegative_id(field(&fields, "callbackId")?)?,
                }
            }
            "disable" => {
                exact_map(payload, &[])?;
                Self::Disable
            }
            "shutdown" => {
                exact_map(payload, &[])?;
                Self::Shutdown
            }
            "ok" => {
                exact_map(payload, &[])?;
                Self::Ok
            }
            "result" => {
                let fields = exact_map(payload, &["output"])?;
                Self::Result {
                    output: bytes(field(&fields, "output")?)?,
                }
            }
            "error" => {
                let fields = exact_map(payload, &["code", "message"])?;
                Self::Error {
                    code: owned_string(field(&fields, "code")?)?,
                    message: owned_string(field(&fields, "message")?)?,
                }
            }
            "bridge-invoke" => {
                let fields = exact_map(payload, &["operation", "input"])?;
                Self::BridgeInvoke {
                    operation: owned_string(field(&fields, "operation")?)?,
                    input: bytes(field(&fields, "input")?)?,
                }
            }
            "retain-handle" => decode_handle(payload, true)?,
            "release-handle" => decode_handle(payload, false)?,
            "callback-result" => {
                let fields = exact_map(payload, &["output"])?;
                Self::CallbackResult {
                    output: bytes(field(&fields, "output")?)?,
                }
            }
            "callback-error" => {
                let fields = exact_map(payload, &["code"])?;
                Self::CallbackError {
                    code: owned_string(field(&fields, "code")?)?,
                }
            }
            _ => return Err(invalid("unsupported message kind")),
        };
        body.validate()?;
        Ok(body)
    }
}

/// Failure while validating or transporting one isolated Host frame.
#[derive(Debug)]
pub enum ProtocolError {
    /// Underlying stream operation failed.
    Io(io::Error),

    /// Frame or message violated the frozen protocol.
    InvalidData(&'static str),
}

impl Display for ProtocolError {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Io(error) => write!(formatter, "isolated Host I/O failed: {error}"),
            Self::InvalidData(message) => {
                write!(formatter, "invalid isolated Host protocol: {message}")
            }
        }
    }
}

impl std::error::Error for ProtocolError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Io(error) => Some(error),
            Self::InvalidData(_) => None,
        }
    }
}

impl From<io::Error> for ProtocolError {
    fn from(error: io::Error) -> Self {
        Self::Io(error)
    }
}

/// Reads one length-prefixed message, returning `None` only for clean EOF before a header.
pub fn read_frame(reader: &mut impl Read) -> Result<Option<Message>, ProtocolError> {
    let mut header = [0_u8; 4];
    loop {
        match reader.read(&mut header[..1]) {
            Ok(0) => return Ok(None),
            Ok(1) => break,
            Ok(_) => unreachable!("one-byte read returned more than one byte"),
            Err(error) if error.kind() == io::ErrorKind::Interrupted => continue,
            Err(error) => return Err(error.into()),
        }
    }
    read_exact(reader, &mut header[1..], "truncated frame header")?;
    let length = u32::from_be_bytes(header);
    if length == 0 || length > MAX_FRAME_BYTES {
        return Err(invalid("frame length is outside bounds"));
    }
    let mut body =
        vec![0_u8; usize::try_from(length).map_err(|_| invalid("frame length overflow"))?];
    read_exact(reader, &mut body, "truncated frame body")?;
    Message::from_wire(&body).map(Some)
}

/// Writes one length-prefixed canonical message.
pub fn write_frame(writer: &mut impl Write, message: &Message) -> Result<(), ProtocolError> {
    let body = message.to_wire()?;
    let length = u32::try_from(body.len()).map_err(|_| invalid("frame length overflow"))?;
    if length == 0 || length > MAX_FRAME_BYTES {
        return Err(invalid("frame length is outside bounds"));
    }
    writer.write_all(&length.to_be_bytes())?;
    writer.write_all(&body)?;
    Ok(())
}

/// Reads a required remainder while classifying partial EOF as protocol truncation.
fn read_exact(
    reader: &mut impl Read,
    output: &mut [u8],
    truncated: &'static str,
) -> Result<(), ProtocolError> {
    reader.read_exact(output).map_err(|error| {
        if error.kind() == io::ErrorKind::UnexpectedEof {
            invalid(truncated)
        } else {
            error.into()
        }
    })
}

/// Requires one map with exactly the named fields.
fn exact_map(value: Value, fields: &[&str]) -> Result<Vec<(String, Value)>, ProtocolError> {
    let Value::Map(entries) = value else {
        return Err(invalid("message value must be a map"));
    };
    if entries.len() != fields.len()
        || !fields
            .iter()
            .all(|name| entries.iter().any(|(key, _)| key == name))
    {
        return Err(invalid("message map has unknown or missing fields"));
    }
    Ok(entries)
}

/// Returns one already validated field.
fn field<'a>(entries: &'a [(String, Value)], name: &str) -> Result<&'a Value, ProtocolError> {
    entries
        .iter()
        .find_map(|(key, value)| (key == name).then_some(value))
        .ok_or_else(|| invalid("message map is missing a required field"))
}

/// Requires one signed integer.
fn integer(value: &Value) -> Result<i64, ProtocolError> {
    match value {
        Value::Integer(integer) => Ok(*integer),
        _ => Err(invalid("message field must be an integer")),
    }
}

/// Requires one positive signed-64-compatible identifier.
fn positive_id(value: &Value) -> Result<u64, ProtocolError> {
    let identifier = integer(value)?;
    u64::try_from(identifier)
        .ok()
        .filter(|identifier| *identifier > 0)
        .ok_or_else(|| invalid("message identifier must be positive"))
}

/// Requires one nonnegative signed-64-compatible identifier.
fn nonnegative_id(value: &Value) -> Result<u64, ProtocolError> {
    u64::try_from(integer(value)?).map_err(|_| invalid("message identifier must be nonnegative"))
}

/// Requires one string.
fn string(value: &Value) -> Result<&str, ProtocolError> {
    match value {
        Value::String(string) => Ok(string),
        _ => Err(invalid("message field must be a string")),
    }
}

/// Copies one required string.
fn owned_string(value: &Value) -> Result<String, ProtocolError> {
    string(value).map(ToOwned::to_owned)
}

/// Copies one required opaque byte sequence.
fn bytes(value: &Value) -> Result<Vec<u8>, ProtocolError> {
    match value {
        Value::Bytes(bytes) => Ok(bytes.clone()),
        _ => Err(invalid("message field must be bytes")),
    }
}

/// Wraps insertion-ordered protocol fields as one Bridge map.
fn map(fields: Vec<(&str, Value)>) -> Value {
    Value::Map(
        fields
            .into_iter()
            .map(|(name, value)| (name.to_owned(), value))
            .collect(),
    )
}

/// Encodes one signed-64-compatible protocol identifier.
fn id_value(identifier: u64) -> Result<Value, ProtocolError> {
    i64::try_from(identifier)
        .map(Value::Integer)
        .map_err(|_| invalid("message identifier exceeds signed-64 range"))
}

/// Builds the shared exact object-handle payload.
fn handle_payload(object_id: u64, generation: u64) -> Result<Value, ProtocolError> {
    Ok(map(vec![
        ("objectId", id_value(object_id)?),
        ("generation", id_value(generation)?),
    ]))
}

/// Decodes one exact object-handle payload into its retain or release kind.
fn decode_handle(payload: Value, retain: bool) -> Result<MessageBody, ProtocolError> {
    let fields = exact_map(payload, &["objectId", "generation"])?;
    let object_id = positive_id(field(&fields, "objectId")?)?;
    let generation = positive_id(field(&fields, "generation")?)?;
    Ok(if retain {
        MessageBody::RetainHandle {
            object_id,
            generation,
        }
    } else {
        MessageBody::ReleaseHandle {
            object_id,
            generation,
        }
    })
}

/// Requires one nonblank protocol string.
fn require_text(value: &str, message: &'static str) -> Result<(), ProtocolError> {
    if value.trim().is_empty() {
        Err(invalid(message))
    } else {
        Ok(())
    }
}

/// Requires one positive signed-64-compatible identifier.
fn require_positive(identifier: u64, message: &'static str) -> Result<(), ProtocolError> {
    if identifier == 0 || identifier > i64::MAX as u64 {
        Err(invalid(message))
    } else {
        Ok(())
    }
}

/// Requires one nonnegative signed-64-compatible identifier.
fn require_nonnegative(identifier: u64, message: &'static str) -> Result<(), ProtocolError> {
    if identifier > i64::MAX as u64 {
        Err(invalid(message))
    } else {
        Ok(())
    }
}

/// Requires one stable lower-case kebab error code.
fn require_code(code: &str) -> Result<(), ProtocolError> {
    let mut bytes = code.bytes();
    let Some(first) = bytes.next() else {
        return Err(invalid("error code must be lower-case kebab text"));
    };
    if !first.is_ascii_lowercase() {
        return Err(invalid("error code must be lower-case kebab text"));
    }
    let mut separator = false;
    for byte in bytes {
        if byte == b'-' {
            if separator {
                return Err(invalid("error code must be lower-case kebab text"));
            }
            separator = true;
        } else if byte.is_ascii_lowercase() || byte.is_ascii_digit() {
            separator = false;
        } else {
            return Err(invalid("error code must be lower-case kebab text"));
        }
    }
    if separator || code.len() > 128 {
        Err(invalid("error code must be lower-case kebab text"))
    } else {
        Ok(())
    }
}

/// Creates one compact protocol validation failure.
const fn invalid(message: &'static str) -> ProtocolError {
    ProtocolError::InvalidData(message)
}
