# ainput — contributor notes

## Product name
**ainput** (not ainput2).

## Build
cargo build --release
cargo test

## Package
.\scripts\make-portable.ps1 -Version 0.1.0 -Overwrite

## Rules
- Local SenseVoice only (no cloud ASR)
- No screen recording
- No hard-coded third-party API keys
- Rewrite = user OpenAI-compatible base_url + api_key + model
- Suspect-term auto analysis out of scope for now
- Do not modify C:\Users\sai\ainput2
