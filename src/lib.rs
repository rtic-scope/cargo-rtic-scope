#![cfg_attr(feature = "tracing", no_std)]
//! Crate documentation

#[cfg(not(any(feature = "tracing", feature = "parsing")))]
compile_error!("This crate is useless without feature \"tracing\" or \"parsing\"");
#[cfg(all(feature = "tracing", feature = "parsing"))]
compile_error!("Crate features \"tracing\" and \"parsing\" are mutually exclusive");

#[cfg(all(feature = "parsing", not(feature = "tracing")))]
pub mod parsing;

#[cfg(all(feature = "tracing", not(feature = "parsing")))]
pub mod tracing;
