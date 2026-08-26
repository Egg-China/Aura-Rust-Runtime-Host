use hmcl_runtime_abi::{
    HMCL_BRIDGE_ABI_V1, HMCL_HOST_API_V1_PREFIX_SIZE, HMCL_PLUGIN_API_V1_PREFIX_SIZE,
    HmclCallbackId, HmclCapabilityToken, HmclHandleId, HmclHostApiV1, HmclOwnedBuffer,
    HmclPluginApiV1, HmclPluginId, HmclSlice, HmclStatus, hmcl_plugin_query_v1,
};
use std::ffi::c_void;
use std::mem::{align_of, offset_of, size_of};
use std::ptr::NonNull;

#[test]
fn host_api_v1_has_stable_prefix() {
    assert_eq!(align_of::<HmclHostApiV1>(), 8);
    assert_eq!(size_of::<HmclHostApiV1>(), 64);
    assert_eq!(HMCL_HOST_API_V1_PREFIX_SIZE, 64);
    assert_eq!(HmclStatus::Ok.into_raw(), 0);
    assert_eq!(HMCL_BRIDGE_ABI_V1, 1);
    assert_eq!(HmclHostApiV1::with_required_prefix().struct_size, 64);
}

#[test]
fn function_tables_start_with_size_and_version() {
    assert_eq!(offset_of!(HmclHostApiV1, struct_size), 0);
    assert_eq!(offset_of!(HmclHostApiV1, abi_version), 4);
    assert_eq!(offset_of!(HmclHostApiV1, context), 8);
    assert_eq!(offset_of!(HmclHostApiV1, allocate), 16);
    assert_eq!(offset_of!(HmclHostApiV1, release_buffer), 24);
    assert_eq!(offset_of!(HmclHostApiV1, log), 32);
    assert_eq!(offset_of!(HmclHostApiV1, invoke), 40);
    assert_eq!(offset_of!(HmclHostApiV1, retain_handle), 48);
    assert_eq!(offset_of!(HmclHostApiV1, release_handle), 56);
    assert_eq!(offset_of!(HmclPluginApiV1, struct_size), 0);
    assert_eq!(offset_of!(HmclPluginApiV1, abi_version), 4);
    assert_eq!(offset_of!(HmclPluginApiV1, context), 8);
    assert_eq!(offset_of!(HmclPluginApiV1, initialize), 16);
    assert_eq!(offset_of!(HmclPluginApiV1, invoke), 24);
    assert_eq!(offset_of!(HmclPluginApiV1, shutdown), 32);
    assert_eq!(align_of::<HmclPluginApiV1>(), 8);
    assert_eq!(size_of::<HmclPluginApiV1>(), 40);
    assert_eq!(HMCL_PLUGIN_API_V1_PREFIX_SIZE, 40);
    assert_eq!(HmclPluginApiV1::with_required_prefix().struct_size, 40);
}

#[test]
fn boundary_values_have_stable_c_layouts() {
    assert_eq!(size_of::<HmclStatus>(), 4);
    assert_eq!(align_of::<HmclStatus>(), 4);
    assert_eq!(size_of::<HmclSlice>(), 16);
    assert_eq!(align_of::<HmclSlice>(), 8);
    assert_eq!(offset_of!(HmclSlice, data), 0);
    assert_eq!(offset_of!(HmclSlice, len), 8);
    assert_eq!(size_of::<HmclOwnedBuffer>(), 24);
    assert_eq!(align_of::<HmclOwnedBuffer>(), 8);
    assert_eq!(offset_of!(HmclOwnedBuffer, data), 0);
    assert_eq!(offset_of!(HmclOwnedBuffer, len), 8);
    assert_eq!(offset_of!(HmclOwnedBuffer, capacity), 16);

    assert_eq!(size_of::<HmclPluginId>(), 8);
    assert_eq!(size_of::<HmclCapabilityToken>(), 8);
    assert_eq!(size_of::<HmclHandleId>(), 8);
    assert_eq!(size_of::<HmclCallbackId>(), 8);
    assert_eq!(align_of::<HmclPluginId>(), 8);
    assert_eq!(align_of::<HmclCapabilityToken>(), 8);
    assert_eq!(align_of::<HmclHandleId>(), 8);
    assert_eq!(align_of::<HmclCallbackId>(), 8);
    assert_eq!(size_of::<Option<hmcl_runtime_abi::HmclAllocateFn>>(), 8);
}

