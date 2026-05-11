# AInput Online Parakeet ASR Preview 73 PLAN

## Steps

- [ ] Add project docs for the temporary online ASR experiment.
- [ ] Add a source-controlled Parakeet HTTP adapter script that exposes `/health`, `/v1/sessions`, `/chunk`, and `/finish`.
- [ ] Deploy the adapter on `vps-jp` in an isolated venv and run it as a user service bound to the Tailnet IP.
- [ ] Extend AInput streaming backend dispatch to recognize `nvidia_parakeet_online` and reuse the sidecar worker without Qwen preload.
- [ ] Update config defaults, README, TASKLIST, OPLOG, and DECISIONS for `preview.74`.
- [ ] Package `1.0.0-preview.74` without overwriting old dist packages.
- [ ] Validate adapter, build, package, live Windows startup, logs, and GPU behavior.

## Rollback

Run `C:\Users\sai\ainput\dist\ainput-1.0.0-preview.72\run-ainput.bat` or the `.72` executable directly. `.72` config keeps `backend = "qwen3_sidecar"`, `gpu_memory_utilization = 0.30`, and one-hour idle unload.
