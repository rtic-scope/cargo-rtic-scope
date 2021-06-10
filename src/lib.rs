#![no_std]
//! This crate exposes functionality that eases the procedure of tracing
//! embedded applications written using RTIC. A single feature-set is
//! available:
//!
//! - `tracing`: which offers setup functions that configures related
//!   peripherals for RTIC task tracing, and a `#[trace]` macro for
//!   software tasks. Example usage:
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

pub mod tracing;
