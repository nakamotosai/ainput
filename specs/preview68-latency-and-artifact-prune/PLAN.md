# Preview 68 Latency And Artifact Prune PLAN

Updated: 2026-05-10
Status: implementing

## Phase 1: tighten the streaming hot path

Files:

- `apps/ainput-desktop/src/worker.rs`
- `config/ainput.toml`
- `crates/ainput-shell/src/lib.rs`

Steps:

1. Lower Qwen local chunk cadence from the current `240ms` to a tighter value.
2. Shorten release drain min / settle / max waits.
3. Shorten the paste stabilize delay.
4. Change preload warm-up so it sends a real silence chunk before closing the warm session.
5. Lower the WSL sidecar auto-start chunk environment so sidecar cadence matches the tighter local cadence better.

## Phase 2: add repeatable artifact cleanup

Files:

- `scripts/prune-artifacts.ps1`

Steps:

1. Detect the current launcher target from `run-ainput.bat`.
2. Preserve explicitly requested versions plus the current launcher target.
3. Delete stale `dist` directories and `dist` zip archives outside the keep set.
4. Delete rebuildable `target*` directories.
5. Delete known root installer residue files.
6. Print a reclaimed-space summary.

## Phase 3: package and switch live version

Files:

- `Cargo.toml`
- `run-ainput.bat`
- `README.md`
- `TASKLIST.md`
- `OPLOG.md`

Steps:

1. Bump package version to `1.0.0-preview.68`.
2. Package the new preview.
3. Point the launcher to `preview.68`.
4. Keep `preview.67` as rollback during this round.

## Phase 4: live prune and verify

Steps:

1. Run the cleanup script on the live Windows workspace.
2. Preserve `preview.68` and `preview.67`.
3. Re-measure top-level space use.
4. Confirm startup log from `preview.68` reflects the new cadence / preload behavior.

## Verification

- `cargo fmt --all`
- `cargo check -p ainput-desktop`
- Windows package build for `preview.68`
- live log readback from `dist\ainput-1.0.0-preview.68\logs\ainput.log`
- live size report before/after cleanup
