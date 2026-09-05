use hmcl_plugin_sdk::{HandleValue, Value};
use hmcl_runtime_protocol::{Message, MessageBody, read_frame, write_frame};
use std::io::{BufReader, BufWriter, Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdin, ChildStdout, Command, ExitStatus, Stdio};
use std::sync::OnceLock;
use std::time::{Duration, Instant};

#[test]
fn probe_exits_successfully_without_protocol_input() {
    let status = Command::new(env!("CARGO_BIN_EXE_hmcl-rust-host-process"))
        .arg("--probe")
        .status()
        .expect("run process Host probe");

    assert!(status.success());
}

#[test]
fn invalid_command_lines_exit_with_failure() {
    for arguments in [vec![], vec!["--unknown"], vec!["--probe", "extra"]] {
        let status = Command::new(env!("CARGO_BIN_EXE_hmcl-rust-host-process"))
            .args(arguments)
            .status()
            .expect("run process Host with invalid arguments");

        assert!(!status.success());
    }
}

#[test]
fn isolated_payload_runs_complete_lifecycle_and_bridge_calls() {
    let fixtures = fixtures();
    let mut client = ProcessClient::start();
    let null = Value::Null.to_wire().expect("encode Bridge null");

    assert_eq!(client.request(MessageBody::Hello), MessageBody::Ok);
    assert_eq!(
        client.request(MessageBody::Load {
            package_root: fixtures.root.to_string_lossy().into_owned(),
            entrypoint: fixtures
                .valid
                .file_name()
                .expect("fixture filename")
                .to_string_lossy()
                .into_owned(),
            plugin_id: 7,
            session: 11,
        }),
        MessageBody::Ok
    );
    assert_eq!(client.request(MessageBody::Enable), MessageBody::Ok);
    assert_eq!(
        client.request(MessageBody::Invoke {
            operation: "bridge".into(),
            input: null.clone(),
            callback_id: 41,
        }),
        MessageBody::Result {
            output: null.clone()
        }
    );
    assert_eq!(client.request(MessageBody::Disable), MessageBody::Ok);
    assert_eq!(client.request(MessageBody::Shutdown), MessageBody::Ok);

    assert_eq!(
        client.bridge_operations,
        vec!["initialize", "fixture.bridge", "shutdown"]
    );
    client.finish();
}

#[test]
fn launch_hook_sample_returns_literal_unchanged_hook_and_patch_responses() {
    let sample = launch_hook_sample();
    let mut client = ProcessClient::start();

    assert_eq!(
        client.request_with_id(1, MessageBody::Hello),
        MessageBody::Ok
    );
    assert_eq!(
        client.request_with_id(
            3,
            MessageBody::Load {
                package_root: sample.root.to_string_lossy().into_owned(),
                entrypoint: sample
                    .library
                    .file_name()
                    .expect("launch-hook library filename")
                    .to_string_lossy()
                    .into_owned(),
                plugin_id: 71,
                session: 73,
            }
        ),
        MessageBody::Ok
    );
    assert_eq!(
        client.request_with_id(5, MessageBody::Enable),
        MessageBody::Ok
    );
    assert_eq!(
        client.request_with_id(
            7,
            MessageBody::Invoke {
                operation: "hook.before-game-launch".into(),
                input: launch_hook_input(),
                callback_id: 0,
            }
        ),
        MessageBody::Result {
            output: literal_unchanged_hook_wire(),
        }
    );
    assert_eq!(
        client.request_with_id(
            9,
            MessageBody::Invoke {
                operation: "aura.patch.v1".into(),
                input: literal_patch_request_wire(),
                callback_id: 0,
            }
        ),
        MessageBody::Result {
            output: literal_unchanged_patch_wire(),
        }
    );
    assert_eq!(
        client.request_with_id(11, MessageBody::Disable),
        MessageBody::Ok
    );
    assert_eq!(
        client.request_with_id(13, MessageBody::Shutdown),
        MessageBody::Ok
    );
    client.assert_no_extra_stdout();
    client.finish();
}

