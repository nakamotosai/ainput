# AInput preview76 Online Streaming Mode

## Goal
Ship a new preview where online NVIDIA Parakeet ASR is a third independent voice mode, not a backend hidden under local streaming mode.

## Scope
- Add `online_streaming` as a distinct `VoiceMode` and tray menu option.
- Add `[voice.online_streaming]` config with Parakeet URL, chunk size, release grace, and background finish flags.
- Preserve local `streaming` mode for Qwen/Sherpa configuration and local GPU behavior.
- Make online release path prioritize current HUD snapshot and paste immediately; finish/cleanup must not block visible paste when HUD text exists.
- Package as `1.0.0-preview.76` without overwriting old dist folders.

## Non-Goals
- Do not change vps-jp cliproxyapi 8317 key routing.
- Do not remove local Qwen/SenseVoice modes.
- Do not tune NVIDIA service internals beyond current adapter parameters.

## Acceptance
- Tray menu exposes Fast, Local Streaming, and Online Streaming separately.
- Default config starts in `online_streaming`.
- Local streaming config remains `qwen3_sidecar` with local sidecar settings.
- Online mode does not load local ASR/GPU models.
- If online HUD has text on hotkey release, paste is done from that HUD text before remote final/finish waits.
- Build/check/package succeeds and preview76 launches in the Windows interactive session.
