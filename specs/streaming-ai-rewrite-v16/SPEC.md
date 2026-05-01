# Streaming AI Rewrite v16

## Goal

Make streaming voice input support live semantic AI rewriting while `Ctrl` is held.

The user-facing rule is strict: HUD is the truth source. While `Ctrl` is still down, the latest unfrozen HUD tail may be corrected by AI. When `Ctrl` is released, AI rewrite is cancelled immediately and the current HUD text is committed; no hidden release-time rewrite may run.

## Current Facts

- Current stable package line: `1.0.0-preview.49`.
- Current streaming rule: hold `Ctrl` to record, release to commit.
- Non-streaming rule: `Alt+Z`; this pack must not touch it.
- Current final delivery chain: clipboard + `Ctrl+V`; this pack must not replace it.
- Current streaming core already has the desired foundation: shared HUD/final revision, release tail drain, post-HUD-flush mutation rejection, raw capture retention, punctuation model, and `committed / stable / volatile / rewrite candidate` state.
- Current repo has a v16 worktree branch in progress: `apps/ainput-desktop/src/ai_rewrite.rs`, `apps/ainput-desktop/src/worker.rs`, `config/ainput.toml`, `crates/ainput-shell/src/lib.rs`, and this spec pack.
- Windows user env `AINPUT_CLIPROXYAPI_8317_KEY` is currently missing and must be set before live AI rewrite can work in the packaged desktop process.
- vps-jp production config exposes `127.0.0.1:8317` through Tailnet and includes NVIDIA provider model `qwen/qwen3.5-122b-a10b`.

## Target Model

- Endpoint: `http://vps-jp.tail4b5213.ts.net:8317/v1/chat/completions`
- Provider family: NVIDIA through `cliproxyapi` 8317.
- Model ID: `qwen/qwen3.5-122b-a10b`
- API key source: Windows user environment variable `AINPUT_CLIPROXYAPI_8317_KEY`.
- Forbidden: writing the API key to TOML, README, logs, test output, package artifacts, or git.

## Scope

- Streaming voice input only.
- AI rewrite client configuration and request/response contract.
- Runtime scheduling, debounce, stale-response handling, cancellation, and fallback.
- HUD update path for AI rewritten tail text.
- Release-time cancellation and final commit semantics.
- Tests, replay/audit, packaging, Windows interactive startup, and README handoff.

## Non-goals

- Do not change `Alt+Z` non-streaming voice input.
- Do not change the streaming hotkey rule: hold `Ctrl`, release to commit.
- Do not replace clipboard + `Ctrl+V` as the main delivery path.
- Do not switch ASR model, punctuation model, GPU setting, or core audio chunking in this pack.
- Do not make AI rewrite a mandatory dependency for streaming ASR.
- Do not let AI rewrite modify already frozen prefix.
- Do not run an AI rewrite after key release to "improve" the final text.

## Architecture

1. Local streaming ASR remains the primary realtime source.
2. The streaming state machine builds a display preview from the current revision.
3. AI rewrite is a sidecar candidate generator, not a second finalization path.
4. Each request contains:
   - frozen prefix for context only.
   - current live tail as the only editable target.
   - output context snapshot: process, title, cursor surroundings when available.
   - session epoch and streaming revision for validation.
5. The model must return JSON: `{"tail":"..."}`.
6. The client parses `tail` first, accepts `text` only as fallback, and keeps a plain-text fallback for provider glitches.
7. The sanitizer strips echoed frozen prefix, rejects empty/too-short/too-long output, and preserves true Chinese-English mixed text.
8. Accepted rewrite is merged back into the current HUD tail; any suffix that arrived after the request is preserved when compatible.
9. Final commit reads only the HUD truth text after final HUD flush.

## Scheduling Rules

- `enabled=true` means AI sidecar may run during active `Ctrl` hold.
- At most one inflight AI request is allowed per streaming session.
- Debounce default: `220ms`.
- Timeout default: `1200ms`.
- Minimum visible tail before request: `6` chars, capped internally to a safe floor.
- Context window default: `160` chars.
- Output cap default: `128` visible chars.
- Identical request input reuses the last accepted/missed result.
- If a request is inflight, newer ASR text keeps updating HUD locally; AI waits for the inflight request to return or time out.
- AI failures enter short backoff and must not block HUD, ASR, punctuation, release, or commit.

## Stale Response Rules

- Same epoch and same revision: may apply after sanitizer.
- Same epoch but older revision: may apply only if the requested tail is still compatible with the current HUD tail.
- Compatible stale output may replace the matched old tail and preserve new suffix.
- Incompatible stale output is dropped silently except for logs.
- Different epoch, closed session, or post-release response is always dropped.

## Release Rules

- Release starts normal streaming tail drain/final HUD flush, but AI rewrite is closed immediately.
- Closing AI rewrite increments epoch and drops pending receiver state.
- No release-time AI drain or wait is allowed.
- Late AI responses after release must not update HUD, final text, clipboard, or commit logs as accepted output.
- The committed text must equal the HUD truth-source revision chosen for final flush.

## Failure Behavior

- Missing API key: AI client is unavailable; streaming continues without AI.
- Network failure / timeout / 4xx / 5xx / invalid JSON: keep local preview, arm short backoff, no user-visible error during dictation.
- Provider returns long explanation / Markdown / echoed prefix: normalize/sanitize, otherwise reject.
- Provider returns hallucinated unrelated text: reject if stale-incompatible or out-of-range; remaining semantic hallucination risk is reduced by tail-only contract and prompt.

## Acceptance

- Unit tests cover:
  - stale-compatible AI result is accepted and preserves suffix.
  - stale-incompatible AI result is rejected.
  - release closes AI rewrite and drops late results.
  - AI output that echoes frozen prefix is stripped.
  - true bilingual tail such as `这个功能支持 Windows 版本` is accepted.
  - JSON `{"tail":"..."}` is preferred over raw text.
- Remote model probe succeeds against `qwen/qwen3.5-122b-a10b` without printing the API key.
- Windows packaged process can read `AINPUT_CLIPROXYAPI_8317_KEY` from user env.
- Windows package config points AI rewrite to the 8317 Tailnet endpoint and model.
- `cargo fmt --check`, `cargo check -p ainput-desktop`, streaming/hotkey/rewrite tests pass.
- Full streaming audit passes with P0=0/P1=0/P2=0.
- A new `preview.50` package is generated without overwriting `preview.49`.
- `preview.50` is started on the Windows interactive desktop.

## Handoff Rule

Before closeout, update README and this pack's `TASKLIST.md` / `RESULTS.md` with exact verification results, package path, started process PID, remaining risks, and rollback point.
