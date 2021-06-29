#![no_std]
//! This crate exposes functionality that eases the procedure of tracing
//! embedded applications written using RTIC. A single feature-set is
//! available:
//!
//! - `tracing`: which offers setup functions that configures related
//!   peripherals for RTIC task tracing, and a `#[trace]` macro for
//!   software tasks. Example usage (TODO update):
//!   ```
//!   #[app(device = stm32f4::stm32f401, peripherals = true, dispatchers = [EXTI1])]
//!   mod app {
//!       use rtic_trace::{self, tracing::trace};
//!       use stm32f4::stm32f401::Interrupt;
//!
//!       #[init]
//!       fn init(mut ctx: init::Context) -> (init::LateResources, init::Monotonics) {
//!           rtic_trace::tracing::setup::core_peripherals(
//!               &mut ctx.core.DCB,
//!               &mut ctx.core.TPIU,
//!               &mut ctx.core.DWT,
//!               &mut ctx.core.ITM,
//!           );
//!           rtic_trace::tracing::setup::device_peripherals(&mut ctx.device.DBGMCU);
//!           rtic_trace::tracing::setup::assign_dwt_unit(&ctx.core.DWT.c[1]);
//!
//!           rtic::pend(Interrupt::EXTI0);
//!
//!           (init::LateResources {}, init::Monotonics())
//!       }
//!
//!       #[task(binds = EXTI0, priority = 1)]
//!       fn spawner(_ctx: spawner::Context) {
//!           software_task::spawn().unwrap();
//!       }
//!
//!       #[task]
//!       #[trace]
//!       fn software_task(_ctx: software_task::Context) {
//!           #[trace]
//!           fn sub_software_task() {
//!           }
//!       }
//!   }
//!   ```

/// The tracing macro. Takes no arguments and should be placed on a
/// function. Refer to crate example usage.
pub use rtic_trace_macros::trace;

// TODO is there an even better way to store this?
static mut WATCH_VARIABLE: u32 = 0;

/// Auxilliary functions for peripheral configuration. Should be called
/// in the init-function, and preferably in order of (1)
/// [setup::core_peripherals]; (2) [setup::device_peripherals]; (3)
/// [setup::assign_dwt_unit]; and last, (4)
/// [setup::send_trace_clk_freq]. Refer to crate example usage.
pub mod setup {
    use cortex_m::peripheral as Core;
    use cortex_m::peripheral::{
        dwt::{AccessType, ComparatorAddressSettings, ComparatorFunction, EmitOption},
        itm::{GlobalTimestampOptions, ITMSettings, LocalTimestampOptions, TimestampClkSrc},
        tpiu::TraceProtocol,
    };
    use stm32f4::stm32f401 as Device;

    /// Configures all related core peripherals for RTIC task tracing.
    // TODO add option to enable/disable global/local timestamps?
    pub fn core_peripherals(
        dcb: &mut Core::DCB,
        tpiu: &mut Core::TPIU,
        dwt: &mut Core::DWT,
        itm: &mut Core::ITM,
    ) {
        // TODO check feature availability; return error if not supported.

        // enable tracing
        dcb.enable_trace();

        tpiu.set_swo_baud_rate(16_000_000, 115_200);
        tpiu.set_trace_output_protocol(TraceProtocol::AsyncSWONRZ);
        tpiu.enable_continuous_formatting(false); // drops ETM packets

        dwt.enable_exception_tracing(true);
        dwt.enable_pc_samples(false);

        itm.unlock();
        itm.configure(ITMSettings {
            enable: true,      // ITMENA: master enable
            forward_dwt: true, // TXENA: forward DWT packets
            local_timestamps: LocalTimestampOptions::Enabled,
            global_timestamps: GlobalTimestampOptions::Disabled,
            bus_id: Some(1),
            timestamp_clk_src: TimestampClkSrc::SystemClock,
        });
    }

    /// Configures all related device peripherals for RTIC task tracing.
    pub fn device_peripherals(dbgmcu: &mut Device::DBGMCU) {
        #[rustfmt::skip]
        dbgmcu.cr.modify(
            |_, w| unsafe {
                w.trace_ioen().set_bit() // master enable for tracing
                 .trace_mode().bits(0b00) // TRACE pin assignment for async mode (SWO)
            },
        );
    }

    /// Assigns and consumes a DWT comparator for RTIC software task
    /// tracing. The unit is indirectly utilized by [super::trace]. Any
    /// changes to the unit after this function yields undefined
    /// behavior, in regards to RTIC task tracing.
    pub fn assign_dwt_unit(dwt: &Core::dwt::Comparator) {
        let watch_address: u32 = unsafe { &super::WATCH_VARIABLE as *const _ } as u32;
        // TODO do we need to clear the MATCHED, bit[24] after every match?
        dwt.configure(ComparatorFunction::Address(ComparatorAddressSettings {
            address: watch_address,
            mask: 0,
            emit: EmitOption::Data,
            access_type: AccessType::WriteOnly,
        }))
        .unwrap(); // NOTE safe: valid (emit, access_type) used
    }

    /// Sends the given frequency over ITM to the RTIC Scope backend to
    /// be registered as the trace clock frequency. Used to convert
    /// timestamp register values to absolute timestamps. Should only be
    /// called once, and after [assign_dwt_unit].
    pub fn send_trace_clk_freq(freq: u32) {
        // Write all 4 bytes, which denote that the payload is a trace
        // clock frequency.
        super::__write_trace_payload(freq);
    }
}

/// The function utilized by [trace] to write the unique software task
/// ID to the watch address. You are discouraged to use this function
/// directly; [trace] uses a sequence of task IDs compatible with the
/// `parsing` module. If used directly, task IDs must also be properly
/// configured for the host application.
#[inline]
pub fn __write_trace_payload(id: u32) {
    // TODO only write as much as needed. e.g. for id < 256, only 8 bits
    // must be written.
    unsafe {
        WATCH_VARIABLE = id;
    }
}
