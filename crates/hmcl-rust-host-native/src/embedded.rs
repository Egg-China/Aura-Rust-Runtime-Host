//! Embedded dynamic-library loading and ABI lifecycle ownership.

use crate::HostError;
use hmcl_runtime_abi::{
    HMCL_BRIDGE_ABI_V1, HMCL_PLUGIN_API_V1_PREFIX_SIZE, HmclCallbackId, HmclCapabilityToken,
    HmclHandleId, HmclHostApiV1, HmclOwnedBuffer, HmclPluginApiV1, HmclPluginId, HmclSlice,
    HmclStatus,
};
use libloading::Library;
use std::collections::HashMap;
use std::ffi::c_void;
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

type QueryV1 = unsafe extern "C" fn(*const HmclHostApiV1, *mut HmclPluginApiV1) -> HmclStatus;

/// Java-facing transport used by Rust payloads to invoke launcher Bridge operations.
///
/// The numeric `session` is a Host-owned lookup handle. It is not a serialized Java capability
/// token; the Java boundary resolves current short-lived authority for every call.
pub trait BridgeTransport: Send + Sync + 'static {
    /// Invokes one canonical Bridge operation for the current payload session.
    fn invoke(
        &self,
        plugin: u64,
        session: u64,
        operation: &str,
        input: &[u8],
    ) -> Result<Vec<u8>, HostError>;

    /// Retains one generation-safe launcher object handle.
    fn retain_handle(&self, session: u64, object_id: u64, generation: u64)
    -> Result<(), HostError>;

    /// Releases one generation-safe launcher object handle.
    fn release_handle(
        &self,
        session: u64,
        object_id: u64,
        generation: u64,
    ) -> Result<(), HostError>;
}

/// Provider-wide owner of embedded payload libraries and their lifecycle state.
pub struct Engine {
    /// Java Bridge transport shared by every loaded payload.
    bridge: Arc<dyn BridgeTransport>,

    /// Mutable payload registry and deterministic load order.
    state: Mutex<EngineState>,
}

impl Engine {
    /// Creates an empty native engine using one Java Bridge transport.
    #[must_use]
    pub fn new(bridge: Arc<dyn BridgeTransport>) -> Self {
        Self {
            bridge,
            state: Mutex::new(EngineState::default()),
        }
    }

    /// Loads and queries one verified embedded `cdylib` without initializing its plugin session.
    pub fn load_payload(
        &self,
        package_root: &Path,
        entrypoint: &Path,
        plugin_id: u64,
        session: u64,
    ) -> Result<u64, HostError> {
        if plugin_id == 0 || session == 0 {
            return Err(HostError::InvalidState(
                "plugin and session identifiers must be nonzero",
            ));
        }
        let library_path = resolve_entrypoint(package_root, entrypoint)?;
        let mut state = lock(&self.state);
        if state.closed {
            return Err(HostError::EngineClosed);
        }
        let payload =
            EmbeddedPayload::load(&library_path, plugin_id, session, Arc::clone(&self.bridge))?;
        let payload_id = state.next_payload_id;
        state.next_payload_id =
            state
                .next_payload_id
                .checked_add(1)
                .ok_or(HostError::InvalidState(
                    "payload identifier space exhausted",
                ))?;
        state
            .payloads
            .insert(payload_id, Arc::new(Mutex::new(payload)));
        state.load_order.push(payload_id);
        Ok(payload_id)
    }

    /// Initializes or re-enables one loaded embedded payload.
    pub fn enable_payload(&self, payload_id: u64) -> Result<(), HostError> {
        let payload = self.payload(payload_id)?;
        lock(&payload).enable()
    }

    /// Stops admitting ordinary invocations while retaining the initialized payload session.
    pub fn disable_payload(&self, payload_id: u64) -> Result<(), HostError> {
        let payload = self.payload(payload_id)?;
        lock(&payload).disable()
    }

    /// Invokes one operation on an enabled payload and copies its Host-owned result bytes.
    pub fn invoke_payload(
        &self,
        payload_id: u64,
        operation: &str,
        input: &[u8],
        callback: u64,
    ) -> Result<Vec<u8>, HostError> {
        let payload = self.payload(payload_id)?;
        lock(&payload).invoke(operation, input, callback)
    }

