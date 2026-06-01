# Post-Commit AI Rewrite Preview84 Plan

1. Add config defaults and rendered TOML for `[voice.post_commit_rewrite]`.
2. Extend the existing AI rewrite client with a full committed-text rewrite prompt and JSON `replacement` parsing.
3. Add native replacement tickets in `ainput-output`: capture focused edit handle/range, re-check original text later, then use `EM_SETSEL + EM_REPLACESEL`.
4. Schedule background rewrite after every successful voice commit path.
5. Update package version, config, and README handoff.
6. Run focused tests, package preview84, update live launcher/startup, and verify the running Windows process.

