# Post-Commit AI Rewrite Preview84 Spec

## Goal

Add an experimental second-stage rewrite path for AInput preview84: keep ASR recognition unchanged, then asynchronously rewrite every committed voice text with the existing OpenAI-compatible route and replace the already-inserted text in place when the target can be proven safe.

## Scope

- Keep the current online streaming ASR model, language, sidecar URL, and recognition parameters unchanged.
- Add `[voice.post_commit_rewrite]` config with the existing route:
  - endpoint: `http://vps-jp.tail4b5213.ts.net:8317/v1/chat/completions`
  - model: `qwen/qwen3.5-122b-a10b`
- Rewrite every committed phrase in the first experimental version.
- Allow aggressive cleanup: semantic polishing, punctuation, normalization, and Chinese/English mixed-term correction.
- Replace only through native Win32 edit/RichEdit range replacement. Do not use Backspace, Delete, clipboard, or a second paste.
- Abort without mutation if the committed text cannot be uniquely and safely identified.

## Non-Goals

- Do not improve or tune ASR in this version.
- Do not switch ASR model or deployment.
- Do not guarantee replacement in every Windows app. Unsupported targets must fail closed.

## Acceptance

- `cargo fmt --check` passes.
- Targeted Rust tests for config, AI rewrite prompt parsing, worker helpers, and output helper logic pass.
- `scripts\package-release.ps1 -Version 1.0.0-preview.84` creates `dist\ainput-1.0.0-preview.84\`.
- The live Windows launch path and startup entry point to preview84.
- Real process readback shows `dist\ainput-1.0.0-preview.84\ainput-desktop.exe` running.

