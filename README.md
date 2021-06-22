# `rtic-trace` - target-side crate for RTIC Scope tracing functionality
`rtic-trace` is an auxiliary crate for ARM Cortex-M platforms to correctly enable tracing of [RTIC](https://rtic.rs) applications via the ITM/DWT subsystems. A set of intialization functions are exposed that are expected to be called at the end of `#[init]`. These will ensure that tracing peripherals are configured as expected. Additionally, a `#[trace]` macro is exposed that enable tracing of RTIC software tasks.

## Examples
Refer to [rtic-scope/examples](https://github.com/rtic-scope/examples) for example uses of `rtic-scope` in combination with the remainder of the RTIC Scope framework.

## License
The code in this repository is distributed under the terms of both the MIT license and the Apache License (Version 2.0).

See [LICENSE-APACHE](LICENSE-APACHE) and [LICENSE-MIT](LICENSE-MIT) for details.