#[test]
fn owned_buffer_validates_pointer_length_capacity_and_allocation_request() {
    assert!(HmclOwnedBuffer::EMPTY.has_valid_layout());
    assert!(HmclOwnedBuffer::EMPTY.satisfies_allocation_request(0));
    assert!(!HmclOwnedBuffer::EMPTY.satisfies_allocation_request(1));

    let data = NonNull::<u8>::dangling().as_ptr();
    let allocated = HmclOwnedBuffer {
        data,
        len: 0,
        capacity: 32,
    };
    assert!(allocated.has_valid_layout());
    assert!(allocated.satisfies_allocation_request(32));
    assert!(!allocated.satisfies_allocation_request(33));

    assert!(
        !HmclOwnedBuffer {
            data: std::ptr::null_mut(),
            len: 0,
            capacity: 1,
        }
        .has_valid_layout()
    );
    assert!(
        !HmclOwnedBuffer {
            data,
            len: 1,
            capacity: 0,
        }
        .has_valid_layout()
    );
    assert!(
        !HmclOwnedBuffer {
            data,
            len: 9,
            capacity: 8,
        }
        .has_valid_layout()
    );
}

fn buffer_fields(buffer: &HmclOwnedBuffer) -> (*mut u8, u64, u64) {
    (buffer.data, buffer.len, buffer.capacity)
}

#[test]
fn failed_buffer_producing_callbacks_leave_output_unchanged() {
    unsafe extern "C" fn failed_allocate(
        _context: *mut c_void,
        _length: u64,
        _out_buffer: *mut HmclOwnedBuffer,
    ) -> HmclStatus {
        HmclStatus::HostError
    }

    unsafe extern "C" fn failed_host_invoke(
        _context: *mut c_void,
        _plugin: HmclPluginId,
        _token: HmclCapabilityToken,
        _operation: HmclSlice,
        _input: HmclSlice,
        _out_buffer: *mut HmclOwnedBuffer,
    ) -> HmclStatus {
        HmclStatus::HostError
    }

    unsafe extern "C" fn failed_plugin_invoke(
        _context: *mut c_void,
        _operation: HmclSlice,
        _input: HmclSlice,
        _callback: HmclCallbackId,
        _out_buffer: *mut HmclOwnedBuffer,
    ) -> HmclStatus {
        HmclStatus::PluginError
    }

    let data = NonNull::<u8>::dangling().as_ptr();
    let mut output = HmclOwnedBuffer {
        data,
        len: 3,
        capacity: 7,
    };
    let sentinel = buffer_fields(&output);
    let empty_slice = HmclSlice {
        data: std::ptr::null(),
        len: 0,
    };

    // SAFETY: Each test callback ignores all pointer inputs and deliberately reports failure.
    assert!(!unsafe { failed_allocate(std::ptr::null_mut(), 16, &mut output) }.is_ok());
    assert_eq!(buffer_fields(&output), sentinel);
    // SAFETY: Each test callback ignores all pointer inputs and deliberately reports failure.
    assert!(
        !unsafe {
            failed_host_invoke(
                std::ptr::null_mut(),
                HmclPluginId::from_raw(1),
                HmclCapabilityToken::from_raw(2),
                empty_slice,
                empty_slice,
                &mut output,
            )
        }
        .is_ok()
    );
    assert_eq!(buffer_fields(&output), sentinel);
    // SAFETY: Each test callback ignores all pointer inputs and deliberately reports failure.
    assert!(
        !unsafe {
            failed_plugin_invoke(
                std::ptr::null_mut(),
                empty_slice,
                empty_slice,
                HmclCallbackId::from_raw(3),
                &mut output,
            )
        }
        .is_ok()
    );
    assert_eq!(buffer_fields(&output), sentinel);
}

