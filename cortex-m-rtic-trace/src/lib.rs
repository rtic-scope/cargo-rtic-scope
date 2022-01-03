#![no_std]
//! This crate exposes functionality that eases the procedure of
//! enabling tracing embedded applications written using RTIC. A single
//! function, [configure] is exposed that configures all relevant
//! peripherals to trace software and hardware tasks. After [configure]
//! has been called, all hardware tasks will be traced. To trace
//! software tasks, each software task must also be decorated with the
//! [trace] macro. See the below example:
//!
//!   ```ignore
//!   #![no_main]
//!   #![no_std]
//!   use rtic;
//!
//!   #[rtic::app(device = stm32f4::stm32f401, dispatchers = [EXTI1])]
//!   mod app {
//!       use cortex_m_rtic_trace::{
//!           self,
//!           trace,
//!           TraceConfiguration,
//!           LocalTimestampOptions,
//!           GlobalTimestampOptions,
//!           TimestampClkSrc,
//!           TraceProtocol
//!       };
//!       use stm32f4::stm32f401::Interrupt;
//!
//!       #[shared]
//!       struct Shared {}
//!
//!       #[local]
//!       struct Local {}
//!
//!       #[init]
//!       fn init(mut ctx: init::Context) -> (Shared, Local, init::Monotonics) {
//!           cortex_m_rtic_trace::configure(
//!               &mut ctx.core.DCB,
//!               &mut ctx.core.TPIU,
//!               &mut ctx.core.DWT,
//!               &mut ctx.core.ITM,
//!               1, // task enter DWT comparator ID
//!               2, // task exit DWT comparator ID
//!               TraceConfiguration {
//!                   delta_timestamps: LocalTimestampOptions::Enabled,
//!                   absolute_timestamps: GlobalTimestampOptions::Disabled,
//!                   timestamp_clk_src: TimestampClkSrc::AsyncTPIU,
//!                   tpiu_freq: 16_000_000, // Hz
//!                   tpiu_baud: 115_200, // B/s
//!                   protocol: TraceProtocol::AsyncSWONRZ,
//!               }
//!           ).unwrap();
//!
//!           rtic::pend(Interrupt::EXTI0);
//!           (Shared {}, Local {}, init::Monotonics())
//!       }
//!
//!       // This hardware task is traced by calling configure above in init.
//!       #[task(binds = EXTI0, priority = 1)]
//!       fn hardware_task(_: hardware_task::Context) {
//!           software_task::spawn().unwrap();
//!       }
//!
//!       // This software task is traced by calling configure above in init and by
//!       //decorating it with #[trace].
//!       #[task]
//!       #[trace]
//!       fn software_task(_: software_task::Context) {
//!       }
//!   }
//!   ```
use cortex_m::peripheral::{
    self as Core,
    dwt::{AccessType, ComparatorAddressSettings, ComparatorFunction, EmitOption},
    itm::ITMSettings,
};
pub use cortex_m::peripheral::{
    itm::{GlobalTimestampOptions, LocalTimestampOptions, TimestampClkSrc},
    tpiu::TraceProtocol,
};

/// The tracing macro. Takes no arguments and should be placed on a
/// function. Refer to crate example usage.
pub use rtic_trace_macros::trace;

/// Trace configuration to apply via [configure].
#[derive(Debug, Eq, PartialEq, Copy, Clone)]
pub struct TraceConfiguration {
    /// Whether delta (local) timestamps should be generated, and with what prescaler.
    pub delta_timestamps: LocalTimestampOptions,
    /// Whether absolute (global) timestamps should be generated, and how often.
    pub absolute_timestamps: GlobalTimestampOptions,
    /// The clock that should source the ITM timestamp.
    pub timestamp_clk_src: TimestampClkSrc,
    /// The frequency of the TPIU source clock.
    pub tpiu_freq: u32,
    /// The baud rate of the TPIU.
    pub tpiu_baud: u32,
    /// The protocol and mode of operation the TPIU should use.
    pub protocol: TraceProtocol,
}

/// Possible errors on [configure].
#[derive(Debug, Eq, PartialEq, Copy, Clone)]
pub enum TraceConfigurationError {
    /// Requested SWO mode of operation is not supported by the target.
    SWOProtocol,
    /// The target dooes not support trace sampling and exception tracing.
    Trace,
    /// Absolute (global) timestamps are not supported by the target.
    GTS,
    /// The TPIU clock frequency or baud rate (or both) are invalid.
    TPIUConfig,
    /// The [Core::ITM] configuration failed to apply. See [Core::ITM::configure].
    ITMConfig(Core::itm::ITMConfigurationError),
}

