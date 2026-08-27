use thiserror::Error;

/// Redacted native Host failure surfaced to Java or conformance tests.
#[derive(Debug, Error)]
pub enum HostError {
    /// Filesystem access failed while resolving or opening a payload.
    #[error("payload I/O failed: {0}")]
    Io(#[from] std::io::Error),

    /// The operating-system dynamic library loader rejected the payload.
    #[error("dynamic library loading failed: {0}")]
    Library(#[from] libloading::Error),

    /// The resolved payload library escaped its verified package root.
    #[error("payload entrypoint escapes its package root")]
    EntrypointEscape,

    /// The payload did not export the frozen version-one query symbol.
    #[error("payload does not export hmcl_plugin_query_v1")]
    MissingQuerySymbol,

    /// The query callback returned a non-success ABI status.
    #[error("payload query returned status {0}")]
    QueryStatus(i32),

    /// The returned plugin function table advertises an unsupported ABI.
    #[error("payload returned unsupported plugin ABI {0}")]
    UnsupportedPluginAbi(u32),

    /// The returned plugin function table is short or omits a required callback.
    #[error("payload returned an incomplete plugin function table")]
    InvalidPluginTable,

    /// A payload lifecycle callback returned a non-success status.
    #[error("payload callback returned status {0}")]
    PluginStatus(i32),

    /// A requested payload handle is not owned by this engine.
    #[error("unknown payload handle {0}")]
    UnknownPayload(u64),

    /// A lifecycle request is incompatible with the payload's current state.
    #[error("invalid payload state: {0}")]
    InvalidState(&'static str),

    /// The provider-wide engine no longer accepts payload work.
    #[error("native engine is closed")]
    EngineClosed,

    /// A Host callback or owned-buffer contract was violated.
    #[error("invalid Host callback result")]
    InvalidHostResult,

    /// A Java Runtime Bridge callback or JNI conversion failed.
    #[error("Java Runtime Bridge call failed")]
    JavaBridge,

    /// A panic was contained before it could cross the JNI boundary.
    #[error("native Host panicked at the JNI boundary")]
    JniPanic,
}
