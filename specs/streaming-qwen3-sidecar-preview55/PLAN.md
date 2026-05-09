# Qwen3 ASR Sidecar Preview 55 PLAN

## Phase 1: Prove Sidecar

- [x] Install original `Qwen/Qwen3-ASR-0.6B` in WSL user-space env under `/home/sai/ainput-qwen3-asr`.
- [x] Verify CUDA/Torch/vLLM on RTX 2080 Ti.
- [x] Start resident FastAPI sidecar on `127.0.0.1:8765`.
- [x] Run HTTP sidecar eval on five recent failure wavs.

## Phase 2: Add Config And Backend Switch

- [x] Add streaming backend config fields to `ainput-shell`.
- [x] Add canonical preview.55 config values for Qwen sidecar.
- [x] Keep `sherpa` as explicit config rollback.

## Phase 3: Rust Qwen Sidecar Runtime

- [x] Add a blocking HTTP client for the local sidecar.
- [x] Auto-start WSL sidecar when health is unavailable and `sidecar_auto_start = true`.
- [x] Add a Qwen streaming worker branch that keeps the V19 HUD truth and commit rules.
- [x] Disable AI rewrite initialization for the Qwen streaming route.

## Phase 4: Package And Verify

- [x] Run targeted Rust checks.
- [x] Run sidecar HTTP regression again after code changes.
- [x] Package `1.0.0-preview.55` into a new dist directory and zip.
- [x] Stop old ainput process and launch preview.55 in the Windows interactive session.
- [x] Verify process path/PID and report rollback path.

## Phase 5: Preview 56 Closeout

- [x] Lower Qwen sidecar streaming parameters to `chunk_size_sec=0.5`, `unfixed_chunk_num=1`, `unfixed_token_num=2`.
- [x] Fix preview.55 final truncation by deriving final paste text directly from Qwen `finish.text`.
- [x] Copy the updated sidecar script into the WSL live env.
- [x] Package `1.0.0-preview.56` into a new dist directory and zip.
- [x] Launch preview.56 in the Windows interactive session.
- [x] Verify process version, sidecar health, GPU residency, and runtime `chunk_ms=500` log.
