# preview72 Qwen Context Echo Guard Plan

## Plan

1. Add a Qwen context echo detector based on the live configured context plus distinctive prompt markers.
2. Call the detector inside `apply_qwen_sidecar_partial_update` before mutating `last_display_text` or sending HUD partials.
3. Call the detector in the release final path before final HUD ack and paste.
4. Add a final fast-commit check so poisoned prompt-like HUD state cannot be pasted.
5. Add targeted tests for blocked prompt partials, poisoned fast commit, and normal dictation.
6. Bump the workspace/package version to `1.0.0-preview.72`.
7. Build/package, launch the new dist in the interactive Windows session, and verify process/config/log state.

## Verification

- `cargo fmt`
- Targeted `cargo test -p ainput-desktop qwen_context_echo`
- Targeted `cargo test -p ainput-desktop qwen_fast_release_commits_current_hud_without_terminal_punctuation`
- Package creation through `scripts/package-release.ps1 -Version 1.0.0-preview.72`
- Read back packaged config and active process path/session.
