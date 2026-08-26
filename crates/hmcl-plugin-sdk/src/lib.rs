#![deny(unsafe_op_in_unsafe_fn)]

//! Safe Rust ownership and Bridge Value bindings for HMCL plugin payloads.

mod error;
mod future;
mod handle;
mod value;

/// The frozen HMCL runtime ABI used by this SDK.
pub use hmcl_runtime_abi as abi;

pub use error::{Error, ErrorCode};
pub use future::{Callback, PluginFuture};
pub use handle::{HandleType, ObjectHandle};
pub use value::{HandleValue, Value};

use abi::{
    HMCL_BRIDGE_ABI_V1, HMCL_HOST_API_V1_PREFIX_SIZE, HmclCapabilityToken, HmclHandleId,
    HmclHostApiV1, HmclOwnedBuffer, HmclPluginId, HmclSlice, HmclStatus,
};
use future::CallbackRegistry;
use std::ffi::c_void;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

#[derive(Clone, Copy)]
struct HostTable(HmclHostApiV1);

// SAFETY: The ABI contract requires the host context and every copied callback to remain valid
// through shutdown and explicitly permits concurrent and reentrant calls from multiple threads.
unsafe impl Send for HostTable {}
// SAFETY: The host owns synchronization for its context under the same concurrent-call contract.
unsafe impl Sync for HostTable {}

struct ContextInner {
    host: HostTable,
    plugin: HmclPluginId,
    token: HmclCapabilityToken,
    callbacks: Arc<CallbackRegistry>,
    cleanup_failures: AtomicUsize,
}

/// Safe access to one copied host API prefix and one plugin capability session.
#[derive(Clone)]
pub struct PluginContext {
    inner: Arc<ContextInner>,
}

impl PluginContext {
    /// Copies and validates a complete v1 host prefix for later SDK calls.
    ///
    /// # Safety
    ///
    /// Every callback and `context` copied from `host` must remain valid for every clone of the
    /// returned context, and the host must uphold the concurrency and no-unwind ABI contract.
    pub unsafe fn from_raw(
        host: &HmclHostApiV1,
        plugin: HmclPluginId,
        token: HmclCapabilityToken,
    ) -> Result<Self, Error> {
        validate_host_header(host.struct_size, host.abi_version)?;
        Ok(Self::from_copied(*host, plugin, token))
    }

    fn from_copied(host: HmclHostApiV1, plugin: HmclPluginId, token: HmclCapabilityToken) -> Self {
        Self {
            inner: Arc::new(ContextInner {
                host: HostTable(host),
                plugin,
                token,
                callbacks: Arc::new(CallbackRegistry::new()),
                cleanup_failures: AtomicUsize::new(0),
            }),
        }
    }

    /// Returns the host-issued identifier for this plugin session.
    #[must_use]
    pub fn plugin_id(&self) -> HmclPluginId {
        self.inner.plugin
    }

    /// Returns the host-issued capability token for this plugin session.
    #[must_use]
    pub fn capability_token(&self) -> HmclCapabilityToken {
        self.inner.token
    }

    /// Invokes a synchronous host Bridge operation with a canonical value payload.
    pub fn invoke(&self, operation: &str, input: &Value) -> Result<Value, Error> {
        let invoke = self
            .inner
            .host
            .0
            .invoke
            .ok_or_else(|| Error::new(ErrorCode::Unavailable))?;
        if self.inner.host.0.release_buffer.is_none() {
            return Err(Error::new(ErrorCode::Unavailable));
        }
        let input = input.to_wire()?;
        let operation = borrowed_slice(operation.as_bytes());
        let input = borrowed_slice(&input);
        let mut output = HmclOwnedBuffer::EMPTY;
        // SAFETY: All borrowed slices live through the call, the output is writable, and copied
        // identifiers and context originate from this exact host table.
        let status = unsafe {
            invoke(
                self.inner.host.0.context,
                self.inner.plugin,
                self.inner.token,
                operation,
                input,
                &mut output,
            )
        };
        if !status.is_ok() {
            return Err(Error::from_host_status(status));
        }
        if !output.has_valid_layout() {
            return Err(Error::new(ErrorCode::InvalidResult));
        }
        let owned = HostBuffer::new(self, output);
        let bytes = owned.copy_bytes()?;
        owned.release()?;
        Value::from_wire(&bytes)
    }

