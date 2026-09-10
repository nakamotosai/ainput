"""FunASR-GGUF sidecar for ainput (protocol matches src/funasr_gguf.rs).

 Galvanic contract (both sides must match):
  POST http://127.0.0.1:8765/transcribe
    Content-Type: audio/wav
    X-Ainput-Audio-Format: wav
    X-Sample-Rate: 16000
    body = WAV bytes (16-bit mono)
  -> 200 {"text": "..."} | 4xx/5xx {"error": "..."} (always JSON)

 Model layout (produced by scripts/setup_funasr_gguf.ps1, DO NOT hand-place):
  F:/projects/ainput/models/funasr-gguf/
    Fun-ASR-Nano-Encoder-Adaptor.fp16.onnx
    Fun-ASR-Nano-CTC.fp16.onnx
    Fun-ASR-Nano-Decoder.q5_k.gguf
    tokens.txt
 plus llama.cpp Vulkan/CUDA dlls under sidecar/funasr-gguf-bin/.

 stdlib only (no fastapi/uvicorn needed): python sidecar/funasr_gguf_server.py
 Health: GET /healthz -> {"ok": true, "ready": <bool>}
"""
import io
import json
import os
import sys
import tempfile
import threading
import wave
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer

HOST = os.environ.get("FUNASR_GGUF_HOST", "127.0.0.1")
PORT = int(os.environ.get("FUNASR_GGUF_PORT", "8765"))
HERE = os.path.dirname(os.path.abspath(__file__))
INSTALL_ROOT = os.path.abspath(os.path.join(HERE, ".."))
MODEL_DIR = os.environ.get(
    "FUNASR_GGUF_MODEL_DIR",
    os.path.join(INSTALL_ROOT, "models", "funasr-gguf"),
)
LLM_USE_GPU = os.environ.get("FUNASR_GGUF_GPU", "1") == "1"

# llama.cpp 的 dll 互相依赖，进 DLL 搜索路径，否则 import 即崩。
_LLAMA_BIN = os.path.join(HERE, "_upstream", "fun_asr_gguf", "inference", "bin")
if os.path.isdir(_LLAMA_BIN):
    os.environ["PATH"] = _LLAMA_BIN + os.pathsep + os.environ.get("PATH", "")
    if hasattr(os, "add_dll_directory"):
        try:
            os.add_dll_directory(_LLAMA_BIN)
        except OSError:
            pass

REQUIRED_GGUF = "Fun-ASR-Nano-Decoder.q5_k.gguf"
REQUIRED_TOKENS = "tokens.txt"
# 编码器优先 INT4（CPU 快数倍），没有才回落 FP16；解码器本来就在 GPU 上。
ENCODER_CANDIDATES = [
    "Fun-ASR-Nano-Encoder-Adaptor.int4.onnx",
    "Fun-ASR-Nano-Encoder-Adaptor.fp16.onnx",
]
CTC_CANDIDATES = [
    "Fun-ASR-Nano-CTC.int4.onnx",
    "Fun-ASR-Nano-CTC.fp16.onnx",
]


def pick_first(names):
    for name in names:
        if os.path.exists(os.path.join(MODEL_DIR, name)):
            return name
    return ""


def model_status():
    missing = []
    if not pick_first(ENCODER_CANDIDATES):
        missing.append("Fun-ASR-Nano-Encoder-Adaptor.(int4|fp16).onnx")
    if not pick_first(CTC_CANDIDATES):
        missing.append("Fun-ASR-Nano-CTC.(int4|fp16).onnx")
    for name in (REQUIRED_GGUF, REQUIRED_TOKENS):
        if not os.path.exists(os.path.join(MODEL_DIR, name)):
            missing.append(name)
    return missing


_engine = None
_engine_error = "not initialized"
_engine_lock = threading.Lock()


def result_text(result) -> str:
    if isinstance(result, dict):
        return str(result.get("text", ""))
    text = getattr(result, "text", None)
    if isinstance(text, str):
        return text
    return str(result)