    /// Shuts down and unloads one disabled payload.
    ///
    /// A failed shutdown is retained as a terminal state. A second unload completes library
    /// removal without calling the payload callback again, matching Supervisor retry semantics.
    pub fn unload_payload(&self, payload_id: u64) -> Result<(), HostError> {
        let payload = self.payload(payload_id)?;
        let shutdown = lock(&payload).shutdown_for_unload();
        match shutdown {
            Ok(()) => {
                let mut state = lock(&self.state);
                state.payloads.remove(&payload_id);
                state.load_order.retain(|current| *current != payload_id);
                Ok(())
            }
            Err(error) => Err(error),
        }
    }

    /// Closes all payloads in reverse load order and rejects future work.
    pub fn close(&self) -> Result<(), HostError> {
        let payloads = {
            let mut state = lock(&self.state);
            if state.closed {
                return Ok(());
            }
            state.closed = true;
            let payloads = state
                .load_order
                .iter()
                .rev()
                .filter_map(|id| state.payloads.get(id).cloned())
                .collect::<Vec<_>>();
            state.load_order.clear();
            state.payloads.clear();
            payloads
        };
        let mut first_failure = None;
        for payload in payloads {
            if let Err(error) = lock(&payload).force_shutdown()
                && first_failure.is_none()
            {
                first_failure = Some(error);
            }
        }
        match first_failure {
            Some(error) => Err(error),
            None => Ok(()),
        }
    }

    fn payload(&self, payload_id: u64) -> Result<Arc<Mutex<EmbeddedPayload>>, HostError> {
        let state = lock(&self.state);
        if state.closed {
            return Err(HostError::EngineClosed);
        }
        state
            .payloads
            .get(&payload_id)
            .cloned()
            .ok_or(HostError::UnknownPayload(payload_id))
    }
}

impl Drop for Engine {
    fn drop(&mut self) {
        let _ = self.close();
    }
}

struct EngineState {
    next_payload_id: u64,
    load_order: Vec<u64>,
    payloads: HashMap<u64, Arc<Mutex<EmbeddedPayload>>>,
    closed: bool,
}

impl Default for EngineState {
    fn default() -> Self {
        Self {
            next_payload_id: 1,
            load_order: Vec::new(),
            payloads: HashMap::new(),
            closed: false,
        }
    }
}

struct EmbeddedPayload {
    _library: Library,
    host: Box<HostCallbacks>,
    host_table: HmclHostApiV1,
    plugin_table: HmclPluginApiV1,
    state: PayloadState,
}

// SAFETY: `Engine` admits access only through this payload's mutex. Both ABI context pointers refer
// to stable allocations retained by this struct or the loaded library, and the ABI contract
// requires plugin callbacks to support invocation from arbitrary attached Host threads.
unsafe impl Send for EmbeddedPayload {}

impl EmbeddedPayload {
    fn load(
        library_path: &Path,
        plugin_id: u64,
        session: u64,
        bridge: Arc<dyn BridgeTransport>,
    ) -> Result<Self, HostError> {
        // SAFETY: The canonical path identifies a verified regular file retained by this owner.
        let library = unsafe { Library::new(library_path) }?;
        let query = {
            // SAFETY: The copied function pointer remains valid while `library` is retained.
            let symbol = unsafe { library.get::<QueryV1>(b"hmcl_plugin_query_v1\0") }
                .map_err(|_| HostError::MissingQuerySymbol)?;
            *symbol
        };
        let mut host = Box::new(HostCallbacks::new(plugin_id, session, bridge));
        let host_table = host.table();
        let mut plugin_table = HmclPluginApiV1::with_required_prefix();
        // SAFETY: Both complete tables remain aligned and valid throughout the query call.
        let status = unsafe { query(&host_table, &mut plugin_table) };
        if !status.is_ok() {
            return Err(HostError::QueryStatus(status.into_raw()));
        }
        if plugin_table.abi_version != HMCL_BRIDGE_ABI_V1 {
            return Err(HostError::UnsupportedPluginAbi(plugin_table.abi_version));
        }
        if plugin_table.struct_size < HMCL_PLUGIN_API_V1_PREFIX_SIZE
            || plugin_table.initialize.is_none()
            || plugin_table.invoke.is_none()
            || plugin_table.shutdown.is_none()
        {
            return Err(HostError::InvalidPluginTable);
        }
        host.arm();
        Ok(Self {
            _library: library,
            host,
            host_table,
            plugin_table,
            state: PayloadState::Loaded,
        })
    }

