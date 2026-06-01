# Plan

## T001 - Lock Current Facts and Secret Boundary

- Confirm Windows repo state, current preview, and active streaming constraints.
- Confirm vps-jp 8317 has NVIDIA-backed `qwen/qwen3.5-122b-a10b`.
- Confirm Windows user env `AINPUT_CLIPROXYAPI_8317_KEY` is present before live packaging.
- Never print or commit the key.

Verification:

- Non-secret config probe shows target endpoint/model.
- Windows env check reports set/missing without printing value.

## T002 - Config Defaults

- Enable streaming AI rewrite only under `[voice.streaming.ai_rewrite]`.
- Set endpoint to `http://vps-jp.tail4b5213.ts.net:8317/v1/chat/completions`.
- Set model to `qwen/qwen3.5-122b-a10b`.
- Set `api_key_env = "AINPUT_CLIPROXYAPI_8317_KEY"`.
- Keep timeout/debounce conservative: `1200ms / 220ms`.
- Do not touch non-streaming config, hotkey config, clipboard output config, ASR model, or punctuation model.

Verification:

- Rendered default config and packaged TOML contain the endpoint/model/env name.
- No key value appears in repo or package config.

## T003 - AI Client Contract

- Use OpenAI-compatible chat completions for 8317.
- Build prompt around "rewrite current tail only".
- Require JSON response: `{"tail":"..."}`.
- Parse `tail`, then fallback `text`, then plain text.
- Strip code fences, quotes, echoed labels, and echoed frozen prefix.
- Reject empty, too-short, too-long, or structurally unsafe output.

Verification:

- Unit tests cover JSON parsing, prefix stripping, bilingual preservation, and invalid output rejection.
- Remote probe command rewrites one short bad ASR phrase without leaking key.

## T004 - Runtime Sidecar Scheduling

- Treat AI rewrite as a sidecar candidate generator.
- Keep local ASR/HUD path independent and always moving.
- Track AI request input by frozen prefix + current tail.
- Allow at most one inflight request per session.
- Reuse cache for identical request.
- Debounce new requests.
- Back off briefly after failures.

Verification:

- Unit tests cover duplicate request suppression, cache reuse, and fallback after no AI response.
- Streaming tests still pass with AI enabled and with AI unavailable.

## T005 - HUD Truth Merge

- Generate candidates from the current HUD preview.
- Split frozen prefix from current live tail.
- Never rewrite frozen prefix.
- If an older AI result returns, compare it to the current HUD tail.
- Accept stale result only when current HUD still matches or extends the requested tail.
- Preserve any suffix that arrived after request dispatch.

Verification:

- Unit tests cover compatible stale accept + suffix preserve.
- Unit tests cover incompatible stale drop.
- Final commit selection still uses HUD truth-source text.

## T006 - Release Cancellation

- On `Ctrl` release, immediately close AI rewrite for the session.
- Increment AI epoch.
- Drop pending receiver state.
- Remove any release-time wait/drain for AI.
- Keep normal streaming release tail drain and final HUD flush.
- Commit only current HUD truth text after final HUD flush.

Verification:

- Unit test proves closed/epoch-moved result is dropped.
- Logs show release cancellation before commit.
- No duplicate or post-release AI commit is possible.

## T007 - Regression and Audit

- Run Rust format/check/test gates on Windows.
- Run streaming/hotkey/rewrite unit test subsets.
- Run full streaming audit with P0=0/P1=0/P2=0.
- Run a direct `test-ai-rewrite` or equivalent probe against 8317.

Verification commands:

- `cargo fmt --check`
- `cargo check -p ainput-desktop`
- `cargo test -p ainput-desktop ai_rewrite -- --nocapture`
- `cargo test -p ainput-desktop streaming -- --nocapture`
- `cargo test -p ainput-desktop hotkey -- --nocapture`
- `cargo test -p ainput-rewrite -- --nocapture`
- `cargo test -p ainput-shell streaming_ai_rewrite -- --nocapture`
- `.\scripts\run-streaming-full-audit.ps1 -Version 1.0.0-preview.50 -LatencyRepeats 1 -LiveCaseLimit 3`

## T008 - Package, Start, and Closeout

- Package a new `preview.50`; do not overwrite `preview.49`.
- Start `preview.50` on the Windows interactive desktop, not as an SSH background process.
- Record package path and PID.
- Update README, TASKLIST, and RESULTS.
- Run README guard and closeout learning triggers.
- Commit/push if the project git remote is available and tree is clean.

Verification:

- `dist\ainput-1.0.0-preview.50\ainput-desktop.exe` exists.
- `dist\ainput-1.0.0-preview.50.zip` exists.
- Running process path is the preview.50 exe.
- README states current preview and handoff status.
