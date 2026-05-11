# preview.80 Plan

1. Freeze the safe boundary.
   - Keep zh-CN Parakeet CTC and current deployment.
   - Treat preview.78 Chinese behavior as the no-regression baseline.

2. Repair app-side English islands.
   - Add only observed or high-confidence Codex/multi mishears.
   - Add negative tests for ordinary Chinese containing similar characters.

3. Enable low-risk boost.
   - Replace the invalid broad boost list with a curated Riva-encodable list.
   - Use a low default boost and expose the active list through `/health`.
   - Keep env/file overrides available for future user-maintained terms.

4. Extend replay gates.
   - Keep raw WAV replay for the preview.79 captured failures.
   - Add text-only cases for the new real outputs from the user.
   - Check pure Chinese unchanged and pure English/product-term normalization.

5. Validate and package.
   - Run Rust formatting/check/tests.
   - Compile the sidecar and live-probe vps-jp.
   - Package `1.0.0-preview.80`.

6. Launch for user testing.
   - Update `run-ainput.bat` and HKCU Run.
   - Start preview.80 in the Windows interactive session.
   - Report exact phrases the user should test and what should appear.