#[repr(C)]
struct FutureHostTable {
    prefix: HmclHostApiV1,
    future_callback: Option<unsafe extern "C" fn()>,
}

#[test]
fn future_table_extensions_do_not_change_the_v1_required_prefix() {
    assert!(size_of::<FutureHostTable>() >= 8 + 6 * size_of::<usize>());
    assert_eq!(HMCL_HOST_API_V1_PREFIX_SIZE, 64);
    assert_eq!(size_of::<FutureHostTable>(), 72);
    assert_ne!(
        size_of::<FutureHostTable>(),
        HMCL_HOST_API_V1_PREFIX_SIZE as usize
    );
    assert_eq!(HmclHostApiV1::with_required_prefix().struct_size, 64);
}

#[test]
fn status_values_are_fixed() {
    assert_eq!(HmclStatus::Ok.into_raw(), 0);
    assert_eq!(HmclStatus::InvalidArgument.into_raw(), 1);
    assert_eq!(HmclStatus::UnsupportedAbi.into_raw(), 2);
    assert_eq!(HmclStatus::BufferTooSmall.into_raw(), 3);
    assert_eq!(HmclStatus::HostError.into_raw(), 4);
    assert_eq!(HmclStatus::PluginError.into_raw(), 5);
}

#[test]
fn status_preserves_unknown_foreign_values_through_callbacks() {
    const FUTURE_STATUS: i32 = -41_337;

    unsafe extern "C" fn future_status(
        _context: *mut c_void,
        _length: u64,
        _out_buffer: *mut HmclOwnedBuffer,
    ) -> HmclStatus {
        HmclStatus::from_raw(FUTURE_STATUS)
    }

    let callback: hmcl_runtime_abi::HmclAllocateFn = future_status;
    let mut output = HmclOwnedBuffer {
        data: std::ptr::null_mut(),
        len: 0,
        capacity: 0,
    };
    // SAFETY: The test callback ignores its context and receives a live output value.
    let status = unsafe { callback(std::ptr::null_mut(), 0, &mut output) };

    assert_eq!(status.into_raw(), FUTURE_STATUS);
    assert!(!status.is_ok());
}

#[test]
fn optional_callbacks_are_null_safe() {
    let host = HmclHostApiV1::EMPTY;
    assert!(host.allocate.is_none());
    assert!(host.release_buffer.is_none());
    assert!(host.log.is_none());
    assert!(host.invoke.is_none());
    assert!(host.retain_handle.is_none());
    assert!(host.release_handle.is_none());

    let plugin = HmclPluginApiV1::EMPTY;
    assert!(plugin.initialize.is_none());
    assert!(plugin.invoke.is_none());
    assert!(plugin.shutdown.is_none());
}

#[test]
fn query_symbol_has_the_versioned_c_signature() {
    let query: unsafe extern "C" fn(*const HmclHostApiV1, *mut HmclPluginApiV1) -> HmclStatus =
        hmcl_plugin_query_v1;
    let _ = query;
}

#[cfg(any(unix, windows))]
fn runtime_abi_cdylib() -> std::path::PathBuf {
    let test_executable = std::env::current_exe().expect("the test executable path is available");
    let deps_directory = test_executable
        .parent()
        .expect("the test executable has a parent directory");
    let library_name = format!(
        "{}hmcl_runtime_abi{}",
        std::env::consts::DLL_PREFIX,
        std::env::consts::DLL_SUFFIX
    );
    let candidates = std::fs::read_dir(deps_directory)
        .expect("the Cargo dependency directory is readable")
        .filter_map(Result::ok)
        .map(|entry| entry.path())
        .filter(|path| {
            path.file_name()
                .is_some_and(|name| name == std::ffi::OsStr::new(&library_name))
        })
        .collect::<Vec<_>>();

    assert_eq!(
        candidates.len(),
        1,
        "expected exactly one {library_name} beside the test executable"
    );
    candidates.into_iter().next().expect("one candidate exists")
}

