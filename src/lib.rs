//! API used between RTIC Scope front- and backends.

use chrono::prelude::Local;
use serde::{Deserialize, Serialize};

#[allow(unused_imports)]
use itm_decode::ExceptionAction;

use itm_decode::{TracePacket, MalformedPacket, TimestampDataRelation};

/// Derivative of [itm_decode::Timestamp]; an absolute timestamp `ts`
/// replaces `base` and `delta`.
#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct Timestamp {
    /// Absolute timestamp value
    pub ts: chrono::DateTime<Local>,

    /// In what manner this timestamp relate to the associated data
    /// packets, if known.
    pub data_relation: Option<TimestampDataRelation>,

    /// An overflow packet was recieved, which may have been caused by a
    /// local timestamp counter overflow. See
    /// [itm_decode::Timestamp::diverged].
    pub diverged: bool,
}

/// A set of events that occurred at a certain timepoint during target
/// execution.
#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct EventChunk {
    /// Collective timestamp for the chunk of [EventChunk::events].
    pub timestamp: Timestamp,

    /// Set of events that occured during [EventChunk::timestamp].
    pub events: Vec<EventType>,
}

/// Verbatim copy of [ExceptionAction], sans the enum name.
#[derive(Serialize, Deserialize, Debug, Clone)]
pub enum TaskAction {
    /// Task was entered.
    Entered,

    /// Task was exited.
    Exited,

    /// Task was returned to.
    Returned,
}

/// Derivative of [TracePacket], where RTIC task information has
/// been resolved.
#[derive(Serialize, Deserialize, Debug, Clone)]
pub enum EventType {
    /// Equivalent to [TracePacket::Overflow].
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

    /// Target emitted package for which no RTIC information could be
    /// associated. Optional string describes why a packet information
    /// could not be mapped.
    Unknown(TracePacket, Option<String>),

    Invalid(MalformedPacket),
}