impl From<Core::itm::ITMConfigurationError> for TraceConfigurationError {
    fn from(itm: Core::itm::ITMConfigurationError) -> Self {
        Self::ITMConfig(itm)
    }
}

/// Container of a variable in memory that is watched by a DWT
/// comparator to enable software task tracing. Word-aligned to help
/// with address comparison.
///
/// XXX Is word-alignment necessary? Can't we use a mask instead?
#[repr(align(4))]
struct WatchVariable {
    /// ID of the software task that was entered or exited.
    pub id: u8,
}

/// Watch variable to which the just entered software task ID is written to. Aligned to 32-bit.
static mut WATCH_VARIABLE_ENTER: WatchVariable = WatchVariable { id: 0 };
/// Watch variable to which the just exited software task ID is written to. Aligned to 32-bit.
static mut WATCH_VARIABLE_EXIT: WatchVariable = WatchVariable { id: 0 };

/// Configures the ARMv7-M peripherals for RTIC hardware and software
/// task tracing. Fails if the configuration cannot be applied.
pub fn configure(
    dcb: &mut Core::DCB,
    tpiu: &mut Core::TPIU,
    dwt: &mut Core::DWT,
    itm: &mut Core::ITM,
    enter_dwt_idx: usize,
    exit_dwt_idx: usize,
    config: &TraceConfiguration,
) -> Result<(), TraceConfigurationError> {
    // Check hardware flags for tracing support, verify input.
    {
        use TraceConfigurationError as Error;

        let supports = tpiu.swo_supports();
        if !{
            match config.protocol {
                TraceProtocol::Parallel => supports.parallel_operation,
                TraceProtocol::AsyncSWOManchester => supports.manchester_encoding,
                TraceProtocol::AsyncSWONRZ => supports.nrz_encoding,
            }
        } {
            return Err(Error::SWOProtocol);
        }

        if config.tpiu_freq == 0 || config.tpiu_baud == 0 {
            return Err(Error::TPIUConfig);
        }

        if !dwt.has_exception_trace() {
            return Err(Error::Trace);
        }
    }

    // Configure DCB, TPIU, DWT, ITM for hardware task tracing.
    dcb.enable_trace();
    tpiu.set_swo_baud_rate(config.tpiu_freq, config.tpiu_baud);
    tpiu.set_trace_output_protocol(config.protocol);
    tpiu.enable_continuous_formatting(false); // drop ETM packets
    dwt.enable_exception_tracing();
    itm.unlock();
    itm.configure(ITMSettings {
        enable: true,      // ITMENA: master enable
        forward_dwt: true, // TXENA: forward DWT packets
        local_timestamps: config.delta_timestamps,
        global_timestamps: config.absolute_timestamps,
        bus_id: None, // only a single trace source is currently supported
        timestamp_clk_src: config.timestamp_clk_src,
    })?;

    // Configure DWT comparators for software task tracing.
    let enter_addr: u32 = unsafe { &WATCH_VARIABLE_ENTER.id as *const _ } as u32;
    let exit_addr: u32 = unsafe { &WATCH_VARIABLE_EXIT.id as *const _ } as u32;
    for (dwt, addr) in [
        (&dwt.c[enter_dwt_idx], enter_addr),
        (&dwt.c[exit_dwt_idx], exit_addr),
    ] {
        // TODO do we need to clear the MATCHED, bit[24] after every match?
        dwt.configure(ComparatorFunction::Address(ComparatorAddressSettings {
            address: addr,
            mask: 0,
            emit: EmitOption::Data,
            access_type: AccessType::WriteOnly,
        }))
        .unwrap(); // NOTE safe: valid (emit, access_type) used
    }

    Ok(())
}

/// Function utilized by [trace] to write the unique ID of the just
/// entered software task to its associated watch address. Only use this
/// function via [trace].
#[inline]
pub fn __write_enter_id(id: u8) {
    unsafe {
        WATCH_VARIABLE_ENTER.id = id;
    }
}

/// Function utilized by [trace] to write the unique ID of the software
/// task about to exit to its associated watch address. Only use this
/// function via [trace].
#[inline]
pub fn __write_exit_id(id: u8) {
    unsafe {
        WATCH_VARIABLE_EXIT.id = id;
    }
}