#[cfg(windows)]
mod dynamic_library {
    use std::ffi::{CStr, c_char, c_void};
    use std::os::windows::ffi::OsStrExt;
    use std::path::Path;
    use std::ptr::NonNull;

    #[link(name = "kernel32")]
    unsafe extern "system" {
        fn LoadLibraryW(file_name: *const u16) -> *mut c_void;
        fn GetProcAddress(module: *mut c_void, procedure_name: *const c_char) -> *mut c_void;
        fn FreeLibrary(module: *mut c_void) -> i32;
    }

    pub(super) struct DynamicLibrary(*mut c_void);

    impl DynamicLibrary {
        pub(super) fn open(path: &Path) -> Result<Self, std::io::Error> {
            let path = path
                .as_os_str()
                .encode_wide()
                .chain(std::iter::once(0))
                .collect::<Vec<_>>();
            // SAFETY: `path` is terminated and remains live for the duration of the call.
            let module = unsafe { LoadLibraryW(path.as_ptr()) };
            NonNull::new(module)
                .map(|module| Self(module.as_ptr()))
                .ok_or_else(std::io::Error::last_os_error)
        }

        pub(super) fn exports(&self, symbol: &CStr) -> bool {
            // SAFETY: `self.0` is a live module and `symbol` is null-terminated.
            !unsafe { GetProcAddress(self.0, symbol.as_ptr()) }.is_null()
        }
    }

    impl Drop for DynamicLibrary {
        fn drop(&mut self) {
            // SAFETY: `self.0` is a module returned by `LoadLibraryW` and is released once here.
            let _ = unsafe { FreeLibrary(self.0) };
        }
    }
}

#[cfg(unix)]
mod dynamic_library {
    use std::ffi::{CStr, CString, c_char, c_int, c_void};
    use std::os::unix::ffi::OsStrExt;
    use std::path::Path;
    use std::ptr::NonNull;

    const RTLD_NOW: c_int = 2;

    #[cfg_attr(not(target_vendor = "apple"), link(name = "dl"))]
    unsafe extern "C" {
        fn dlopen(file_name: *const c_char, flags: c_int) -> *mut c_void;
        fn dlsym(handle: *mut c_void, symbol: *const c_char) -> *mut c_void;
        fn dlclose(handle: *mut c_void) -> c_int;
        fn dlerror() -> *const c_char;
    }

    pub(super) struct DynamicLibrary(*mut c_void);

    impl DynamicLibrary {
        pub(super) fn open(path: &Path) -> Result<Self, String> {
            let path = CString::new(path.as_os_str().as_bytes())
                .map_err(|_| "the dynamic library path contains a null byte".to_owned())?;
            // SAFETY: `path` is null-terminated and `RTLD_NOW` is a valid loader flag.
            let module = unsafe { dlopen(path.as_ptr(), RTLD_NOW) };
            NonNull::new(module)
                .map(|module| Self(module.as_ptr()))
                .ok_or_else(last_error)
        }

        pub(super) fn exports(&self, symbol: &CStr) -> bool {
            // SAFETY: `self.0` is a live module and `symbol` is null-terminated.
            !unsafe { dlsym(self.0, symbol.as_ptr()) }.is_null()
        }
    }

    impl Drop for DynamicLibrary {
        fn drop(&mut self) {
            // SAFETY: `self.0` is a module returned by `dlopen` and is closed once here.
            let _ = unsafe { dlclose(self.0) };
        }
    }

    fn last_error() -> String {
        // SAFETY: `dlerror` returns either null or a null-terminated loader-owned string.
        let error = unsafe { dlerror() };
        if error.is_null() {
            "dynamic loader did not provide an error".to_owned()
        } else {
            // SAFETY: The non-null result is a null-terminated string valid until the next loader
            // operation on this thread; it is copied before returning.
            unsafe { CStr::from_ptr(error) }
                .to_string_lossy()
                .into_owned()
        }
    }
}

