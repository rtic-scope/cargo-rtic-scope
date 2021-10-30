#![warn(unsafe_code)]
#![deny(warnings)]
#![no_main]
#![no_std]

use panic_semihosting as _;
use rtic::app;

#[app(device = stm32f4::stm32f401, dispatchers = [EXTI0, EXTI1])]
mod app {
    use cortex_m_semihosting::{debug, hprintln};
    use cortex_m_rtic_trace::{trace};

    #[shared]
    struct Shared {}

    #[local]
    struct Local {}

    #[init]
    fn init(_: init::Context) -> (Shared, Local, init::Monotonics) {
        foo::spawn().unwrap();

        (Shared {}, Local {}, init::Monotonics())
    }

    #[task]
    #[trace]
    fn foo(_: foo::Context) {
        hprintln!("foo - start").unwrap();

        // spawns `bar` onto the task scheduler
        // `foo` and `bar` have the same priority so `bar` will not run until
        // after `foo` terminates
        bar::spawn().unwrap();

        hprintln!("foo - middle").unwrap();

        // spawns `baz` onto the task scheduler
        // `baz` has higher priority than `foo` so it immediately preempts `foo`
        baz::spawn().unwrap();

        hprintln!("foo - end").unwrap();
    }

    #[task]
    #[trace]
    fn bar(_: bar::Context) {
        hprintln!("bar").unwrap();

        debug::exit(debug::EXIT_SUCCESS); // Exit QEMU simulator
    }

    #[task(binds = ADC)]
    fn blah(_: blah::Context) {
    }

    #[task(binds = SysTick)]
    fn blah2(_: blah2::Context) {
    }

    #[task(priority = 2)]
    #[trace]
    fn baz(_: baz::Context) {
        hprintln!("baz").unwrap();
    }
}