    /// Creates one context-scoped completion capability and its paired future.
    pub fn callback(&self) -> Result<(Callback, PluginFuture), Error> {
        self.inner.callbacks.pair()
    }

    /// Returns the number of callback entries which are still pending.
    #[must_use]
    pub fn pending_callbacks(&self) -> usize {
        self.inner.callbacks.len()
    }

    /// Returns the number of release failures contained by infallible cleanup paths.
    #[must_use]
    pub fn cleanup_failures(&self) -> usize {
        self.inner.cleanup_failures.load(Ordering::Relaxed)
    }

    pub(crate) fn ensure_release_handle(&self) -> Result<(), Error> {
        self.inner
            .host
            .0
            .release_handle
            .map(|_| ())
            .ok_or_else(|| Error::new(ErrorCode::Unavailable))
    }

    pub(crate) fn ensure_handle_callbacks(&self) -> Result<(), Error> {
        if self.inner.host.0.retain_handle.is_none() || self.inner.host.0.release_handle.is_none() {
            return Err(Error::new(ErrorCode::Unavailable));
        }
        Ok(())
    }

    pub(crate) fn retain_handle(&self, handle: HmclHandleId) -> Result<(), Error> {
        let retain = self
            .inner
            .host
            .0
            .retain_handle
            .ok_or_else(|| Error::new(ErrorCode::Unavailable))?;
        // SAFETY: This context and handle originate from the host. The caller is acquiring a
        // reference before constructing an owned SDK wrapper.
        let status = unsafe { retain(self.inner.host.0.context, handle) };
        handle::retain_status(status)
    }

    pub(crate) fn release_handle(&self, handle: HmclHandleId) -> Result<(), Error> {
        let release = self
            .inner
            .host
            .0
            .release_handle
            .ok_or_else(|| Error::new(ErrorCode::Unavailable))?;
        // SAFETY: `ObjectHandle` calls this exactly once for the live reference it owns.
        let status = unsafe { release(self.inner.host.0.context, handle) };
        if status.is_ok() {
            Ok(())
        } else {
            Err(Error::from_host_status(status))
        }
    }

    pub(crate) fn record_cleanup_failure(&self) {
        self.inner.cleanup_failures.fetch_add(1, Ordering::Relaxed);
    }

    fn allocate_wire(&self, bytes: &[u8]) -> Result<HmclOwnedBuffer, Error> {
        let allocate = self
            .inner
            .host
            .0
            .allocate
            .ok_or_else(|| Error::new(ErrorCode::Unavailable))?;
        if self.inner.host.0.release_buffer.is_none() {
            return Err(Error::new(ErrorCode::Unavailable));
        }
        let requested =
            u64::try_from(bytes.len()).map_err(|_| Error::new(ErrorCode::InvalidArgument))?;
        let mut output = HmclOwnedBuffer::EMPTY;
        // SAFETY: The output token is writable and the copied host context remains valid.
        let status = unsafe { allocate(self.inner.host.0.context, requested, &mut output) };
        if !status.is_ok() {
            return Err(Error::from_host_status(status));
        }
        if !output.satisfies_allocation_request(requested) {
            return Err(Error::new(ErrorCode::InvalidResult));
        }
        if !bytes.is_empty() {
            // SAFETY: Successful allocation guarantees at least `requested` writable bytes, and
            // source and host-owned destination do not overlap.
            unsafe { std::ptr::copy_nonoverlapping(bytes.as_ptr(), output.data, bytes.len()) };
        }
        output.len = requested;
        Ok(output)
    }
}

fn validate_host_header(struct_size: u32, abi_version: u32) -> Result<(), Error> {
    if struct_size < HMCL_HOST_API_V1_PREFIX_SIZE {
        return Err(Error::new(ErrorCode::InvalidArgument));
    }
    if abi_version != HMCL_BRIDGE_ABI_V1 {
        return Err(Error::new(ErrorCode::Unavailable));
    }
    Ok(())
}

fn borrowed_slice(bytes: &[u8]) -> HmclSlice {
    HmclSlice {
        data: if bytes.is_empty() {
            std::ptr::null()
        } else {
            bytes.as_ptr()
        },
        len: bytes.len() as u64,
    }
}

struct HostBuffer<'a> {
    context: &'a PluginContext,
    buffer: Option<HmclOwnedBuffer>,
}