#[cfg(any(unix, windows))]
#[test]
fn cdylib_exports_the_versioned_query_symbol() {
    let path = runtime_abi_cdylib();
    let library = dynamic_library::DynamicLibrary::open(&path)
        .unwrap_or_else(|error| panic!("failed to load {}: {error}", path.display()));

    assert!(
        library.exports(c"hmcl_plugin_query_v1"),
        "{} does not export hmcl_plugin_query_v1",
        path.display()
    );
}

#[test]
fn query_rejects_null_and_short_tables() {
    let host = HmclHostApiV1::EMPTY;
    let mut plugin = HmclPluginApiV1::EMPTY;

    // SAFETY: Null pointers are intentionally supplied to exercise query validation.
    assert_eq!(
        unsafe { hmcl_plugin_query_v1(std::ptr::null(), &mut plugin) },
        HmclStatus::InvalidArgument
    );
    // SAFETY: Null pointers are intentionally supplied to exercise query validation.
    assert_eq!(
        unsafe { hmcl_plugin_query_v1(&host, std::ptr::null_mut()) },
        HmclStatus::InvalidArgument
    );

    let short_host = HmclHostApiV1 {
        struct_size: HMCL_HOST_API_V1_PREFIX_SIZE - 1,
        ..HmclHostApiV1::EMPTY
    };
    // SAFETY: Both pointers refer to live, correctly aligned table values.
    assert_eq!(
        unsafe { hmcl_plugin_query_v1(&short_host, &mut plugin) },
        HmclStatus::BufferTooSmall
    );

    let host = HmclHostApiV1::with_required_prefix();
    let mut short_plugin = HmclPluginApiV1 {
        struct_size: HMCL_PLUGIN_API_V1_PREFIX_SIZE - 1,
        ..HmclPluginApiV1::EMPTY
    };
    // SAFETY: Both pointers refer to live, correctly aligned table values.
    assert_eq!(
        unsafe { hmcl_plugin_query_v1(&host, &mut short_plugin) },
        HmclStatus::BufferTooSmall
    );
}

struct SizeOnlyPrefix {
    allocation: NonNull<u8>,
    layout: std::alloc::Layout,
}

impl SizeOnlyPrefix {
    fn new() -> Self {
        let table_alignment = align_of::<HmclHostApiV1>().max(align_of::<HmclPluginApiV1>());
        let layout = std::alloc::Layout::from_size_align(size_of::<u32>(), table_alignment)
            .expect("the ABI prefix layout is valid");
        // SAFETY: `layout` has non-zero size and valid power-of-two alignment.
        let allocation = NonNull::new(unsafe { std::alloc::alloc(layout) })
            .expect("the test prefix allocation succeeds");
        // SAFETY: The allocation is aligned for and contains exactly one `u32`.
        unsafe {
            allocation
                .cast::<u32>()
                .as_ptr()
                .write(size_of::<u32>() as u32)
        };
        Self { allocation, layout }
    }

    fn as_host(&self) -> *const HmclHostApiV1 {
        self.allocation.as_ptr().cast()
    }

    fn as_plugin(&mut self) -> *mut HmclPluginApiV1 {
        self.allocation.as_ptr().cast()
    }

    fn struct_size(&self) -> u32 {
        // SAFETY: The allocation contains one initialized `u32` for its entire lifetime.
        unsafe { self.allocation.cast::<u32>().as_ptr().read() }
    }
}

impl Drop for SizeOnlyPrefix {
    fn drop(&mut self) {
        // SAFETY: `allocation` was returned by `alloc` for this exact layout and is still live.
        unsafe { std::alloc::dealloc(self.allocation.as_ptr(), self.layout) };
    }
}

