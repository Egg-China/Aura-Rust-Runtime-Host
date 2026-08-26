#![deny(unsafe_op_in_unsafe_fn)]
#![deny(missing_docs)]

//! Stable, versioned C ABI shared by HMCL runtime hosts and Rust plugin payloads.
//!
//! All structures are prefix-versioned. Consumers must check `struct_size` before reading a
//! field and ignore fields beyond the prefix they understand. Byte buffers are always released
//! through the allocator table that created them; Rust allocator ownership never crosses the ABI.
//! ABI v1 supports only 64-bit targets, matching every platform supported by this runtime host.
//!
//! Every exported function and callback is a no-unwind boundary. Rust implementations must catch
//! panics before returning through it, and foreign implementations must catch their language's
//! exceptions. Allowing a panic, unwind, or foreign exception to cross any ABI call is a contract
//! violation.

#[cfg(not(target_pointer_width = "64"))]
compile_error!("HMCL runtime ABI v1 supports only 64-bit targets");

use std::ffi::c_void;
use std::mem::size_of;

const TABLE_HEADER_SIZE: usize = 2 * size_of::<u32>();

/// Version number implemented by the v1 host and plugin tables.
pub const HMCL_BRIDGE_ABI_V1: u32 = 1;

/// Frozen number of bytes required for the complete [`HmclHostApiV1`] prefix.
pub const HMCL_HOST_API_V1_PREFIX_SIZE: u32 = 64;

/// Frozen number of bytes required for the complete [`HmclPluginApiV1`] prefix.
pub const HMCL_PLUGIN_API_V1_PREFIX_SIZE: u32 = 40;

/// An integer result code returned across the runtime ABI boundary.
///
/// This transparent value accepts every possible `i32`, including status codes introduced by a
/// newer foreign implementation. Consumers compare recognized values with the named constants and
/// preserve unknown values when forwarding diagnostics.
#[repr(transparent)]
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct HmclStatus(i32);

#[allow(non_upper_case_globals)]
impl HmclStatus {
    /// The operation completed successfully.
    pub const Ok: Self = Self(0);
    /// A pointer, identifier, or other argument was invalid.
    pub const InvalidArgument: Self = Self(1);
    /// The requested ABI version is not supported.
    pub const UnsupportedAbi: Self = Self(2);
    /// A supplied versioned table or output buffer is shorter than the required prefix.
    pub const BufferTooSmall: Self = Self(3);
    /// A host-owned operation failed.
    pub const HostError: Self = Self(4);
    /// A plugin-owned operation failed.
    pub const PluginError: Self = Self(5);

    /// Preserves any status value received from foreign code.
    #[must_use]
    pub const fn from_raw(value: i32) -> Self {
        Self(value)
    }

    /// Returns the unchanged integer representation used by the C ABI.
    #[must_use]
    pub const fn into_raw(self) -> i32 {
        self.0
    }

    /// Returns whether this value is exactly [`Self::Ok`].
    #[must_use]
    pub const fn is_ok(self) -> bool {
        self.0 == Self::Ok.0
    }
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
/// A valid buffer always has `len <= capacity`. Zero capacity requires a null pointer and zero
/// length; nonzero capacity requires a non-null pointer. The host allocator that produced the
/// buffer owns its storage. The receiver must pass the complete value to that exact host table's
/// matching release callback exactly once and must not read, write, copy, or release it afterward.
/// If a producer violates the contract by returning a malformed descriptor with
/// [`HmclStatus::Ok`], the receiver must not dereference it, but still passes the exact unchanged
/// descriptor to the matching release callback once before reporting the malformed result.
/// This type deliberately does not implement `Copy` or `Clone` so Rust callers do not duplicate the
/// ownership token accidentally.
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

impl HmclOwnedBuffer {
    /// Canonical valid buffer with no allocation or ownership to release.
    pub const EMPTY: Self = Self {
        data: std::ptr::null_mut(),
        len: 0,
        capacity: 0,
    };