    fn enable(&mut self) -> Result<(), HostError> {
        match self.state {
            PayloadState::Loaded => {
                let initialize = self
                    .plugin_table
                    .initialize
                    .ok_or(HostError::InvalidPluginTable)?;
                // SAFETY: Query supplied this callback and context. Host tables and IDs stay live.
                let status = unsafe {
                    initialize(
                        self.plugin_table.context,
                        &self.host_table,
                        HmclPluginId::from_raw(self.host.plugin_id),
                        HmclCapabilityToken::from_raw(self.host.session),
                    )
                };
                require_plugin_status(status)?;
                self.state = PayloadState::Enabled;
                Ok(())
            }
            PayloadState::Disabled => {
                self.state = PayloadState::Enabled;
                Ok(())
            }
            PayloadState::Enabled => Ok(()),
            PayloadState::ShutdownFailed | PayloadState::Shutdown => {
                Err(HostError::InvalidState("payload has shut down"))
            }
        }
    }

    fn disable(&mut self) -> Result<(), HostError> {
        match self.state {
            PayloadState::Enabled => {
                self.state = PayloadState::Disabled;
                Ok(())
            }
            PayloadState::Disabled => Ok(()),
            PayloadState::Loaded => Err(HostError::InvalidState("payload is not enabled")),
            PayloadState::ShutdownFailed | PayloadState::Shutdown => {
                Err(HostError::InvalidState("payload has shut down"))
            }
        }
    }

    fn invoke(
        &mut self,
        operation: &str,
        input: &[u8],
        callback: u64,
    ) -> Result<Vec<u8>, HostError> {
        if self.state != PayloadState::Enabled {
            return Err(HostError::InvalidState("payload is not enabled"));
        }
        let invoke = self
            .plugin_table
            .invoke
            .ok_or(HostError::InvalidPluginTable)?;
        let operation = slice(operation.as_bytes())?;
        let input = slice(input)?;
        let mut output = HmclOwnedBuffer::EMPTY;
        // SAFETY: Query supplied the callback and context; borrowed inputs and output remain valid.
        let status = unsafe {
            invoke(
                self.plugin_table.context,
                operation,
                input,
                HmclCallbackId::from_raw(callback),
                &mut output,
            )
        };
        if !status.is_ok() {
            return Err(HostError::PluginStatus(status.into_raw()));
        }
        self.host.take_output(output)
    }

    fn shutdown_for_unload(&mut self) -> Result<(), HostError> {
        match self.state {
            PayloadState::Enabled => Err(HostError::InvalidState("payload is still enabled")),
            PayloadState::Loaded | PayloadState::Shutdown => {
                self.state = PayloadState::Shutdown;
                Ok(())
            }
            PayloadState::Disabled => self.shutdown_once(),
            PayloadState::ShutdownFailed => {
                self.state = PayloadState::Shutdown;
                Ok(())
            }
        }
    }

    fn force_shutdown(&mut self) -> Result<(), HostError> {
        match self.state {
            PayloadState::Loaded | PayloadState::Shutdown | PayloadState::ShutdownFailed => {
                self.state = PayloadState::Shutdown;
                Ok(())
            }
            PayloadState::Enabled | PayloadState::Disabled => self.shutdown_once(),
        }
    }

    fn shutdown_once(&mut self) -> Result<(), HostError> {
        let shutdown = self
            .plugin_table
            .shutdown
            .ok_or(HostError::InvalidPluginTable)?;
        // SAFETY: Query supplied the callback and context; the engine serializes final shutdown.
        let status = unsafe {
            shutdown(
                self.plugin_table.context,
                HmclPluginId::from_raw(self.host.plugin_id),
            )
        };
        if status.is_ok() {
            self.state = PayloadState::Shutdown;
            Ok(())
        } else {
            self.state = PayloadState::ShutdownFailed;
            Err(HostError::PluginStatus(status.into_raw()))
        }
    }
}

#[derive(Clone, Copy, Eq, PartialEq)]
enum PayloadState {
    Loaded,
    Enabled,
    Disabled,
    ShutdownFailed,
    Shutdown,
}

struct HostCallbacks {
    plugin_id: u64,
    session: u64,
    bridge: Arc<dyn BridgeTransport>,
    allocations: Mutex<HashMap<usize, Box<[u8]>>>,
    armed: bool,
}

impl HostCallbacks {
    fn new(plugin_id: u64, session: u64, bridge: Arc<dyn BridgeTransport>) -> Self {
        Self {
            plugin_id,
            session,
            bridge,
            allocations: Mutex::new(HashMap::new()),
            armed: false,
        }
    }

    fn arm(&mut self) {
        self.armed = true;
    }

