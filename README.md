`cargo-rtic-trace` will be a single-click solution for tracing [RTIC](https://rtic.rs) applications running on embedded ARM Cortex-M targets. If you clone this repository and run `cargo install --path . && cd examples && cargo rtic-trace --bin blinky --dev /dev/ttyUSB3` it will

1. attach to your target;
2. configure `/dev/ttyUSB3` for trace reception;
3. build blinky;
4. generate resolve maps for exceptions and interrupt numbers from
   blinky's source code;
5. flash blinky to target;
6. reset the target;
7. deserialize the trace from `/dev/ttyUSB3` into human-readable types and
   serialize these types to JSON and save to disk under
   `target/rtic-traces`.

When done, `cargo-rtic-trace` will also stream the resolved trace to a
frontend. For example, a graphical web application.