#[test]
fn incomplete_open_launch_hook_frame_times_out_then_is_killed_and_reaped() {
    let mut child = BoundedChild::start();
    let mut input = child.take_stdin();
    input
        .write_all(&[0, 0, 0, 2, 0x92])
        .expect("write incomplete frame");
    input.flush().expect("flush incomplete frame");

    assert!(
        child
            .wait_for_exit_until(Instant::now() + Duration::from_millis(250))
            .is_none(),
        "process Host unexpectedly exited before the parent incomplete-frame timeout"
    );
    let status = child.kill_and_reap_within(Instant::now() + Duration::from_secs(2));
    assert!(
        !status.success(),
        "killed incomplete-frame process Host must not report success"
    );
}

#[test]
fn reports_load_and_lifecycle_failures_without_losing_the_session() {
    let fixtures = fixtures();
    let mut client = ProcessClient::start();
    assert_eq!(client.request(MessageBody::Hello), MessageBody::Ok);
    assert_error(client.request(MessageBody::Enable), "invalid-state");
    assert_error(
        client.request(load_body(fixtures, &fixtures.missing)),
        "missing-query",
    );
    assert_error(
        client.request(load_body(fixtures, &fixtures.wrong_abi)),
        "unsupported-abi",
    );
    assert_eq!(
        client.request(load_body(fixtures, &fixtures.valid)),
        MessageBody::Ok
    );
    assert_error(
        client.request(load_body(fixtures, &fixtures.valid)),
        "invalid-state",
    );
    assert_eq!(client.request(MessageBody::Shutdown), MessageBody::Ok);
    client.finish();
}

#[test]
fn forwards_handle_ownership_callbacks_and_redacts_bridge_failures() {
    let fixtures = fixtures();
    let mut client = ProcessClient::start();
    assert_eq!(client.request(MessageBody::Hello), MessageBody::Ok);
    assert_eq!(
        client.request(load_body(fixtures, &fixtures.valid)),
        MessageBody::Ok
    );
    assert_eq!(client.request(MessageBody::Enable), MessageBody::Ok);
    let handle = Value::Handle(HandleValue::new(7, 9, "ui.page").expect("valid handle"));
    let handle_wire = handle.to_wire().expect("encode handle");
    assert_eq!(
        client.request(MessageBody::Invoke {
            operation: "handle".into(),
            input: handle_wire.clone(),
            callback_id: 43,
        }),
        MessageBody::Result {
            output: handle_wire
        }
    );
    assert_eq!(client.handle_events, vec!["retain:7:9", "release:7:9"]);

    client.bridge_failure = Some("permission-denied".into());
    assert_error(
        client.request(MessageBody::Invoke {
            operation: "bridge".into(),
            input: Value::Null.to_wire().expect("encode null"),
            callback_id: 45,
        }),
        "plugin-status",
    );
    client.bridge_failure = None;
    assert_eq!(client.request(MessageBody::Disable), MessageBody::Ok);
    assert_eq!(client.request(MessageBody::Shutdown), MessageBody::Ok);
    client.finish();
}

#[test]
fn contains_plugin_panics_and_preserves_lifecycle_cleanup() {
    let fixtures = fixtures();
    let mut client = ProcessClient::start();
    assert_eq!(client.request(MessageBody::Hello), MessageBody::Ok);
    assert_eq!(
        client.request(load_body(fixtures, &fixtures.valid)),
        MessageBody::Ok
    );
    assert_eq!(client.request(MessageBody::Enable), MessageBody::Ok);
    assert_error(
        client.request(MessageBody::Invoke {
            operation: "panic".into(),
            input: Value::Null.to_wire().expect("encode null"),
            callback_id: 47,
        }),
        "plugin-status",
    );
    assert_eq!(client.request(MessageBody::Disable), MessageBody::Ok);
    assert_eq!(client.request(MessageBody::Shutdown), MessageBody::Ok);
    client.finish();
}

#[test]
fn rejects_mismatched_callback_ids_and_unexpected_callback_kinds() {
    let fixtures = fixtures();
    for callback_mode in [CallbackMode::MismatchedId, CallbackMode::UnexpectedKind] {
        let mut client = ProcessClient::start();
        assert_eq!(client.request(MessageBody::Hello), MessageBody::Ok);
        assert_eq!(
            client.request(load_body(fixtures, &fixtures.valid)),
            MessageBody::Ok
        );
        assert_eq!(client.request(MessageBody::Enable), MessageBody::Ok);
        client.callback_mode = callback_mode;
        assert_error(
            client.request(MessageBody::Invoke {
                operation: "bridge".into(),
                input: Value::Null.to_wire().expect("encode null"),
                callback_id: 49,
            }),
            "plugin-status",
        );
        client.callback_mode = CallbackMode::Normal;
        assert_eq!(client.request(MessageBody::Disable), MessageBody::Ok);
        assert_eq!(client.request(MessageBody::Shutdown), MessageBody::Ok);
        client.finish();
    }
}

