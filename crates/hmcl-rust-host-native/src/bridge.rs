//! Java-owned Runtime Bridge transport for embedded Rust payload callbacks.

use crate::HostError;
use crate::embedded::BridgeTransport;
use jni::objects::{GlobalRef, JByteArray, JObject, JValue};
use jni::{JNIEnv, JavaVM};

/// Runtime Bridge endpoint backed by one Java engine owner global reference.
pub(crate) struct JavaBridge {
    vm: JavaVM,
    owner: GlobalRef,
}

impl JavaBridge {
    /// Captures the process Java VM and one owner global reference.
    pub(crate) fn new(env: &mut JNIEnv<'_>, owner: &JObject<'_>) -> Result<Self, HostError> {
        let vm = env.get_java_vm().map_err(|_| HostError::JavaBridge)?;
        let owner = env
            .new_global_ref(owner)
            .map_err(|_| HostError::JavaBridge)?;
        Ok(Self { vm, owner })
    }

    fn environment(&self) -> Result<JNIEnv<'_>, HostError> {
        self.vm
            .attach_current_thread_as_daemon()
            .map_err(|_| HostError::JavaBridge)
    }
}

impl BridgeTransport for JavaBridge {
    fn invoke(
        &self,
        _plugin: u64,
        session: u64,
        operation: &str,
        input: &[u8],
    ) -> Result<Vec<u8>, HostError> {
        let mut env = self.environment()?;
        let operation = env
            .new_string(operation)
            .map_err(|_| HostError::JavaBridge)?;
        let input = env
            .byte_array_from_slice(input)
            .map_err(|_| HostError::JavaBridge)?;
        let operation = JObject::from(operation);
        let input = JObject::from(input);
        let result = env.call_method(
            self.owner.as_obj(),
            "invokeBridge",
            "(JLjava/lang/String;[B)[B",
            &[
                JValue::Long(session as i64),
                JValue::Object(&operation),
                JValue::Object(&input),
            ],
        );
        let result = match result.and_then(|value| value.l()) {
            Ok(result) => result,
            Err(_) => {
                clear_pending_exception(&mut env);
                return Err(HostError::JavaBridge);
            }
        };
        if result.is_null() {
            return Err(HostError::JavaBridge);
        }
        env.convert_byte_array(JByteArray::from(result))
            .map_err(|_| HostError::JavaBridge)
    }

    fn retain_handle(
        &self,
        session: u64,
        object_id: u64,
        generation: u64,
    ) -> Result<(), HostError> {
        self.handle_call("retainBridgeHandle", session, object_id, generation)
    }

    fn release_handle(
        &self,
        session: u64,
        object_id: u64,
        generation: u64,
    ) -> Result<(), HostError> {
        self.handle_call("releaseBridgeHandle", session, object_id, generation)
    }
}

impl JavaBridge {
    fn handle_call(
        &self,
        method: &str,
        session: u64,
        object_id: u64,
        generation: u64,
    ) -> Result<(), HostError> {
        let mut env = self.environment()?;
        let result = env.call_method(
            self.owner.as_obj(),
            method,
            "(JJJ)V",
            &[
                JValue::Long(session as i64),
                JValue::Long(object_id as i64),
                JValue::Long(generation as i64),
            ],
        );
        match result {
            Ok(_) => Ok(()),
            Err(_) => {
                clear_pending_exception(&mut env);
                Err(HostError::JavaBridge)
            }
        }
    }
}

fn clear_pending_exception(env: &mut JNIEnv<'_>) {
    if env.exception_check().unwrap_or(false) {
        let _ = env.exception_clear();
    }
}
