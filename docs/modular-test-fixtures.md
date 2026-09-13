# Modular/legacy unit-test separation

The complete no-default-features library suite initially failed 43 tests:
multilingual scenarios implicitly used production defaults (which correctly have
only English in the modular variant), and three installer/store expectations
assumed legacy bootstrap data. The installer/store tests now verify refusal of
implicit RU/ET migration and byte-preservation under the modular feature set.

`src/test_support.rs` is guarded by `cfg(test)` and supplies explicit multilingual
registries/detectors. It reads the existing RU gzip and ET ISO-8859-15 word sources
only into unit-test executables, attaches explicitly parsed scoring/profile data,
and exercises the ordinary bounded runtime constructors. Production defaults,
build-script dictionary gating and the English-only integration checks are not
changed. The decompressor is a development dependency, not a new runtime one.

Multilingual detector/session/registry scenarios now request that fixture. Tests
specifically about embedded descriptors/models still assert feature-dependent
presence, including RU/ET absence in the base. Optional-language behavior is not
made implicitly available to production base code to satisfy tests.

Portable verification commands:

```sh
cargo test --locked --offline --no-default-features --features signing-tools --lib
cargo test --locked --offline --features signing-tools --lib
cargo clippy --locked --offline --no-default-features --features signing-tools --all-targets -- -D warnings
```

Native Windows regression/typing and installer acceptance remain separate gates.
