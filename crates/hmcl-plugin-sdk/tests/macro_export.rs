use hmcl_plugin_sdk::Value;
use hmcl_plugin_sdk::abi::{
    HMCL_PLUGIN_API_V1_PREFIX_SIZE, HmclCallbackId, HmclCapabilityToken, HmclHostApiV1,
    HmclOwnedBuffer, HmclPluginApiV1, HmclPluginId, HmclSlice, HmclStatus,
};
use libloading::{Library, Symbol};
use std::ffi::c_void;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::mpsc;
use std::time::{Duration, Instant};

type Query = unsafe extern "C" fn(*const HmclHostApiV1, *mut HmclPluginApiV1) -> HmclStatus;

static FREES: AtomicUsize = AtomicUsize::new(0);
static HOST_INVOKES: AtomicUsize = AtomicUsize::new(0);

unsafe extern "C" fn allocate(
    _context: *mut c_void,
    length: u64,
    out: *mut HmclOwnedBuffer,
) -> HmclStatus {
    let Ok(capacity) = usize::try_from(length) else {
        return HmclStatus::InvalidArgument;
    };
    let mut bytes = Vec::<u8>::with_capacity(capacity);
    let buffer = HmclOwnedBuffer {
        data: bytes.as_mut_ptr(),
        len: 0,
        capacity: bytes.capacity() as u64,
    };
    std::mem::forget(bytes);
    unsafe { out.write(buffer) };
    HmclStatus::Ok
}

unsafe extern "C" fn release_buffer(
    _context: *mut c_void,
    buffer: *mut HmclOwnedBuffer,
) -> HmclStatus {
    let owned = unsafe { buffer.read() };
    if owned.capacity != 0 {
        unsafe {
            drop(Vec::from_raw_parts(
                owned.data,
                owned.len as usize,
                owned.capacity as usize,
            ));
        }
    }
    FREES.fetch_add(1, Ordering::SeqCst);
    HmclStatus::Ok
}

unsafe extern "C" fn host_invoke(
    _context: *mut c_void,
    _plugin: HmclPluginId,
    _token: HmclCapabilityToken,
    _operation: HmclSlice,
    input: HmclSlice,
    output: *mut HmclOwnedBuffer,
) -> HmclStatus {
    HOST_INVOKES.fetch_add(1, Ordering::SeqCst);
    let input = unsafe { std::slice::from_raw_parts(input.data, input.len as usize) };
    let mut bytes = input.to_vec();
    let buffer = HmclOwnedBuffer {
        data: bytes.as_mut_ptr(),
        len: bytes.len() as u64,
        capacity: bytes.capacity() as u64,
    };
    std::mem::forget(bytes);
    unsafe { output.write(buffer) };
    HmclStatus::Ok
}

fn fixture_library(target: &Path) -> PathBuf {
    let name = if cfg!(target_os = "windows") {
        "downstream_plugin.dll"
    } else if cfg!(target_os = "macos") {
        "libdownstream_plugin.dylib"
    } else {
        "libdownstream_plugin.so"
    };
    target.join("debug").join(name)
}

fn dependency_abi_library(target: &Path) -> PathBuf {
    target.join("debug").join("deps").join(format!(
        "{}hmcl_runtime_abi{}",
        std::env::consts::DLL_PREFIX,
        std::env::consts::DLL_SUFFIX
    ))
}

fn slice(bytes: &[u8]) -> HmclSlice {
    HmclSlice {
        data: bytes.as_ptr(),
        len: bytes.len() as u64,
    }
}

