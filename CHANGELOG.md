# Changelog

All notable changes to this project are documented in this file.

## 2025-05-17

### Added
- 7 new semgrep rules enforcing code quality patterns:
  - `no-boxed-future-factory-alias` — ban opaque type alias indirection
  - `no-boxjob-alias-names` — ban `BoxJob*` naming convention
  - `no-eprintln-in-library` — require proper error propagation over stderr logging
  - `no-inline-duration-literals` — require named constants for durations
  - `no-manual-panic-in-tests` — require idiomatic assertions over manual panic
  - `no-manual-pin-box-future` — ban manual `Pin<Box<dyn Future>>` construction
  - `no-public-fields-outside-config` — enforce encapsulation
- README documents human code review findings and guardrail-driven development approach
- `.gitignore` entry for macOS `Library/` directory

### Changed
- Semgrep CI target now scans `tests/` in addition to `src/` and `scripts/`
- Semgrep runs with telemetry and version checks disabled
- `src/lib.rs`: removed `BoxJobFuture`/`BoxJobFactory` type aliases, removed `eprintln!` calls, simplified job spawning via direct `tokio::spawn`
- `src/main.rs`: extracted duration constants with `const fn` helpers
- `tests/`: replaced inline duration literals with named constants and const helpers

## 2025-05-16

### Added
- MIT licence
- Session-extraction script and data directory scaffolding
- Integration and adversarial regression test suites
- Actor-style async request queue implementation
- Session transcripts and challenge prompts
- AI agent rules (`AGENTS.md`) and challenge README
- Semgrep static analysis and codespell configuration
- `rustfmt`, `cargo-deny`, and pre-commit configurations
- Justfile with CI pipeline and quality gates
- Cargo manifest and lockfile
