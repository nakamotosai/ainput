# Streaming Offline Final Garbled Tail v15

## Goal

Fix the streaming release path where HUD shows the correct text, but final commit appends a corrupted offline-final tail after key release.

Observed user failure:

- Spoken / HUD: `还是得稳住慎重`
- Final commit: `还是得稳住慎重We住慎重ong。`

## Scope

- Only streaming voice input.
- Only final text selection after releasing `Ctrl`.
- Add deterministic guards for offline final tail repair and final commit selection.
- Add tests and audit coverage for the observed failure class.

## Non-goals

- Do not change non-streaming `Alt+Z`.
- Do not change the streaming hotkey: hold `Ctrl` to record, release to commit.
- Do not replace the clipboard + `Ctrl+V` delivery path.
- Do not add AI semantic rewrite.
- Do not enable GPU or change the default streaming model.

## Requirements

1. HUD remains the final truth source when the final candidate is lower quality.
2. If HUD already contains CJK text and offline final contains ASCII letter noise around a CJK tail that already appears at the HUD tail, reject the offline tail.
3. If a polluted final candidate is already built as `HUD + garbled tail`, commit selection must still fall back to the HUD text.
4. True bilingual text must be preserved. Examples such as `这个功能支持 Windows 版本。`, `OpenAI API`, and `Rust Windows 版本` must not be rejected merely because they contain ASCII letters.
5. Final punctuation may still be added by the existing finalization path, but no extra hallucinated letters or repeated tail may be committed.

## Acceptance

- Unit tests cover the exact observed pattern:
  - `display = 还是得稳住慎重`
  - `offline = We住慎重ong。`
  - final commit resolves to `还是得稳住慎重。`
- Unit tests cover candidate-stage pollution:
  - `candidate = 还是得稳住慎重We住慎重ong。`
  - final commit resolves to `还是得稳住慎重。`
- Unit tests preserve true bilingual additions:
  - `display = 这个功能支持`
  - `candidate = 这个功能支持 Windows 版本。`
  - final commit keeps the bilingual candidate.
- Existing streaming, hotkey, and rewrite tests pass.
- A new preview package is generated and started on the Windows interactive desktop.