impl<'a> HostBuffer<'a> {
    fn new(context: &'a PluginContext, buffer: HmclOwnedBuffer) -> Self {
        Self {
            context,
            buffer: Some(buffer),
        }
    }

    fn copy_bytes(&self) -> Result<Vec<u8>, Error> {
        let buffer = self.buffer.as_ref().expect("owned buffer is present");
        let length =
            usize::try_from(buffer.len).map_err(|_| Error::new(ErrorCode::InvalidResult))?;
        if length == 0 {
            return Ok(Vec::new());
        }
        // SAFETY: A successful host callback guarantees `len` initialized readable bytes.
        Ok(unsafe { std::slice::from_raw_parts(buffer.data, length) }.to_vec())
    }

    fn release(mut self) -> Result<(), Error> {
        let mut buffer = self.buffer.take().expect("owned buffer is present");
        let release = self
            .context
            .inner
            .host
            .0
            .release_buffer
            .expect("release callback was checked before host invocation");
        // SAFETY: This exact host table allocated `buffer`, and taking the option ensures this is
        // its sole explicit release. Ownership is consumed regardless of diagnostic status.
        let status = unsafe { release(self.context.inner.host.0.context, &mut buffer) };
        if status.is_ok() {
            Ok(())
        } else {
            Err(Error::from_host_status(status))
        }
    }
}

impl Drop for HostBuffer<'_> {
    fn drop(&mut self) {
        let Some(mut buffer) = self.buffer.take() else {
            return;
        };
        let release = self
            .context
            .inner
            .host
            .0
            .release_buffer
            .expect("release callback was checked before host invocation");
        // SAFETY: The option is the unique ownership token and is cleared before this call.
        let status = unsafe { release(self.context.inner.host.0.context, &mut buffer) };
        if !status.is_ok() {
            self.context.record_cleanup_failure();
        }
    }
}

/// A Rust plugin implementation hosted behind [`hmcl_plugin!`].
pub trait Plugin: Default + Send + Sync + 'static {
    /// Initializes one plugin session after the host table has been copied and validated.
    fn initialize(&mut self, _context: &PluginContext) -> Result<(), Error> {
        Ok(())
    }

    /// Handles one decoded operation and returns its Bridge Value result.
    fn invoke(
        &self,
        context: &PluginContext,
        operation: &str,
        input: Value,
        callback: abi::HmclCallbackId,
    ) -> Result<Value, Error>;

    /// Releases plugin-owned state after the host has quiesced the session.
    fn shutdown(&mut self, _context: &PluginContext) -> Result<(), Error> {
        Ok(())
    }
}

/// Exports a panic-contained `hmcl_plugin_query_v1` entry point for a [`Plugin`] type.
#[macro_export]
macro_rules! hmcl_plugin {
    ($plugin:ty) => {
        /// Negotiates the HMCL v1 plugin table for this Rust payload.
        ///
        /// # Safety
        ///
        /// The pointers must satisfy the staged table contracts defined by `hmcl-runtime-abi`.
        #[unsafe(no_mangle)]
        pub unsafe extern "C" fn hmcl_plugin_query_v1(
            host: *const $crate::abi::HmclHostApiV1,
            out_plugin: *mut $crate::abi::HmclPluginApiV1,
        ) -> $crate::abi::HmclStatus {
            static RUNTIME: std::sync::OnceLock<$crate::__private::PluginRuntime<$plugin>> =
                std::sync::OnceLock::new();
            // SAFETY: This exported function forwards its documented pointer contract unchanged.
            unsafe { $crate::__private::query_plugin(&RUNTIME, host, out_plugin) }
        }
    };
}

/// Implementation details used only by [`hmcl_plugin!`].
#[doc(hidden)]
pub mod __private {
    use super::*;
    use abi::{HmclCallbackId, HmclPluginApiV1};
    use std::panic::{AssertUnwindSafe, catch_unwind};
    use std::sync::{Mutex, OnceLock};

    /// Process-static state backing one macro-exported plugin entry point.
    pub struct PluginRuntime<P: Plugin> {
        session: Mutex<Option<Arc<Session<P>>>>,
    }

    struct Session<P: Plugin> {
        plugin: P,
        context: PluginContext,
    }

    impl<P: Plugin> Default for PluginRuntime<P> {
        fn default() -> Self {
            Self {
                session: Mutex::new(None),
            }
        }
    }

