use hmcl_plugin_sdk::Value;
use hmcl_rust_host_native::HostError;
use hmcl_rust_host_native::embedded::{BridgeTransport, Engine};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, OnceLock};

#[test]
fn embedded_payload_runs_complete_lifecycle_and_bridge_calls() {
    let fixtures = fixtures();
    let bridge = Arc::new(RecordingBridge::default());
    let engine = Engine::new(bridge.clone());
    let valid = fixtures.valid_copy();
    let payload = engine
        .load_payload(&fixtures.root, &valid, 7, 11)
        .expect("load valid payload");
    assert_ne!(payload, 0);

    engine.enable_payload(payload).expect("enable payload");
    let input = Value::Map(vec![("answer".to_owned(), Value::Integer(42))])
        .to_wire()
        .expect("encode input");
    let output = engine
        .invoke_payload(payload, "bridge", &input, 19)
        .expect("invoke payload");
    engine.disable_payload(payload).expect("disable payload");
    engine.unload_payload(payload).expect("unload payload");

    assert_eq!(
        Value::from_wire(&output).expect("decode output"),
        Value::from_wire(&input).unwrap()
    );
    assert_eq!(
        bridge.events(),
        vec!["initialize", "fixture.bridge", "shutdown"]
    );
}

#[test]
fn missing_query_symbol_is_rejected_without_initialization() {
    let fixtures = fixtures();
    let engine = Engine::new(Arc::new(RecordingBridge::default()));

    let error = engine
        .load_payload(&fixtures.root, &fixtures.missing, 7, 11)
        .expect_err("missing query symbol must fail");

    assert!(matches!(error, HostError::MissingQuerySymbol));
}

#[test]
fn incompatible_plugin_table_is_rejected() {
    let fixtures = fixtures();
    let engine = Engine::new(Arc::new(RecordingBridge::default()));

    let error = engine
        .load_payload(&fixtures.root, &fixtures.wrong_abi, 7, 11)
        .expect_err("wrong ABI must fail");

    assert!(matches!(error, HostError::UnsupportedPluginAbi(2)));
}

#[test]
fn payload_panic_is_contained_as_plugin_status() {
    let fixtures = fixtures();
    let engine = Engine::new(Arc::new(RecordingBridge::default()));
    let valid = fixtures.valid_copy();
    let payload = engine
        .load_payload(&fixtures.root, &valid, 7, 11)
        .expect("load valid payload");
    engine.enable_payload(payload).expect("enable payload");

    let error = engine
        .invoke_payload(
            payload,
            "panic",
            &Value::Null.to_wire().expect("encode null"),
            1,
        )
        .expect_err("panic must be translated");

    assert!(matches!(error, HostError::PluginStatus(5)));
    engine
        .disable_payload(payload)
        .expect("disable after panic");
    engine.unload_payload(payload).expect("unload after panic");
}

#[test]
fn entrypoint_escape_is_rejected_before_library_loading() {
    let fixtures = fixtures();
    let engine = Engine::new(Arc::new(RecordingBridge::default()));
    let outside = fixtures
        .root
        .parent()
        .unwrap()
        .join(fixtures.valid.file_name().unwrap());

    let error = engine
        .load_payload(&fixtures.root, &outside, 7, 11)
        .expect_err("outside entrypoint must fail");

    assert!(matches!(error, HostError::EntrypointEscape));
}

#[test]
fn engine_close_unloads_payloads_in_reverse_order() {
    let fixtures = fixtures();
    let bridge = Arc::new(RecordingBridge::default());
    let engine = Engine::new(bridge.clone());
    let first_library = fixtures.valid_copy();
    let second_library = fixtures.valid_copy();
    let first = engine
        .load_payload(&fixtures.root, &first_library, 1, 101)
        .expect("load first payload");
    let second = engine
        .load_payload(&fixtures.root, &second_library, 2, 202)
        .expect("load second payload");
    engine.enable_payload(first).expect("enable first");
    engine.enable_payload(second).expect("enable second");
    engine.disable_payload(first).expect("disable first");
    engine.disable_payload(second).expect("disable second");

    engine.close().expect("close engine");
    engine.close().expect("close engine twice");

    assert_eq!(
        bridge.session_events(),
        vec![
            (101, "initialize".to_owned()),
            (202, "initialize".to_owned()),
            (202, "shutdown".to_owned()),
            (101, "shutdown".to_owned())
        ]
    );
}

#[derive(Default)]
struct RecordingBridge {
    calls: Mutex<Vec<(u64, String)>>,
}

impl RecordingBridge {
    fn events(&self) -> Vec<String> {
        self.calls
            .lock()
            .unwrap()
            .iter()
            .map(|(_, operation)| operation.clone())
            .collect()
    }

    fn session_events(&self) -> Vec<(u64, String)> {
        self.calls.lock().unwrap().clone()
    }
}

impl BridgeTransport for RecordingBridge {
    fn invoke(
        &self,
        _plugin: u64,
        session: u64,
        operation: &str,
        input: &[u8],
    ) -> Result<Vec<u8>, HostError> {
        self.calls
            .lock()
            .unwrap()
            .push((session, operation.to_owned()));
        Ok(input.to_vec())
    }

    fn retain_handle(
        &self,
        _session: u64,
        _object_id: u64,
        _generation: u64,
    ) -> Result<(), HostError> {
        Ok(())
    }

    fn release_handle(
        &self,
        _session: u64,
        _object_id: u64,
        _generation: u64,
    ) -> Result<(), HostError> {
        Ok(())
    }
}

struct FixtureArtifacts {
    root: PathBuf,
    valid: PathBuf,
    missing: PathBuf,
    wrong_abi: PathBuf,
}

impl FixtureArtifacts {
    fn valid_copy(&self) -> PathBuf {
        static NEXT_COPY: AtomicU64 = AtomicU64::new(1);
        let id = NEXT_COPY.fetch_add(1, Ordering::Relaxed);
        let extension = std::env::consts::DLL_EXTENSION;
        let copy = self.root.join(format!("embedded-fixture-{id}.{extension}"));
        std::fs::copy(&self.valid, &copy).expect("copy isolated fixture library");
        copy
    }
}

fn fixtures() -> &'static FixtureArtifacts {
    static ARTIFACTS: OnceLock<FixtureArtifacts> = OnceLock::new();
    ARTIFACTS.get_or_init(build_fixtures)
}

fn build_fixtures() -> FixtureArtifacts {
    let fixture_root = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures");
    let target = fixture_root.join("target");
    let status = Command::new(env!("CARGO"))
        .args(["build", "--workspace", "--quiet"])
        .current_dir(&fixture_root)
        .env("CARGO_TARGET_DIR", &target)
        .status()
        .expect("run Cargo for loader fixtures");
    assert!(status.success(), "fixture build failed");
    let extension = std::env::consts::DLL_EXTENSION;
    let prefix = std::env::consts::DLL_PREFIX;
    let artifact = |name: &str| {
        target
            .join("debug")
            .join(format!("{prefix}{name}.{extension}"))
    };
    FixtureArtifacts {
        root: target.join("debug"),
        valid: artifact("hmcl_embedded_fixture"),
        missing: artifact("hmcl_missing_query_fixture"),
        wrong_abi: artifact("hmcl_wrong_abi_fixture"),
    }
}
