# preview.78 Online Parakeet zh-CN Fast Release

## Goal

Ship `1.0.0-preview.78` as the next AInput preview with Online Streaming as the default mode, using NVIDIA Parakeet CTC zh-CN, faster release-to-paste behavior, conservative Chinese number rewriting, and history-derived English speech context boosts.

## Scope

- Keep three independent modes: Fast, Local Streaming, Online Streaming.
- Keep Local Streaming Qwen/Sherpa configuration, GPU settings, and idle unload behavior unchanged.
- Revert Online Streaming default model from failed multilingual RNNT to `nvidia/parakeet-ctc-0_6b-zh-cn`.
- Reduce online `/chunk` blocking so hotkey release is processed quickly.
- Prevent natural Chinese words such as `一起`, `一边`, `一下`, and `一个` from being pasted as `1起`, `1边`, `1下`, and `1个`.
- Add NVIDIA Riva speech context boosts only from high-frequency English terms found in vps-jp Codex user prompt history.

## Non-Goals

- Do not repair Local Streaming release behavior in this round beyond shared rewrite safety.
- Do not make multilingual RNNT the default.
- Do not modify or restart `cliproxyapi` 8317.
- Do not write NVIDIA API keys into repo, dist, config, logs, docs, or responses.
- Do not delete `preview.77`; keep it as failed experiment evidence.

## Constraints

- Live adapter remains on vps-jp under `ainput-parakeet-asr.service`.
- Windows runtime and repo remain under `C:\Users\sai\ainput`.
- Default launched package must become `dist\ainput-1.0.0-preview.78\ainput-desktop.exe`.
- English boost list must be traceable to vps-jp `~/.codex/sessions` user messages.

## Acceptance

- `cargo fmt --all -- --check` passes.
- `cargo check -p ainput-desktop` passes.
- `cargo test -p ainput-rewrite` passes.
- `cargo test -p ainput-shell` passes.
- vps-jp sidecar `python3 -m py_compile` passes.
- vps-jp `/health` reports `nvidia/parakeet-ctc-0_6b-zh-cn`, `zh-CN`, `partial_wait_sec=0.06`, and nonzero `boost_phrases`.
- Windows packaged preview.78 is built and launched.
- `run-ainput.bat` and HKCU Run point to preview.78.
- Startup logs show Online Streaming default and no local Qwen preload.
- Rewrite tests prove `等会儿1起修` becomes `等会儿一起修`, while pure Chinese digit runs such as `一二三四五六` still convert.
- User confirms real Windows hotkey usage is good, recognition is fast, and release-to-paste is fast.

## Frozen Baseline

- Status: frozen on 2026-05-11 as the future modification baseline.
- Baseline version: `1.0.0-preview.78`.
- Baseline runtime: `C:\Users\sai\ainput\dist\ainput-1.0.0-preview.78\ainput-desktop.exe`.
- Baseline rule: future changes that touch model choice, HUD realtime updates, release-to-paste, or rewrite behavior must compare against this version's real user-facing behavior, not only automated probes.
