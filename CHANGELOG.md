# Changelog
All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]
### Added
- `cargo rtic-scope replay --list`: print out a non-exhaustive header describing the index and trace file name, but not the comment (#140).
### Changed
- `cortex-m-rtic-trace::trace`: write watch variables using `ptr::volatile_write` instead, signaling that the write should not be optimized out.
- `rtic-scope-frontend-dummy`: correctly report absolute timestamps as nanoseconds, not microseconds.
- `cargo rtic-scope replay --list`: only print the trace comment if it exists (previously printed "None").
### Fixed
- `/contrib`: update lock file; `cargo-rtic-scope` now builds again inside a `nix develop` shell.
- Builds of PACs with default features like `device` or `rt` during recovery stage, which previously resulted in fatal linker errors.
### Deprecated
### Security

## [0.3.2] 2022-03-17
Maintenance release.

### Changed
- Bump `itm` which improves the denotation of quality in the downstream `api::Timestamp`: a previous `api::Timestamp { offset: 1, data_relation: TimestampDataRelation::Sync }` is now represented as `api::Timestamp::Sync(1)`.
- `rtic-scope-frontend-dummy`: print qualities (`good` or `bad`, abstracted above the `api::Timestamp` enum) of timestamped events.

## [0.3.1] 2022-01-19
Mostly a maintenance release with some quality-of-life changes.
The largest change is that on `trace --serial`, the given device is not unconditionally configured for 115200bps.

### Added
- `/docs/`, a submodule that contains the overarching documentation of RTIC Scope, which is rendered at [the organization profile](https://github.com/rtic-scope).
- `/rtic-scope-frontend-dummy/`, a submodule of the frontend reference implementation.
- Crate documentation for `cargo-rtic-scope`, which is the same `README.md` used for the organization documentation.

### Changed
- On `--serial /path/to/dev`, `dev` will no longer unconditionally configure for 115200 B/s; the baud rate specified with `tpiu_baud` in the `[package.metadata.rtic-scope]` block in `Cargo.toml` will instead be applied.
  For example, `tpiu_baud = 9600` will configure `dev` for 9600 B/s.
  Valid baud rates are listed in [`nix::sys::termios::BaudRate`](https://docs.rs/nix/0.23.1/nix/sys/termios/enum.BaudRate.html), with the exception of `B0`.
- Improved the warning message when an overflow packet is decoded.
  It will now detail that non-timestamp packets have been dropped and/or that the local timestamp counter wrapped which means that timestamps from then on are *potentially* diverged.
- Ignore enters and exits relating to the `ThreadMode` interrupt: RTIC always executes tasks in handler mode and then returns to `ThreadMode` on `cortex_m::asm::wfi()`.
- Bumped `itm` to v0.7.0 with its `"serial"` feature; the latter used to configure a TTY source.
- Emit a warning if a DWT watch address used for software task tracing is read. Such an address should only ever be written to. This error would indicate that something has gone very wrong.
- Crate documentation for `rtic-scope-frontend-dummy`, `cortex-m-rtic-trace`, and `rtic-scope-api` which is now the same as `README.md` used for the organization documentation but with a small header summarizing the crate.
- Bumped `cortex-m`, ensuring additional target support verification during `cortex_m_rtic_trace::configure`.
- Messages from frontends are now prefixed by a cyan "Frontend" instead of a red "Error".

### Fixed
- No longer prints "Target reset and flashed." or "preparing target" on `trace --dont-touch-target`.

## [0.3.0] - 2022-01-05
Initial release tracked by this changelog.

[Unreleased]: https://github.com/rtic-scope/cargo-rtic-scope/compare/v0.3.1...HEAD
[v0.3.1]: https://github.com/rtic-scope/cargo-rtic-scope/compare/v0.3.0...v0.3.1
[0.3.0]: https://github.com/rtic-scope/cargo-rtic-scope/releases/tag/v0.3.0
