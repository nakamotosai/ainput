# Plan

## T001 - Document and gate the failure

- Create this spec pack.
- Add the observed live failure to the project task list.
- Treat HUD-correct/final-polluted divergence as P0 for streaming.

## T002 - Add offline raw guard

- In `select_streaming_final_raw_text`, reject offline-final text that is mostly a duplicate of the HUD tail after removing ASCII-letter noise and punctuation.
- Keep true bilingual additions by requiring the cleaned CJK content to already be covered by the HUD tail and requiring the ASCII letters to be embedded in a short garbled tail, not a normal word/token addition.

## T003 - Add commit-selection guard

- In `select_streaming_commit_text`, detect candidates shaped like `display + garbled_duplicate_tail`.
- Fall back to the display text before final punctuation normalization.

## T004 - Regression tests

- Add tests for:
  - raw offline `We住慎重ong。` rejection after correct HUD display.
  - already-polluted candidate rejection.
  - true bilingual append preservation.

## T005 - Windows verification and packaging

- Sync the patched source to `C:\Users\sai\ainput`.
- Run formatting/check/tests on Windows.
- Package a new preview.
- Run full streaming audit.
- Start the new preview in the interactive desktop.

