use hmcl_plugin_sdk::abi::HmclCallbackId;
use hmcl_plugin_sdk::{Error, ErrorCode, Plugin, PluginContext, Value, hmcl_plugin};

#[derive(Default)]
struct RustLaunchHookPlugin;

impl Plugin for RustLaunchHookPlugin {
    fn invoke(
        &self,
        _context: &PluginContext,
        operation: &str,
        input: Value,
        _callback: HmclCallbackId,
    ) -> Result<Value, Error> {
        if operation != "hook.before-game-launch" || !is_launch_event(&input) {
            return Err(Error::new(ErrorCode::InvalidArgument));
        }
        Ok(Value::Map(vec![
            ("contractVersion".into(), Value::Integer(1)),
            ("action".into(), Value::String("unchanged".into())),
        ]))
    }
}

fn is_launch_event(input: &Value) -> bool {
    let Value::Map(fields) = input else {
        return false;
    };
    matches!(field(fields, "contractVersion"), Some(Value::Integer(1)))
        && matches!(field(fields, "dispatchId"), Some(Value::String(value)) if !value.is_empty())
        && matches!(
            field(fields, "point"),
            Some(Value::String(value)) if value == "before-game-launch"
        )
        && matches!(field(fields, "occurredAt"), Some(Value::String(value)) if !value.is_empty())
        && matches!(field(fields, "data"), Some(Value::Map(_)))
}

fn field<'a>(fields: &'a [(String, Value)], name: &str) -> Option<&'a Value> {
    fields
        .iter()
        .find_map(|(key, value)| (key == name).then_some(value))
}

hmcl_plugin!(RustLaunchHookPlugin);
