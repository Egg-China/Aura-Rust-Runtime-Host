//! JNI exports owned by `RustNativeEngine.JniBindings`.

use crate::HostError;
use crate::bridge::JavaBridge;
use crate::embedded::Engine;
use jni::JNIEnv;
use jni::objects::{JByteArray, JClass, JObject, JString};
use jni::sys::{JNI_FALSE, JNI_TRUE, jboolean, jbyteArray, jlong};
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::path::Path;
use std::sync::Arc;

struct JniEngine {
    engine: Engine,
}

/// Creates one provider-wide native engine and binds its Java callback owner.
#[unsafe(no_mangle)]
pub extern "system" fn Java_dev_hmclce_runtime_rust_RustNativeEngine_00024JniBindings_createEngine(
    mut env: JNIEnv<'_>,
    _class: JClass<'_>,
    owner: JObject<'_>,
) -> jlong {
    jni_result(&mut env, 0, |env| {
        let bridge = Arc::new(JavaBridge::new(env, &owner)?);
        let engine = Box::new(JniEngine {
            engine: Engine::new(bridge),
        });
        Ok(Box::into_raw(engine) as jlong)
    })
}

/// Reports whether one nonzero provider-wide native engine handle is available.
#[unsafe(no_mangle)]
pub extern "system" fn Java_dev_hmclce_runtime_rust_RustNativeEngine_00024JniBindings_checkEngineHealth(
    mut env: JNIEnv<'_>,
    _class: JClass<'_>,
    handle: jlong,
) -> jboolean {
    jni_result(&mut env, JNI_FALSE, |_env| {
        let _ = unsafe { engine(handle)? };
        Ok(JNI_TRUE)
    })
}

/// Loads and queries one embedded Rust payload without initializing it.
#[unsafe(no_mangle)]
pub extern "system" fn Java_dev_hmclce_runtime_rust_RustNativeEngine_00024JniBindings_loadEmbeddedPayload(
    mut env: JNIEnv<'_>,
    _class: JClass<'_>,
    handle: jlong,
    package_root: JString<'_>,
    entrypoint: JString<'_>,
    plugin_id: jlong,
    session: jlong,
) -> jlong {
    jni_result(&mut env, 0, |env| {
        let package_root = java_string(env, &package_root)?;
        let entrypoint = java_string(env, &entrypoint)?;
        let owner = unsafe { engine(handle)? };
        let payload = owner.engine.load_payload(
            Path::new(&package_root),
            Path::new(&entrypoint),
            plugin_id as u64,
            session as u64,
        )?;
        Ok(payload as jlong)
    })
}

/// Enables one loaded embedded Rust payload.
#[unsafe(no_mangle)]
pub extern "system" fn Java_dev_hmclce_runtime_rust_RustNativeEngine_00024JniBindings_enableEmbeddedPayload(
    mut env: JNIEnv<'_>,
    _class: JClass<'_>,
    handle: jlong,
    payload_id: jlong,
) {
    jni_void(&mut env, |_env| {
        let owner = unsafe { engine(handle)? };
        owner.engine.enable_payload(payload_id as u64)
    });
}

/// Disables one enabled embedded Rust payload.
#[unsafe(no_mangle)]
pub extern "system" fn Java_dev_hmclce_runtime_rust_RustNativeEngine_00024JniBindings_disableEmbeddedPayload(
    mut env: JNIEnv<'_>,
    _class: JClass<'_>,
    handle: jlong,
    payload_id: jlong,
) {
    jni_void(&mut env, |_env| {
        let owner = unsafe { engine(handle)? };
        owner.engine.disable_payload(payload_id as u64)
    });
}

/// Invokes one operation on an enabled embedded Rust payload and returns Host-owned Java bytes.
#[unsafe(no_mangle)]
pub extern "system" fn Java_dev_hmclce_runtime_rust_RustNativeEngine_00024JniBindings_invokeEmbeddedPayload(
    mut env: JNIEnv<'_>,
    _class: JClass<'_>,
    handle: jlong,
    payload_id: jlong,
    operation: JString<'_>,
    input: JByteArray<'_>,
    callback_id: jlong,
) -> jbyteArray {
    jni_result(&mut env, std::ptr::null_mut(), |env| {
        let operation = java_string(env, &operation)?;
        let input = env
            .convert_byte_array(&input)
            .map_err(|_| HostError::JavaBridge)?;
        let owner = unsafe { engine(handle)? };
        let result = owner.engine.invoke_payload(
            payload_id as u64,
            &operation,
            &input,
            callback_id as u64,
        )?;
        env.byte_array_from_slice(&result)
            .map(JByteArray::into_raw)
            .map_err(|_| HostError::JavaBridge)
    })
}

/// Shuts down and unloads one disabled embedded Rust payload.
#[unsafe(no_mangle)]
pub extern "system" fn Java_dev_hmclce_runtime_rust_RustNativeEngine_00024JniBindings_unloadEmbeddedPayload(
    mut env: JNIEnv<'_>,
    _class: JClass<'_>,
    handle: jlong,
    payload_id: jlong,
) {
    jni_void(&mut env, |_env| {
        let owner = unsafe { engine(handle)? };
        owner.engine.unload_payload(payload_id as u64)
    });
}

/// Closes and releases one provider-wide native engine exactly once.
#[unsafe(no_mangle)]
pub extern "system" fn Java_dev_hmclce_runtime_rust_RustNativeEngine_00024JniBindings_destroyEngine(
    _env: JNIEnv<'_>,
    _class: JClass<'_>,
    handle: jlong,
) {
    if handle == 0 {
        return;
    }
    let _ = catch_unwind(AssertUnwindSafe(|| {
        // SAFETY: Java transfers its unique live handle exactly once during synchronized close.
        let owner = unsafe { Box::from_raw(handle as *mut JniEngine) };
        let _ = owner.engine.close();
    }));
}

fn java_string(env: &mut JNIEnv<'_>, value: &JString<'_>) -> Result<String, HostError> {
    env.get_string(value)
        .map(|value| value.to_string_lossy().into_owned())
        .map_err(|_| HostError::JavaBridge)
}

unsafe fn engine(handle: jlong) -> Result<&'static JniEngine, HostError> {
    if handle == 0 {
        return Err(HostError::EngineClosed);
    }
    // SAFETY: Java owns the boxed pointer until synchronized destroy and never mutates the box.
    unsafe { (handle as *const JniEngine).as_ref() }.ok_or(HostError::EngineClosed)
}

fn jni_result<T: Copy>(
    env: &mut JNIEnv<'_>,
    fallback: T,
    action: impl FnOnce(&mut JNIEnv<'_>) -> Result<T, HostError>,
) -> T {
    match catch_unwind(AssertUnwindSafe(|| action(env))) {
        Ok(Ok(value)) => value,
        Ok(Err(error)) => {
            throw_io(env, &error);
            fallback
        }
        Err(_) => {
            throw_io(env, &HostError::JniPanic);
            fallback
        }
    }
}

fn jni_void(env: &mut JNIEnv<'_>, action: impl FnOnce(&mut JNIEnv<'_>) -> Result<(), HostError>) {
    jni_result(env, (), action);
}

fn throw_io(env: &mut JNIEnv<'_>, error: &HostError) {
    if env.exception_check().unwrap_or(false) {
        let _ = env.exception_clear();
    }
    let _ = env.throw_new("java/io/IOException", error.to_string());
}
