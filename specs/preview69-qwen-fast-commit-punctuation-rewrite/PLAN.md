# Preview 69 Qwen Fast Commit Punctuation Rewrite PLAN

Updated: 2026-05-10
Status: implementing

## Phase 1: config surface

Files:

- `crates/ainput-shell/src/lib.rs`
- `config/ainput.toml`

Steps:

1. Add `StreamingQwen3Config`.
2. Render `[voice.streaming.qwen3]`.
3. Set preview69 defaults:
   - context text
   - `chunk_size_sec = 0.18`
   - `unfixed_chunk_num = 4`
   - `unfixed_token_num = 5`
   - `max_new_tokens = 64`
   - `enforce_eager = false`
   - `sidecar_idle_unload_ms = 3600000`
4. Enable streaming AI rewrite in the live config, using the existing Windows env key.

## Phase 2: sidecar

Files:

- `tmp/qwen3_asr_sidecar.py`

Steps:

1. Add `StartRequest`.
2. Pass `context`, `language`, `unfixed_chunk_num`, `unfixed_token_num`, and `chunk_size_sec` into `init_streaming_state`.
3. Change defaults to 4 / 5 / 0.18 / 64 / `enforce_eager=false`.
4. If `enforce_eager=false` fails during vLLM engine startup on this machine, retry once with `enforce_eager=true` and log the fallback.
5. Add idle watchdog that exits after configured idle time when no sessions are active.
6. Expose `idle_unload_ms`, `idle_for_ms`, requested enforce-eager, and effective enforce-eager in `/health`.

## Phase 3: Qwen worker path

Files:

- `apps/ainput-desktop/src/worker.rs`

Steps:

1. Send `QwenStartRequest` when creating sessions.
2. Launch WSL sidecar with config-driven env vars.
3. Launch WSL sidecar with `powershell.exe Start-Process wsl.exe -ArgumentList ...`; the detached WSL command must still be `wsl.exe --exec env ... python -m uvicorn`, and the sidecar writes its own log file.
4. Increase sidecar ready wait from 90 seconds to 240 seconds so `enforce_eager=false` cold compile is not misclassified as a launch failure.
5. Add Qwen HUD-layer AI rewrite state.
6. During active hold, allow AI rewrite to update HUD text.
7. On release, cancel pending AI rewrite and commit only the current HUD snapshot.
8. If HUD text is usable and not freshly unstable, use fast commit and run sidecar cleanup after paste.
9. If fast commit is not available, use the slow fallback without spawning new AI rewrite work.
10. Replace Qwen final forced terminal punctuation with Qwen-only cleanup.
11. Leave the old terminal punctuation call commented near the Qwen cleanup for future restore.

## Phase 4: AI rewrite prompt

Files:

- `apps/ainput-desktop/src/ai_rewrite.rs`

Steps:

1. Change the prompt from conservative correction to formal normalized rewrite.
2. Keep hard limits:
   - only rewrite current tail
   - never repeat frozen prefix
   - preserve real mixed Chinese/English terms
   - do not invent facts
3. Add/update tests proving the prompt asks for formal normalization.

## Phase 5: docs/version/package

Files:

- `Cargo.toml`
- `README.md`
- `TASKLIST.md`
- `OPLOG.md`
- `specs/preview69-qwen-fast-commit-punctuation-rewrite/SPEC.md`
- `specs/preview69-qwen-fast-commit-punctuation-rewrite/PLAN.md`

Steps:

1. Bump version to `1.0.0-preview.69`.
2. Update handoff docs after verification.
3. Package preview69.
4. Switch launcher to preview69 only after build succeeds.

## Verification

Run on Windows:

1. `cargo fmt --all`
2. `cargo test -p ainput-shell`
3. `cargo test -p ainput-desktop`
4. Package release.
5. Start preview69.
6. Verify logs/config:
   - Qwen params are 4 / 5 / 64 / `enforce_eager=false`
   - if this GPU/WSL stack rejects `enforce_eager=false`, health shows fallback to effective `enforce_eager=true`
   - idle unload is 3600000
   - fast release commit log appears
   - no Qwen forced terminal punctuation path is used
7. Manual smoke:
   - say `这个怎么回事啊`
   - hold until HUD is complete
   - release
   - pasted text should match HUD and should not receive a forced `。` or `？`
