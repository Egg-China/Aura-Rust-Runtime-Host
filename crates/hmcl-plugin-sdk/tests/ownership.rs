use hmcl_plugin_sdk::abi::{
    HmclCapabilityToken, HmclHandleId, HmclHostApiV1, HmclOwnedBuffer, HmclPluginId, HmclSlice,
    HmclStatus,
};
use hmcl_plugin_sdk::{ErrorCode, HandleType, HandleValue, ObjectHandle, PluginContext, Value};
use std::ffi::c_void;
use std::sync::atomic::{AtomicI32, AtomicUsize, Ordering};
use std::sync::{Arc, Barrier, Mutex};

struct FakeHost {
    output: Mutex<Vec<u8>>,
    last_input: Mutex<Vec<u8>>,
    invoke_status: AtomicI32,
    retain_status: AtomicI32,
    release_status: AtomicI32,
    buffer_frees: AtomicUsize,
    invokes: AtomicUsize,
    retains: AtomicUsize,
    releases: AtomicUsize,
    retained_handles: Mutex<Vec<(u64, u64)>>,
    released_handles: Mutex<Vec<(u64, u64)>>,
}

impl FakeHost {
    fn new(output: Vec<u8>) -> Self {
        Self {
            output: Mutex::new(output),
            last_input: Mutex::new(Vec::new()),
            invoke_status: AtomicI32::new(HmclStatus::Ok.into_raw()),
            retain_status: AtomicI32::new(HmclStatus::Ok.into_raw()),
            release_status: AtomicI32::new(HmclStatus::Ok.into_raw()),
            buffer_frees: AtomicUsize::new(0),
            invokes: AtomicUsize::new(0),
            retains: AtomicUsize::new(0),
            releases: AtomicUsize::new(0),
            retained_handles: Mutex::new(Vec::new()),
            released_handles: Mutex::new(Vec::new()),
        }
    }
}

unsafe extern "C" fn allocate(
    context: *mut c_void,
    length: u64,
    out: *mut HmclOwnedBuffer,
) -> HmclStatus {
    let _host = unsafe { &*(context.cast::<FakeHost>()) };
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
    context: *mut c_void,
    buffer: *mut HmclOwnedBuffer,
) -> HmclStatus {
    let host = unsafe { &*(context.cast::<FakeHost>()) };
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
    host.buffer_frees.fetch_add(1, Ordering::SeqCst);
    HmclStatus::Ok
}

unsafe extern "C" fn invoke(
    context: *mut c_void,
    _plugin: HmclPluginId,
    _token: HmclCapabilityToken,
    _operation: HmclSlice,
    input: HmclSlice,
    out: *mut HmclOwnedBuffer,
) -> HmclStatus {
    let host = unsafe { &*(context.cast::<FakeHost>()) };
    host.invokes.fetch_add(1, Ordering::SeqCst);
    let input = unsafe { std::slice::from_raw_parts(input.data, input.len as usize) };
    *host.last_input.lock().expect("last input lock") = input.to_vec();
    let status = HmclStatus::from_raw(host.invoke_status.load(Ordering::SeqCst));
    if !status.is_ok() {
        return status;
    }
    let mut bytes = host.output.lock().expect("output lock").clone();
    let buffer = HmclOwnedBuffer {
        data: bytes.as_mut_ptr(),
        len: bytes.len() as u64,
        capacity: bytes.capacity() as u64,
    };
    std::mem::forget(bytes);
    unsafe { out.write(buffer) };
    status
}

unsafe extern "C" fn retain(context: *mut c_void, handle: HmclHandleId) -> HmclStatus {
    let host = unsafe { &*(context.cast::<FakeHost>()) };
    host.retains.fetch_add(1, Ordering::SeqCst);
    host.retained_handles
        .lock()
        .expect("retained handles lock")
        .push(handle.into_parts());
    HmclStatus::from_raw(host.retain_status.load(Ordering::SeqCst))
}

unsafe extern "C" fn release_handle(context: *mut c_void, handle: HmclHandleId) -> HmclStatus {
    let host = unsafe { &*(context.cast::<FakeHost>()) };
    host.releases.fetch_add(1, Ordering::SeqCst);
    host.released_handles
        .lock()
        .expect("released handles lock")
        .push(handle.into_parts());
    HmclStatus::from_raw(host.release_status.load(Ordering::SeqCst))
}

