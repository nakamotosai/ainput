# Streaming final tail insert-overlap v18
Goal: fix preview.51 streaming final commit duplication where an offline final tail corrects a displayed tail by inserting a small word.
Scope: worker final commit merge/repair logic, exact user failure regression, package and launch preview.52.
Constraints: keep soft flush enabled, keep AI rewrite/provider config unchanged, do not revert unrelated dirty changes.
Acceptance: cargo test passes; saved raw capture 1778293694109 replays without duplicate tail; packaged raw corpus passes; preview.52 dist and zip exist; preview.52 runs in the interactive Windows session.
