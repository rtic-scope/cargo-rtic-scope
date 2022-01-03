#![no_std]
//! This crate exposes functionality that eases the procedure of tracing
//! embedded applications written using RTIC. A single feature-set is
//! available:
//!
//! - `tracing`: which offers setup functions that configures related
//!   peripherals for RTIC task tracing, and a `#[trace]` macro for
//!   software tasks. Example usage (TODO update):
//!   ```ignore
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

struct WatchVars {
    /// Watch variable to which the just entered software task ID is written to.
    enter: u32,

    /// Watch variable to which the just exited software task ID is written to.
    exit: u32,
}
static mut WATCH_VARIABLES: WatchVars = WatchVars { enter: 0, exit: 0 };

/// Auxilliary functions for peripheral configuration. Should be called
/// in the init-function, and preferably in order of (1)
/// [setup::core_peripherals]; (2) [setup::device_peripherals]; and last, (3)
/// [setup::assign_dwt_unit]. Refer to crate example usage.
pub mod setup {
    use cortex_m::peripheral as Core;
    use cortex_m::peripheral::{
        dwt::{AccessType, ComparatorAddressSettings, ComparatorFunction, EmitOption},
        itm::{GlobalTimestampOptions, ITMSettings, LocalTimestampOptions, TimestampClkSrc},
        tpiu::TraceProtocol,
    };

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

        dwt.enable_exception_tracing();
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

    /// Assigns and consumes a DWT comparator for RTIC software task
    /// tracing. The unit is indirectly utilized by [super::trace]. Any
    /// changes to the unit after this function yields undefined
    /// behavior, in regards to RTIC task tracing.
    pub fn assign_dwt_units(enter_dwt: &Core::dwt::Comparator, exit_dwt: &Core::dwt::Comparator) {
        let enter_addr: u32 = unsafe { &super::WATCH_VARIABLES.enter as *const _ } as u32;
        let exit_addr: u32 = unsafe { &super::WATCH_VARIABLES.exit as *const _ } as u32;

        for (dwt, addr) in [(enter_dwt, enter_addr), (exit_dwt, exit_addr)] {
            // TODO do we need to clear the MATCHED, bit[24] after every match?
            dwt.configure(ComparatorFunction::Address(ComparatorAddressSettings {
                address: addr,
                mask: 0,
                emit: EmitOption::Data,
                access_type: AccessType::WriteOnly,
            }))
            .unwrap(); // NOTE safe: valid (emit, access_type) used
        }
    }
}

// TODO only write as much as needed. e.g. for id < 256, only 8 bits
// must be written.

/// The function utilized by [trace] to write the unique software task
/// ID to the watch address. You are discouraged to use this function
/// directly; [trace] uses a sequence of task IDs compatible with the
/// `parsing` module. If used directly, task IDs must also be properly
/// configured for the host application.
#[inline]
pub fn __write_enter_id(id: u32) {
    unsafe {
        WATCH_VARIABLES.enter = id;
    }
}

#[inline]
pub fn __write_exit_id(id: u32) {
    unsafe {
        WATCH_VARIABLES.exit = id;
    }
}