    /// Checks the pointer, length, and capacity invariants without dereferencing `data`.
    #[must_use]
    pub const fn has_valid_layout(&self) -> bool {
        if self.capacity == 0 {
            self.data.is_null() && self.len == 0
        } else {
            !self.data.is_null() && self.len <= self.capacity
        }
    }

    /// Checks a successful allocation response for a requested minimum capacity.
    ///
    /// A host allocator returns zero initialized bytes and capacity greater than or equal to the
    /// request. The caller initializes bytes and updates `len` before exposing payload data.
    #[must_use]
    pub const fn satisfies_allocation_request(&self, requested_capacity: u64) -> bool {
        self.has_valid_layout() && self.len == 0 && self.capacity >= requested_capacity
    }
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
/// Generation-safe identifier for one host-managed Bridge handle slot.
#[repr(C)]
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct HmclHandleId {
    id: u64,
    generation: u64,
}

impl HmclHandleId {
    /// Creates an exact slot and generation identity from its two transport fields.
    #[must_use]
    pub const fn from_raw(id: u64, generation: u64) -> Self {
        Self { id, generation }
    }

    /// Returns the unchanged slot and generation transport fields.
    #[must_use]
    pub const fn into_parts(self) -> (u64, u64) {
        (self.id, self.generation)
    }
}
opaque_id!(
    HmclCallbackId,
    "Opaque identifier correlating an asynchronous request with its completion."
);

/// Host callback that allocates a buffer using the host allocator.
///
/// Concurrency and unwinding follow the contract on [`HmclHostApiV1`].
///
/// # Safety
///
/// `length` is a minimum requested capacity, not an exact size. On [`HmclStatus::Ok`], the callback
/// replaces `out_buffer` with a valid host-owned buffer having `len == 0` and `capacity >= length`;
/// the caller must release it through this exact host table's [`HmclReleaseBufferFn`] once. The
/// ownership transfer occurs on `Ok` even if the returned descriptor violates those invariants. In
/// that case, the caller must not dereference it, must release the exact unchanged descriptor once,
/// and reports the producer contract violation as an invalid result. On any other status,
/// `out_buffer` remains byte-for-byte unchanged and no ownership transfers.
///
/// `context` must be the context from the same host table. `out_buffer` must be non-null, aligned,
/// and writable for one [`HmclOwnedBuffer`].
pub type HmclAllocateFn = unsafe extern "C" fn(
    context: *mut c_void,
    length: u64,
    out_buffer: *mut HmclOwnedBuffer,
) -> HmclStatus;

/// Host callback that consumes a host-allocated buffer exactly once.
///
/// Calling this function transfers the buffer ownership token back to its allocating host before
/// the call begins. The buffer is consumed regardless of the returned diagnostic status: the
/// caller must never inspect, use, or retry releasing it after the call. An error reports a release
/// diagnostic; it does not restore ownership to the caller.
/// Concurrency and unwinding follow the contract on [`HmclHostApiV1`].
///
/// # Safety
///
/// `context` must be the context from the same host table. `buffer` must be non-null, aligned, and
/// point to an unconsumed ownership token returned with [`HmclStatus::Ok`] by that table's
/// [`HmclAllocateFn`] or [`HmclHostInvokeFn`]. For a valid allocation, the caller may initialize
/// storage and update `len` within `capacity` as specified by [`HmclAllocateFn`]. If the producing
/// callback returned a malformed descriptor, the release callback must accept and consume that
/// exact unchanged descriptor; it must not require the caller to dereference invalid data. Callers
/// must never fabricate descriptors or pass tokens from another callback or table.
pub type HmclReleaseBufferFn =
    unsafe extern "C" fn(context: *mut c_void, buffer: *mut HmclOwnedBuffer) -> HmclStatus;

/// Host callback that records a UTF-8 diagnostic message.
///
/// Concurrency and unwinding follow the contract on [`HmclHostApiV1`].
///
/// # Safety
///
/// `context` must be the context from the same host table. `message` must describe readable bytes
/// for the duration of the call, and those bytes must contain valid UTF-8.
pub type HmclLogFn =
    unsafe extern "C" fn(context: *mut c_void, level: u32, message: HmclSlice) -> HmclStatus;

/// Host callback that dispatches a Bridge operation for a plugin capability session.
///
/// Concurrency and unwinding follow the contract on [`HmclHostApiV1`].
///
/// # Safety
///
/// `context` must be the context from the same host table. All identifiers must originate from the
/// host, `operation` and `input` must describe readable bytes for the call, and `out_buffer` must be
/// non-null, aligned, and writable for one [`HmclOwnedBuffer`]. On [`HmclStatus::Ok`], the callback
/// replaces it with a valid buffer owned by this exact host table; the caller releases that buffer
/// through the table's [`HmclReleaseBufferFn`] once. The ownership transfer occurs on `Ok` even if
/// the descriptor is malformed. The caller then releases the exact unchanged descriptor without
/// dereferencing it and reports the producer contract violation as an invalid result. On any other
/// status, `out_buffer` remains byte-for-byte unchanged and no ownership transfers.
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
/// Concurrency and unwinding follow the contract on [`HmclHostApiV1`].
///
/// # Safety
///
/// `context` must be the context from the same host table and `handle` must identify a live handle.
pub type HmclRetainHandleFn =
    unsafe extern "C" fn(context: *mut c_void, handle: HmclHandleId) -> HmclStatus;

/// Host callback that releases one reference to a Bridge handle.
///
/// Concurrency and unwinding follow the contract on [`HmclHostApiV1`].
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
///
/// A plugin may use the borrowed table only during the call that receives it. If callbacks or
/// `context` are needed later, the plugin copies no more than the compatible
/// [`HMCL_HOST_API_V1_PREFIX_SIZE`] bytes after validating `struct_size` and `abi_version`; it never
/// retains the table pointer. The host keeps every copied callback and `context` valid until the
/// plugin's shutdown callback has completed.
///
/// Host callbacks may be invoked concurrently from multiple threads and may be reentered from
/// another ABI callback unless an individual callback contract explicitly says otherwise. Host
/// implementations provide any synchronization required by their context. No callback may allow a
/// panic, unwind, or foreign exception to cross its ABI boundary.
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
            struct_size: HMCL_HOST_API_V1_PREFIX_SIZE,
            abi_version: HMCL_BRIDGE_ABI_V1,
            ..Self::EMPTY
        }
    }
}

