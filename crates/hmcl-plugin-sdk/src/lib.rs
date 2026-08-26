#![deny(unsafe_op_in_unsafe_fn)]

//! Safe Rust ownership and Bridge Value bindings for HMCL plugin payloads.

mod error;
mod future;
mod handle;
mod value;

/// The frozen HMCL runtime ABI used by this SDK.
pub use hmcl_runtime_abi as abi;

pub use error::{BridgeErrorKind, Error, ErrorCode};
pub use future::{Callback, PluginFuture};
pub use handle::{HandleType, ObjectHandle};
pub use value::{HandleValue, Value};

use abi::{
    HMCL_BRIDGE_ABI_V1, HMCL_HOST_API_V1_PREFIX_SIZE, HmclCapabilityToken, HmclHandleId,
    HmclHostApiV1, HmclOwnedBuffer, HmclPluginId, HmclSlice, HmclStatus,
};
use future::CallbackRegistry;
use std::cell::RefCell;
use std::collections::HashMap;
use std::ffi::c_void;
use std::marker::PhantomData;
use std::rc::Rc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Condvar, Mutex};

thread_local! {
    static HOST_LEASE_DEPTHS: RefCell<HashMap<usize, usize>> = RefCell::new(HashMap::new());
}

#[derive(Clone, Copy)]
struct HostTable(HmclHostApiV1);

// SAFETY: The ABI contract requires the host context and every copied callback to remain valid
// through shutdown and explicitly permits concurrent and reentrant calls from multiple threads.
unsafe impl Send for HostTable {}
// SAFETY: The host owns synchronization for its context under the same concurrent-call contract.
unsafe impl Sync for HostTable {}

struct ContextInner {
    host: HostState,
    plugin: HmclPluginId,
    token: HmclCapabilityToken,
    callbacks: Arc<CallbackRegistry>,
    cleanup_failures: AtomicUsize,
}

struct HostState {
    table: HostTable,
    admission: Mutex<HostAdmission>,
    quiesced: Condvar,
}

struct HostAdmission {
    active: bool,
    in_flight: usize,
}

struct HostLease<'a> {
    host: &'a HostState,
    thread_bound: PhantomData<Rc<()>>,
}

impl HostState {
    fn new(table: HmclHostApiV1) -> Self {
        Self {
            table: HostTable(table),
            admission: Mutex::new(HostAdmission {
                active: true,
                in_flight: 0,
            }),
            quiesced: Condvar::new(),
        }
    }

    fn lease(&self) -> Result<HostLease<'_>, Error> {
        let key = std::ptr::from_ref(self).addr();
        let nested = HOST_LEASE_DEPTHS.with(|depths| depths.borrow().contains_key(&key));
        let mut admission = self
            .admission
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        if !admission.active && !nested {
            return Err(Error::new(ErrorCode::Unavailable));
        }
        admission.in_flight = admission
            .in_flight
            .checked_add(1)
            .ok_or_else(|| Error::new(ErrorCode::Unavailable))?;
        drop(admission);
        HOST_LEASE_DEPTHS.with(|depths| {
            let mut depths = depths.borrow_mut();
            *depths.entry(key).or_default() += 1;
        });
        Ok(HostLease {
            host: self,
            thread_bound: PhantomData,
        })
    }

    fn deactivate(&self) {
        let mut admission = self
            .admission
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        admission.active = false;
        while admission.in_flight != 0 {
            admission = self
                .quiesced
                .wait(admission)
                .unwrap_or_else(|error| error.into_inner());
        }
    }
}

impl HostLease<'_> {
    fn table(&self) -> &HostTable {
        &self.host.table
    }
}