    fn table(&mut self) -> HmclHostApiV1 {
        HmclHostApiV1 {
            context: std::ptr::from_mut(self).cast::<c_void>(),
            allocate: Some(host_allocate),
            release_buffer: Some(host_release_buffer),
            log: Some(host_log),
            invoke: Some(host_invoke),
            retain_handle: Some(host_retain_handle),
            release_handle: Some(host_release_handle),
            ..HmclHostApiV1::with_required_prefix()
        }
    }

    fn allocate(
        &self,
        length: usize,
        initialized: Option<&[u8]>,
    ) -> Result<HmclOwnedBuffer, HostError> {
        if length == 0 {
            return Ok(HmclOwnedBuffer::EMPTY);
        }
        let mut allocation = vec![0_u8; length].into_boxed_slice();
        if let Some(bytes) = initialized {
            if bytes.len() != length {
                return Err(HostError::InvalidHostResult);
            }
            allocation.copy_from_slice(bytes);
        }
        let pointer = allocation.as_mut_ptr();
        lock(&self.allocations).insert(pointer as usize, allocation);
        Ok(HmclOwnedBuffer {
            data: pointer,
            len: initialized.map_or(0, <[u8]>::len) as u64,
            capacity: length as u64,
        })
    }

    fn release(&self, buffer: HmclOwnedBuffer) -> Result<(), HostError> {
        if buffer.capacity == 0 {
            return if buffer.data.is_null() && buffer.len == 0 {
                Ok(())
            } else {
                Err(HostError::InvalidHostResult)
            };
        }
        let allocation = lock(&self.allocations).remove(&(buffer.data as usize));
        let Some(allocation) = allocation else {
            return Err(HostError::InvalidHostResult);
        };
        if allocation.len() as u64 != buffer.capacity || buffer.len > buffer.capacity {
            return Err(HostError::InvalidHostResult);
        }
        Ok(())
    }

    fn take_output(&self, buffer: HmclOwnedBuffer) -> Result<Vec<u8>, HostError> {
        if buffer.capacity == 0 {
            return if buffer.data.is_null() && buffer.len == 0 {
                Ok(Vec::new())
            } else {
                Err(HostError::InvalidHostResult)
            };
        }
        let allocation = lock(&self.allocations).remove(&(buffer.data as usize));
        let Some(allocation) = allocation else {
            return Err(HostError::InvalidHostResult);
        };
        if allocation.len() as u64 != buffer.capacity || buffer.len > buffer.capacity {
            return Err(HostError::InvalidHostResult);
        }
        let length = usize::try_from(buffer.len).map_err(|_| HostError::InvalidHostResult)?;
        Ok(allocation[..length].to_vec())
    }
}

unsafe extern "C" fn host_allocate(
    context: *mut c_void,
    length: u64,
    out_buffer: *mut HmclOwnedBuffer,
) -> HmclStatus {
    boundary(|| {
        let host = unsafe { host_context(context)? };
        if out_buffer.is_null() || !out_buffer.is_aligned() {
            return Err(HmclStatus::InvalidArgument);
        }
        let length = usize::try_from(length).map_err(|_| HmclStatus::InvalidArgument)?;
        let allocation = host
            .allocate(length, None)
            .map_err(|_| HmclStatus::HostError)?;
        // SAFETY: The callback contract supplies aligned writable output storage.
        unsafe { out_buffer.write(allocation) };
        Ok(())
    })
}

unsafe extern "C" fn host_release_buffer(
    context: *mut c_void,
    buffer: *mut HmclOwnedBuffer,
) -> HmclStatus {
    boundary(|| {
        let host = unsafe { host_context(context)? };
        if buffer.is_null() || !buffer.is_aligned() {
            return Err(HmclStatus::InvalidArgument);
        }
        // SAFETY: The callback contract transfers one writable ownership descriptor.
        let owned = unsafe { std::ptr::replace(buffer, HmclOwnedBuffer::EMPTY) };
        host.release(owned).map_err(|_| HmclStatus::InvalidArgument)
    })
}

unsafe extern "C" fn host_log(context: *mut c_void, _level: u32, message: HmclSlice) -> HmclStatus {
    boundary(|| {
        let _host = unsafe { host_context(context)? };
        let bytes = unsafe { read_slice(message)? };
        std::str::from_utf8(bytes).map_err(|_| HmclStatus::InvalidArgument)?;
        Ok(())
    })
}

