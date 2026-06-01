# preview.78 Plan

1. Verify the staged implementation.
   - Remove any speech boost phrase not derived from vps-jp Codex user prompt frequency.
   - Confirm Online Streaming chunk feed is limited only during active recording ticks.
   - Confirm final release drain still sends remaining audio.

2. Update source and docs.
   - Set workspace version to `1.0.0-preview.78`.
   - Update sidecar defaults to Parakeet CTC zh-CN.
   - Update README, OPLOG, DECISIONS, TASKLIST, and package README notes.

3. Sync runtime surfaces.
   - Copy changed source files back to `C:\Users\sai\ainput`.
   - Copy sidecar to vps-jp live adapter path.
   - Restart only `ainput-parakeet-asr.service`.
   - Do not touch `cliproxyapi` 8317.

4. Validate before packaging.
   - Run Rust fmt/check/tests on Windows.
   - Run Python compile checks for packaged and live sidecar.
   - Probe `/health` from Windows.

5. Package and launch preview.78.
   - Run `scripts\package-release.ps1 -Version 1.0.0-preview.78`.
   - Update `run-ainput.bat` and HKCU Run to preview.78.
   - Stop old AInput process and start preview.78 in the interactive Windows session.

6. Final verification and closeout.
   - Verify live process path.
   - Verify startup logs show Online Streaming and no local Qwen preload.
   - Verify README closeout guard.
   - Commit and push the release.

7. Freeze as baseline.
   - Record user acceptance that preview.78 is fast and usable.
   - Update README / OPLOG / DECISIONS / TASKLIST with freeze status.
   - Tag the Git baseline.
   - Keep runtime behavior unchanged during freeze closeout.