impl Drop for HostLease<'_> {
    fn drop(&mut self) {
        let key = std::ptr::from_ref(self.host).addr();
        HOST_LEASE_DEPTHS.with(|depths| {
            let mut depths = depths.borrow_mut();
            let depth = depths
                .get_mut(&key)
                .expect("a host lease records its thread-local admission");
            *depth -= 1;
            if *depth == 0 {
                depths.remove(&key);
            }
        });
        let mut admission = self
            .host
            .admission
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        admission.in_flight -= 1;
        if admission.in_flight == 0 {
            self.host.quiesced.notify_all();
        }
    }
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
                host: HostState::new(host),
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
        let lease = self.inner.host.lease()?;
        let table = lease.table();
        let invoke = table
            .0
            .invoke
            .ok_or_else(|| Error::new(ErrorCode::Unavailable))?;
        if table.0.release_buffer.is_none() {
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
                table.0.context,
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
        let owned = HostBuffer::new(table, &self.inner.cleanup_failures, output);
        if !owned.has_valid_layout() {
            return Err(Error::new(ErrorCode::InvalidResult));
        }
        let bytes = owned.copy_bytes()?;
        owned.release()?;
        Value::from_wire(&bytes)
    }

    /// Creates one context-scoped completion capability and its paired future.
    pub fn callback(&self) -> Result<(Callback, PluginFuture), Error> {
        let _lease = self.inner.host.lease()?;
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
            .lease()?
            .table()
            .0
            .release_handle
            .map(|_| ())
            .ok_or_else(|| Error::new(ErrorCode::Unavailable))
    }

    pub(crate) fn ensure_handle_callbacks(&self) -> Result<(), Error> {
        let lease = self.inner.host.lease()?;
        if lease.table().0.retain_handle.is_none() || lease.table().0.release_handle.is_none() {
            return Err(Error::new(ErrorCode::Unavailable));
        }
        Ok(())
    }

    pub(crate) fn retain_handle(&self, handle: HmclHandleId) -> Result<(), Error> {
        let lease = self.inner.host.lease()?;
        let table = lease.table();
        if table.0.release_handle.is_none() {
            return Err(Error::new(ErrorCode::Unavailable));
        }
        let retain = table
            .0
            .retain_handle
            .ok_or_else(|| Error::new(ErrorCode::Unavailable))?;
        // SAFETY: This context and handle originate from the host. The caller is acquiring a
        // reference before constructing an owned SDK wrapper.
        let status = unsafe { retain(table.0.context, handle) };
        handle::retain_status(status)
    }

    pub(crate) fn release_handle_for_drop(&self, handle: HmclHandleId) -> bool {
        let Ok(lease) = self.inner.host.lease() else {
            return false;
        };
        let table = lease.table();
        let Some(release) = table.0.release_handle else {
            return true;
        };
        // SAFETY: `ObjectHandle` calls this exactly once for the live reference it owns.
        let status = unsafe { release(table.0.context, handle) };
        !status.is_ok()
    }

    pub(crate) fn record_cleanup_failure(&self) {
        self.inner.cleanup_failures.fetch_add(1, Ordering::Relaxed);
    }

    fn allocate_wire(&self, bytes: &[u8]) -> Result<HmclOwnedBuffer, Error> {
        let lease = self.inner.host.lease()?;
        let table = lease.table();
        let allocate = table
            .0
            .allocate
            .ok_or_else(|| Error::new(ErrorCode::Unavailable))?;
        if table.0.release_buffer.is_none() {
            return Err(Error::new(ErrorCode::Unavailable));
        }
        let requested =
            u64::try_from(bytes.len()).map_err(|_| Error::new(ErrorCode::InvalidArgument))?;
        let mut output = HmclOwnedBuffer::EMPTY;
        // SAFETY: The output token is writable and the copied host context remains valid.
        let status = unsafe { allocate(table.0.context, requested, &mut output) };
        if !status.is_ok() {
            return Err(Error::from_host_status(status));
        }
        let mut owned = HostBuffer::new(table, &self.inner.cleanup_failures, output);
        if !owned.satisfies_allocation_request(requested) {
            return Err(Error::new(ErrorCode::InvalidResult));
        }
        if !bytes.is_empty() {
            // SAFETY: Successful allocation guarantees at least `requested` writable bytes, and
            // source and host-owned destination do not overlap.
            unsafe {
                std::ptr::copy_nonoverlapping(bytes.as_ptr(), owned.buffer_mut().data, bytes.len());
            }
        }
        owned.buffer_mut().len = requested;
        Ok(owned.into_buffer())
    }

    fn deactivate(&self) {
        self.inner.host.deactivate();
        self.inner.callbacks.close();
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
    table: &'a HostTable,
    cleanup_failures: &'a AtomicUsize,
    buffer: Option<HmclOwnedBuffer>,
}