const _: () = {
    assert!(size_of::<HmclHostApiV1>() == HMCL_HOST_API_V1_PREFIX_SIZE as usize);
    assert!(std::mem::align_of::<HmclHostApiV1>() == 8);
};

/// Plugin callback that starts one plugin capability session.
///
/// Initialization completes before the host admits ordinary plugin invocations for this session.
/// If the plugin needs host services afterward, it copies only the compatible V1 prefix and does
/// not retain `host`. Concurrency and unwinding otherwise follow [`HmclPluginApiV1`].
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
/// Concurrency and unwinding follow the contract on [`HmclPluginApiV1`].
///
/// # Safety
///
/// `context` must be the context returned in the same plugin table. `operation` and `input` must
/// describe readable bytes for the duration of the call. `out_buffer` must be non-null, aligned,
/// and writable for one [`HmclOwnedBuffer`]. On [`HmclStatus::Ok`], the plugin replaces it with a
/// valid buffer obtained from the same host table supplied at initialization. That host remains the
/// owner and calls its matching [`HmclReleaseBufferFn`] exactly once. On any other status,
/// `out_buffer` remains byte-for-byte unchanged and no ownership transfers.
pub type HmclPluginInvokeFn = unsafe extern "C" fn(
    context: *mut c_void,
    operation: HmclSlice,
    input: HmclSlice,
    callback: HmclCallbackId,
    out_buffer: *mut HmclOwnedBuffer,
) -> HmclStatus;

