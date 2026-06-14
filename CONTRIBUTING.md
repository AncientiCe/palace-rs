# Contributing

Thanks for helping improve `palace-rs`.

Before opening a pull request:

- Add or update behavioral tests for the change.
- Run `cargo fmt`.
- Run `cargo clippy --all-targets --all-features -- -D warnings`.
- Run `cargo test --all-features`.
- Run `cargo audit`.

Keep public API changes small and document behavior changes in `CHANGELOG.md`.
