//! API used between RTIC Scope front- and backends.

pub use itm::Timestamp;
use itm::{ExceptionAction, MalformedPacket, TracePacket};
use serde::{Deserialize, Serialize};

/// [RTIC](https://rtic.rs) nomenclature alias.
pub type TaskAction = ExceptionAction;

/// A set of events that occurred at a certain timepoint during target
/// execution.
#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct EventChunk {
    /// Collective timestamp for the chunk of [`EventChunk::events`].
    pub timestamp: Timestamp,

    /// Set of events that occured during [`EventChunk::timestamp`].
    pub events: Vec<EventType>,
}

/// Derivative of [`TracePacket`], where RTIC task information has
/// been resolved.
#[derive(Serialize, Deserialize, Debug, Clone)]
pub enum EventType {
    /// Equivalent to [`TracePacket::Overflow`].
    Overflow,

    /// An RTIC task performed an action. Either a software or a
    /// hardware task.
    Task {
        /// What RTIC task did something?

        /// Name of the RTIC task that did something. For example,
        /// `"app::some_task"`.
        name: String,

        /// What did the task do?
        action: TaskAction,
    },

    /// RTIC Scope does not know how to map this packet.
    Unknown(TracePacket),

    /// RTIC Scope knows how to map this packet, but recovered
    /// translation maps does not contain the correct information.
    Unmappable(TracePacket, String),

    /// Packet could not be decoded.
    Invalid(MalformedPacket),
}