#[test]
fn malformed_frame_is_fatal_but_clean_eof_before_enable_is_successful() {
    let mut malformed = spawn_process();
    let mut malformed_input = malformed.stdin.take().expect("malformed child stdin");
    malformed_input
        .write_all(&[0, 0, 0, 0])
        .expect("write malformed frame");
    drop(malformed_input);
    assert!(
        !malformed
            .wait()
            .expect("wait for malformed child")
            .success()
    );

    let fixtures = fixtures();
    let mut clean = ProcessClient::start();
    assert_eq!(clean.request(MessageBody::Hello), MessageBody::Ok);
    assert_eq!(
        clean.request(load_body(fixtures, &fixtures.valid)),
        MessageBody::Ok
    );
    drop(clean.input.take());
    let status = clean.child.wait().expect("wait for clean EOF child");
    assert!(status.success());
}

struct ProcessClient {
    child: Child,
    input: Option<BufWriter<ChildStdin>>,
    output: BufReader<ChildStdout>,
    next_request_id: u64,
    bridge_operations: Vec<String>,
    handle_events: Vec<String>,
    bridge_failure: Option<String>,
    callback_mode: CallbackMode,
}

#[derive(Clone, Copy)]
enum CallbackMode {
    Normal,
    MismatchedId,
    UnexpectedKind,
}

impl ProcessClient {
    fn start() -> Self {
        let mut child = spawn_process();
        let input = BufWriter::new(child.stdin.take().expect("child stdin"));
        let output = BufReader::new(child.stdout.take().expect("child stdout"));
        Self {
            child,
            input: Some(input),
            output,
            next_request_id: 1,
            bridge_operations: Vec::new(),
            handle_events: Vec::new(),
            bridge_failure: None,
            callback_mode: CallbackMode::Normal,
        }
    }

    fn request(&mut self, body: MessageBody) -> MessageBody {
        let request_id = self.next_request_id;
        self.next_request_id += 2;
        write_frame(
            self.input.as_mut().expect("open child stdin"),
            &Message::new(request_id, body).expect("valid parent request"),
        )
        .expect("write parent request");
        self.input
            .as_mut()
            .expect("open child stdin")
            .flush()
            .expect("flush parent request");
        loop {
            let message = read_frame(&mut self.output)
                .expect("read child frame")
                .expect("child response before EOF");
            if message.request_id() == request_id {
                return message.body().clone();
            }
            self.answer_callback(message);
        }
    }

    fn request_with_id(&mut self, expected_request_id: u64, body: MessageBody) -> MessageBody {
        assert_eq!(self.next_request_id, expected_request_id);
        self.request(body)
    }

    fn assert_no_extra_stdout(&mut self) {
        let mut trailing = Vec::new();
        self.output
            .read_to_end(&mut trailing)
            .expect("read remaining process stdout");
        assert!(
            trailing.is_empty(),
            "process Host emitted unexpected stdout: {trailing:?}"
        );
    }

