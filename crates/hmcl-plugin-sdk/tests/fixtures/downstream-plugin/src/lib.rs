use hmcl_plugin_sdk::abi::HmclCallbackId;
use hmcl_plugin_sdk::{Error, Plugin, PluginContext, Value, hmcl_plugin};
use std::sync::atomic::{AtomicBool, Ordering};

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
            panic!("initialize panic");
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
            _ => Ok(input),
        }
    }

    fn shutdown(&mut self, _context: &PluginContext) -> Result<(), Error> {
        if self.panic_on_shutdown.load(Ordering::Relaxed) {
            panic!("shutdown panic");
        }
        Ok(())
    }
}

hmcl_plugin!(PanicPlugin);