    /// Validates query tables and installs panic-contained lifecycle callbacks.
    ///
    /// # Safety
    ///
    /// `host` and `out_plugin` must satisfy the staged table pointer contract documented by the
    /// frozen runtime ABI query entry point.
    pub unsafe fn query_plugin<P: Plugin>(
        runtime: &'static OnceLock<PluginRuntime<P>>,
        host: *const HmclHostApiV1,
        out_plugin: *mut HmclPluginApiV1,
    ) -> HmclStatus {
        catch_unwind(AssertUnwindSafe(|| {
            // SAFETY: The exported query's documented ABI contract is forwarded unchanged.
            unsafe { query_inner(runtime, host, out_plugin) }
        }))
        .unwrap_or(HmclStatus::PluginError)
    }

    unsafe fn query_inner<P: Plugin>(
        runtime: &'static OnceLock<PluginRuntime<P>>,
        host: *const HmclHostApiV1,
        out_plugin: *mut HmclPluginApiV1,
    ) -> HmclStatus {
        // SAFETY: The exported query forwards the exact staged table contract required here.
        let status = unsafe { abi::negotiate_plugin_api_v1(host, out_plugin) };
        if !status.is_ok() {
            return status;
        }
        let context = std::ptr::from_ref(runtime.get_or_init(PluginRuntime::default))
            .cast_mut()
            .cast::<c_void>();
        let table = HmclPluginApiV1 {
            context,
            initialize: Some(initialize_plugin::<P>),
            invoke: Some(invoke_plugin::<P>),
            shutdown: Some(shutdown_plugin::<P>),
            ..HmclPluginApiV1::with_required_prefix()
        };
        // SAFETY: The capacity check proves the caller advertised the complete writable v1 prefix.
        unsafe { out_plugin.write(table) };
        HmclStatus::Ok
    }

    unsafe extern "C" fn initialize_plugin<P: Plugin>(
        context: *mut c_void,
        host: *const HmclHostApiV1,
        plugin: HmclPluginId,
        token: HmclCapabilityToken,
    ) -> HmclStatus {
        catch_unwind(AssertUnwindSafe(|| {
            // SAFETY: The host invokes this callback with the context emitted by `query_inner`.
            unsafe { initialize_inner::<P>(context, host, plugin, token) }
        }))
        .unwrap_or(HmclStatus::PluginError)
    }

    unsafe fn initialize_inner<P: Plugin>(
        context: *mut c_void,
        host: *const HmclHostApiV1,
        plugin_id: HmclPluginId,
        token: HmclCapabilityToken,
    ) -> HmclStatus {
        if context.is_null() || host.is_null() || !host.is_aligned() {
            return HmclStatus::InvalidArgument;
        }
        // SAFETY: The callback contract guarantees the first field is readable.
        let size = unsafe { std::ptr::read(host.cast::<u32>()) };
        if size < 8 {
            return HmclStatus::BufferTooSmall;
        }
        // SAFETY: The advertised size permits the version field read.
        let version = unsafe { std::ptr::read(host.cast::<u32>().add(1)) };
        if size < HMCL_HOST_API_V1_PREFIX_SIZE {
            return HmclStatus::BufferTooSmall;
        }
        if version != HMCL_BRIDGE_ABI_V1 {
            return HmclStatus::UnsupportedAbi;
        }
        // SAFETY: The complete-prefix check proves this copy reads only advertised bytes.
        let copied_host = unsafe { host.read() };
        let plugin_context = PluginContext::from_copied(copied_host, plugin_id, token);
        // SAFETY: `context` came from a process-static `PluginRuntime<P>` in `query_inner`.
        let runtime = unsafe { &*(context.cast::<PluginRuntime<P>>()) };
        if runtime
            .session
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .is_some()
        {
            return HmclStatus::PluginError;
        }
        let mut implementation = P::default();
        if implementation.initialize(&plugin_context).is_err() {
            return HmclStatus::PluginError;
        }
        let mut session = runtime
            .session
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        if session.is_some() {
            return HmclStatus::PluginError;
        }
        *session = Some(Arc::new(Session {
            plugin: implementation,
            context: plugin_context,
        }));
        HmclStatus::Ok
    }

