#![deny(unsafe_op_in_unsafe_fn)]
#![deny(missing_docs)]

//! Stable, versioned C ABI shared by HMCL runtime hosts and Rust plugin payloads.
//!
//! All structures are prefix-versioned. Consumers must check `struct_size` before reading a
//! field and ignore fields beyond the prefix they understand. Byte buffers are always released
//! through the allocator table that created them; Rust allocator ownership never crosses the ABI.

use std::ffi::c_void;
use std::mem::size_of;

const TABLE_HEADER_SIZE: usize = 2 * size_of::<u32>();

/// Version number implemented by the v1 host and plugin tables.
pub const HMCL_BRIDGE_ABI_V1: u32 = 1;

/// Result codes returned across the runtime ABI boundary.
#[repr(i32)]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum HmclStatus {
    /// The operation completed successfully.
    Ok = 0,
    /// A pointer, identifier, or other argument was invalid.
    InvalidArgument = 1,
    /// The requested ABI version is not supported.
    UnsupportedAbi = 2,
    /// A supplied versioned table or output buffer is shorter than the required prefix.
    BufferTooSmall = 3,
    /// A host-owned operation failed.
    HostError = 4,
    /// A plugin-owned operation failed.
    PluginError = 5,
}

/// A borrowed byte sequence whose storage remains owned by the caller.
#[repr(C)]
#[derive(Clone, Copy, Debug)]
pub struct HmclSlice {
    /// First byte, or null when `len` is zero.
    pub data: *const u8,
    /// Number of readable bytes beginning at `data`.
    pub len: u64,
}

/// A byte buffer allocated by one side of the ABI.
///
/// The allocator that produced the buffer retains allocator ownership. The receiver must pass the
/// complete value to that allocator's matching release callback exactly once and must not use the
/// bytes afterward.
#[repr(C)]
#[derive(Debug)]
pub struct HmclOwnedBuffer {
    /// First writable byte, or null when `capacity` is zero.
    pub data: *mut u8,
    /// Number of initialized bytes beginning at `data`.
    pub len: u64,
    /// Number of allocated bytes beginning at `data`.
    pub capacity: u64,
}

macro_rules! opaque_id {
    ($name:ident, $doc:literal) => {
        #[doc = $doc]
        #[repr(C)]
        #[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
        pub struct $name {
            value: u64,
        }

        impl $name {
            /// Creates an opaque identifier from its transport representation.
            #[must_use]
            pub const fn from_raw(raw: u64) -> Self {
                Self { value: raw }
            }

            /// Returns the identifier's transport representation.
            #[must_use]
            pub const fn into_raw(self) -> u64 {
                self.value
            }
        }
    };
}

opaque_id!(
    HmclPluginId,
    "Opaque identifier for one loaded plugin instance."
);
opaque_id!(
    HmclCapabilityToken,
    "Opaque token authorizing operations for one plugin capability session."
);
opaque_id!(
    HmclHandleId,
    "Opaque identifier for a host-managed Bridge handle."
);
opaque_id!(
    HmclCallbackId,
    "Opaque identifier correlating an asynchronous request with its completion."
);

/// Host callback that allocates a buffer using the host allocator.
///
/// # Safety
///
/// `context` must be the context from the same host table. `out_buffer` must be non-null, aligned,
/// and writable for one [`HmclOwnedBuffer`]. A successful result initializes it. The resulting
/// buffer must be passed to the matching [`HmclReleaseBufferFn`] exactly once.
pub type HmclAllocateFn = unsafe extern "C" fn(
    context: *mut c_void,
    length: u64,
    out_buffer: *mut HmclOwnedBuffer,
) -> HmclStatus;

/// Host callback that releases a host-allocated buffer exactly once.
///
/// # Safety
///
/// `context` must be the context from the same host table. `buffer` must be non-null, aligned, and
/// point to a buffer returned by that table's [`HmclAllocateFn`] which has not already been released.
pub type HmclReleaseBufferFn =
    unsafe extern "C" fn(context: *mut c_void, buffer: *mut HmclOwnedBuffer) -> HmclStatus;

/// Host callback that records a UTF-8 diagnostic message.
///
/// # Safety
///
/// `context` must be the context from the same host table. `message` must describe readable bytes
/// for the duration of the call, and those bytes must contain valid UTF-8.
pub type HmclLogFn =
    unsafe extern "C" fn(context: *mut c_void, level: u32, message: HmclSlice) -> HmclStatus;

/// Host callback that dispatches a Bridge operation for a plugin capability session.
///
/// # Safety
///
/// `context` must be the context from the same host table. All identifiers must originate from the
/// host, `operation` and `input` must describe readable bytes for the call, and `out_buffer` must be
/// non-null, aligned, and writable for one [`HmclOwnedBuffer`]. A successful output is host-owned
/// and must be released with the matching [`HmclReleaseBufferFn`] exactly once.
pub type HmclHostInvokeFn = unsafe extern "C" fn(
    context: *mut c_void,
    plugin: HmclPluginId,
    token: HmclCapabilityToken,
    operation: HmclSlice,
    input: HmclSlice,
    out_buffer: *mut HmclOwnedBuffer,
) -> HmclStatus;

