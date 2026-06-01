# preview72 Qwen Context Echo Guard

## Goal

Prevent Qwen3-ASR from ever showing or committing its configured `voice.streaming.qwen3.context` prompt when the sidecar echoes that prompt as ASR text.

## Scope

- Qwen sidecar streaming partial handling before HUD truth update.
- Qwen sidecar release/final commit path before HUD final ack and paste.
- Fast HUD snapshot path that commits current HUD text on hotkey release.
- Version/package rollout as `1.0.0-preview.72`.

## Non-Goals

- Do not change the Qwen context prompt.
- Do not redesign punctuation behavior in this patch.
- Do not re-enable application-layer `voice.streaming.ai_rewrite`.

## Constraints

- The Qwen ASR core path stays intact; the guard is only a safety boundary for prompt echo output.
- The guard must run before `last_display_text`, HUD partial event, voice history, or paste can receive leaked prompt text.
- Normal dictation text must still stream to HUD and commit normally.
- Desktop runtime verification must prove `ainput-desktop.exe` runs in the real Windows user session, not only that the Qwen sidecar is healthy.

## Acceptance

- Unit tests prove prompt fragments are rejected before HUD truth changes.
- Unit tests prove fast commit refuses a poisoned prompt-like `last_display_text`.
- Unit tests prove normal Qwen dictation still updates HUD truth.
- `voice.streaming.ai_rewrite.enabled = false` remains true in source and packaged config.
- New package `dist/ainput-1.0.0-preview.72` exists and is launched.
- Running process is `dist/ainput-1.0.0-preview.72/ainput-desktop.exe` in `SessionId=1`.
