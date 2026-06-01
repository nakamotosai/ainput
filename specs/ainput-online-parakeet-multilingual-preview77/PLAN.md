# Plan: AInput preview77 Online Parakeet Multilingual RNNT

## Steps

1. Change vps-jp live adapter defaults to `nvidia/parakeet-1_1b-rnnt-multilingual-asr`, function id `71203149-d3b7-4460-8231-1be2543a1fca`, and language `multi`.
2. Restart only `ainput-parakeet-asr.service`; do not touch `cliproxyapi` 8317.
3. Verify vps-jp and Windows `/health` expose the new model configuration.
4. Update Windows repo source defaults:
   - `Cargo.toml` / `Cargo.lock` to `1.0.0-preview.77`
   - `config\ainput.toml` online language to `multi`
   - `crates\ainput-shell\src\lib.rs` online default language to `multi`
   - `sidecars\nvidia_parakeet_online_sidecar.py` model/function/language defaults
   - `scripts\package-release.ps1` preview text
5. Update README, OPLOG, DECISIONS, and TASKLIST with preview77 handoff facts.
6. Run static and unit verification.
7. Package `scripts\package-release.ps1 -Version 1.0.0-preview.77`.
8. Verify package config and package sidecar defaults.
9. Update `run-ainput.bat` and HKCU Run to preview77.
10. Launch preview77 in the Windows interactive desktop session and verify process path/logs.
11. Run closeout guards, commit, push, and postflight writeback.

## Current Verification Evidence

- vps-jp sidecar `python3 -m py_compile` passed.
- Windows package sidecar `python -m py_compile` passed.
- `cargo fmt --all -- --check` passed.
- `cargo check -p ainput-desktop` passed with existing warnings only.
- `cargo test -p ainput-shell` passed, 6/6.
- `scripts\package-release.ps1 -Version 1.0.0-preview.77` produced the dist directory and zip.
- Windows process is running preview77 in `SessionId=1`.
- Runtime log confirms `OnlineStreaming`, `NVIDIA Parakeet online ASR`, model `nvidia/parakeet-1_1b-rnnt-multilingual-asr`, and `local model preload skipped`.

## Residual Risk

The NVIDIA multilingual model is expected to cover the user's Japanese / Chinese / English usage, but final quality must be judged with the user's real speech. `preview.76` and local Qwen rollback paths remain available.
