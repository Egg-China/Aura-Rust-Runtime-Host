#![deny(unsafe_op_in_unsafe_fn)]
#![deny(missing_docs)]

//! JNI-capable native owner for embedded and isolated HMCL Rust plugin payloads.

mod bridge;
pub mod embedded;
mod error;
mod jni_api;

pub use error::HostError;
