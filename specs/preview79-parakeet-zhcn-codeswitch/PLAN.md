# preview.79 Plan

1. Freeze the constraint.
   - Keep online ASR on `nvidia/parakeet-ctc-0_6b-zh-cn`.
   - Treat multilingual RNNT as failed evidence, not the new default.

2. Add the app-side repair path.
   - Add exact-match online Parakeet code-switch repairs for the real dropped `multi` captures.
   - Keep local Qwen behavior unchanged.
   - Add common high-confidence product-name repairs for repeated user terms such as Codex.

3. Add a replay gate.
   - Replay preview.78 raw wavs through the live sidecar.
   - Apply the same app repair entrypoint.
   - Fail if the repaired text does not recover the expected English term or if pure Chinese changes unexpectedly.

4. Correct config truth.
   - Change online streaming `language` default/config from misleading `multi` to `zh-CN`.
   - Leave model/deployment unchanged.

5. Validate and package.
   - Run Rust fmt/check/tests.
   - Run the online code-switch replay script.
   - Package `1.0.0-preview.79`.
   - Launch the Windows app from preview.79 and verify live process path.

6. Hand back for human microphone testing.
   - User should test short mixed utterances containing `multi`, model names, CLI terms, and normal Chinese.
   - If new English islands fail, add them to the exact-match fixture set before broadening repair rules.