    fn answer_callback(&mut self, message: Message) {
        let response_body = match message.body() {
            MessageBody::BridgeInvoke { operation, input } => {
                self.bridge_operations.push(operation.clone());
                match &self.bridge_failure {
                    Some(code) => MessageBody::CallbackError { code: code.clone() },
                    None => MessageBody::CallbackResult {
                        output: input.clone(),
                    },
                }
            }
            MessageBody::RetainHandle {
                object_id,
                generation,
            } => {
                self.handle_events
                    .push(format!("retain:{object_id}:{generation}"));
                MessageBody::CallbackResult { output: Vec::new() }
            }
            MessageBody::ReleaseHandle {
                object_id,
                generation,
            } => {
                self.handle_events
                    .push(format!("release:{object_id}:{generation}"));
                MessageBody::CallbackResult { output: Vec::new() }
            }
            _ => panic!("unexpected child callback: {:?}", message.body()),
        };
        let response_id = match self.callback_mode {
            CallbackMode::Normal | CallbackMode::UnexpectedKind => message.request_id(),
            CallbackMode::MismatchedId => message.request_id() + 2,
        };
        let response_body = match self.callback_mode {
            CallbackMode::UnexpectedKind => MessageBody::BridgeInvoke {
                operation: "unexpected.callback".into(),
                input: Vec::new(),
            },
            CallbackMode::Normal | CallbackMode::MismatchedId => response_body,
        };
        let response = Message::new(response_id, response_body).expect("valid callback response");
        write_frame(self.input.as_mut().expect("open child stdin"), &response)
            .expect("write callback response");
        self.input
            .as_mut()
            .expect("open child stdin")
            .flush()
            .expect("flush callback response");
    }

    fn finish(mut self) {
        let status = self.child.wait().expect("wait for process Host");
        assert!(status.success(), "process Host exited with {status}");
    }
}

impl Drop for ProcessClient {
    fn drop(&mut self) {
        if self.child.try_wait().ok().flatten().is_none() {
            let _ = self.child.kill();
            let _ = self.child.wait();
        }
    }
}

struct BoundedChild {
    child: Child,
}

impl BoundedChild {
    fn start() -> Self {
        Self {
            child: spawn_process(),
        }
    }

    fn take_stdin(&mut self) -> ChildStdin {
        self.child.stdin.take().expect("bounded child stdin")
    }

    fn wait_for_exit_until(&mut self, deadline: Instant) -> Option<ExitStatus> {
        loop {
            if let Some(status) = self.child.try_wait().expect("poll bounded child") {
                return Some(status);
            }
            if Instant::now() >= deadline {
                return None;
            }
            std::thread::sleep(Duration::from_millis(10));
        }
    }

    fn kill_and_reap_within(&mut self, deadline: Instant) -> ExitStatus {
        if let Some(status) = self.child.try_wait().expect("poll child before kill") {
            return status;
        }
        self.child.kill().expect("kill timed-out process Host");
        self.wait_for_exit_until(deadline)
            .expect("process Host did not reap within the parent timeout after kill")
    }
}

impl Drop for BoundedChild {
    fn drop(&mut self) {
        if self.child.try_wait().ok().flatten().is_none() {
            let _ = self.child.kill();
            let _ = self.wait_for_exit_until(Instant::now() + Duration::from_secs(2));
        }
    }
}

struct FixtureArtifacts {
    root: PathBuf,
    valid: PathBuf,
    missing: PathBuf,
    wrong_abi: PathBuf,
}

struct LaunchHookSample {
    root: PathBuf,
    library: PathBuf,
}

fn fixtures() -> &'static FixtureArtifacts {
    static ARTIFACTS: OnceLock<FixtureArtifacts> = OnceLock::new();
    ARTIFACTS.get_or_init(build_fixtures)
}

fn launch_hook_sample() -> &'static LaunchHookSample {
    static SAMPLE: OnceLock<LaunchHookSample> = OnceLock::new();
    SAMPLE.get_or_init(build_launch_hook_sample)
}

fn build_fixtures() -> FixtureArtifacts {
    let fixture_root =
        Path::new(env!("CARGO_MANIFEST_DIR")).join("../hmcl-rust-host-native/tests/fixtures");
    let target = fixture_root.join("target");
    let status = Command::new(env!("CARGO"))
        .args(["build", "--workspace", "--quiet"])
        .current_dir(&fixture_root)
        .env("CARGO_TARGET_DIR", &target)
        .status()
        .expect("run Cargo for process Host fixtures");
    assert!(status.success(), "fixture build failed");
    let artifact = |name: &str| {
        target.join("debug").join(format!(
            "{}{name}.{}",
            std::env::consts::DLL_PREFIX,
            std::env::consts::DLL_EXTENSION
        ))
    };
    FixtureArtifacts {
        root: target.join("debug"),
        valid: artifact("hmcl_embedded_fixture"),
        missing: artifact("hmcl_missing_query_fixture"),
        wrong_abi: artifact("hmcl_wrong_abi_fixture"),
    }
}

