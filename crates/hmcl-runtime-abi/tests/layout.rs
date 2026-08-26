use hmcl_runtime_abi::{
    HMCL_BRIDGE_ABI_V1, HmclCallbackId, HmclCapabilityToken, HmclHandleId, HmclHostApiV1,
    HmclOwnedBuffer, HmclPluginApiV1, HmclPluginId, HmclSlice, HmclStatus, hmcl_plugin_query_v1,
};
use std::ffi::c_void;
use std::mem::{align_of, offset_of, size_of};
use std::ptr::NonNull;

#[test]
fn host_api_v1_has_stable_prefix() {
    assert_eq!(align_of::<HmclHostApiV1>(), align_of::<usize>());
    assert_eq!(HmclStatus::Ok as i32, 0);
    assert_eq!(HMCL_BRIDGE_ABI_V1, 1);
    assert!(size_of::<HmclHostApiV1>() >= 8 + 6 * size_of::<usize>());
}

#[test]
fn function_tables_start_with_size_and_version() {
    let pointer_size = size_of::<usize>();
    assert_eq!(offset_of!(HmclHostApiV1, struct_size), 0);
    assert_eq!(offset_of!(HmclHostApiV1, abi_version), 4);
    assert_eq!(offset_of!(HmclHostApiV1, context), 8);
    assert_eq!(offset_of!(HmclHostApiV1, allocate), 8 + pointer_size);
    assert_eq!(
        offset_of!(HmclHostApiV1, release_buffer),
        8 + 2 * pointer_size
    );
    assert_eq!(offset_of!(HmclHostApiV1, log), 8 + 3 * pointer_size);
    assert_eq!(offset_of!(HmclHostApiV1, invoke), 8 + 4 * pointer_size);
    assert_eq!(
        offset_of!(HmclHostApiV1, retain_handle),
        8 + 5 * pointer_size
    );
    assert_eq!(
        offset_of!(HmclHostApiV1, release_handle),
        8 + 6 * pointer_size
    );
    assert_eq!(offset_of!(HmclPluginApiV1, struct_size), 0);
    assert_eq!(offset_of!(HmclPluginApiV1, abi_version), 4);
    assert_eq!(offset_of!(HmclPluginApiV1, context), 8);
    assert_eq!(offset_of!(HmclPluginApiV1, initialize), 8 + pointer_size);
    assert_eq!(offset_of!(HmclPluginApiV1, invoke), 8 + 2 * pointer_size);
    assert_eq!(offset_of!(HmclPluginApiV1, shutdown), 8 + 3 * pointer_size);
}

#[test]
fn boundary_values_have_stable_c_layouts() {
    assert_eq!(offset_of!(HmclSlice, data), 0);
    assert_eq!(offset_of!(HmclSlice, len), size_of::<usize>());
    assert_eq!(align_of::<HmclOwnedBuffer>(), align_of::<usize>());
    assert_eq!(offset_of!(HmclOwnedBuffer, data), 0);
    assert_eq!(offset_of!(HmclOwnedBuffer, len), size_of::<usize>());
    assert_eq!(
        offset_of!(HmclOwnedBuffer, capacity),
        size_of::<usize>() + 8
    );

    assert_eq!(size_of::<HmclPluginId>(), 8);
    assert_eq!(size_of::<HmclCapabilityToken>(), 8);
    assert_eq!(size_of::<HmclHandleId>(), 8);
    assert_eq!(size_of::<HmclCallbackId>(), 8);
    assert_eq!(align_of::<HmclPluginId>(), align_of::<u64>());
    assert_eq!(
        size_of::<Option<hmcl_runtime_abi::HmclAllocateFn>>(),
        size_of::<usize>()
    );
}

#[test]
fn status_discriminants_are_fixed() {
    assert_eq!(HmclStatus::Ok as i32, 0);
    assert_eq!(HmclStatus::InvalidArgument as i32, 1);
    assert_eq!(HmclStatus::UnsupportedAbi as i32, 2);
    assert_eq!(HmclStatus::BufferTooSmall as i32, 3);
    assert_eq!(HmclStatus::HostError as i32, 4);
    assert_eq!(HmclStatus::PluginError as i32, 5);
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
        struct_size: (size_of::<HmclHostApiV1>() - 1) as u32,
        ..HmclHostApiV1::EMPTY
    };
    // SAFETY: Both pointers refer to live, correctly aligned table values.
    assert_eq!(
        unsafe { hmcl_plugin_query_v1(&short_host, &mut plugin) },
        HmclStatus::BufferTooSmall
    );

    let host = HmclHostApiV1::with_required_prefix();
    let mut short_plugin = HmclPluginApiV1 {
        struct_size: (size_of::<HmclPluginApiV1>() - 1) as u32,
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
    assert_eq!(plugin.struct_size, size_of::<HmclPluginApiV1>() as u32);
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
    assert_eq!(
        plugin.prefix.struct_size,
        size_of::<HmclPluginApiV1>() as u32
    );
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
