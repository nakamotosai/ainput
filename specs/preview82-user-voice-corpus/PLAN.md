# preview82 user voice corpus plan

## Steps

1. Add stable corpus write support in `worker.rs`.
   - Keep `logs\streaming-raw-captures\` at the current 20-file short-term cap.
   - Derive project root from either source checkout logs or versioned `dist\ainput-*` logs.
   - Save stable corpus copies to `user-voice-corpus\streaming-raw-captures\`.
   - Skip stable saves once 1000 wav files already exist.

2. Cover real capture entry points.
   - Streaming hotkey release saves short-term and stable copies.
   - Sidecar fast-commit cleanup saves short-term and stable copies.
   - Fast voice hotkey saves stable copies after silence filtering.
   - `record-once` saves stable copies before transcription.

3. Update replay tooling.
   - Prefer `user-voice-corpus\streaming-raw-captures\` when present.
   - Random-sample stable corpus by default; keep deterministic short/long selection via `-Deterministic`.

4. Bump and document release.
   - Update version to `1.0.0-preview.82`.
   - Update README, OPLOG, DECISIONS, package README note, launcher path, and handoff.

5. Verify and publish locally.
   - Run formatting and targeted raw-capture tests.
   - Run README closeout guard and diff check.
   - Package `1.0.0-preview.82`.
   - Start the live Windows app from `dist\ainput-1.0.0-preview.82`.
   - Verify HKCU Run, process path, corpus directory, and raw-corpus script behavior.