/// Host callback that acquires one additional reference to a Bridge handle.
///
/// # Safety
///
/// `context` must be the context from the same host table and `handle` must identify a live handle.
pub type HmclRetainHandleFn =
    unsafe extern "C" fn(context: *mut c_void, handle: HmclHandleId) -> HmclStatus;

/// Host callback that releases one reference to a Bridge handle.
///
/// # Safety
///
/// `context` must be the context from the same host table and `handle` must identify a live handle
/// reference owned by the caller. Each acquired reference may be released exactly once.
pub type HmclReleaseHandleFn =
    unsafe extern "C" fn(context: *mut c_void, handle: HmclHandleId) -> HmclStatus;

/// Version-one services supplied by the HMCL host to a plugin payload.
///
/// Callback slots are nullable so a producer can explicitly omit an optional service. A consumer
/// must test a slot before calling it and report an appropriate status when it is absent.
#[repr(C)]
#[derive(Clone, Copy, Debug)]
pub struct HmclHostApiV1 {
    /// Number of bytes available in this table.
    pub struct_size: u32,
    /// ABI version implemented by this table.
    pub abi_version: u32,
    /// Opaque host state passed unchanged to every host callback.
    pub context: *mut c_void,
    /// Optional host-owned allocation callback.
    pub allocate: Option<HmclAllocateFn>,
    /// Optional matching callback for host-owned allocations.
    pub release_buffer: Option<HmclReleaseBufferFn>,
    /// Optional diagnostic callback.
    pub log: Option<HmclLogFn>,
    /// Optional synchronous Bridge dispatch callback.
    pub invoke: Option<HmclHostInvokeFn>,
    /// Optional Bridge handle retain callback.
    pub retain_handle: Option<HmclRetainHandleFn>,
    /// Optional Bridge handle release callback.
    pub release_handle: Option<HmclReleaseHandleFn>,
}

impl HmclHostApiV1 {
    /// An absent host table useful for initialization before negotiation.
    pub const EMPTY: Self = Self {
        struct_size: 0,
        abi_version: 0,
        context: std::ptr::null_mut(),
        allocate: None,
        release_buffer: None,
        log: None,
        invoke: None,
        retain_handle: None,
        release_handle: None,
    };

    /// Returns a v1 table header with every optional callback absent.
    #[must_use]
    pub const fn with_required_prefix() -> Self {
        Self {
            struct_size: size_of::<Self>() as u32,
            abi_version: HMCL_BRIDGE_ABI_V1,
            ..Self::EMPTY
        }
    }
}

/// Plugin callback that starts one plugin capability session.
///
/// # Safety
///
/// `context` must be the context returned in the same plugin table. `host` must remain non-null,
/// aligned, and readable for its advertised prefix throughout the call. The identifiers must have
/// been issued by that host.
pub type HmclPluginInitializeFn = unsafe extern "C" fn(
    context: *mut c_void,
    host: *const HmclHostApiV1,
    plugin: HmclPluginId,
    token: HmclCapabilityToken,
) -> HmclStatus;

/// Plugin callback that invokes a named payload operation.
///
/// # Safety
///
/// `context` must be the context returned in the same plugin table. `operation` and `input` must
/// describe readable bytes for the duration of the call. `out_buffer` must be non-null, aligned,
/// and writable for one [`HmclOwnedBuffer`]. The plugin must obtain successful output storage from
/// the host's [`HmclAllocateFn`]; the host remains its owner and calls the matching
/// [`HmclReleaseBufferFn`] exactly once.
pub type HmclPluginInvokeFn = unsafe extern "C" fn(
    context: *mut c_void,
    operation: HmclSlice,
    input: HmclSlice,
    callback: HmclCallbackId,
    out_buffer: *mut HmclOwnedBuffer,
) -> HmclStatus;

/// Plugin callback that ends a plugin capability session.
///
/// # Safety
///
/// `context` must be the context returned in the same plugin table and `plugin` must identify a
/// session previously accepted by [`HmclPluginInitializeFn`] that has not already been shut down.
pub type HmclPluginShutdownFn =
    unsafe extern "C" fn(context: *mut c_void, plugin: HmclPluginId) -> HmclStatus;

/// Version-one services supplied by a Rust plugin payload to the host.
#[repr(C)]
#[derive(Clone, Copy, Debug)]
pub struct HmclPluginApiV1 {
    /// Number of bytes implemented by the plugin table.
    pub struct_size: u32,
    /// ABI version implemented by this table.
    pub abi_version: u32,
    /// Opaque plugin state passed unchanged to every plugin callback.
    pub context: *mut c_void,
    /// Optional plugin initialization callback.
    pub initialize: Option<HmclPluginInitializeFn>,
    /// Optional payload dispatch callback.
    pub invoke: Option<HmclPluginInvokeFn>,
    /// Optional plugin shutdown callback.
    pub shutdown: Option<HmclPluginShutdownFn>,
}

impl HmclPluginApiV1 {
    /// An absent plugin table useful for initializing query output storage.
    pub const EMPTY: Self = Self {
        struct_size: 0,
        abi_version: 0,
        context: std::ptr::null_mut(),
        initialize: None,
        invoke: None,
        shutdown: None,
    };

