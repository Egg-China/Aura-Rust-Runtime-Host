use crate::{Error, ErrorCode};
use std::collections::HashSet;

const MAX_DEPTH: usize = 31;
const MAX_STRING_BYTES: usize = 1024 * 1024;
const MAX_BYTE_BYTES: usize = 16 * 1024 * 1024;
const MAX_CONTAINER_ENTRIES: usize = 1024;
const MAX_TOTAL_CONTENT_BYTES: usize = 16 * 1024 * 1024;
const MAX_TOTAL_VALUES: usize = 65_536;

const TAG_NULL: u8 = 0;
const TAG_BOOL: u8 = 1;
const TAG_INTEGER: u8 = 2;
const TAG_FLOAT: u8 = 3;
const TAG_STRING: u8 = 4;
const TAG_BYTES: u8 = 5;
const TAG_ARRAY: u8 = 6;
const TAG_MAP: u8 = 7;
const TAG_HANDLE: u8 = 8;
const TAG_ERROR: u8 = 9;

/// A validated, untyped host-managed object reference.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct HandleValue {
    object_id: u64,
    generation: u64,
    type_name: String,
}

impl HandleValue {
    /// Creates a handle value after validating positive identifiers and its canonical type name.
    pub fn new(
        object_id: u64,
        generation: u64,
        type_name: impl Into<String>,
    ) -> Result<Self, Error> {
        let type_name = type_name.into();
        if object_id == 0
            || generation == 0
            || object_id > i64::MAX as u64
            || generation > i64::MAX as u64
            || !is_valid_type_name(&type_name)
            || type_name.len() > 128
        {
            return Err(invalid_argument());
        }
        Ok(Self {
            object_id,
            generation,
            type_name,
        })
    }

    /// Returns the host object identifier.
    #[must_use]
    pub const fn object_id(&self) -> u64 {
        self.object_id
    }

    /// Returns the host generation used to detect stale references.
    #[must_use]
    pub const fn generation(&self) -> u64 {
        self.generation
    }

    /// Returns the canonical object type name.
    #[must_use]
    pub fn type_name(&self) -> &str {
        &self.type_name
    }
}

/// A deterministic Bridge Value v1 payload.
#[derive(Clone, Debug, PartialEq)]
pub enum Value {
    /// The absence of a value.
    Null,
    /// A Boolean value.
    Bool(bool),
    /// A signed 64-bit integer.
    Integer(i64),
    /// A finite IEEE-754 double-precision number.
    Float(f64),
    /// A UTF-8 string.
    String(String),
    /// An opaque byte sequence.
    Bytes(Vec<u8>),
    /// An ordered sequence of values.
    Array(Vec<Value>),
    /// An insertion-ordered map with unique UTF-8 keys.
    Map(Vec<(String, Value)>),
    /// A host-managed object reference.
    Handle(HandleValue),
    /// A redacted error category.
    Error(Error),
}

impl Value {
    /// Encodes this value using the canonical, explicitly tagged Bridge Value v1 MessagePack form.
    pub fn to_wire(&self) -> Result<Vec<u8>, Error> {
        let mut encoder = Encoder::default();
        encoder.value(self, 0)?;
        Ok(encoder.output)
    }

    /// Decodes one complete canonical Bridge Value v1 payload.
    pub fn from_wire(input: &[u8]) -> Result<Self, Error> {
        let mut decoder = Decoder::new(input);
        let value = decoder.value(0)?;
        if decoder.offset != input.len() {
            return Err(invalid_result());
        }
        Ok(value)
    }
}

fn is_valid_type_name(value: &str) -> bool {
    if value.is_empty() || value.len() > 128 {
        return false;
    }
    let mut bytes = value.bytes();
    if !bytes.next().is_some_and(|byte| byte.is_ascii_lowercase()) {
        return false;
    }
    let mut separator = false;
    for byte in bytes {
        if matches!(byte, b'.' | b'-') {
            if separator {
                return false;
            }
            separator = true;
        } else if byte.is_ascii_lowercase() || byte.is_ascii_digit() {
            separator = false;
        } else {
            return false;
        }
    }
    !separator
}

fn invalid_argument() -> Error {
    Error::new(ErrorCode::InvalidArgument)
}

fn invalid_result() -> Error {
    Error::new(ErrorCode::InvalidResult)
}

#[derive(Default)]
struct Encoder {
    output: Vec<u8>,
    content_bytes: usize,
    values: usize,
}

