#![deny(missing_docs)]

//! Stdio process owner for one isolated Rust Runtime payload.

mod server;

use crate::server::Server;
use std::io::{self, BufReader, BufWriter};

fn main() {
    if let Err(error) = run() {
        eprintln!("Rust isolated Host failed: {error}");
        std::process::exit(2);
    }
}

fn run() -> Result<(), Box<dyn std::error::Error>> {
    let mut arguments = std::env::args_os();
    let _program = arguments.next();
    let mode = arguments.next();
    if arguments.next().is_some() {
        return Err("Rust isolated Host accepts exactly one mode argument".into());
    }
    match mode.as_deref().and_then(std::ffi::OsStr::to_str) {
        Some("--probe") => Ok(()),
        Some("--stdio") => Server::new(BufReader::new(io::stdin()), BufWriter::new(io::stdout()))
            .serve()
            .map_err(Into::into),
        _ => Err("Rust isolated Host requires --stdio or --probe".into()),
    }
}
