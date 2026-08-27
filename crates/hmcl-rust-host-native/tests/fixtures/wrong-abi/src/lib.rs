use hmcl_runtime_abi::{HmclHostApiV1, HmclPluginApiV1, HmclStatus};

#[unsafe(no_mangle)]
pub unsafe extern "C" fn hmcl_plugin_query_v1(
    _host: *const HmclHostApiV1,
    out_plugin: *mut HmclPluginApiV1,
) -> HmclStatus {
    if out_plugin.is_null() {
        return HmclStatus::InvalidArgument;
    }
    let table = HmclPluginApiV1 {
        abi_version: 2,
        ..HmclPluginApiV1::with_required_prefix()
    };
    // SAFETY: The test Host supplies writable storage for the complete table.
    unsafe { out_plugin.write(table) };
    HmclStatus::Ok
}
