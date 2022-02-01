#![allow(rustdoc::bare_urls)]
//! Reference frontend implementation for RTIC Scope.
#![doc = include_str!("../../docs/profile/README.md")]

use anyhow::{Context, Result};
use pyo3::prelude::*;
use pyo3::types::{PyDict, PyString};
use rtic_scope_api as api;
use serde_json::Deserializer;
use std::string::String;

fn main() -> Result<()> {
    // Create frontend socket in a temporary directory, print it for the parent backend.
    let socket_dir = tempfile::TempDir::new()
        .context("Failed to create temporary directory for frontend socket")?;
    let socket_path = socket_dir.path().join("rtic-scope-frontend.socket");
    let listener = std::os::unix::net::UnixListener::bind(&socket_path)
        .context("Failed to bind frontend socket")?;
    println!("{}", socket_path.display());

    let mut task_enters = std::collections::HashMap::new();
    let mut csv = String::new();

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
        eprintln!("@{nanos} µs (+{diff} ns) [{quality}]: {events:?}");
        prev_nanos = nanos;

        for e in events {
            match e {
                api::EventType::Task { name, action } if action == api::TaskAction::Entered => {
                    task_enters.insert(name, nanos);
                }
                api::EventType::Task { name, action } if action == api::TaskAction::Exited => {
                    let start = task_enters.get(&name).unwrap_or(&0);
                    let end = nanos;
                    let entry = format!("{name},{start},{end}\n");
                    eprintln!("CSV: {entry}");
                    csv.push_str(entry.as_str());
                }
                _ => (),
            }
        }
    }

    pyo3::prepare_freethreaded_python();
    Python::with_gil(|py| {
        let locals = PyDict::new(py);
        let csv = PyString::new(py, &csv);
        locals.set_item("csv", csv).unwrap();
        py.run(
            r#"
import pandas as pd
import io
import matplotlib.pyplot as plt

df = pd.read_csv(io.StringIO(csv), header=None, names=['Task', 'Start', 'End'])
df['Diff'] = df.End - df.Start

fig, ax = plt.subplots(figsize=(6,3))

labels = []
for i, task in enumerate(df.groupby('Task')):
    labels.append(task[0])
    data = task[1][["Start", "Diff"]]
    ax.broken_barh(data.values, (i - 0.4, 0.8))

ax.set_yticks(range(len(labels)))
ax.set_yticklabels(labels)
ax.set_xlabel('time [ns]')
ax.set_ylabel('RTIC task')
plt.tight_layout()
plt.grid(visible=True, which='both', axis='x')
plt.show()
        "#,
            None,
            Some(locals),
        )
        .unwrap();
    });

    Ok(())
}
