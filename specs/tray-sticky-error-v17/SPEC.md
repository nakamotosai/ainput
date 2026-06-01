# Tray Sticky Error v17

## Goal

Stop the tray icon from staying in the red X error state forever after a recoverable microphone-start failure.

## Scope

- Voice hotkey error handling in the desktop app.
- Fast and streaming microphone-start failures only.
- Packaging and Windows startup of a new preview build.

## Non-goals

- Do not change `Alt+Z` or `Ctrl` bindings.
- Do not change clipboard paste delivery.
- Do not change ASR models or AI rewrite behavior.
- Do not solve Windows default-microphone routing in this pack.

## Requirements

1. If microphone start fails on hotkey press, the app must return to idle visual state instead of staying in tray `Error`.
2. The tray/menu status should still expose the failure message.
3. Fatal startup/output/HUD commit failures must remain real error-state failures.
4. The fix must apply to both fast voice and streaming voice start failures.

## Acceptance

- `cargo fmt --check` passes.
- `cargo check -p ainput-desktop` passes.
- The new `RecoverableError` event compiles through the desktop event loop.
- A new preview package is built and started on Windows interactive desktop.