def get_engine():
    global _engine, _engine_error
    with _engine_lock:
        if _engine is not None:
            return _engine
        missing = model_status()
        if missing:
            _engine_error = (
                "model not exported yet, missing: " + ", ".join(missing)
                + ". Run scripts/run_gguf_export.py first."
            )
            raise RuntimeError(_engine_error)
        try:
            sys.path.insert(
                0, os.path.join(INSTALL_ROOT, "sidecar", "_upstream", "fun_asr_gguf")
            )
            sys.path.insert(0, os.path.join(INSTALL_ROOT, "sidecar", "_upstream"))
            from fun_asr_gguf import ASREngineConfig, FunASREngine

            config = ASREngineConfig(
                encoder_onnx_path=os.path.join(MODEL_DIR, pick_first(ENCODER_CANDIDATES)),
                ctc_onnx_path=os.path.join(MODEL_DIR, pick_first(CTC_CANDIDATES)),
                decoder_gguf_path=os.path.join(MODEL_DIR, REQUIRED_GGUF),
                tokens_path=os.path.join(MODEL_DIR, REQUIRED_TOKENS),
                hotwords=[],
                enable_ctc=True,
                onnx_provider="cpu",
                llm_use_gpu=LLM_USE_GPU,
                verbose=False,
                n_threads=12,
            )
            _engine = FunASREngine(config)
            _engine_error = ""
            return _engine
        except Exception as e:
            _engine_error = f"engine init failed: {e!r}"
            raise


def wav_duration_sec(raw: bytes) -> float:
    try:
        with wave.open(io.BytesIO(raw), "rb") as w:
            return w.getnframes() / float(w.getframerate() or 16000)
    except Exception:
        return -1.0


class Handler(BaseHTTPRequestHandler):
    server_version = "ainput-gguf/0.1.6"

    def log_message(self, *args):
        pass

    def _json(self, code: int, obj: dict):
        body = json.dumps(obj, ensure_ascii=False).encode("utf-8")
        self.send_response(code)
        self.send_header("Content-Type", "application/json; charset=utf-8")
        self.send_header("Content-Length", str(len(body)))
        self.end_headers()
        self.wfile.write(body)

    def do_GET(self):
        if self.path == "/healthz":
            missing = model_status()
            ready = not missing and _engine is not None
            self._json(
                200,
                {
                    "ok": True,
                    "ready": ready,
                    "missing": missing,
                    "error": "" if ready else _engine_error,
                },
            )
        else:
            self._json(404, {"error": "unknown path"})

    def do_POST(self):
        if self.path != "/transcribe":
            self._json(404, {"error": "unknown path"})
            return
        length = int(self.headers.get("Content-Length") or 0)
        if length <= 0 or length > 20 * 1024 * 1024:
            self._json(400, {"error": f"bad Content-Length: {length}"})
            return
        raw = self.rfile.read(length)
        if wav_duration_sec(raw) < 0:
            self._json(400, {"error": "body is not a valid WAV file"})
            return
        try:
            engine = get_engine()
        except Exception as e:
            self._json(503, {"error": str(e)[:500]})
            return
        tmp = None
        try:
            fd, tmp = tempfile.mkstemp(suffix=".wav", prefix="gguf-")
            with os.fdopen(fd, "wb") as f:
                f.write(raw)
            result = engine.transcribe(tmp, language=None, verbose=False)
            text = result_text(result)
            if "====解码有误" in text:
                # 上游 LLM 熔断标记是内部调试串，不进用户文档：转报错，
                # Rust 侧转 HUD 人话（这句没转出来，重说一遍）。
                self._json(500, {"error": "llm decode fused after retries"})
                return
            self._json(200, {"text": text})
        except Exception as e:
            self._json(500, {"error": f"transcribe failed: {e!r}"[:500]})
        finally:
            if tmp:
                try:
                    os.remove(tmp)
                except OSError:
                    pass
if __name__ == "__main__":
    import time as _time

    missing = model_status()
    if missing:
        print(f"[gguf] model not ready, missing: {missing}", flush=True)
        print("[gguf] run scripts/run_gguf_export.py to export.", flush=True)
    else:
        print("[gguf] model files present, warming up...", flush=True)
        try:
            _t0 = _time.time()
            _eng = get_engine()
            # 预热一次完整转写：Vulkan 着色器编译等一次性成本在这里付，
            # 用户第一句不再等（实测冷启动 40s+，热机后 RTF 0.07）。
            _buf = io.BytesIO()
            with wave.open(_buf, "wb") as _w:
                _w.setnchannels(1)
                _w.setsampwidth(2)
                _w.setframerate(16000)
                _w.writeframes(b"\x00" * 16000 * 2 * 2)
            _fd, _tmp = tempfile.mkstemp(suffix=".wav", prefix="gguf-warmup-")
            with os.fdopen(_fd, "wb") as _f:
                _f.write(_buf.getvalue())
            try:
                _eng.transcribe(_tmp, language="中文", verbose=False)
            finally:
                os.remove(_tmp)
            print(f"[gguf] warmed up in {_time.time() - _t0:.1f}s", flush=True)
        except Exception as _e:
            print(f"[gguf] warmup failed (first request will be slow): {_e!r}"[:300], flush=True)
    print(f"[gguf] listening on http://{HOST}:{PORT}/transcribe", flush=True)
    ThreadingHTTPServer((HOST, PORT), Handler).serve_forever()