#[test]
fn downstream_cdylib_exports_query_and_contains_all_panics() {
    let fixture =
        Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/downstream-plugin/Cargo.toml");
    let target =
        std::env::temp_dir().join(format!("hmcl-plugin-sdk-fixture-{}", std::process::id()));
    let output = Command::new(env!("CARGO"))
        .args(["build", "--manifest-path"])
        .arg(&fixture)
        .env("CARGO_TARGET_DIR", &target)
        .output()
        .expect("build downstream fixture");
    assert!(
        output.status.success(),
        "fixture build failed:\n{}",
        String::from_utf8_lossy(&output.stderr)
    );

    let dependency = unsafe {
        Library::new(dependency_abi_library(&target)).expect("load no-default-features ABI")
    };
    assert!(
        unsafe { dependency.get::<Query>(b"hmcl_plugin_query_v1\0") }.is_err(),
        "the SDK ABI dependency unexpectedly exported the query symbol"
    );

    let library = unsafe { Library::new(fixture_library(&target)).expect("load fixture") };
    let query: Symbol<'_, Query> = unsafe {
        library
            .get(b"hmcl_plugin_query_v1\0")
            .expect("query symbol")
    };
    let failed_context_inactive: Symbol<'_, unsafe extern "C" fn() -> u32> = unsafe {
        library
            .get(b"hmcl_fixture_failed_context_inactive\0")
            .expect("failed-context probe")
    };
    let shutdown_context_inactive: Symbol<'_, unsafe extern "C" fn() -> u32> = unsafe {
        library
            .get(b"hmcl_fixture_shutdown_context_inactive\0")
            .expect("shutdown-context probe")
    };
    let concurrent_context_inactive: Symbol<'_, unsafe extern "C" fn() -> u32> = unsafe {
        library
            .get(b"hmcl_fixture_concurrent_context_inactive\0")
            .expect("concurrent-context probe")
    };
    let invoke_blocked: Symbol<'_, unsafe extern "C" fn() -> u32> = unsafe {
        library
            .get(b"hmcl_fixture_invoke_blocked\0")
            .expect("blocked-invoke probe")
    };
    let allow_invoke: Symbol<'_, unsafe extern "C" fn()> = unsafe {
        library
            .get(b"hmcl_fixture_allow_invoke\0")
            .expect("allow-invoke control")
    };
    let host = HmclHostApiV1 {
        allocate: Some(allocate),
        release_buffer: Some(release_buffer),
        invoke: Some(host_invoke),
        ..HmclHostApiV1::with_required_prefix()
    };
    let mut plugin = HmclPluginApiV1 {
        struct_size: HMCL_PLUGIN_API_V1_PREFIX_SIZE,
        abi_version: hmcl_plugin_sdk::abi::HMCL_BRIDGE_ABI_V1,
        ..HmclPluginApiV1::EMPTY
    };
    let query_sentinel = plugin.context;
    assert_eq!(
        unsafe { query(std::ptr::null(), &mut plugin) },
        HmclStatus::InvalidArgument
    );
    assert_eq!(plugin.context, query_sentinel);
    assert_eq!(unsafe { query(&host, &mut plugin) }, HmclStatus::Ok);
    let initialize = plugin.initialize.expect("initialize callback");
    let invoke = plugin.invoke.expect("invoke callback");
    let shutdown = plugin.shutdown.expect("shutdown callback");

    assert_eq!(
        unsafe {
            initialize(
                plugin.context,
                &host,
                HmclPluginId::from_raw(99),
                HmclCapabilityToken::from_raw(1),
            )
        },
        HmclStatus::PluginError
    );
    assert_eq!(unsafe { failed_context_inactive() }, 1);
    assert_eq!(HOST_INVOKES.load(Ordering::SeqCst), 0);
    assert_eq!(
        unsafe {
            initialize(
                plugin.context,
                &host,
                HmclPluginId::from_raw(7),
                HmclCapabilityToken::from_raw(11),
            )
        },
        HmclStatus::Ok
    );

    let input = Value::String("echo".into()).to_wire().expect("wire");
    let mut output = HmclOwnedBuffer::EMPTY;
    assert_eq!(
        unsafe {
            invoke(
                plugin.context,
                slice(b"echo"),
                slice(&input),
                HmclCallbackId::from_raw(0),
                &mut output,
            )
        },
        HmclStatus::Ok
    );
    let encoded = unsafe { std::slice::from_raw_parts(output.data, output.len as usize) };
    assert_eq!(Value::from_wire(encoded), Ok(Value::String("echo".into())));
    assert_eq!(
        unsafe { release_buffer(std::ptr::null_mut(), &mut output) },
        HmclStatus::Ok
    );

    let sentinel = HmclOwnedBuffer {
        data: std::ptr::dangling_mut(),
        len: 0,
        capacity: 1,
    };
    let sentinel_fields = (sentinel.data, sentinel.len, sentinel.capacity);
    let mut panic_output = sentinel;
    assert_eq!(
        unsafe {
            invoke(
                plugin.context,
                slice(b"panic"),
                slice(&input),
                HmclCallbackId::from_raw(0),
                &mut panic_output,
            )
        },
        HmclStatus::PluginError
    );
    assert_eq!(panic_output.data, sentinel_fields.0);
    assert_eq!(panic_output.len, sentinel_fields.1);
    assert_eq!(panic_output.capacity, sentinel_fields.2);

    let null = Value::Null.to_wire().expect("wire");
    let mut arm_output = HmclOwnedBuffer::EMPTY;
    assert_eq!(
        unsafe {
            invoke(
                plugin.context,
                slice(b"arm-shutdown-panic"),
                slice(&null),
                HmclCallbackId::from_raw(0),
                &mut arm_output,
            )
        },
        HmclStatus::Ok
    );
    assert_eq!(
        unsafe { release_buffer(std::ptr::null_mut(), &mut arm_output) },
        HmclStatus::Ok
    );
    assert_eq!(
        unsafe { shutdown(plugin.context, HmclPluginId::from_raw(7)) },
        HmclStatus::PluginError
    );
    assert_eq!(HOST_INVOKES.load(Ordering::SeqCst), 1);
    assert_eq!(unsafe { shutdown_context_inactive() }, 1);
    assert_eq!(HOST_INVOKES.load(Ordering::SeqCst), 1);
    assert_eq!(FREES.load(Ordering::SeqCst), 3);

    assert_eq!(
        unsafe {
            initialize(
                plugin.context,
                &host,
                HmclPluginId::from_raw(8),
                HmclCapabilityToken::from_raw(13),
            )
        },
        HmclStatus::Ok
    );
    let invoke_context = plugin.context.addr();
    let invoke_callback = invoke;
    let (invoke_tx, invoke_rx) = mpsc::channel();
    let invoke_thread = std::thread::spawn(move || {
        let input = Value::Null.to_wire().expect("wire");
        let mut output = HmclOwnedBuffer::EMPTY;
        let status = unsafe {
            invoke_callback(
                std::ptr::with_exposed_provenance_mut(invoke_context),
                slice(b"block"),
                slice(&input),
                HmclCallbackId::from_raw(0),
                &mut output,
            )
        };
        if status.is_ok() {
            assert_eq!(
                unsafe { release_buffer(std::ptr::null_mut(), &mut output) },
                HmclStatus::Ok
            );
        }
        invoke_tx.send(status).expect("invoke result");
    });
    let blocked_deadline = Instant::now() + Duration::from_secs(1);
    while unsafe { invoke_blocked() } == 0 {
        assert!(Instant::now() < blocked_deadline, "invoke did not block");
        std::thread::yield_now();
    }

    let shutdown_context = plugin.context.addr();
    let shutdown_callback = shutdown;
    let (shutdown_tx, shutdown_rx) = mpsc::channel();
    let shutdown_thread = std::thread::spawn(move || {
        let status = unsafe {
            shutdown_callback(
                std::ptr::with_exposed_provenance_mut(shutdown_context),
                HmclPluginId::from_raw(8),
            )
        };
        shutdown_tx.send(status).expect("shutdown result");
    });
    let shutdown_finished_early = shutdown_rx.recv_timeout(Duration::from_millis(50)).ok();
    unsafe { allow_invoke() };
    assert_eq!(
        invoke_rx
            .recv_timeout(Duration::from_secs(1))
            .expect("blocked invoke completed"),
        HmclStatus::Ok
    );
    invoke_thread.join().expect("invoke thread");
    let shutdown_status = shutdown_finished_early.unwrap_or_else(|| {
        shutdown_rx
            .recv_timeout(Duration::from_secs(1))
            .expect("shutdown drained invoke")
    });
    shutdown_thread.join().expect("shutdown thread");
    assert!(
        shutdown_finished_early.is_none(),
        "shutdown returned before the admitted invoke completed"
    );
    assert_eq!(shutdown_status, HmclStatus::Ok);
    assert_eq!(unsafe { concurrent_context_inactive() }, 1);
    assert_eq!(HOST_INVOKES.load(Ordering::SeqCst), 2);
    assert_eq!(FREES.load(Ordering::SeqCst), 5);
}