    unsafe extern "C" fn invoke_plugin<P: Plugin>(
        context: *mut c_void,
        operation: HmclSlice,
        input: HmclSlice,
        callback: HmclCallbackId,
        out_buffer: *mut HmclOwnedBuffer,
    ) -> HmclStatus {
        catch_unwind(AssertUnwindSafe(|| {
            // SAFETY: The host invokes this callback under the frozen v1 invoke contract.
            unsafe { invoke_inner::<P>(context, operation, input, callback, out_buffer) }
        }))
        .unwrap_or(HmclStatus::PluginError)
    }

    unsafe fn invoke_inner<P: Plugin>(
        context: *mut c_void,
        operation: HmclSlice,
        input: HmclSlice,
        callback: HmclCallbackId,
        out_buffer: *mut HmclOwnedBuffer,
    ) -> HmclStatus {
        if context.is_null() || out_buffer.is_null() || !out_buffer.is_aligned() {
            return HmclStatus::InvalidArgument;
        }
        let operation = match unsafe { read_slice(operation) } {
            Ok(bytes) => bytes,
            Err(status) => return status,
        };
        let input = match unsafe { read_slice(input) } {
            Ok(bytes) => bytes,
            Err(status) => return status,
        };
        let operation = match std::str::from_utf8(operation) {
            Ok(operation) => operation,
            Err(_) => return HmclStatus::InvalidArgument,
        };
        let input = match Value::from_wire(input) {
            Ok(input) => input,
            Err(_) => return HmclStatus::InvalidArgument,
        };
        // SAFETY: `context` came from a process-static `PluginRuntime<P>` in `query_inner`.
        let runtime = unsafe { &*(context.cast::<PluginRuntime<P>>()) };
        let state = runtime
            .session
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        let Some(session) = state.as_ref().map(Arc::clone) else {
            return HmclStatus::PluginError;
        };
        drop(state);
        let value = match session
            .plugin
            .invoke(&session.context, operation, input, callback)
        {
            Ok(value) => value,
            Err(_) => return HmclStatus::PluginError,
        };
        let wire = match value.to_wire() {
            Ok(wire) => wire,
            Err(_) => return HmclStatus::PluginError,
        };
        let output = match session.context.allocate_wire(&wire) {
            Ok(output) => output,
            Err(_) => return HmclStatus::HostError,
        };
        // SAFETY: The callback contract guarantees writable output storage. This is the sole
        // success-path write and transfers the host allocation token to the caller.
        unsafe { out_buffer.write(output) };
        HmclStatus::Ok
    }

    unsafe extern "C" fn shutdown_plugin<P: Plugin>(
        context: *mut c_void,
        plugin: HmclPluginId,
    ) -> HmclStatus {
        catch_unwind(AssertUnwindSafe(|| {
            // SAFETY: The host invokes this callback with the process-static runtime context.
            unsafe { shutdown_inner::<P>(context, plugin) }
        }))
        .unwrap_or(HmclStatus::PluginError)
    }

    unsafe fn shutdown_inner<P: Plugin>(context: *mut c_void, plugin: HmclPluginId) -> HmclStatus {
        if context.is_null() {
            return HmclStatus::InvalidArgument;
        }
        // SAFETY: `context` came from a process-static `PluginRuntime<P>` in `query_inner`.
        let runtime = unsafe { &*(context.cast::<PluginRuntime<P>>()) };
        let mut state = runtime
            .session
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        let Some(session) = state.take() else {
            return HmclStatus::PluginError;
        };
        if session.context.plugin_id() != plugin {
            *state = Some(session);
            return HmclStatus::InvalidArgument;
        }
        drop(state);
        let Ok(mut session) = Arc::try_unwrap(session) else {
            return HmclStatus::PluginError;
        };
        if session.plugin.shutdown(&session.context).is_ok() {
            HmclStatus::Ok
        } else {
            HmclStatus::PluginError
        }
    }

    unsafe fn read_slice<'a>(slice: HmclSlice) -> Result<&'a [u8], HmclStatus> {
        let length = usize::try_from(slice.len).map_err(|_| HmclStatus::InvalidArgument)?;
        if length == 0 {
            if slice.data.is_null() {
                return Ok(&[]);
            }
            return Err(HmclStatus::InvalidArgument);
        }
        if slice.data.is_null() {
            return Err(HmclStatus::InvalidArgument);
        }
        // SAFETY: The callback contract guarantees `len` readable bytes for every valid slice.
        Ok(unsafe { std::slice::from_raw_parts(slice.data, length) })
    }
}
