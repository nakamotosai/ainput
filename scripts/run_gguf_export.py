"""FunASR-GGUF export pipeline runner (01 -> 06).

Usage: python scripts/run_gguf_export.py
- Points upstream export_config at our origin model + output dir (backs up
  upstream file first, restores afterwards).
- Runs 01..06 sequentially, logging to models/_gguf_export.log.
- Safe to re-run: each upstream step skips existing outputs.
"""
import os
import pathlib
import shutil
import subprocess
import sys

UPSTREAM = pathlib.Path("F:/projects/ainput/sidecar/_upstream")
ORIGIN = pathlib.Path("F:/projects/ainput/models/funasr-gguf-origin")
EXPORT = pathlib.Path("F:/projects/ainput/models/funasr-gguf")
LOG = pathlib.Path("F:/projects/ainput/models/_gguf_export.log")
CFG = UPSTREAM / "export_config.py"
BACKUP = CFG.with_suffix(".py.ainput-bak")


def log(msg):
    print(msg, flush=True)
    with open(LOG, "a", encoding="utf-8") as f:
        f.write(msg + "\n")


STEPS = [
    "01-Export-ONNX-FP32.py",
    "02-Optimize-ONNX.py",
    "03-Quantize-ONNX.py",
    "04-Export-Decoder-GGUF-FP16.py",
    "05-Quantize-Decoder-GGUF.py",
]


def main():
    LOG.write_text("", encoding="utf-8")
    if not (ORIGIN / "model.pt").exists():
        log(f"FATAL origin model missing: {ORIGIN / 'model.pt'}")
        return 2
    EXPORT.mkdir(parents=True, exist_ok=True)
    if not BACKUP.exists():
        shutil.copy(CFG, BACKUP)
        log(f"backed up export_config.py -> {BACKUP.name}")
    # Point upstream config at our dirs (restored at the end).
    text = BACKUP.read_text(encoding="utf-8")
    patched = text.replace(
        "MODEL_DIR =  model_home / 'Fun-ASR-Nano-2512'",
        f"MODEL_DIR =  Path(r'{ORIGIN}')",
    ).replace(
        "EXPORT_DIR = Path(r'./model')",
        f"EXPORT_DIR = Path(r'{EXPORT}')",
    )
    if patched == text:
        log("FATAL export_config.py pattern mismatch, refusing to patch")
        return 2
    CFG.write_text(patched, encoding="utf-8")
    log(f"patched export_config: MODEL_DIR={ORIGIN} EXPORT_DIR={EXPORT}")
    # llama.cpp 的 dll 互相依赖（ggml.dll -> ggml-base.dll），必须进 PATH，
    # 否则 ctypes 加载报 FileNotFoundError（缺的是兄弟依赖不是本文件）。
    dll_dir = str(UPSTREAM / "fun_asr_gguf" / "inference" / "bin")
    env = dict(os.environ)
    env["PATH"] = dll_dir + os.pathsep + env.get("PATH", "")
    # 导出在独立 venv 跑：主环境 torch/torchvision 版本打架会拖死 transformers，
    # venv 里无 torchvision，纯 CPU，刚好导出全程本来就不用 GPU。
    venv_py = UPSTREAM.parent / "export-venv" / "Scripts" / "python.exe"
    export_py = str(venv_py) if venv_py.exists() else sys.executable
    log(f"export python: {export_py}")
    try:
        for step in STEPS:
            log(f"===== {step} =====")
            r = subprocess.run(
                [export_py, step],
                cwd=str(UPSTREAM),
                capture_output=False,
                env=env,
                timeout=7200,
            )
            log(f"----- {step} exit={r.returncode} -----")
            if r.returncode != 0:
                log(f"FATAL stopped at {step}")
                return r.returncode
        log("EXPORT PIPELINE DONE")
        for name in [
            "Fun-ASR-Nano-Encoder-Adaptor.fp16.onnx",
            "Fun-ASR-Nano-CTC.fp16.onnx",
            "Fun-ASR-Nano-Decoder.q5_k.gguf",
            "tokens.txt",
        ]:
            p = EXPORT / name
            log(f"{'OK ' if p.exists() else 'MISSING '} {name} "
                f"{p.stat().st_size if p.exists() else 0}")
        return 0
    finally:
        shutil.copy(BACKUP, CFG)
        log("restored export_config.py")
        # 06 inference check runs separately after llama binaries land.


if __name__ == "__main__":
    raise SystemExit(main())
