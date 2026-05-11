# preview82 user voice corpus

## Goal

Ship `1.0.0-preview.82` so AInput automatically keeps a stable corpus of the user's real microphone recordings for future regression tests.

## Scope

- Save real recordings after normal voice capture finishes.
- Keep the existing short-term debug raw captures under `logs\streaming-raw-captures\` capped at 20 wav/json pairs.
- Add a project-level corpus at `user-voice-corpus\streaming-raw-captures\`.
- Stop adding new corpus files once the corpus already has 1000 wav files.
- Update raw-corpus replay tooling to prefer the project-level corpus and support random sampling.
- Bump package, docs, launcher, and live Windows entry to `1.0.0-preview.82`.

## Non-Goals

- Do not change ASR model, language, provider, sidecar deployment, function id, speech context, or code-switch correction behavior.
- Do not delete or prune existing user recordings.
- Do not move historical dist-level raw captures into the new corpus in this release.

## Constraints

- Current online ASR remains `nvidia/parakeet-ctc-0_6b-zh-cn`, function id `9add5ef7-322e-47e0-ad7a-5653fb8d259b`, `language=zh-CN`.
- `1.0.0-preview.78` remains the Chinese-quality rollback baseline.
- The stable corpus must live outside versioned `dist\ainput-*` folders so new packages can keep using the same recordings.

## Acceptance

- Normal streaming voice capture writes the existing short-term raw capture and also writes a corpus wav/json pair under `user-voice-corpus\streaming-raw-captures\`.
- Fast voice capture and `record-once` also write to the stable user corpus.
- When the stable corpus contains 1000 wav files, the app skips saving new stable corpus files and does not delete old files.
- `scripts\run-streaming-raw-corpus.ps1` auto-selects the stable corpus when it has usable wavs and random-samples it by default.
- `Cargo.toml`, `run-ainput.bat`, README, OPLOG, DECISIONS, packaged dist, HKCU Run, and live process all point to `1.0.0-preview.82`.
