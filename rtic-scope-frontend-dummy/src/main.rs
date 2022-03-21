#![allow(rustdoc::bare_urls)]
//! Reference frontend implementation for RTIC Scope.
#![doc = include_str!("../../docs/profile/README.md")]

use anyhow::{Context, Result};
use rtic_scope_api as api;
use serde_json::Deserializer;

fn main() -> Result<()> {
    // Create frontend socket in a temporary directory, print it for the parent backend.
    let socket_dir = tempfile::TempDir::new()
        .context("Failed to create temporary directory for frontend socket")?;
    let socket_path = socket_dir.path().join("rtic-scope-frontend.socket");
    let listener = std::os::unix::net::UnixListener::bind(&socket_path)
        .context("Failed to bind frontend socket")?;
    println!("{}", socket_path.display());

    // Deserialize api::EventChunks from socket and print events to
    // stderr along with nanoseconds timestamp.
    let (socket, _addr) = listener.accept().context("Failed to accept()")?;
    let stream = Deserializer::from_reader(socket).into_iter::<api::EventChunk>();
    let mut prev_nanos = 0;
    for chunk in stream {
        let api::EventChunk { timestamp, events } = chunk.context("Failed to deserialize chunk")?;
        let (quality, nanos) = match timestamp {
            api::Timestamp::Sync(offset) | api::Timestamp::AssocEventDelay(offset) => {
                ("good", offset.as_nanos())
            }
            api::Timestamp::UnknownDelay { prev: _, curr }
            | api::Timestamp::UnknownAssocEventDelay { prev: _, curr } => ("bad!", curr.as_nanos()),
        };
        let diff = nanos - prev_nanos;
        eprintln!("@{nanos} ns (+{diff} ns) [{quality}]: {events:?}");
        prev_nanos = nanos;
    }

    Ok(())
}