    /// Returns a v1 table header with every optional callback absent.
    #[must_use]
    pub const fn with_required_prefix() -> Self {
        Self {
            struct_size: size_of::<Self>() as u32,
            abi_version: HMCL_BRIDGE_ABI_V1,
            ..Self::EMPTY
        }
    }
}

#[derive(Clone, Copy, Debug)]
struct TableHeader {
    struct_size: u32,
    abi_version: u32,
}

fn decode_struct_size(prefix: &[u8]) -> Result<u32, HmclStatus> {
    let encoded = prefix
        .get(..size_of::<u32>())
        .ok_or(HmclStatus::BufferTooSmall)?;
    let encoded =
        <[u8; size_of::<u32>()]>::try_from(encoded).map_err(|_| HmclStatus::BufferTooSmall)?;
    Ok(u32::from_ne_bytes(encoded))
}

fn decode_table_header(prefix: &[u8]) -> Result<TableHeader, HmclStatus> {
    let struct_size = decode_struct_size(prefix)?;
    if struct_size < TABLE_HEADER_SIZE as u32 {
        return Err(HmclStatus::BufferTooSmall);
    }

    let encoded_version = prefix
        .get(size_of::<u32>()..TABLE_HEADER_SIZE)
        .ok_or(HmclStatus::BufferTooSmall)?;
    let encoded_version = <[u8; size_of::<u32>()]>::try_from(encoded_version)
        .map_err(|_| HmclStatus::BufferTooSmall)?;
    Ok(TableHeader {
        struct_size,
        abi_version: u32::from_ne_bytes(encoded_version),
    })
}

// SAFETY: `table` must be aligned and readable for its first `u32`. When that value advertises a
// prefix of at least eight bytes, `table` must be readable for those eight bytes.
unsafe fn read_table_header(table: *const u8) -> Result<TableHeader, HmclStatus> {
    // SAFETY: The caller guarantees that the first fixed-width field is readable.
    let size_prefix = unsafe { std::slice::from_raw_parts(table, size_of::<u32>()) };
    let struct_size = decode_struct_size(size_prefix)?;
    if struct_size < TABLE_HEADER_SIZE as u32 {
        return Err(HmclStatus::BufferTooSmall);
    }

    // SAFETY: A size of at least `TABLE_HEADER_SIZE` advertises a readable complete table header.
    let header_prefix = unsafe { std::slice::from_raw_parts(table, TABLE_HEADER_SIZE) };
    decode_table_header(header_prefix)
}

/// Negotiates the version-one host and plugin function-table prefixes.
///
/// The host initializes `out_plugin.struct_size` to its writable capacity and
/// `out_plugin.abi_version` to the requested version. On success, this reference implementation
/// installs an empty v1 plugin table. Concrete payload crates can provide callbacks by building on
/// the same table contract.
///
/// # Safety
///
/// `host` must be non-null, aligned, and readable for its first `u32` and for the complete prefix
/// advertised by that `struct_size`. `out_plugin` must be non-null, aligned, and readable and
/// writable under the same staged rule. The two allocations must not overlap. Both pointers must
/// remain valid for the duration of this call.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn hmcl_plugin_query_v1(
    host: *const HmclHostApiV1,
    out_plugin: *mut HmclPluginApiV1,
) -> HmclStatus {
    if host.is_null() || out_plugin.is_null() || !host.is_aligned() || !out_plugin.is_aligned() {
        return HmclStatus::InvalidArgument;
    }

    // SAFETY: Null and alignment checks are complete. The function contract guarantees each first
    // field and any subsequently advertised header bytes are readable.
    let host_header = match unsafe { read_table_header(host.cast()) } {
        Ok(header) => header,
        Err(status) => return status,
    };
    // SAFETY: The same staged-read guarantees apply independently to the output table.
    let output_header = match unsafe { read_table_header(out_plugin.cast()) } {
        Ok(header) => header,
        Err(status) => return status,
    };

    if host_header.struct_size < size_of::<HmclHostApiV1>() as u32
        || output_header.struct_size < size_of::<HmclPluginApiV1>() as u32
    {
        return HmclStatus::BufferTooSmall;
    }
    if host_header.abi_version != HMCL_BRIDGE_ABI_V1
        || output_header.abi_version != HMCL_BRIDGE_ABI_V1
    {
        return HmclStatus::UnsupportedAbi;
    }

    // SAFETY: Capacity validation above proves that the caller advertised writable storage for the
    // entire v1 prefix. Writing one prefix deliberately leaves unknown trailing fields untouched.
    unsafe { out_plugin.write(HmclPluginApiV1::with_required_prefix()) };
    HmclStatus::Ok
}

#[cfg(test)]
mod tests {
    use super::HmclStatus;

    #[test]
    fn header_decoder_rejects_an_exact_four_byte_prefix() {
        let prefix = 4_u32.to_ne_bytes();

        assert!(matches!(
            super::decode_table_header(&prefix),
            Err(HmclStatus::BufferTooSmall)
        ));
    }
}
