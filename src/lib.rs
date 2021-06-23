//! API used between RTIC Scope front- and backends.

use chrono::prelude::Local;
use itm_decode::ExceptionAction;
#[allow(unused_imports)]
use itm_decode::TracePacket;
use serde::{Deserialize, Serialize};
use anyhow::Result;

type Timestamp = chrono::DateTime<Local>;

/// A set of events that occurred at a certain timepoint after target
/// reset.
#[derive(Serialize, Deserialize, Debug)]
pub struct EventChunk {
    /// Collective timestamp for the chunk of [EventChunk::events].
    pub timestamp: Timestamp,

    pub events: Vec<EventType>,
}

/// Verbatim copy of [ExceptionAction], sans the enum name.
#[derive(Serialize, Deserialize, Debug)]
pub enum TaskAction {
    /// Task was entered.
    Entered,

    /// Task was exited.
    Exited,

    /// Task was returned to.
    Returned,
}

/// Derivative subset of [TracePacket], where RTIC task information has
/// been resolved.
#[derive(Serialize, Deserialize, Debug)]
pub enum EventType {
    /// [TracePacket::Overflow] equivalent.
    Overflow,

    /// An RTIC task performed an action.
    Task {
        /// What RTIC task did something?
        name: String,

        /// What did the RTIC task do?
        action: TaskAction,
    },
}

/// Required functionality of a RTIC Scope frontend.
pub trait Frontend {
    /// Spawns a thread that manages the frontend. Returned handle must
    /// be joinable after the tx-sibling of rx has dropped.
    fn spawn(rx: std::sync::mpsc::Receiver<EventChunk>) -> Result<std::thread::JoinHandle<Result<()>>>;
}
