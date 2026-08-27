use hmcl_runtime_protocol::{Message, MessageBody, ProtocolError, read_frame, write_frame};
use hmcl_rust_host_native::HostError;
use hmcl_rust_host_native::embedded::{BridgeTransport, Engine};
use std::io::{Read, Write};
use std::path::Path;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

pub(crate) struct Server<R, W> {
    reader: Arc<Mutex<R>>,
    writer: Arc<Mutex<W>>,
    state: State,
}

impl<R, W> Server<R, W>
where
    R: Read + Send + 'static,
    W: Write + Send + 'static,
{
    pub(crate) fn new(reader: R, writer: W) -> Self {
        Self {
            reader: Arc::new(Mutex::new(reader)),
            writer: Arc::new(Mutex::new(writer)),
            state: State::AwaitHello,
        }
    }

    pub(crate) fn serve(mut self) -> Result<(), ProtocolError> {
        loop {
            let message = {
                let mut reader = self
                    .reader
                    .lock()
                    .map_err(|_| invalid_protocol("process reader lock is poisoned"))?;
                read_frame(&mut *reader)?
            };
            let Some(message) = message else {
                self.close_on_eof();
                return Ok(());
            };
            if message.request_id().is_multiple_of(2) {
                return Err(invalid_protocol(
                    "callback response arrived outside a callback",
                ));
            }
            let request_id = message.request_id();
            let (response, close) = self.handle(message.body().clone())?;
            {
                let mut writer = self
                    .writer
                    .lock()
                    .map_err(|_| invalid_protocol("process writer lock is poisoned"))?;
                write_frame(
                    &mut *writer,
                    &Message::new(request_id, response)
                        .map_err(|_| invalid_protocol("invalid process response"))?,
                )?;
                writer.flush()?;
            }
            if close {
                return Ok(());
            }
        }
    }

    fn handle(&mut self, body: MessageBody) -> Result<(MessageBody, bool), ProtocolError> {
        let (result, close) = match body {
            MessageBody::Hello => (self.hello(), false),
            MessageBody::Load {
                package_root,
                entrypoint,
                plugin_id,
                session,
            } => (
                self.load(&package_root, &entrypoint, plugin_id, session),
                false,
            ),
            MessageBody::Enable => (self.enable(), false),
            MessageBody::Invoke {
                operation,
                input,
                callback_id,
            } => (self.invoke(&operation, &input, callback_id), false),
            MessageBody::Disable => (self.disable(), false),
            MessageBody::Shutdown => {
                let result = self.shutdown();
                let close = result.is_ok();
                (result.map(|_| MessageBody::Ok), close)
            }
            MessageBody::Ok
            | MessageBody::Result { .. }
            | MessageBody::Error { .. }
            | MessageBody::BridgeInvoke { .. }
            | MessageBody::RetainHandle { .. }
            | MessageBody::ReleaseHandle { .. }
            | MessageBody::CallbackResult { .. }
            | MessageBody::CallbackError { .. } => {
                return Err(invalid_protocol(
                    "child-only message arrived as a parent command",
                ));
            }
        };
        Ok((result.unwrap_or_else(error_body), close))
    }

    fn hello(&mut self) -> Result<MessageBody, HostError> {
        if !matches!(self.state, State::AwaitHello) {
            return Err(HostError::InvalidState("hello already completed"));
        }
        self.state = State::AwaitLoad;
        Ok(MessageBody::Ok)
    }

    fn load(
        &mut self,
        package_root: &str,
        entrypoint: &str,
        plugin_id: u64,
        session: u64,
    ) -> Result<MessageBody, HostError> {
        if !matches!(self.state, State::AwaitLoad) {
            return Err(HostError::InvalidState("payload load is not available"));
        }
        let bridge = Arc::new(ProcessBridge {
            reader: Arc::clone(&self.reader),
            writer: Arc::clone(&self.writer),
            plugin_id,
            session,
            next_callback_id: AtomicU64::new(2),
        });
        let engine = Engine::new(bridge);
        let payload_id = engine.load_payload(
            Path::new(package_root),
            Path::new(entrypoint),
            plugin_id,
            session,
        )?;
        self.state = State::Loaded { engine, payload_id };
        Ok(MessageBody::Ok)
    }

    fn enable(&self) -> Result<MessageBody, HostError> {
        let (engine, payload_id) = self.loaded()?;
        engine.enable_payload(payload_id)?;
        Ok(MessageBody::Ok)
    }

    fn invoke(
        &self,
        operation: &str,
        input: &[u8],
        callback_id: u64,
    ) -> Result<MessageBody, HostError> {
        let (engine, payload_id) = self.loaded()?;
        engine
            .invoke_payload(payload_id, operation, input, callback_id)
            .map(|output| MessageBody::Result { output })
    }

    fn disable(&self) -> Result<MessageBody, HostError> {
        let (engine, payload_id) = self.loaded()?;
        engine.disable_payload(payload_id)?;
        Ok(MessageBody::Ok)
    }

    fn shutdown(&mut self) -> Result<(), HostError> {
        let (engine, payload_id) = self.loaded()?;
        engine.unload_payload(payload_id)?;
        self.state = State::Closed;
        Ok(())
    }

    fn loaded(&self) -> Result<(&Engine, u64), HostError> {
        match &self.state {
            State::Loaded { engine, payload_id } => Ok((engine, *payload_id)),
            State::AwaitHello => Err(HostError::InvalidState("hello is required")),
            State::AwaitLoad => Err(HostError::InvalidState("payload is not loaded")),
            State::Closed => Err(HostError::EngineClosed),
        }
    }

    fn close_on_eof(&self) {
        if let State::Loaded { engine, .. } = &self.state {
            let _ = engine.close();
        }
    }
}