impl Encoder {
    fn value(&mut self, value: &Value, depth: usize) -> Result<(), Error> {
        if depth > MAX_DEPTH || self.values >= MAX_TOTAL_VALUES {
            return Err(invalid_argument());
        }
        self.values += 1;
        self.output.extend_from_slice(&[0x92, tag(value)]);
        match value {
            Value::Null => self.output.push(0xc0),
            Value::Bool(value) => self.output.push(if *value { 0xc3 } else { 0xc2 }),
            Value::Integer(value) => {
                self.output.push(0xd3);
                self.output.extend_from_slice(&value.to_be_bytes());
            }
            Value::Float(value) => {
                if !value.is_finite() {
                    return Err(invalid_argument());
                }
                self.output.push(0xcb);
                self.output
                    .extend_from_slice(&value.to_bits().to_be_bytes());
            }
            Value::String(value) => self.string(value, MAX_STRING_BYTES)?,
            Value::Bytes(value) => {
                self.add_content(value.len(), MAX_BYTE_BYTES)?;
                self.output.push(0xc6);
                self.write_length(value.len())?;
                self.output.extend_from_slice(value);
            }
            Value::Array(values) => {
                self.container_length(values.len())?;
                for value in values {
                    self.value(value, depth + 1)?;
                }
            }
            Value::Map(entries) => {
                self.container_length(entries.len())?;
                let mut keys = HashSet::with_capacity(entries.len());
                for (key, value) in entries {
                    if !keys.insert(key.as_str()) {
                        return Err(invalid_argument());
                    }
                    self.output.push(0x92);
                    self.string(key, MAX_STRING_BYTES)?;
                    self.value(value, depth + 1)?;
                }
            }
            Value::Handle(handle) => {
                HandleValue::new(
                    handle.object_id,
                    handle.generation,
                    handle.type_name.clone(),
                )?;
                self.output.push(0x93);
                self.uint64(handle.object_id);
                self.uint64(handle.generation);
                self.string(&handle.type_name, 128)?;
            }
            Value::Error(error) => self.string(error.code().wire_code(), 128)?,
        }
        Ok(())
    }

    fn add_content(&mut self, length: usize, individual_limit: usize) -> Result<(), Error> {
        if length > individual_limit {
            return Err(invalid_argument());
        }
        self.content_bytes = self
            .content_bytes
            .checked_add(length)
            .ok_or_else(invalid_argument)?;
        if self.content_bytes > MAX_TOTAL_CONTENT_BYTES {
            return Err(invalid_argument());
        }
        Ok(())
    }

    fn string(&mut self, value: &str, limit: usize) -> Result<(), Error> {
        self.add_content(value.len(), limit)?;
        self.output.push(0xdb);
        self.write_length(value.len())?;
        self.output.extend_from_slice(value.as_bytes());
        Ok(())
    }

    fn container_length(&mut self, length: usize) -> Result<(), Error> {
        if length > MAX_CONTAINER_ENTRIES {
            return Err(invalid_argument());
        }
        self.output.push(0xdd);
        self.write_length(length)
    }

    fn write_length(&mut self, length: usize) -> Result<(), Error> {
        let length = u32::try_from(length).map_err(|_| invalid_argument())?;
        self.output.extend_from_slice(&length.to_be_bytes());
        Ok(())
    }

    fn uint64(&mut self, value: u64) {
        self.output.push(0xcf);
        self.output.extend_from_slice(&value.to_be_bytes());
    }
}

fn tag(value: &Value) -> u8 {
    match value {
        Value::Null => TAG_NULL,
        Value::Bool(_) => TAG_BOOL,
        Value::Integer(_) => TAG_INTEGER,
        Value::Float(_) => TAG_FLOAT,
        Value::String(_) => TAG_STRING,
        Value::Bytes(_) => TAG_BYTES,
        Value::Array(_) => TAG_ARRAY,
        Value::Map(_) => TAG_MAP,
        Value::Handle(_) => TAG_HANDLE,
        Value::Error(_) => TAG_ERROR,
    }
}

struct Decoder<'a> {
    input: &'a [u8],
    offset: usize,
    content_bytes: usize,
    values: usize,
}

impl<'a> Decoder<'a> {
    fn new(input: &'a [u8]) -> Self {
        Self {
            input,
            offset: 0,
            content_bytes: 0,
            values: 0,
        }
    }

