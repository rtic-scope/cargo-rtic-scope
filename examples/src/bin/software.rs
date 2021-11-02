#![warn(unsafe_code)]
#![deny(warnings)]
#![no_main]
#![no_std]

use panic_semihosting as _;
use rtic::app;

#[app(device = stm32f4::stm32f401, dispatchers = [EXTI0, EXTI1])]
mod app {
    use cortex_m_rtic_trace::{setup, trace};

    #[shared]
    struct Shared {}

    #[local]
    struct Local {}

    #[init]
    fn init(mut ctx: init::Context) -> (Shared, Local, init::Monotonics) {
        // Allow debugger to attach while sleeping (WFI)
        ctx.device.DBGMCU.cr.modify(|_, w| {
            w.dbg_sleep().set_bit();
            w.dbg_standby().set_bit();
            w.dbg_stop().set_bit()
        });

        // setup software tracing
        setup::core_peripherals(
            &mut ctx.core.DCB,
            &mut ctx.core.TPIU,
            &mut ctx.core.DWT,
            &mut ctx.core.ITM,
        );
        setup::device_peripherals(&mut ctx.device.DBGMCU);
        setup::assign_dwt_units(&ctx.core.DWT.c[1], &ctx.core.DWT.c[2]);

        sw_task::spawn().unwrap();

        (Shared {}, Local {}, init::Monotonics())
    }

    #[task]
    #[trace]
    fn sw_task(_: sw_task::Context) {}
}