enum State {
    AwaitHello,
    AwaitLoad,
    Loaded { engine: Engine, payload_id: u64 },
    Closed,
}

struct ProcessBridge<R, W> {
    reader: Arc<Mutex<R>>,
    writer: Arc<Mutex<W>>,
    plugin_id: u64,
    session: u64,
    next_callback_id: AtomicU64,
}

impl<R, W> ProcessBridge<R, W>
where
    R: Read + Send + 'static,
    W: Write + Send + 'static,
{
    fn call(&self, body: MessageBody) -> Result<MessageBody, HostError> {
        let request_id = self.next_callback_id.fetch_add(2, Ordering::Relaxed);
        if request_id == 0 || request_id > i64::MAX as u64 {
            return Err(HostError::InvalidHostResult);
        }
        let message = Message::new(request_id, body).map_err(protocol_as_host)?;
        {
            let mut writer = self
                .writer
                .lock()
                .map_err(|_| HostError::InvalidHostResult)?;
            write_frame(&mut *writer, &message).map_err(protocol_as_host)?;
            writer.flush()?;
        }
        let response = {
            let mut reader = self
                .reader
                .lock()
                .map_err(|_| HostError::InvalidHostResult)?;
            read_frame(&mut *reader).map_err(protocol_as_host)?
        }
        .ok_or(HostError::InvalidHostResult)?;
        if response.request_id() != request_id {
            return Err(HostError::InvalidHostResult);
        }
        Ok(response.body().clone())
    }
}

impl<R, W> BridgeTransport for ProcessBridge<R, W>
where
    R: Read + Send + 'static,
    W: Write + Send + 'static,
{
    fn invoke(
        &self,
        plugin: u64,
        session: u64,
        operation: &str,
        input: &[u8],
    ) -> Result<Vec<u8>, HostError> {
        if plugin != self.plugin_id || session != self.session {
            return Err(HostError::InvalidHostResult);
        }
        match self.call(MessageBody::BridgeInvoke {
            operation: operation.to_owned(),
            input: input.to_vec(),
        })? {
            MessageBody::CallbackResult { output } => Ok(output),
            MessageBody::CallbackError { .. } => Err(HostError::JavaBridge),
            _ => Err(HostError::InvalidHostResult),
        }
    }

    fn retain_handle(
        &self,
        session: u64,
        object_id: u64,
        generation: u64,
    ) -> Result<(), HostError> {
        self.handle_call(
            session,
            MessageBody::RetainHandle {
                object_id,
                generation,
            },
        )
    }

    fn release_handle(
        &self,
        session: u64,
        object_id: u64,
        generation: u64,
    ) -> Result<(), HostError> {
        self.handle_call(
            session,
            MessageBody::ReleaseHandle {
                object_id,
                generation,
            },
        )
    }
}

impl<R, W> ProcessBridge<R, W>
where
    R: Read + Send + 'static,
    W: Write + Send + 'static,
{
    fn handle_call(&self, session: u64, body: MessageBody) -> Result<(), HostError> {
        if session != self.session {
            return Err(HostError::InvalidHostResult);
        }
        match self.call(body)? {
            MessageBody::CallbackResult { output } if output.is_empty() => Ok(()),
            MessageBody::CallbackError { .. } => Err(HostError::JavaBridge),
            _ => Err(HostError::InvalidHostResult),
        }
    }
}

fn error_body(error: HostError) -> MessageBody {
    let code = match &error {
        HostError::Io(_) => "payload-io",
        HostError::Library(_) => "library-load",
        HostError::EntrypointEscape => "entrypoint-escape",
        HostError::MissingQuerySymbol => "missing-query",
        HostError::QueryStatus(_) => "query-status",
        HostError::UnsupportedPluginAbi(_) => "unsupported-abi",
        HostError::InvalidPluginTable => "invalid-plugin-table",
        HostError::PluginStatus(_) => "plugin-status",
        HostError::UnknownPayload(_) => "unknown-payload",
        HostError::InvalidState(_) => "invalid-state",
        HostError::EngineClosed => "engine-closed",
        HostError::InvalidHostResult => "invalid-host-result",
        HostError::JavaBridge => "bridge-callback",
        HostError::JniPanic => "native-panic",
    };
    let mut message = error.to_string();
    if message.len() > 4096 {
        let mut end = 4096;
        while !message.is_char_boundary(end) {
            end -= 1;
        }
        message.truncate(end);
    }
    MessageBody::Error {
        code: code.to_owned(),
        message,
    }
}

fn protocol_as_host(error: ProtocolError) -> HostError {
    match error {
        ProtocolError::Io(error) => HostError::Io(error),
        ProtocolError::InvalidData(_) => HostError::InvalidHostResult,
    }
}

const fn invalid_protocol(message: &'static str) -> ProtocolError {
    ProtocolError::InvalidData(message)
}