    fn value(&mut self, depth: usize) -> Result<Value, Error> {
        if depth > MAX_DEPTH || self.values >= MAX_TOTAL_VALUES {
            return Err(invalid_result());
        }
        self.values += 1;
        self.expect(0x92)?;
        let tag = self.byte()?;
        match tag {
            TAG_NULL => {
                self.expect(0xc0)?;
                Ok(Value::Null)
            }
            TAG_BOOL => match self.byte()? {
                0xc2 => Ok(Value::Bool(false)),
                0xc3 => Ok(Value::Bool(true)),
                _ => Err(invalid_result()),
            },
            TAG_INTEGER => {
                self.expect(0xd3)?;
                Ok(Value::Integer(i64::from_be_bytes(self.array()?)))
            }
            TAG_FLOAT => {
                self.expect(0xcb)?;
                let value = f64::from_bits(u64::from_be_bytes(self.array()?));
                if !value.is_finite() {
                    return Err(invalid_result());
                }
                Ok(Value::Float(value))
            }
            TAG_STRING => Ok(Value::String(self.string(MAX_STRING_BYTES)?)),
            TAG_BYTES => {
                self.expect(0xc6)?;
                let length = self.length()?;
                self.add_content(length, MAX_BYTE_BYTES)?;
                Ok(Value::Bytes(self.take(length)?.to_vec()))
            }
            TAG_ARRAY => {
                let length = self.container_length()?;
                let mut values = Vec::with_capacity(length);
                for _ in 0..length {
                    values.push(self.value(depth + 1)?);
                }
                Ok(Value::Array(values))
            }
            TAG_MAP => {
                let length = self.container_length()?;
                let mut entries = Vec::with_capacity(length);
                let mut keys = HashSet::with_capacity(length);
                for _ in 0..length {
                    self.expect(0x92)?;
                    let key = self.string(MAX_STRING_BYTES)?;
                    if !keys.insert(key.clone()) {
                        return Err(invalid_result());
                    }
                    entries.push((key, self.value(depth + 1)?));
                }
                Ok(Value::Map(entries))
            }
            TAG_HANDLE => {
                self.expect(0x93)?;
                let object_id = self.uint64()?;
                let generation = self.uint64()?;
                let type_name = self.string(128)?;
                HandleValue::new(object_id, generation, type_name)
                    .map(Value::Handle)
                    .map_err(|_| invalid_result())
            }
            TAG_ERROR => {
                let encoded = self.string(128)?;
                let code = ErrorCode::from_wire(&encoded).ok_or_else(invalid_result)?;
                Ok(Value::Error(Error::new(code)))
            }
            _ => Err(invalid_result()),
        }
    }

    fn byte(&mut self) -> Result<u8, Error> {
        let byte = *self.input.get(self.offset).ok_or_else(invalid_result)?;
        self.offset += 1;
        Ok(byte)
    }

    fn expect(&mut self, expected: u8) -> Result<(), Error> {
        if self.byte()? == expected {
            Ok(())
        } else {
            Err(invalid_result())
        }
    }

    fn take(&mut self, length: usize) -> Result<&'a [u8], Error> {
        let end = self.offset.checked_add(length).ok_or_else(invalid_result)?;
        let value = self
            .input
            .get(self.offset..end)
            .ok_or_else(invalid_result)?;
        self.offset = end;
        Ok(value)
    }

    fn array<const N: usize>(&mut self) -> Result<[u8; N], Error> {
        self.take(N)?.try_into().map_err(|_| invalid_result())
    }

    fn length(&mut self) -> Result<usize, Error> {
        Ok(u32::from_be_bytes(self.array()?) as usize)
    }

    fn string(&mut self, limit: usize) -> Result<String, Error> {
        self.expect(0xdb)?;
        let length = self.length()?;
        self.add_content(length, limit)?;
        let value = std::str::from_utf8(self.take(length)?).map_err(|_| invalid_result())?;
        Ok(value.to_owned())
    }

    fn container_length(&mut self) -> Result<usize, Error> {
        self.expect(0xdd)?;
        let length = self.length()?;
        if length > MAX_CONTAINER_ENTRIES {
            return Err(invalid_result());
        }
        Ok(length)
    }

    fn uint64(&mut self) -> Result<u64, Error> {
        self.expect(0xcf)?;
        Ok(u64::from_be_bytes(self.array()?))
    }

    fn add_content(&mut self, length: usize, individual_limit: usize) -> Result<(), Error> {
        if length > individual_limit {
            return Err(invalid_result());
        }
        self.content_bytes = self
            .content_bytes
            .checked_add(length)
            .ok_or_else(invalid_result)?;
        if self.content_bytes > MAX_TOTAL_CONTENT_BYTES {
            return Err(invalid_result());
        }
        Ok(())
    }
}
