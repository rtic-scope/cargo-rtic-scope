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
        let chunk = chunk.context("Failed to deserialize chunk")?;
        let nanos = chunk.timestamp.offset.as_nanos();
        let diff = nanos - prev_nanos;
        eprintln!("@{} µs (+{} ns): {:?}", nanos, diff, chunk.events);
        prev_nanos = nanos;
    }

    Ok(())
}
