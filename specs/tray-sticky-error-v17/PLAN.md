# Plan

## T001

- Split recoverable microphone-start failures from fatal worker errors.
- Add a non-sticky idle-state handler in the desktop event loop.

## T002

- Use the recoverable event for:
  - fast voice start-recording failure
  - streaming voice start-recording failure

## T003

- Verify compile/format on Windows.
- Package a new preview build.
- Start the new preview on the Windows interactive desktop.
