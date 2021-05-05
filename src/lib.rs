#![cfg_attr(feature = "tracing", no_std)]
//! This crate exposes functionality that eases the procedure of tracing
//! embedded applications written using RTIC. Two mutually exclusive
//! feature-sets (modules, locked behind crate features) are exposed:
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
//!
//! - `parsing`: which offers functions that parses an RTIC application
//!   source `#[app(device = ...)] mod app { ... }` declaration,
//!   recovers type informations, and associates the trace packets
//!   received from the target back to RTIC tasks. For use on the host
//!   system that receives the trace information from the embedded
//!   target.
//!
//! Refer to the generated documentation when this crate is built with
//! the `tracing` or `parsing` feature. Because `tracing` is a
//! `#[no_std]` module, and `parsing` required `std`, this crate does
//! nothing by default.

#[cfg(all(feature = "tracing", feature = "parsing"))]
compile_error!("Crate features \"tracing\" and \"parsing\" are mutually exclusive");

#[cfg(all(feature = "parsing", not(feature = "tracing")))]
pub mod parsing;

#[cfg(all(feature = "tracing", not(feature = "parsing")))]
pub mod tracing;
