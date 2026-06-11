# Contributing to Hatch

Thanks for your interest in improving Hatch. This is an independent, third-party
project and is not affiliated with Google, Flutter, or Dart.

## Getting set up

```bash
git clone https://github.com/mario-chamuty/hatch.git
cd hatch
cargo build --locked
```

You'll need a recent stable Rust toolchain. Flutter/FVM and the iOS toolchain are
only required if you work on those subsystems.

## Before opening a pull request

```bash
cargo build --locked
cargo test --locked      # iOS end-to-end tests are #[ignore] and skipped by default
cargo fmt --all
cargo clippy --all-targets
```

- Keep changes focused; one logical change per PR.
- Add or update tests for behavior changes. Tests must pass.
- Match the style of the surrounding code.
- Update the relevant doc under `docs/` when you change user-facing behavior.

## Reporting issues

Please include your OS, `hatch --version`, the exact command, and the full output
(run with `-vvv` for detail). For dependency-resolution problems, attach the
`hatch.json` (or a minimal reproduction).

## License

By contributing, you agree that your contributions will be dual licensed under the
MIT and Apache-2.0 licenses, as described in the [README](README.md#-license).