/// Plugin callback that ends a plugin capability session.
///
/// Before calling this callback, the host stops admitting work for the session, waits for every
/// in-flight or reentrant plugin callback to return, and ensures every session-owned Bridge handle
/// reference has been released. The shutdown callback therefore runs after quiescence and is not
/// concurrent with another callback for the same session. It must release plugin state before
/// returning and must not unwind or throw across the ABI boundary.
///
/// # Safety
///
/// `context` must be the context returned in the same plugin table and `plugin` must identify a
/// session previously accepted by [`HmclPluginInitializeFn`] that has not already been shut down.
pub type HmclPluginShutdownFn =
    unsafe extern "C" fn(context: *mut c_void, plugin: HmclPluginId) -> HmclStatus;

/// Version-one services supplied by a Rust plugin payload to the host.
///
/// The host may use the borrowed table only during the query call. If it needs the table afterward,
/// it copies no more than the compatible [`HMCL_PLUGIN_API_V1_PREFIX_SIZE`] bytes and never retains
/// the table pointer. The plugin keeps copied callbacks and `context` valid from successful query
/// and initialization until shutdown returns.
///
/// Except for initialization and quiesced shutdown, plugin callbacks may run concurrently on
/// multiple threads and may be reentered through host callbacks. Plugin state must synchronize that
/// access. No callback may allow a panic, unwind, or foreign exception to cross its ABI boundary.
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
            struct_size: HMCL_PLUGIN_API_V1_PREFIX_SIZE,
            abi_version: HMCL_BRIDGE_ABI_V1,
            ..Self::EMPTY
        }
    }
}

const _: () = {
    assert!(size_of::<HmclPluginApiV1>() == HMCL_PLUGIN_API_V1_PREFIX_SIZE as usize);
    assert!(std::mem::align_of::<HmclPluginApiV1>() == 8);
};

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
/// the same table contract. Callers may invoke independent queries concurrently or reentrantly.
/// Each side copies only the compatible frozen V1 prefix if it needs table contents after this call;
/// neither side retains the other side's table pointer.
///
/// # Safety
///
/// `host` must be non-null, aligned, and readable for its first `u32` and for the complete prefix
/// advertised by that `struct_size`. `out_plugin` must be non-null, aligned, and readable and
/// writable under the same staged rule. The two allocations must not overlap. Both pointers must
/// remain valid for the duration of this call. The caller initializes output header fields before
/// entry. This function does not panic or unwind; foreign callers likewise must not throw through
/// the call.
pub unsafe extern "C" fn negotiate_plugin_api_v1(
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

    if host_header.struct_size < HMCL_HOST_API_V1_PREFIX_SIZE
        || output_header.struct_size < HMCL_PLUGIN_API_V1_PREFIX_SIZE
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

/// Exports the reference version-one query entry point for standalone ABI artifacts.
///
/// This thin wrapper is available only with the default `reference-query-export` feature. Plugin
/// SDK dependencies disable that feature so a payload can export its own implementation without a
/// duplicate C symbol.
///
/// # Safety
///
/// `host` and `out_plugin` must satisfy the complete staged pointer, capacity, alignment,
/// lifetime, and non-overlap contract documented by [`negotiate_plugin_api_v1`].
#[cfg(feature = "reference-query-export")]
#[unsafe(no_mangle)]
pub unsafe extern "C" fn hmcl_plugin_query_v1(
    host: *const HmclHostApiV1,
    out_plugin: *mut HmclPluginApiV1,
) -> HmclStatus {
    // SAFETY: This wrapper forwards the caller's unchanged pointers under the same contract.
    unsafe { negotiate_plugin_api_v1(host, out_plugin) }
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
