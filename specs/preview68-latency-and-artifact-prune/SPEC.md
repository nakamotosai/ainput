# Preview 68 Latency And Artifact Prune SPEC

Updated: 2026-05-10
Status: implementing

## Goal

Ship `1.0.0-preview.68` with two concrete improvements:

1. make streaming feel faster at the user surface, especially:
   - first visible partial
   - release-to-commit delay after the user has already finished speaking
2. slim `C:\Users\sai\ainput` so the workspace no longer carries months of stale build/package artifacts

## Scope

- Qwen streaming latency path:
  - worker-side warm-up
  - local stream chunk cadence
  - release drain timing
  - final paste stabilization delay
  - sidecar auto-start chunk environment
- artifact cleanup:
  - old `dist\ainput-*.zip`
  - old `dist\ainput-*` package directories
  - `target*` build directories
  - known installer residue files in repo root
- docs and operator tooling:
  - package version
  - launcher
  - cleanup script
  - README / TASKLIST / OPLOG handoff

## Non-goals

- do not change the V19 single-chain HUD truth contract
- do not reintroduce offline final, HUD/offline merge, or release-time hidden correction
- do not switch away from `Qwen/Qwen3-ASR-0.6B`
- do not delete active models under `models\`
- do not delete current running preview or immediate rollback preview

## Live findings

### 1. "HUD already done but paste still feels slow" is mostly not clipboard time

Recent `preview.67` live logs show:

- `hud_final_flush_elapsed_ms`: about `16-18ms`
- `output_elapsed_ms`: about `72-98ms`
- `release_to_commit_elapsed_ms`: about `545-712ms`

So once final HUD text is acknowledged, the actual output step is already fast. The larger delay is before commit:

- release drain wait: about `139-266ms`
- final decode: about `150-190ms`
- plus the current `35ms` paste stabilize delay

Conclusion: Preview 68 should squeeze release drain and chunk cadence first, not chase clipboard ghosts.

### 2. First sentence lag is a cold-path symptom

`preview.67` already preloads the model, but first-use feel can still be colder than later utterances because:

- hotkey still creates a fresh sidecar session
- the warm preload path only created and finished a session; it did not send a real chunk through the chunk endpoint
- local `chunk_ms = 240` is still conservative

Conclusion: Preview 68 should warm the chunk path itself and lower chunk cadence.

### 3. Disk bloat is dominated by rebuildable artifacts

Live Windows measurements:

- `dist`: `84.03GB`
  - `94` package directories
  - `93` zip archives
  - zip files alone: `35.41GB`
- `target`: `44.10GB`
- `target-r109` through `target-r112b`: about `12.89GB`
- `models`: `3.62GB`
- `tmp`: `1.40GB`

Conclusion: the storage problem is mainly stale `dist` and `target*`, not the active bilingual model set.

## Acceptance

1. Version and runtime
   - package version is `1.0.0-preview.68`
   - `run-ainput.bat` points to `preview.68`
   - startup log shows the new Qwen cadence values

2. Latency tuning
   - config defaults and packaged config use the tighter cadence values
   - preload path sends a real warm-up chunk before first real user utterance
   - release drain config is shorter than `preview.67`
   - paste stabilize delay is shorter than `preview.67`

3. Cleanup tooling
   - repo contains a repeatable Windows cleanup script
   - script preserves the active preview and one rollback preview by default or explicit input

4. Live slimming
   - old `dist` packages and zips are pruned on the live Windows workspace
   - `target*` build directories are pruned on the live Windows workspace
   - before/after size report proves large reclaim

5. Safety
   - keep the active preview package
   - keep one rollback preview
   - keep `models\`
   - keep source files and docs
