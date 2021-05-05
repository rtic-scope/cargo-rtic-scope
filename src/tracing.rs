//! Tracing module, for use on the embedded target.
//!
//! TODO explain how it works
//!
//! Example usage:
//!
//! ```
//! #[app(device = stm32f4::stm32f401, peripherals = true, dispatchers = [EXTI1])]
//! mod app {
//!     use cortex_m::asm;
//!     use rtic_trace::{self, trace};
//!     use stm32f4::stm32f401::Interrupt;
//!
//!     #[init]
//!     fn init(mut ctx: init::Context) -> (init::LateResources, init::Monotonics) {
//!         rtic_trace::setup::core_peripherals(
//!             &mut ctx.core.DCB,
//!             &mut ctx.core.TPIU,
//!             &mut ctx.core.DWT,
//!             &mut ctx.core.ITM,
//!         );
//!         rtic_trace::setup::device_peripherals(&mut ctx.device.DBGMCU);
//!         rtic_trace::setup::assign_dwt_unit(&ctx.core.DWT.c[1]);
//!
//!         rtic::pend(Interrupt::EXTI0);
//!
//!         (init::LateResources {}, init::Monotonics())
//!     }
//!
//!     #[task(binds = EXTI0)]
//!     fn spawner(_ctx: spawner::Context) {
//!         software_task::spawn().unwrap();
//!     }
//!
//!     #[task]
//!     #[trace]
//!     fn software_task(_ctx: software_task::Context) {
//!         asm::delay(1024);
//!
//!         #[trace]
//!         fn func() {
//!             let _x = 42;
//!         }
//!
//!         asm::delay(1024);
//!         func();
//!     }
//! }
//! ```
//! TODO which yields the following trace...

/// The tracing macro. Takes no arguments and should be placed on a
/// function. Refer to crate example usage.
#[cfg(not(feature = "std"))]
pub use rtic_trace_macros::trace;

// TODO is there an even better way to store this?
#[cfg(not(feature = "std"))]
static mut WATCH_VARIABLE: u32 = 0;

/// Auxilliary functions for peripheral configuration. Should be called
/// in the init-function, and preferably in order of (1)
/// [setup::core_peripherals]; (2) [setup::device_peripherals]; and
/// last, (3) [setup::assign_dwt_unit]. Refer to crate example usage.
#[cfg(not(feature = "std"))]
pub mod setup {
    use cortex_m::peripheral as Core;
    use cortex_m::peripheral::{
        dwt::{AccessType, ComparatorAddressSettings, ComparatorFunction, EmitOption},
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
        // itm.configure(ITMSettings {
        //     enable: true, // ITMENA
        //     enable_local_timestamps: true, // TSENA XXX also take prescalar?
        //     forward_dwt: true, // TXENA
        //     global_timestamps: GlobalTimestamps::Every8192Cycles, // GTSFREQ
        //     bus_id: 1, // TraceBusID
        //     // XXX What about SYNCENA, SWOENA?
        // });

        // TODO PR new functions to cortex-m
        unsafe {
            itm.lar.write(0xc5acce55); // unlock ITM register
            itm.tcr.modify(|mut r| {
                r |= 1 << 0; // ITMENA: master enable
                r |= 1 << 1; // TSENA: enable local timestamps
                r |= 1 << 3; // TXENA: forward DWT event packets to ITM
                r |= 0b00 << 10; // GTSFREQ: disable global timestamps
                r |= 1 << 16; // TraceBusID=1
                r
            });
        }
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
}

/// The function utilized by [trace] to write the unique software task
/// ID to the watch address. You are discouraged to use this function
/// directly; [trace] uses a sequence of task IDs compatible with the
/// `parsing` module. If used directly, task IDs must also be properly
/// configured for the host application.
#[cfg(not(feature = "std"))]
#[inline]
pub fn __write_trace_payload(id: u32) {
    // TODO only write as much as needed. e.g. for id < 256, only 8 bits
    // must be written.
    unsafe {
        WATCH_VARIABLE = id;
    }
}
