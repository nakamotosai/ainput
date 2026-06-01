# AInput Online Parakeet Realtime Preview 75 PLAN

## Steps

- [x] Confirm `preview.74` HUD blank root cause is adapter `/chunk` returning empty text.
- [x] Change adapter to keep one NVIDIA streaming gRPC session per AInput session.
- [x] Return latest partial text from `/chunk`.
- [x] Deploy and restart the `vps-jp` adapter.
- [x] Verify paced WAV partials before `/finish`.
- [x] Update docs and package `1.0.0-preview.75`.
- [x] Launch `preview.75` in the Windows interactive desktop session.

## Rollback

Run `C:\Users\sai\ainput\dist\ainput-1.0.0-preview.74\run-ainput.bat` for the previous online-final-only adapter package, or `preview.72` for the local Qwen rollback point.
