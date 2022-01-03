#![no_std]
#![no_main]

use panic_halt as _; // panic handler
use rtic;

#[rtic::app(device = stm32f4::stm32f401, peripherals = true, dispatchers = [EXTI0])]
mod app {
    use cortex_m::asm;
    use cortex_m::peripheral::syst::SystClkSource;
    use cortex_m_rtic_trace::{self, trace};
    use stm32f4xx_hal::stm32;

    #[shared]
    struct Shared {}

    #[local]
    struct Local {}

    #[init]
    fn init(mut ctx: init::Context) -> (Shared, Local, init::Monotonics) {
        let mut syst = ctx.core.SYST;

        // Allow debugger to attach while sleeping (WFI)
        ctx.device.DBGMCU.cr.modify(|_, w| {
            w.dbg_sleep().set_bit();
            w.dbg_standby().set_bit();
            w.dbg_stop().set_bit()
        });

        // configures the system timer to trigger a SysTick exception every second
        syst.set_clock_source(SystClkSource::Core);
        syst.set_reload(16_000_000); // period = 1s
        syst.enable_counter();
        syst.enable_interrupt();

        (Shared {}, Local {}, init::Monotonics())
    }

    #[task(binds = SysTick)]
    fn hardware(_: hardware::Context) {
        software::spawn().unwrap();
    }

    #[task]
    #[trace]
    fn software(_: software::Context) {}
}
