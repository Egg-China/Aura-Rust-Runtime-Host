use hmcl_plugin_sdk::abi::HmclCallbackId;
use hmcl_plugin_sdk::{Error, ErrorCode, Plugin, PluginContext, Value, hmcl_plugin};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::OnceLock;

static FAILED_CONTEXT: OnceLock<PluginContext> = OnceLock::new();
static ACTIVE_CONTEXT: OnceLock<PluginContext> = OnceLock::new();
static CONCURRENT_CONTEXT: OnceLock<PluginContext> = OnceLock::new();
static INVOKE_BLOCKED: AtomicBool = AtomicBool::new(false);
static ALLOW_INVOKE: AtomicBool = AtomicBool::new(false);

struct PanicPlugin {
    panic_on_shutdown: AtomicBool,
}

impl Default for PanicPlugin {
    fn default() -> Self {
        std::panic::set_hook(Box::new(|_| {}));
        Self {
            panic_on_shutdown: AtomicBool::new(false),
        }
    }
}

impl Plugin for PanicPlugin {
    fn initialize(&mut self, context: &PluginContext) -> Result<(), Error> {
        if context.plugin_id().into_raw() == 99 {
            assert!(FAILED_CONTEXT.set(context.clone()).is_ok());
            panic!("initialize panic");
        }
        if context.plugin_id().into_raw() == 8 {
            assert!(CONCURRENT_CONTEXT.set(context.clone()).is_ok());
        } else {
            assert!(ACTIVE_CONTEXT.set(context.clone()).is_ok());
        }
        Ok(())
    }

    fn invoke(
        &self,
        _context: &PluginContext,
        operation: &str,
        input: Value,
        _callback: HmclCallbackId,
    ) -> Result<Value, Error> {
        match operation {
            "panic" => panic!("invoke panic"),
            "arm-shutdown-panic" => {
                self.panic_on_shutdown.store(true, Ordering::Relaxed);
                Ok(Value::Null)
            }
            "block" => {
                INVOKE_BLOCKED.store(true, Ordering::SeqCst);
                while !ALLOW_INVOKE.load(Ordering::SeqCst) {
                    std::thread::yield_now();
                }
                Ok(input)
            }
            _ => Ok(input),
        }
    }

    fn shutdown(&mut self, context: &PluginContext) -> Result<(), Error> {
        context.invoke("shutdown-service", &Value::Null)?;
        if self.panic_on_shutdown.load(Ordering::Relaxed) {
            panic!("shutdown panic");
        }
        Ok(())
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn hmcl_fixture_failed_context_inactive() -> u32 {
    context_is_inactive(FAILED_CONTEXT.get())
}

#[unsafe(no_mangle)]
pub extern "C" fn hmcl_fixture_shutdown_context_inactive() -> u32 {
    context_is_inactive(ACTIVE_CONTEXT.get())
}

#[unsafe(no_mangle)]
pub extern "C" fn hmcl_fixture_concurrent_context_inactive() -> u32 {
    context_is_inactive(CONCURRENT_CONTEXT.get())
}

#[unsafe(no_mangle)]
pub extern "C" fn hmcl_fixture_invoke_blocked() -> u32 {
    u32::from(INVOKE_BLOCKED.load(Ordering::SeqCst))
}

#[unsafe(no_mangle)]
pub extern "C" fn hmcl_fixture_allow_invoke() {
    ALLOW_INVOKE.store(true, Ordering::SeqCst);
}

fn context_is_inactive(context: Option<&PluginContext>) -> u32 {
    u32::from(matches!(
        context.map(|context| context.invoke("probe", &Value::Null)),
        Some(Err(error)) if error.code() == ErrorCode::Unavailable
    ))
}

hmcl_plugin!(PanicPlugin);