fn context(host: &FakeHost) -> PluginContext {
    let table = HmclHostApiV1 {
        context: std::ptr::from_ref(host).cast_mut().cast(),
        allocate: Some(allocate),
        release_buffer: Some(release_buffer),
        invoke: Some(invoke),
        retain_handle: Some(retain),
        release_handle: Some(release_handle),
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
fn bridge_value_wire_v1_round_trips_every_value_and_preserves_map_order() {
    let values = vec![
        Value::Null,
        Value::Bool(true),
        Value::Integer(-42),
        Value::Float(3.5),
        Value::String("hello".into()),
        Value::Bytes(vec![0, 1, 255]),
        Value::Array(vec![Value::Integer(1), Value::Null]),
        Value::Map(vec![
            ("second".into(), Value::Bool(false)),
            ("first".into(), Value::Integer(1)),
        ]),
        Value::Handle(HandleValue::new(1, 2, "document.node").expect("valid handle")),
        Value::Error(hmcl_plugin_sdk::Error::new(ErrorCode::PermissionDenied)),
    ];
    for value in values {
        let bytes = value.to_wire().expect("encodes");
        assert_eq!(Value::from_wire(&bytes).expect("decodes"), value);
    }
    assert_eq!(
        Value::Null.to_wire().expect("null wire"),
        [0x92, 0x00, 0xc0]
    );
}

#[test]
fn bridge_errors_have_exact_java_parity_wire_codes() {
    let cases = [
        (ErrorCode::InvalidArgument, "invalid-argument"),
        (ErrorCode::InvalidResult, "invalid-result"),
        (ErrorCode::PermissionDenied, "permission-denied"),
        (ErrorCode::StaleHandle, "stale-handle"),
        (ErrorCode::TypeMismatch, "type-mismatch"),
        (ErrorCode::Cancelled, "cancelled"),
        (ErrorCode::CallbackFailed, "callback-failed"),
        (ErrorCode::Unavailable, "unavailable"),
        (ErrorCode::Internal, "internal"),
    ];
    for (kind, code) in cases {
        assert_eq!(kind.wire_code(), code);
        let value = Value::Error(hmcl_plugin_sdk::Error::new(kind));
        let mut expected = vec![0x92, 0x09, 0xdb];
        expected.extend_from_slice(&(code.len() as u32).to_be_bytes());
        expected.extend_from_slice(code.as_bytes());
        assert_eq!(value.to_wire().expect("error wire"), expected);
        assert_eq!(Value::from_wire(&expected), Ok(value));
    }
}

#[test]
fn bridge_value_rejects_malformed_unknown_and_bounded_payloads() {
    assert_eq!(
        Value::Float(f64::NAN)
            .to_wire()
            .expect_err("NaN rejected")
            .code(),
        ErrorCode::InvalidArgument
    );
    assert_eq!(
        Value::from_wire(&[0x92, 0x0f, 0xc0])
            .expect_err("unknown tag rejected")
            .code(),
        ErrorCode::InvalidResult
    );
    assert_eq!(
        Value::from_wire(&[0x92, 0x09, 0xdb, 0, 0, 0, 3, b'b', b'a', b'd'])
            .expect_err("unknown code rejected")
            .code(),
        ErrorCode::InvalidResult
    );
    assert!(
        Value::String("x".repeat(1024 * 1024 + 1))
            .to_wire()
            .is_err()
    );
    assert!(
        Value::Bytes(vec![0; 16 * 1024 * 1024 + 1])
            .to_wire()
            .is_err()
    );
    assert!(Value::Array(vec![Value::Null; 1025]).to_wire().is_err());
    assert!(
        Value::Map(vec![("x".repeat(1024 * 1024 + 1), Value::Null)])
            .to_wire()
            .is_err()
    );

    let mut deep = Value::Null;
    for _ in 0..32 {
        deep = Value::Array(vec![deep]);
    }
    assert!(deep.to_wire().is_err());
    let many = Value::Array(
        (0..65)
            .map(|_| Value::Array(vec![Value::Null; 1024]))
            .collect(),
    );
    assert!(many.to_wire().is_err());
    let content = Value::Array(vec![
        Value::Bytes(vec![0; 9 * 1024 * 1024]),
        Value::Bytes(vec![0; 9 * 1024 * 1024]),
    ]);
    assert!(content.to_wire().is_err());
}

#[test]
fn bridge_value_decoder_rejects_noncanonical_trailing_duplicate_and_direct_limits() {
    assert!(Value::from_wire(&[0x92, 0x00, 0xc0, 0x00]).is_err());
    assert!(Value::from_wire(&[0x92, 0x02, 0x00]).is_err());

    let mut duplicate_map = vec![0x92, 0x07, 0xdd, 0, 0, 0, 2];
    for _ in 0..2 {
        duplicate_map.extend_from_slice(&[0x92, 0xdb, 0, 0, 0, 1, b'x', 0x92, 0, 0xc0]);
    }
    assert!(Value::from_wire(&duplicate_map).is_err());

    assert!(Value::from_wire(&[0x92, 0x06, 0xdd, 0, 0, 4, 1]).is_err());
    assert!(Value::from_wire(&[0x92, 0x04, 0xdb, 0, 0x10, 0, 1]).is_err());
    assert!(Value::from_wire(&[0x92, 0x05, 0xc6, 1, 0, 0, 1]).is_err());

    let mut too_deep = Vec::new();
    for _ in 0..32 {
        too_deep.extend_from_slice(&[0x92, 0x06, 0xdd, 0, 0, 0, 1]);
    }
    too_deep.extend_from_slice(&[0x92, 0, 0xc0]);
    assert!(Value::from_wire(&too_deep).is_err());
}

#[test]
fn handles_enforce_semantics_and_exact_retain_release_ownership() {
    assert!(HandleValue::new(0, 1, "document").is_err());
    assert!(HandleValue::new(1, 0, "document").is_err());
    assert!(HandleValue::new(1, 1, "Document").is_err());
    assert!(HandleValue::new(1, 1, "a".repeat(129)).is_err());
    assert!(HandleValue::new(i64::MAX as u64 + 1, 1, "document").is_err());
    assert!(HandleValue::new(1, i64::MAX as u64 + 1, "document").is_err());
    for valid in ["a", "a0", "document.node", "plugin-type", "a.b-c9"] {
        assert!(HandleValue::new(i64::MAX as u64, i64::MAX as u64, valid).is_ok());
    }
    for invalid in [
        "", "0a", "_a", "a_b", "a..b", "a--b", "a.-b", "a.", "a-", "A", "é",
    ] {
        assert!(
            HandleValue::new(1, 1, invalid).is_err(),
            "accepted {invalid:?}"
        );
    }

    struct Document;
    impl HandleType for Document {
        const TYPE_NAME: &'static str = "document";
    }
    let host = FakeHost::new(Vec::new());
    let context = context(&host);
    let value = HandleValue::new(9, 3, "document").expect("valid");
    let owned =
        unsafe { ObjectHandle::<Document>::from_owned(&context, value.clone()) }.expect("owned");
    let cloned = owned.try_clone().expect("retained clone");
    assert_eq!(host.retains.load(Ordering::SeqCst), 1);
    assert_eq!(
        host.retained_handles
            .lock()
            .expect("retained handles")
            .as_slice(),
        &[(9, 3)]
    );
    drop(cloned);
    drop(owned);
    assert_eq!(host.releases.load(Ordering::SeqCst), 2);

    let borrowed = ObjectHandle::<Document>::from_borrowed(&context, value).expect("borrowed");
    assert_eq!(host.retains.load(Ordering::SeqCst), 2);
    drop(borrowed);
    assert_eq!(host.releases.load(Ordering::SeqCst), 3);
    assert_eq!(
        host.released_handles
            .lock()
            .expect("released handles")
            .as_slice(),
        &[(9, 3), (9, 3), (9, 3)]
    );

    struct Other;
    impl HandleType for Other {
        const TYPE_NAME: &'static str = "other";
    }
    let mismatch = HandleValue::new(11, 4, "document").expect("valid");
    assert_eq!(
        ObjectHandle::<Other>::from_borrowed(&context, mismatch)
            .expect_err("type mismatch")
            .code(),
        ErrorCode::TypeMismatch
    );
    assert_eq!(host.retains.load(Ordering::SeqCst), 2);

    host.retain_status
        .store(HmclStatus::HostError.into_raw(), Ordering::SeqCst);
    let stale = HandleValue::new(10, 1, "document").expect("valid");
    assert_eq!(
        ObjectHandle::<Document>::from_borrowed(&context, stale)
            .expect_err("stale")
            .code(),
        ErrorCode::StaleHandle
    );
}

#[test]
fn host_outputs_are_copied_and_freed_once_even_when_decode_fails() {
    let host = FakeHost::new(Value::String("result".into()).to_wire().expect("wire"));
    let context = context(&host);
    assert_eq!(
        context.invoke("echo", &Value::Null).expect("invoke"),
        Value::String("result".into())
    );
    assert_eq!(host.buffer_frees.load(Ordering::SeqCst), 1);

    *host.output.lock().expect("output") = vec![0xff];
    assert_eq!(
        context
            .invoke("bad", &Value::Null)
            .expect_err("decode failure")
            .code(),
        ErrorCode::InvalidResult
    );
    assert_eq!(host.buffer_frees.load(Ordering::SeqCst), 2);

    host.invoke_status.store(-41_337, Ordering::SeqCst);
    assert_eq!(
        context
            .invoke("future", &Value::Null)
            .expect_err("unknown status")
            .code(),
        ErrorCode::Internal
    );
    assert_eq!(host.buffer_frees.load(Ordering::SeqCst), 2);
}

#[test]
fn host_invoke_round_trips_bytes_and_ordered_maps_through_owned_buffers() {
    let ordered = Value::Map(vec![
        ("second".into(), Value::Integer(2)),
        ("first".into(), Value::Bytes(vec![4, 5, 6])),
    ]);
    let host = FakeHost::new(ordered.to_wire().expect("ordered wire"));
    let context = context(&host);
    let bytes = Value::Bytes(vec![0, 1, 2, 255]);
    assert_eq!(context.invoke("bytes", &bytes), Ok(ordered.clone()));
    assert_eq!(
        Value::from_wire(&host.last_input.lock().expect("last input")),
        Ok(bytes)
    );

    *host.output.lock().expect("output") =
        Value::Bytes(vec![9, 8, 7]).to_wire().expect("bytes wire");
    assert_eq!(
        context.invoke("map", &ordered),
        Ok(Value::Bytes(vec![9, 8, 7]))
    );
    assert_eq!(
        Value::from_wire(&host.last_input.lock().expect("last input")),
        Ok(ordered)
    );
    assert_eq!(host.buffer_frees.load(Ordering::SeqCst), 2);
}

#[test]
fn missing_optional_callbacks_return_redacted_errors() {
    let host = FakeHost::new(Vec::new());
    let table = HmclHostApiV1 {
        context: std::ptr::from_ref(&host).cast_mut().cast(),
        ..HmclHostApiV1::with_required_prefix()
    };
    let context = unsafe {
        PluginContext::from_raw(
            &table,
            HmclPluginId::from_raw(1),
            HmclCapabilityToken::from_raw(2),
        )
        .expect("valid host")
    };
    assert_eq!(
        context
            .invoke("missing", &Value::Null)
            .expect_err("missing invoke")
            .code(),
        ErrorCode::Unavailable
    );
}

#[test]
fn missing_owner_callbacks_prevent_calls_and_handle_adoption() {
    struct Document;
    impl HandleType for Document {
        const TYPE_NAME: &'static str = "document";
    }

    let host = FakeHost::new(Value::Null.to_wire().expect("wire"));
    let table = HmclHostApiV1 {
        context: std::ptr::from_ref(&host).cast_mut().cast(),
        invoke: Some(invoke),
        ..HmclHostApiV1::with_required_prefix()
    };
    let context = unsafe {
        PluginContext::from_raw(
            &table,
            HmclPluginId::from_raw(1),
            HmclCapabilityToken::from_raw(2),
        )
        .expect("valid host")
    };
    assert_eq!(
        context
            .invoke("missing-owner", &Value::Null)
            .expect_err("release is required")
            .code(),
        ErrorCode::Unavailable
    );
    assert_eq!(host.invokes.load(Ordering::SeqCst), 0);

    let value = HandleValue::new(9, 1, "document").expect("valid handle");
    assert_eq!(
        ObjectHandle::<Document>::from_borrowed(&context, value.clone())
            .expect_err("retain and release are required")
            .code(),
        ErrorCode::Unavailable
    );
    assert_eq!(
        unsafe { ObjectHandle::<Document>::from_owned(&context, value) }
            .expect_err("release is required")
            .code(),
        ErrorCode::Unavailable
    );
}

#[test]
fn handle_drop_contains_release_failures() {
    struct Document;
    impl HandleType for Document {
        const TYPE_NAME: &'static str = "document";
    }

    let host = FakeHost::new(Vec::new());
    host.release_status
        .store(HmclStatus::HostError.into_raw(), Ordering::SeqCst);
    let context = context(&host);
    let value = HandleValue::new(9, 1, "document").expect("valid handle");
    let owned = unsafe { ObjectHandle::<Document>::from_owned(&context, value) }.expect("owned");
    drop(owned);
    assert_eq!(host.releases.load(Ordering::SeqCst), 1);
    assert_eq!(context.cleanup_failures(), 1);
}

#[test]
fn callback_future_terminal_transitions_are_exactly_once_and_scoped() {
    let host = FakeHost::new(Vec::new());
    let context = context(&host);
    let (callback, future) = context.callback().expect("pair");
    assert!(future.poll().is_pending());
    callback.complete(Ok(Value::Integer(7))).expect("complete");
    assert_eq!(future.wait(), Ok(Value::Integer(7)));
    assert_eq!(
        callback.complete(Ok(Value::Null)).expect_err("late").code(),
        ErrorCode::CallbackFailed
    );

    let (callback, future) = context.callback().expect("pair");
    assert!(future.cancel());
    assert!(!future.cancel());
    assert_eq!(
        future.wait().expect_err("cancelled").code(),
        ErrorCode::Cancelled
    );
    assert_eq!(
        callback.complete(Ok(Value::Null)).expect_err("late").code(),
        ErrorCode::Cancelled
    );

    let (callback, future) = context.callback().expect("pair");
    drop(future);
    assert_eq!(
        callback
            .complete(Ok(Value::Null))
            .expect_err("dropped")
            .code(),
        ErrorCode::Cancelled
    );

    let (callback, future) = context.callback().expect("pair");
    let barrier = Arc::new(Barrier::new(3));
    let callback_barrier = Arc::clone(&barrier);
    let callback_thread = std::thread::spawn(move || {
        callback_barrier.wait();
        callback.complete(Ok(Value::Bool(true)))
    });
    let future_barrier = Arc::clone(&barrier);
    let future_thread = std::thread::spawn(move || {
        future_barrier.wait();
        let cancelled = future.cancel();
        (cancelled, future.wait())
    });
    barrier.wait();
    let completion = callback_thread.join().expect("completion thread");
    let (cancelled, terminal) = future_thread.join().expect("future thread");
    assert_ne!(completion.is_ok(), cancelled);
    assert!(matches!(terminal, Ok(Value::Bool(true)) | Err(_)));
    assert_eq!(context.pending_callbacks(), 0);
}

#[test]
fn callback_registry_is_bounded_and_drop_removes_every_entry() {
    let host = FakeHost::new(Vec::new());
    let context = context(&host);
    let pairs = (0..1024)
        .map(|_| context.callback().expect("within registry bound"))
        .collect::<Vec<_>>();
    assert_eq!(context.pending_callbacks(), 1024);
    let error = match context.callback() {
        Ok(_) => panic!("registry exceeded its bound"),
        Err(error) => error,
    };
    assert_eq!(error.code(), ErrorCode::Unavailable);
    drop(pairs);
    assert_eq!(context.pending_callbacks(), 0);
}