#[test]
fn query_rejects_genuinely_four_byte_table_allocations_without_writing_output() {
    let short_host = SizeOnlyPrefix::new();
    let sentinel_context = NonNull::<u8>::dangling().as_ptr().cast::<c_void>();
    let mut plugin = HmclPluginApiV1 {
        context: sentinel_context,
        ..HmclPluginApiV1::with_required_prefix()
    };

    // SAFETY: `short_host` is aligned for the host table and contains its advertised four-byte
    // prefix. The implementation must reject that size before reading the absent version field.
    assert_eq!(
        unsafe { hmcl_plugin_query_v1(short_host.as_host(), &mut plugin) },
        HmclStatus::BufferTooSmall
    );
    assert_eq!(plugin.context, sentinel_context);
    assert_eq!(plugin.struct_size, HMCL_PLUGIN_API_V1_PREFIX_SIZE);
    assert_eq!(plugin.abi_version, HMCL_BRIDGE_ABI_V1);

    let host = HmclHostApiV1::with_required_prefix();
    let mut short_plugin = SizeOnlyPrefix::new();
    // SAFETY: `short_plugin` is aligned for the plugin table and contains its advertised four-byte
    // prefix. The implementation must reject that size before reading or writing any other field.
    assert_eq!(
        unsafe { hmcl_plugin_query_v1(&host, short_plugin.as_plugin()) },
        HmclStatus::BufferTooSmall
    );
    assert_eq!(short_plugin.struct_size(), size_of::<u32>() as u32);
}

#[test]
fn query_rejects_unknown_versions() {
    let host = HmclHostApiV1 {
        abi_version: HMCL_BRIDGE_ABI_V1 + 1,
        ..HmclHostApiV1::with_required_prefix()
    };
    let mut plugin = HmclPluginApiV1::with_required_prefix();
    // SAFETY: Both pointers refer to live, correctly aligned table values.
    assert_eq!(
        unsafe { hmcl_plugin_query_v1(&host, &mut plugin) },
        HmclStatus::UnsupportedAbi
    );

    let host = HmclHostApiV1::with_required_prefix();
    plugin.abi_version = HMCL_BRIDGE_ABI_V1 + 1;
    // SAFETY: Both pointers refer to live, correctly aligned table values.
    assert_eq!(
        unsafe { hmcl_plugin_query_v1(&host, &mut plugin) },
        HmclStatus::UnsupportedAbi
    );
    assert_eq!(plugin.abi_version, HMCL_BRIDGE_ABI_V1 + 1);
}

#[repr(C)]
struct ExtendedHostTable {
    prefix: HmclHostApiV1,
    trailing_field: u64,
}

#[repr(C)]
struct ExtendedPluginTable {
    prefix: HmclPluginApiV1,
    trailing_field: u64,
}

#[test]
fn query_accepts_larger_tables_without_touching_trailing_fields() {
    let host = ExtendedHostTable {
        prefix: HmclHostApiV1 {
            struct_size: size_of::<ExtendedHostTable>() as u32,
            ..HmclHostApiV1::with_required_prefix()
        },
        trailing_field: 0x0123_4567_89ab_cdef,
    };
    let mut plugin = ExtendedPluginTable {
        prefix: HmclPluginApiV1 {
            struct_size: size_of::<ExtendedPluginTable>() as u32,
            ..HmclPluginApiV1::with_required_prefix()
        },
        trailing_field: 0xfeed_face_cafe_beef,
    };

    // SAFETY: `prefix` begins at offset zero and advertises the complete allocation size.
    let status = unsafe { hmcl_plugin_query_v1(&host.prefix, &mut plugin.prefix) };
    assert_eq!(status, HmclStatus::Ok);
    assert_eq!(plugin.prefix.struct_size, HMCL_PLUGIN_API_V1_PREFIX_SIZE);
    assert_eq!(plugin.prefix.abi_version, HMCL_BRIDGE_ABI_V1);
    assert_eq!(host.trailing_field, 0x0123_4567_89ab_cdef);
    assert_eq!(plugin.trailing_field, 0xfeed_face_cafe_beef);
}

#[test]
fn callback_types_use_c_compatible_arguments() {
    unsafe extern "C" fn allocate(
        _context: *mut c_void,
        _length: u64,
        _out_buffer: *mut HmclOwnedBuffer,
    ) -> HmclStatus {
        HmclStatus::Ok
    }

    let mut host = HmclHostApiV1::with_required_prefix();
    host.allocate = Some(allocate);
    assert!(host.allocate.is_some());
}