impl<'a> HostBuffer<'a> {
    fn new(
        table: &'a HostTable,
        cleanup_failures: &'a AtomicUsize,
        buffer: HmclOwnedBuffer,
    ) -> Self {
        Self {
            table,
            cleanup_failures,
            buffer: Some(buffer),
        }
    }

    fn has_valid_layout(&self) -> bool {
        self.buffer
            .as_ref()
            .expect("owned buffer is present")
            .has_valid_layout()
    }

    fn satisfies_allocation_request(&self, requested: u64) -> bool {
        self.buffer
            .as_ref()
            .expect("owned buffer is present")
            .satisfies_allocation_request(requested)
    }

    fn buffer_mut(&mut self) -> &mut HmclOwnedBuffer {
        self.buffer.as_mut().expect("owned buffer is present")
    }

    fn into_buffer(mut self) -> HmclOwnedBuffer {
        self.buffer.take().expect("owned buffer is present")
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
            .table
            .0
            .release_buffer
            .expect("release callback was checked before host invocation");
        // SAFETY: This exact host table allocated `buffer`, and taking the option ensures this is
        // its sole explicit release. Ownership is consumed regardless of diagnostic status.
        let status = unsafe { release(self.table.0.context, &mut buffer) };
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
            .table
            .0
            .release_buffer
            .expect("release callback was checked before host invocation");
        // SAFETY: The option is the unique ownership token and is cleared before this call.
        let status = unsafe { release(self.table.0.context, &mut buffer) };
        if !status.is_ok() {
            self.cleanup_failures.fetch_add(1, Ordering::Relaxed);
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
    use std::sync::{Condvar, Mutex, OnceLock};

    /// Process-static state backing one macro-exported plugin entry point.
    pub struct PluginRuntime<P: Plugin> {
        session: Mutex<Option<Arc<Session<P>>>>,
    }

    struct Session<P: Plugin> {
        plugin: P,
        context: PluginContext,
        invocations: Arc<InvocationGate>,
    }

    struct InvocationGate {
        state: Mutex<InvocationState>,
        quiesced: Condvar,
    }

    struct InvocationState {
        accepting: bool,
        in_flight: usize,
    }

    struct InvocationLease<P: Plugin> {
        session: Option<Arc<Session<P>>>,
    }

    impl InvocationGate {
        fn new() -> Self {
            Self {
                state: Mutex::new(InvocationState {
                    accepting: true,
                    in_flight: 0,
                }),
                quiesced: Condvar::new(),
            }
        }

        fn admit(&self) -> bool {
            let mut state = self.state.lock().unwrap_or_else(|error| error.into_inner());
            if !state.accepting {
                return false;
            }
            let Some(in_flight) = state.in_flight.checked_add(1) else {
                return false;
            };
            state.in_flight = in_flight;
            true
        }

        fn close(&self) {
            self.state
                .lock()
                .unwrap_or_else(|error| error.into_inner())
                .accepting = false;
        }

        fn wait(&self) {
            let mut state = self.state.lock().unwrap_or_else(|error| error.into_inner());
            while state.in_flight != 0 {
                state = self
                    .quiesced
                    .wait(state)
                    .unwrap_or_else(|error| error.into_inner());
            }
        }

        fn leave(&self) {
            let mut state = self.state.lock().unwrap_or_else(|error| error.into_inner());
            state.in_flight -= 1;
            if state.in_flight == 0 {
                self.quiesced.notify_all();
            }
        }
    }

    impl<P: Plugin> InvocationLease<P> {
        fn new(session: Arc<Session<P>>) -> Option<Self> {
            if !session.invocations.admit() {
                return None;
            }
            Some(Self {
                session: Some(session),
            })
        }

        fn session(&self) -> &Session<P> {
            self.session
                .as_deref()
                .expect("an invocation lease owns its session until drop")
        }
    }

    impl<P: Plugin> Drop for InvocationLease<P> {
        fn drop(&mut self) {
            let session = self
                .session
                .take()
                .expect("an invocation lease drops its session exactly once");
            let invocations = Arc::clone(&session.invocations);
            // Shutdown relies on the final session clone disappearing before the active count.
            drop(session);
            invocations.leave();
        }
    }

    struct ContextDeactivationGuard {
        context: PluginContext,
        armed: bool,
    }

    impl ContextDeactivationGuard {
        fn new(context: &PluginContext) -> Self {
            Self {
                context: context.clone(),
                armed: true,
            }
        }

        fn disarm(&mut self) {
            self.armed = false;
        }
    }

    impl Drop for ContextDeactivationGuard {
        fn drop(&mut self) {
            if self.armed {
                self.context.deactivate();
            }
        }
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
        let mut deactivation = ContextDeactivationGuard::new(&plugin_context);
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
            invocations: Arc::new(InvocationGate::new()),
        }));
        deactivation.disarm();
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
        let Some(invocation) = state
            .as_ref()
            .map(Arc::clone)
            .and_then(InvocationLease::new)
        else {
            return HmclStatus::PluginError;
        };
        drop(state);
        let session = invocation.session();
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
        session.invocations.close();
        drop(state);
        session.invocations.wait();
        let session = match Arc::try_unwrap(session) {
            Ok(session) => session,
            Err(session) => {
                session.context.deactivate();
                return HmclStatus::PluginError;
            }
        };
        let Session {
            mut plugin,
            context,
            ..
        } = session;
        let shutdown = catch_unwind(AssertUnwindSafe(|| plugin.shutdown(&context)));
        let dropped = catch_unwind(AssertUnwindSafe(|| drop(plugin)));
        context.deactivate();
        if matches!(shutdown, Ok(Ok(()))) && dropped.is_ok() {
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

#[cfg(test)]
mod host_lifetime_tests {
    use super::*;
    use std::sync::atomic::{AtomicBool, AtomicI32, AtomicUsize, Ordering};
    use std::sync::{Arc, Condvar, Mutex, mpsc};
    use std::time::Duration;

    struct CallGate {
        entered: Mutex<bool>,
        entered_changed: Condvar,
        allowed: Mutex<bool>,
        allowed_changed: Condvar,
    }

    impl CallGate {
        fn new() -> Self {
            Self {
                entered: Mutex::new(false),
                entered_changed: Condvar::new(),
                allowed: Mutex::new(false),
                allowed_changed: Condvar::new(),
            }
        }

        fn enter_and_wait(&self) {
            *self
                .entered
                .lock()
                .unwrap_or_else(|error| error.into_inner()) = true;
            self.entered_changed.notify_all();
            let mut allowed = self
                .allowed
                .lock()
                .unwrap_or_else(|error| error.into_inner());
            while !*allowed {
                allowed = self
                    .allowed_changed
                    .wait(allowed)
                    .unwrap_or_else(|error| error.into_inner());
            }
        }

        fn wait_until_entered(&self) {
            let mut entered = self
                .entered
                .lock()
                .unwrap_or_else(|error| error.into_inner());
            while !*entered {
                entered = self
                    .entered_changed
                    .wait(entered)
                    .unwrap_or_else(|error| error.into_inner());
            }
        }

        fn allow(&self) {
            *self
                .allowed
                .lock()
                .unwrap_or_else(|error| error.into_inner()) = true;
            self.allowed_changed.notify_all();
        }
    }

    struct BlockingHost {
        gate: CallGate,
        valid: AtomicBool,
        calls_after_invalidation: AtomicUsize,
        calls: AtomicUsize,
    }

    struct MalformedBufferHost {
        descriptor: (usize, u64, u64),
        release_status: AtomicI32,
        releases: Mutex<Vec<(usize, u64, u64)>>,
    }

    impl MalformedBufferHost {
        fn new(descriptor: (usize, u64, u64), release_status: HmclStatus) -> Self {
            Self {
                descriptor,
                release_status: AtomicI32::new(release_status.into_raw()),
                releases: Mutex::new(Vec::new()),
            }
        }
    }

    impl BlockingHost {
        fn new() -> Arc<Self> {
            Arc::new(Self {
                gate: CallGate::new(),
                valid: AtomicBool::new(true),
                calls_after_invalidation: AtomicUsize::new(0),
                calls: AtomicUsize::new(0),
            })
        }

        fn record_call(&self) {
            self.calls.fetch_add(1, Ordering::SeqCst);
            if !self.valid.load(Ordering::SeqCst) {
                self.calls_after_invalidation.fetch_add(1, Ordering::SeqCst);
            }
        }
    }

    unsafe extern "C" fn blocking_invoke(
        context: *mut c_void,
        _plugin: HmclPluginId,
        _token: HmclCapabilityToken,
        _operation: HmclSlice,
        _input: HmclSlice,
        output: *mut HmclOwnedBuffer,
    ) -> HmclStatus {
        let host = unsafe { &*context.cast::<BlockingHost>() };
        host.record_call();
        host.gate.enter_and_wait();
        let mut wire = Value::Null.to_wire().expect("null wire");
        let buffer = HmclOwnedBuffer {
            data: wire.as_mut_ptr(),
            len: wire.len() as u64,
            capacity: wire.capacity() as u64,
        };
        std::mem::forget(wire);
        unsafe { output.write(buffer) };
        HmclStatus::Ok
    }

    unsafe extern "C" fn release_buffer(
        _context: *mut c_void,
        buffer: *mut HmclOwnedBuffer,
    ) -> HmclStatus {
        let buffer = unsafe { buffer.read() };
        if buffer.capacity != 0 {
            unsafe {
                drop(Vec::from_raw_parts(
                    buffer.data,
                    buffer.len as usize,
                    buffer.capacity as usize,
                ));
            }
        }
        HmclStatus::Ok
    }

    unsafe extern "C" fn blocking_release_handle(
        context: *mut c_void,
        _handle: HmclHandleId,
    ) -> HmclStatus {
        let host = unsafe { &*context.cast::<BlockingHost>() };
        host.record_call();
        host.gate.enter_and_wait();
        HmclStatus::Ok
    }

    unsafe extern "C" fn malformed_invoke(
        context: *mut c_void,
        _plugin: HmclPluginId,
        _token: HmclCapabilityToken,
        _operation: HmclSlice,
        _input: HmclSlice,
        output: *mut HmclOwnedBuffer,
    ) -> HmclStatus {
        let host = unsafe { &*context.cast::<MalformedBufferHost>() };
        let (data, len, capacity) = host.descriptor;
        unsafe {
            output.write(HmclOwnedBuffer {
                data: std::ptr::with_exposed_provenance_mut(data),
                len,
                capacity,
            });
        }
        HmclStatus::Ok
    }

    unsafe extern "C" fn malformed_allocate(
        context: *mut c_void,
        _length: u64,
        output: *mut HmclOwnedBuffer,
    ) -> HmclStatus {
        let host = unsafe { &*context.cast::<MalformedBufferHost>() };
        let (data, len, capacity) = host.descriptor;
        unsafe {
            output.write(HmclOwnedBuffer {
                data: std::ptr::with_exposed_provenance_mut(data),
                len,
                capacity,
            });
        }
        HmclStatus::Ok
    }

    unsafe extern "C" fn record_malformed_release(
        context: *mut c_void,
        buffer: *mut HmclOwnedBuffer,
    ) -> HmclStatus {
        let host = unsafe { &*context.cast::<MalformedBufferHost>() };
        let buffer = unsafe { buffer.read() };
        host.releases
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .push((buffer.data.addr(), buffer.len, buffer.capacity));
        HmclStatus::from_raw(host.release_status.load(Ordering::SeqCst))
    }

    fn malformed_buffer_context(
        host: &MalformedBufferHost,
        allocate: bool,
        invoke: bool,
    ) -> PluginContext {
        let table = HmclHostApiV1 {
            context: std::ptr::from_ref(host).cast_mut().cast(),
            allocate: allocate.then_some(malformed_allocate),
            release_buffer: Some(record_malformed_release),
            invoke: invoke.then_some(malformed_invoke),
            ..HmclHostApiV1::with_required_prefix()
        };
        unsafe {
            PluginContext::from_raw(
                &table,
                HmclPluginId::from_raw(7),
                HmclCapabilityToken::from_raw(11),
            )
            .expect("valid malformed-buffer host")
        }
    }

    fn context(host: &Arc<BlockingHost>, invoke: bool, handles: bool) -> PluginContext {
        let table = HmclHostApiV1 {
            context: std::ptr::from_ref(host.as_ref()).cast_mut().cast(),
            release_buffer: invoke.then_some(release_buffer),
            invoke: invoke.then_some(blocking_invoke),
            release_handle: handles.then_some(blocking_release_handle),
            ..HmclHostApiV1::with_required_prefix()
        };
        unsafe {
            PluginContext::from_raw(
                &table,
                HmclPluginId::from_raw(7),
                HmclCapabilityToken::from_raw(11),
            )
            .expect("valid host")
        }
    }

    #[test]
    fn deactivation_waits_for_admitted_invoke_and_rejects_late_context_clones() {
        let host = BlockingHost::new();
        let context = context(&host, true, false);
        let (callback, future) = context.callback().expect("callback");
        let invoke_context = context.clone();
        let invoke_thread =
            std::thread::spawn(move || invoke_context.invoke("block", &Value::Null));
        host.gate.wait_until_entered();

        let deactivate_context = context.clone();
        let (done_tx, done_rx) = mpsc::channel();
        let deactivate_thread = std::thread::spawn(move || {
            deactivate_context.deactivate();
            done_tx.send(()).expect("deactivation completion");
        });
        assert!(done_rx.recv_timeout(Duration::from_millis(50)).is_err());
        host.gate.allow();
        assert_eq!(
            invoke_thread.join().expect("invoke thread"),
            Ok(Value::Null)
        );
        done_rx
            .recv_timeout(Duration::from_secs(1))
            .expect("deactivation waited for invoke");
        deactivate_thread.join().expect("deactivation thread");

        host.valid.store(false, Ordering::SeqCst);
        assert_eq!(
            context
                .invoke("late", &Value::Null)
                .expect_err("inactive")
                .code(),
            ErrorCode::Unavailable
        );
        assert_eq!(
            future.wait().expect_err("cancelled").code(),
            ErrorCode::Cancelled
        );
        assert_eq!(
            callback
                .complete(Ok(Value::Null))
                .expect_err("inactive")
                .code(),
            ErrorCode::Cancelled
        );
        assert_eq!(host.calls.load(Ordering::SeqCst), 1);
        assert_eq!(host.calls_after_invalidation.load(Ordering::SeqCst), 0);
    }

    #[test]
    fn deactivation_waits_for_admitted_handle_drop_and_suppresses_late_drop() {
        struct Document;
        impl HandleType for Document {
            const TYPE_NAME: &'static str = "document";
        }

        let host = BlockingHost::new();
        let context = context(&host, false, true);
        let first = unsafe {
            ObjectHandle::<Document>::from_owned(
                &context,
                HandleValue::new(5, 7, "document").expect("handle"),
            )
            .expect("owned")
        };
        let late = unsafe {
            ObjectHandle::<Document>::from_owned(
                &context,
                HandleValue::new(6, 8, "document").expect("handle"),
            )
            .expect("owned")
        };
        let drop_thread = std::thread::spawn(move || drop(first));
        host.gate.wait_until_entered();

        let deactivate_context = context.clone();
        let (done_tx, done_rx) = mpsc::channel();
        let deactivate_thread = std::thread::spawn(move || {
            deactivate_context.deactivate();
            done_tx.send(()).expect("deactivation completion");
        });
        assert!(done_rx.recv_timeout(Duration::from_millis(50)).is_err());
        host.gate.allow();
        drop_thread.join().expect("drop thread");
        done_rx
            .recv_timeout(Duration::from_secs(1))
            .expect("deactivation waited for release");
        deactivate_thread.join().expect("deactivation thread");

        host.valid.store(false, Ordering::SeqCst);
        drop(late);
        assert_eq!(host.calls.load(Ordering::SeqCst), 1);
        assert_eq!(host.calls_after_invalidation.load(Ordering::SeqCst), 0);
        assert_eq!(context.cleanup_failures(), 0);
    }

    #[test]
    fn admitted_thread_can_reenter_after_closing_but_external_calls_are_rejected() {
        let host = Arc::new(HostState::new(HmclHostApiV1::with_required_prefix()));
        let worker_host = Arc::clone(&host);
        let (outer_tx, outer_rx) = mpsc::channel();
        let (reenter_tx, reenter_rx) = mpsc::channel();
        let (attempt_tx, attempt_rx) = mpsc::channel();
        let worker = std::thread::spawn(move || {
            let outer = worker_host.lease().expect("outer admission");
            outer_tx.send(()).expect("outer entered");
            reenter_rx.recv().expect("reentry signal");
            let nested = worker_host.lease().map(|_| ());
            attempt_tx.send(nested).expect("reentry result");
            drop(outer);
        });
        outer_rx.recv().expect("outer admission signal");

        let deactivate_host = Arc::clone(&host);
        let (done_tx, done_rx) = mpsc::channel();
        let deactivate = std::thread::spawn(move || {
            deactivate_host.deactivate();
            done_tx.send(()).expect("deactivation completion");
        });
        let closing_deadline = std::time::Instant::now() + Duration::from_secs(1);
        loop {
            let active = host
                .admission
                .lock()
                .unwrap_or_else(|error| error.into_inner())
                .active;
            if !active {
                break;
            }
            assert!(
                std::time::Instant::now() < closing_deadline,
                "deactivation did not close admission"
            );
            std::thread::yield_now();
        }
        reenter_tx.send(()).expect("allow nested admission");
        assert!(
            attempt_rx
                .recv_timeout(Duration::from_secs(1))
                .expect("nested admission did not block")
                .is_ok(),
            "the already-admitted thread must retain reentrant host access"
        );
        worker.join().expect("worker thread");
        done_rx
            .recv_timeout(Duration::from_secs(1))
            .expect("deactivation waited for nested lease");
        deactivate.join().expect("deactivation thread");
        match host.lease() {
            Err(error) => assert_eq!(error.code(), ErrorCode::Unavailable),
            Ok(_) => panic!("late external call was admitted"),
        }
    }

    #[test]
    fn malformed_successful_invoke_releases_exact_token_once_before_invalid_result() {
        let descriptor = (0, 1, 1);
        let host = MalformedBufferHost::new(descriptor, HmclStatus::HostError);
        let context = malformed_buffer_context(&host, false, true);

        assert_eq!(
            context
                .invoke("malformed", &Value::Null)
                .expect_err("malformed successful invoke")
                .code(),
            ErrorCode::InvalidResult
        );
        assert_eq!(
            host.releases
                .lock()
                .unwrap_or_else(|error| error.into_inner())
                .as_slice(),
            &[descriptor]
        );
        assert_eq!(context.cleanup_failures(), 1);
    }

    #[test]
    fn malformed_successful_allocate_releases_exact_token_once_before_invalid_result() {
        let descriptor = (std::ptr::dangling_mut::<u8>().addr(), 0, 2);
        let host = MalformedBufferHost::new(descriptor, HmclStatus::Ok);
        let context = malformed_buffer_context(&host, true, false);

        assert_eq!(
            context
                .allocate_wire(b"abc")
                .expect_err("undersized successful allocation")
                .code(),
            ErrorCode::InvalidResult
        );
        assert_eq!(
            host.releases
                .lock()
                .unwrap_or_else(|error| error.into_inner())
                .as_slice(),
            &[descriptor]
        );
        assert_eq!(context.cleanup_failures(), 0);
    }
}
