# Agent guide

Read `ARCHITECTURE.md` before changing code. Load only the module named by the change-routing section, then inspect its direct dependencies.

## Invariants

- Never perform filesystem scans or duplicate analysis on the egui update thread.
- Every background job must have an `AtomicBool` cancellation flag and communicate through typed messages from `model.rs`.
- Keep the Windows MFT/USN path optional: access or filesystem errors must fall back to `jwalk`.
- Search requests must use `FileIndex` after indexing; never rescan the filesystem for a query.
- A duplicate is reported only after both fingerprinting and exact byte comparison.
- Protected system paths must never become cleanup candidates.
- File deletion must use the `trash` crate and require UI confirmation.
- Keep `main.rs` limited to shared visual constants, module declarations, and the process entry point.

## Verification

Run these commands after Rust changes:

```text
cargo fmt -- --check
cargo check
cargo test
```

Do not add a dependency when the standard library or an existing crate already covers the requirement.
