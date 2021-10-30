#![warn(unsafe_code)]
#![deny(warnings)]
#![no_main]
#![no_std]

use panic_semihosting as _;
// use rtic::app;

#[rtic::app(device = stm32f4::stm32f401, dispatchers = [EXTI0, EXTI1])]
mod app {
    use cortex_m_rtic_trace::{trace};

    #[shared]
    struct Shared {}

    #[local]
    struct Local {}

    #[init]
    fn init(_: init::Context) -> (Shared, Local, init::Monotonics) {
        (Shared {}, Local {}, init::Monotonics())
    }

    #[task]
    #[trace]
    fn foo(_: foo::Context) {
    }

    #[task]
    #[trace]
    fn bar(_: bar::Context) {
    }

    #[task(binds = ADC)]
    fn adc(_: adc::Context) {
    }

    #[task(binds = SysTick)]
    fn systick(_: systick::Context) {
    }

    #[task(priority = 2)]
    #[trace]
    fn baz(_: baz::Context) {
    }
}
