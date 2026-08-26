use hmcl_runtime_abi::{
    HMCL_BRIDGE_ABI_V1, HmclCallbackId, HmclCapabilityToken, HmclHandleId, HmclHostApiV1,
    HmclOwnedBuffer, HmclPluginApiV1, HmclPluginId, HmclSlice, HmclStatus, hmcl_plugin_query_v1,
};
use std::ffi::c_void;
use std::mem::{align_of, offset_of, size_of};

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
