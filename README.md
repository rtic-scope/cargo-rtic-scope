# `cargo-rtic-scope` - single-click solution for tracing [RTIC](https://rtic.rs) applications running on embedded ARM Cortex-M targets.
See https://rtic-scope.github.io for documentation.

# `cortex-m-rtic-trace` - target-side crate for RTIC Scope tracing functionality
`cortex-m-rtic-trace` is an auxiliary crate for ARM Cortex-M platforms to correctly enable tracing of [RTIC](https://rtic.rs) applications via the ITM/DWT subsystems. A set of intialization functions are exposed that are expected to be called at the end of `#[init]`. These will ensure that tracing peripherals are configured as expected. Additionally, a `#[trace]` macro is exposed that enable tracing of RTIC software tasks.

## Examples
Refer to [rtic-scope/examples](https://github.com/rtic-scope/examples) for example uses of `rtic-scope` in combination with the remainder of the RTIC Scope framework.

## License
For non-commercial purposes, the code in this repository is distributed under the terms of both the MIT license and the Apache License (Version 2.0).
See [LICENSE-APACHE](LICENSE-APACHE) and [LICENSE-MIT](LICENSE-MIT) for details.

This project is maintained in cooperation with @GrepitAB and Luleå Technical University.
For commercial support and alternative licensing, inquire via <contact@grepit.se>.
