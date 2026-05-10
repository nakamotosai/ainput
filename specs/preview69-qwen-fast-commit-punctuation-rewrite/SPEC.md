# Preview 69 Qwen Fast Commit Punctuation Rewrite SPEC

Updated: 2026-05-10
Status: implementing

## Goal

Ship `1.0.0-preview.69` for the Qwen streaming path with these user-facing changes:

1. Qwen streaming uses the requested official streaming/vLLM parameters:
   - `unfixed_token_num = 5`
   - `unfixed_chunk_num = 4`
   - `max_new_tokens = 64`
   - `enforce_eager = false`, with automatic runtime fallback to `true` if vLLM engine initialization fails on the current Windows GPU / WSL stack
2. Qwen sidecar releases GPU memory after 1 hour of idle time.
3. Qwen streaming receives a user-specific context that favors mixed Chinese/English recognition plus normalized formal output.
4. When the HUD already contains stable text, hotkey release immediately commits the current HUD snapshot.
5. Qwen streaming must not locally synthesize terminal punctuation on pause or release.
6. AI rewrite means formal normalization: convert oral/rough ASR text into correct, formal, natural language when enabled.
7. Qwen sidecar startup wait must cover `enforce_eager=false` cold compile on this Windows GPU / WSL stack; 90 seconds is too short, so preview69 waits up to 240 seconds before declaring startup failed.
8. Non-streaming / fast SenseVoice mode remains independent and keeps its manual punctuation path.
9. WSL auto-start must launch uvicorn through `powershell.exe Start-Process wsl.exe -ArgumentList ...`, where the WSL command itself is `wsl.exe --exec env ... python -m uvicorn`; do not use `bash -lc "... &"` shell backgrounding, and the sidecar must write `qwen3_asr_sidecar.log` itself for diagnosis.

## Scope

- `apps/ainput-desktop/src/worker.rs`
  - Qwen session request body
  - Qwen sidecar launch environment
  - Qwen release fast HUD commit
  - Qwen-only final cleanup that does not force sentence-ending punctuation
  - Qwen HUD-layer AI rewrite lifecycle
- `apps/ainput-desktop/src/ai_rewrite.rs`
  - strengthen prompt toward formal normalized rewrite
- `crates/ainput-shell/src/lib.rs`
  - add `[voice.streaming.qwen3]` config schema and rendered TOML
- `tmp/qwen3_asr_sidecar.py`
  - accept Qwen session params/context
  - set default 4/5/0.18/64/`enforce_eager=false`
  - idle watchdog exits process after configured idle time
- `config/ainput.toml`
  - preview69 default config
- `Cargo.toml`
  - version bump

## Non-goals

- Do not reintroduce offline final merging for Qwen streaming.
- Do not make release wait for a hidden final correction when HUD already has text.
- Do not delete the old punctuation functions; Qwen should bypass/comment the old forced terminal behavior instead.
- Do not remove manual punctuation from non-streaming / fast mode.
- Do not switch model family.
- Do not change disk cleanup policy in this preview.

## Qwen context

The Qwen ASR context is:

```text
这是 Windows 语音输入法的实时转写场景。用户主要说中文，但经常中英文混杂，包含模型名、版本号、路径、快捷键、代码词、参数名和聊天软件输入内容。请输出用户最可能真正想输入的文字，可以把明显口语化、重复、同音错字、近音错字整理成正式、通顺、无错字的表达，但不能改变原意或添加新信息。不要因为短暂停顿就强行结束句子；不要机械插入逗号、句号或问号；只有语气和语义都明确时才保留合适标点。不要重复已经识别过的内容。中文按中文输出，英文、数字、路径、参数名、快捷键和专有名词尽量原样保留。不要解释。
```

## Architecture contract

```text
Ctrl down
  -> show HUD
  -> ensure Qwen model/session
  -> record audio
  -> feed Qwen streaming chunks
  -> Qwen partial text
  -> light cleanup / optional HUD AI normalization
  -> HUD update
Ctrl up
  -> if HUD snapshot is usable and not freshly unstable: paste HUD immediately
  -> cancel pending AI rewrite for this hold
  -> background finish/cleanup only, no late text mutation
```

## Punctuation contract

- Qwen streaming path:
  - no local forced `。` on release
  - no local forced `？`
  - no local pause-as-sentence
  - only dedupe/cleanup existing punctuation
  - old terminal punctuation line remains commented for future restore
- Non-streaming / fast path:
  - keeps existing manual punctuation and `ensure_terminal_sentence_boundary` behavior where it currently relies on it
  - must compile and pass existing punctuation tests

## AI rewrite contract

- AI rewrite is a HUD text-layer normalization tool.
- It is allowed to rewrite oral ASR text into formal, correct, natural language.
- It must not change user intent or invent facts.
- It must not participate in audio recognition.
- It must not block release commit.
- Releasing the hotkey cancels/drops pending rewrite results.
- Paste text is the current HUD text, not a hidden rewrite result applied only at release.

## Acceptance

1. Version is `1.0.0-preview.69`.
2. Rendered config and live config include `[voice.streaming.qwen3]`.
3. Sidecar launch/defaults show:
   - `QWEN3_UNFIXED_CHUNK_NUM=4`
   - `QWEN3_UNFIXED_TOKEN_NUM=5`
   - `QWEN3_MAX_NEW_TOKENS=64`
   - `QWEN3_ENFORCE_EAGER=0`
   - `QWEN3_ENFORCE_EAGER_FALLBACK=1`
   - idle unload `3600000`
4. Qwen session start POSTs context plus chunk/token params to `/v1/sessions`.
5. Qwen fast release logs `Qwen sidecar fast HUD snapshot delivered` and pastes current HUD text without calling final punctuation insertion.
6. Qwen final fallback does not call `ensure_terminal_sentence_boundary`.
7. AI rewrite prompt explicitly requires formal normalized rewrite.
8. Existing non-Qwen punctuation functions remain present and tested.
9. `cargo fmt --all` passes.
10. `cargo test -p ainput-desktop` and `cargo test -p ainput-shell` pass.
11. Release package builds and preview69 can be launched on Windows.
12. If `enforce_eager=false` fails during vLLM startup, the sidecar retries once with `enforce_eager=true`, `/health` exposes `requested_enforce_eager=false` and `effective_enforce_eager=true`, and the app still becomes usable instead of staying stuck in preload.
13. If `enforce_eager=false` succeeds but needs a long cold compile, the app does not time out at 90 seconds; it waits long enough for `/health` to return.
14. Auto-start creates a WSL uvicorn process visible in `pgrep -af uvicorn`, and `qwen3_asr_sidecar.log` is non-empty while loading.
15. The auto-start implementation matches the GUI runtime reality: Windows starts `powershell.exe`, PowerShell uses `Start-Process` to detach `wsl.exe`, and WSL runs `env ... .venv/bin/python -m uvicorn qwen3_asr_sidecar:app`.
