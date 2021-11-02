use crate::sources::{BufferStatus, Source, SourceError};
use crate::TraceData;

use itm_decode::{
    cortex_m::{Exception, VectActive},
    ExceptionAction, MalformedPacket, MemoryAccessType, Timestamp, TimestampDataRelation,
    TimestampedTracePackets, TracePacket,
};

use rand::prelude::*;

type SoftwareTaskID = u8;
type SoftwareTaskName = String;

struct Task {
    res: TaskType,
    state: ExceptionAction,
}

impl Task {
    pub fn flip_state(&mut self) {
        self.state = match self.state {
            ExceptionAction::Exited => ExceptionAction::Entered,
            ExceptionAction::Entered => ExceptionAction::Exited,
            ExceptionAction::Returned => panic!("invalid bogus trace task state"),
        };
    }
}

enum TaskType {
    Hardware(VectActive),
    Software(SoftwareTaskID),
}

pub struct BogusSource {
    timestamp: Timestamp,
    rng: ThreadRng,
    tasks: Vec<Task>,
}

impl BogusSource {
    pub fn new() -> Self {
        Self {
            timestamp: Timestamp {
                base: Some(0),
                delta: Some(0),
                data_relation: Some(TimestampDataRelation::Sync),
                diverged: false,
            },
            rng: thread_rng(),
            tasks: vec![
                Task {
                    res: TaskType::Hardware(VectActive::Exception(Exception::SysTick)),
                    state: ExceptionAction::Exited,
                },
                Task {
                    res: TaskType::Software(0),
                    state: ExceptionAction::Exited,
                },
                Task {
                    res: TaskType::Software(1),
                    state: ExceptionAction::Exited,
                },
                Task {
                    res: TaskType::Software(2),
                    state: ExceptionAction::Exited,
                },
            ],
        }
    }
}

impl Iterator for BogusSource {
    type Item = Result<TraceData, SourceError>;

    fn next(&mut self) -> Option<Self::Item> {
        // update the timestamp
        self.timestamp = Timestamp {
            base: Some(
                self.timestamp.base.unwrap()
                    + self.timestamp.delta.unwrap()
                    + self.rng.gen_range(1..5) * (10e6 as usize),
            ),
            delta: Some(self.rng.gen_range(1..5) * (10e3 as usize)),
            data_relation: self.timestamp.data_relation.clone(),
            diverged: false,
        };

        let mut packets = vec![];

        // from the pool of possible tasks, pick one and flip its state
        let task = self.tasks.iter_mut().choose(&mut self.rng).unwrap();
        task.flip_state();
        match task {
            Task {
                res: TaskType::Hardware(vect),
                state,
            } => {
                packets.push(TracePacket::ExceptionTrace {
                    exception: vect.clone(),
                    action: state.clone(),
                });
            }
            Task {
                res: TaskType::Software(id),
                state,
            } => {
                packets.push(TracePacket::DataTraceValue {
                    comparator: match state {
                        ExceptionAction::Entered => 1,
                        ExceptionAction::Exited => 2,
                        ExceptionAction::Returned => panic!("invalid bogus trace task state"),
                    },
                    access_type: MemoryAccessType::Write,
                    value: vec![*id],
                });
            }
        };

        // maybe include an unmappable packet
        let unmappable = [
            TracePacket::PCSample { pc: None },
            TracePacket::DataTracePC {
                comparator: self.rng.gen_range(1..8),
                pc: self.rng.gen_range(1..8) * (10e3 as u32),
            },
            TracePacket::EventCounterWrap {
                cyc: self.rng.gen(),
                fold: self.rng.gen(),
                lsu: self.rng.gen(),
                sleep: self.rng.gen(),
                exc: self.rng.gen(),
                cpi: self.rng.gen(),
            },
        ];
        if self.rng.gen() {
            packets.push(unmappable.iter().choose(&mut self.rng).unwrap().clone());
        }

        // include an unmappable packet
        let mut malformed_packets = vec![];
        let malformed = [
            MalformedPacket::InvalidHeader(self.rng.gen_range(0..u8::MAX)),
            MalformedPacket::InvalidExceptionTrace {
                exception: self.rng.gen_range(0..u16::MAX),
                function: self.rng.gen_range(0..u8::MAX),
            },
            MalformedPacket::InvalidSync(self.rng.gen_range(0..45)),
        ];
        if self.rng.gen() {
            malformed_packets.push(malformed.iter().choose(&mut self.rng).unwrap().clone());
        }

        Some(Ok(TimestampedTracePackets {
            timestamp: self.timestamp.clone(),
            packets,
            malformed_packets,
            packets_consumed: self.rng.gen_range(4..12),
        }))
    }
}

impl Source for BogusSource {
    fn avail_buffer(&self) -> BufferStatus {
        BufferStatus::NotApplicable
    }

    fn describe(&self) -> String {
        format!("randomly generated (bogus) trace source")
    }
}