fn build_launch_hook_sample() -> LaunchHookSample {
    let repository = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let target = repository.join("target").join("launch-hook-test");
    let status = Command::new(env!("CARGO"))
        .args(["build", "--package", "hmcl-rust-launch-hook", "--locked"])
        .current_dir(&repository)
        .env("CARGO_TARGET_DIR", &target)
        .status()
        .expect("build Rust launch-hook sample");
    assert!(status.success(), "launch-hook sample build failed");
    let library = target.join("debug").join(format!(
        "{}hmcl_rust_launch_hook.{}",
        std::env::consts::DLL_PREFIX,
        std::env::consts::DLL_EXTENSION
    ));
    LaunchHookSample {
        root: library
            .parent()
            .expect("launch-hook library parent")
            .to_path_buf(),
        library,
    }
}

fn launch_hook_input() -> Vec<u8> {
    Value::Map(vec![
        ("contractVersion".into(), Value::Integer(1)),
        (
            "dispatchId".into(),
            Value::String("rust-stdio-hook-71".into()),
        ),
        ("point".into(), Value::String("before-game-launch".into())),
        (
            "occurredAt".into(),
            Value::String("2026-09-05T00:00:00Z".into()),
        ),
        ("data".into(), Value::Map(Vec::new())),
    ])
    .to_wire()
    .expect("encode launch Hook input")
}

fn literal_patch_request_wire() -> Vec<u8> {
    vec![
        0x92, 0x07, 0xdd, 0x00, 0x00, 0x00, 0x01, 0x92, 0xdb, 0x00, 0x00, 0x00, 0x0d, b's', b'c',
        b'h', b'e', b'm', b'a', b'V', b'e', b'r', b's', b'i', b'o', b'n', 0x92, 0x02, 0xd3, 0x00,
        0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x01,
    ]
}

fn literal_unchanged_hook_wire() -> Vec<u8> {
    vec![
        0x92, 0x07, 0xdd, 0x00, 0x00, 0x00, 0x02, 0x92, 0xdb, 0x00, 0x00, 0x00, 0x0f, b'c', b'o',
        b'n', b't', b'r', b'a', b'c', b't', b'V', b'e', b'r', b's', b'i', b'o', b'n', 0x92, 0x02,
        0xd3, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x01, 0x92, 0xdb, 0x00, 0x00, 0x00, 0x06,
        b'a', b'c', b't', b'i', b'o', b'n', 0x92, 0x04, 0xdb, 0x00, 0x00, 0x00, 0x09, b'u', b'n',
        b'c', b'h', b'a', b'n', b'g', b'e', b'd',
    ]
}

fn literal_unchanged_patch_wire() -> Vec<u8> {
    vec![
        0x92, 0x07, 0xdd, 0x00, 0x00, 0x00, 0x02, 0x92, 0xdb, 0x00, 0x00, 0x00, 0x0d, b's', b'c',
        b'h', b'e', b'm', b'a', b'V', b'e', b'r', b's', b'i', b'o', b'n', 0x92, 0x02, 0xd3, 0x00,
        0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x01, 0x92, 0xdb, 0x00, 0x00, 0x00, 0x06, b'a', b'c',
        b't', b'i', b'o', b'n', 0x92, 0x04, 0xdb, 0x00, 0x00, 0x00, 0x09, b'u', b'n', b'c', b'h',
        b'a', b'n', b'g', b'e', b'd',
    ]
}

fn spawn_process() -> Child {
    Command::new(env!("CARGO_BIN_EXE_hmcl-rust-host-process"))
        .arg("--stdio")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::inherit())
        .spawn()
        .expect("start process Host")
}

fn load_body(fixtures: &FixtureArtifacts, entrypoint: &Path) -> MessageBody {
    MessageBody::Load {
        package_root: fixtures.root.to_string_lossy().into_owned(),
        entrypoint: entrypoint
            .file_name()
            .expect("fixture filename")
            .to_string_lossy()
            .into_owned(),
        plugin_id: 7,
        session: 11,
    }
}

fn assert_error(body: MessageBody, expected_code: &str) {
    let MessageBody::Error { code, message } = body else {
        panic!("expected process error, got {body:?}");
    };
    assert_eq!(code, expected_code);
    assert!(!message.is_empty());
    assert!(message.len() <= 4096);
}
