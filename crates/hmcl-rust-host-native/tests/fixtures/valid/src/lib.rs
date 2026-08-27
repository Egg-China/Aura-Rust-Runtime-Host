use hmcl_plugin_sdk::abi::HmclCallbackId;
use hmcl_plugin_sdk::{
    Error, ErrorCode, HandleType, ObjectHandle, Plugin, PluginContext, Value, hmcl_plugin,
};

struct FixturePlugin;

struct UiPage;

impl HandleType for UiPage {
    const TYPE_NAME: &'static str = "ui.page";
}

impl Default for FixturePlugin {
    fn default() -> Self {
        std::panic::set_hook(Box::new(|_| {}));
        Self
    }
}

impl Plugin for FixturePlugin {
    fn initialize(&mut self, context: &PluginContext) -> Result<(), Error> {
        context.invoke("initialize", &Value::Null)?;
        Ok(())
    }

    fn invoke(
        &self,
        context: &PluginContext,
        operation: &str,
        input: Value,
        _callback: HmclCallbackId,
    ) -> Result<Value, Error> {
        match operation {
            "bridge" => context.invoke("fixture.bridge", &input),
            "handle" => {
                let Value::Handle(handle) = input else {
                    return Err(Error::new(ErrorCode::InvalidArgument));
                };
                let retained = ObjectHandle::<UiPage>::from_borrowed(context, handle)?;
                let output = Value::Handle(retained.value().clone());
                drop(retained);
                Ok(output)
            }
            "panic" => panic!("fixture panic"),
            _ => Ok(input),
        }
    }

    fn shutdown(&mut self, context: &PluginContext) -> Result<(), Error> {
        context.invoke("shutdown", &Value::Null)?;
        Ok(())
    }
}

hmcl_plugin!(FixturePlugin);