unsafe extern "C" fn host_invoke(
    context: *mut c_void,
    plugin: HmclPluginId,
    token: HmclCapabilityToken,
    operation: HmclSlice,
    input: HmclSlice,
    out_buffer: *mut HmclOwnedBuffer,
) -> HmclStatus {
    boundary(|| {
        let host = unsafe { host_context(context)? };
        if plugin.into_raw() != host.plugin_id
            || token.into_raw() != host.session
            || out_buffer.is_null()
            || !out_buffer.is_aligned()
        {
            return Err(HmclStatus::InvalidArgument);
        }
        let operation = unsafe { read_slice(operation)? };
        let operation = std::str::from_utf8(operation).map_err(|_| HmclStatus::InvalidArgument)?;
        let input = unsafe { read_slice(input)? };
        let result = host
            .bridge
            .invoke(host.plugin_id, host.session, operation, input)
            .map_err(|_| HmclStatus::HostError)?;
        let output = host
            .allocate(result.len(), Some(&result))
            .map_err(|_| HmclStatus::HostError)?;
        // SAFETY: The callback contract supplies aligned writable output storage.
        unsafe { out_buffer.write(output) };
        Ok(())
    })
}

unsafe extern "C" fn host_retain_handle(context: *mut c_void, handle: HmclHandleId) -> HmclStatus {
    boundary(|| {
        let host = unsafe { host_context(context)? };
        let (object_id, generation) = handle.into_parts();
        host.bridge
            .retain_handle(host.session, object_id, generation)
            .map_err(|_| HmclStatus::HostError)
    })
}

unsafe extern "C" fn host_release_handle(context: *mut c_void, handle: HmclHandleId) -> HmclStatus {
    boundary(|| {
        let host = unsafe { host_context(context)? };
        let (object_id, generation) = handle.into_parts();
        host.bridge
            .release_handle(host.session, object_id, generation)
            .map_err(|_| HmclStatus::HostError)
    })
}

fn boundary(action: impl FnOnce() -> Result<(), HmclStatus>) -> HmclStatus {
    match catch_unwind(AssertUnwindSafe(action)) {
        Ok(Ok(())) => HmclStatus::Ok,
        Ok(Err(status)) => status,
        Err(_) => HmclStatus::HostError,
    }
}

unsafe fn host_context<'a>(context: *mut c_void) -> Result<&'a HostCallbacks, HmclStatus> {
    if context.is_null() || !context.is_aligned() {
        return Err(HmclStatus::InvalidArgument);
    }
    // SAFETY: Every host table stores a live `HostCallbacks` allocation in its context field.
    let host = unsafe { &*context.cast::<HostCallbacks>() };
    if !host.armed {
        return Err(HmclStatus::HostError);
    }
    Ok(host)
}

unsafe fn read_slice<'a>(slice: HmclSlice) -> Result<&'a [u8], HmclStatus> {
    let length = usize::try_from(slice.len).map_err(|_| HmclStatus::InvalidArgument)?;
    if length == 0 {
        return if slice.data.is_null() {
            Ok(&[])
        } else {
            Err(HmclStatus::InvalidArgument)
        };
    }
    if slice.data.is_null() {
        return Err(HmclStatus::InvalidArgument);
    }
    // SAFETY: The ABI callback contract guarantees `length` readable bytes.
    Ok(unsafe { std::slice::from_raw_parts(slice.data, length) })
}

fn slice(bytes: &[u8]) -> Result<HmclSlice, HostError> {
    let len = u64::try_from(bytes.len()).map_err(|_| HostError::InvalidHostResult)?;
    Ok(HmclSlice {
        data: if bytes.is_empty() {
            std::ptr::null()
        } else {
            bytes.as_ptr()
        },
        len,
    })
}

fn require_plugin_status(status: HmclStatus) -> Result<(), HostError> {
    if status.is_ok() {
        Ok(())
    } else {
        Err(HostError::PluginStatus(status.into_raw()))
    }
}

fn resolve_entrypoint(package_root: &Path, entrypoint: &Path) -> Result<PathBuf, HostError> {
    let root = package_root.canonicalize()?;
    let candidate = if entrypoint.is_absolute() {
        if !entrypoint.starts_with(package_root) && !entrypoint.starts_with(&root) {
            return Err(HostError::EntrypointEscape);
        }
        entrypoint.to_path_buf()
    } else {
        root.join(entrypoint)
    };
    let canonical = candidate.canonicalize()?;
    if !canonical.starts_with(&root) || !canonical.is_file() {
        return Err(HostError::EntrypointEscape);
    }
    Ok(canonical)
}

fn lock<T>(mutex: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(|error| error.into_inner())
}
