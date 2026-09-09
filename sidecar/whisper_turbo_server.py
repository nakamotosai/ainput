#!/usr/bin/env python3
# -*- coding: utf-8 -*-
"""Whisper-turbo 流式边车（ainput 的 whisper-turbo 引擎用）。
显卡 faster-whisper turbo + 会话式增量解码 + LocalAgreement-2 确认。

协议（全部 JSON，除 append 收原始 PCM16 外）：
  GET  /healthz                  -> {ok, ready, model, device, missing, error}
  POST /session/start            -> {id}
  POST /session/<id>/append      body=pcm16le mono 16k 原始字节
                                 -> {stable, provisional}（两次整段解码 agree 的前缀才 stable）
  POST /session/<id>/finish      body 可空（允许最后一段音频）
                                 -> {text}（beam5 + vad 收尾，会话销毁）
  POST /session/<id>/cancel      -> {}（销毁）

按住说话即一个会话：Rust 侧每来一块麦克风音频就 append，
HUD 显示 stable+provisional；松开调 finish 定稿。
"""
import struct
import json
import os
import time
import threading
import uuid
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer

SAMPLE_RATE = 16000
MODEL_NAME = "turbo"
DEVICE = "cuda"
COMPUTE = "float16"
# 两次解码之间至少新增这么多音频才重算（秒）。
MIN_NEW_AUDIO_SEC = 0.8
# whisper 窗口 30 秒，输入法按住很少超；超了如实报错不截断欺骗。
MAX_BUFFER_SEC = 30.0
MAX_SESSIONS = 8

_model = None
_model_lock = threading.Lock()
_sessions = {}
_sessions_lock = threading.Lock()


def _now():
    return time.strftime("%H:%M:%S")


def load_model():
    global _model
    from faster_whisper import WhisperModel
    print("[%s] loading %s on %s (%s) ..." % (_now(), MODEL_NAME, DEVICE, COMPUTE), flush=True)
    _model = WhisperModel(MODEL_NAME, device=DEVICE, compute_type=COMPUTE)
    # 预热：空解一次，把 CUDA/cuDNN 初始化交掉，首句不慢。
    import tempfile, wave
    with tempfile.NamedTemporaryFile(suffix=".wav", delete=False) as tmp:
        with wave.open(tmp.name, "wb") as w:
            w.setnchannels(1)
            w.setsampwidth(2)
            w.setframerate(SAMPLE_RATE)
            w.writeframes(b"\x00\x00" * SAMPLE_RATE)
        try:
            segs, _ = _model.transcribe(tmp.name, beam_size=1, language="zh")
            for _ in segs:
                pass
        finally:
            try:
                os.remove(tmp.name)
            except OSError:
                pass

def _pcm_to_float32(pcm):
    import numpy as np
    return (np.frombuffer(bytes(pcm), dtype=np.int16).astype(np.float32) / 32768.0)


def decode_full(pcm):
    """整段解码（调用方持有 _model_lock）。beam1 求快，用于 append 中间态。"""
    audio = _pcm_to_float32(pcm)
    segs, _ = _model.transcribe(audio, beam_size=1, language=None,
                                temperature=0.0, condition_on_previous_text=False,
                                no_speech_threshold=0.6)
    return "".join(s.text for s in segs).strip()


def decode_final(pcm):
    audio = _pcm_to_float32(pcm)
    segs, _ = _model.transcribe(audio, beam_size=5, language=None,
                                temperature=0.0, condition_on_previous_text=False,
                                vad_filter=True,
                                vad_parameters=dict(min_silence_duration_ms=300))
    return "".join(s.text for s in segs).strip()


def common_prefix_stable(a, b):
    """LocalAgreement-2：两版整段文本的最长公共前缀；
    中英文混排按字比，英文单词中间断开就退到上一个空格。"""
    n = min(len(a), len(b))
    i = 0
    while i < n and a[i] == b[i]:
        i += 1
    cut = i
    # 别把英文单词拦腰砍断。
    while cut > 0 and cut < len(b) and b[cut - 1].isascii() and b[cut - 1].isalpha() \
            and b[cut].isascii() and b[cut].isalpha():
        cut -= 1
    return b[:cut]


class Session:
    def __init__(self):
        self.buf = bytearray()
        self.prev_full = ""
        self.prevprev_full = ""
        self.stable = ""
        self.last_decode_samples = 0
        self.created = time.time()


def get_session(sid):
    with _sessions_lock:
        return _sessions.get(sid)


def new_session():
    with _sessions_lock:
        if len(_sessions) >= MAX_SESSIONS:
            oldest = min(_sessions, key=lambda k: _sessions[k].created)
            del _sessions[oldest]
        sid = uuid.uuid4().hex[:12]
        _sessions[sid] = Session()
        return sid


def drop_session(sid):
    with _sessions_lock:
        _sessions.pop(sid, None)


class Handler(BaseHTTPRequestHandler):
    server_version = "TurboSidecar/0.1"

    def log_message(self, *a):
        pass

    def _send(self, code, obj):
        body = json.dumps(obj, ensure_ascii=False).encode("utf-8")
        self.send_response(code)
        self.send_header("Content-Type", "application/json; charset=utf-8")
        self.send_header("Content-Length", str(len(body)))
        self.end_headers()
        self.wfile.write(body)

    def _body(self):
        try:
            n = int(self.headers.get("Content-Length") or 0)
        except ValueError:
            n = 0
        return self.rfile.read(n) if n > 0 else b""

    def do_GET(self):
        if self.path == "/healthz":
            self._send(200, {"ok": True, "ready": _model is not None,
                             "model": MODEL_NAME, "device": DEVICE,
                             "missing": [], "error": ""})
        else:
            self._send(404, {"error": "not found"})

    def do_POST(self):
        parts = self.path.strip("/").split("/")
        if parts == ["session", "start"]:
            self._send(200, {"id": new_session()})
            return
        if len(parts) == 3 and parts[0] == "session" and parts[2] in ("append", "finish", "cancel"):
            sid, action = parts[1], parts[2]
            ses = get_session(sid)
            if ses is None:
                self._send(404, {"error": "unknown session (expired?)"})
                return
            if action == "cancel":
                drop_session(sid)
                self._send(200, {})
                return
            data = self._body()
            if data:
                if len(data) % 2 == 1:
                    data = data[:-1]
                ses.buf.extend(data)
            if action == "finish":
                secs = len(ses.buf) // 2 / SAMPLE_RATE
                if secs < 0.3:
                    drop_session(sid)
                    self._send(200, {"text": ""})
                    return
                if secs > MAX_BUFFER_SEC:
                    drop_session(sid)
                    self._send(413, {"error": "utterance too long (>%ds), press shorter" % int(MAX_BUFFER_SEC)})
                    return
                with _model_lock:
                    try:
                        text = decode_final(bytes(ses.buf))
                    except Exception as e:
                        drop_session(sid)
                        self._send(500, {"error": "decode failed: %s" % str(e)[:200]})
                        return
                drop_session(sid)
                self._send(200, {"text": text})
                return
            # append：攒够新音频才重算，否则回上次结果（省显卡）。
            new_samples = len(ses.buf) - ses.last_decode_samples
            if new_samples >= int(MIN_NEW_AUDIO_SEC * SAMPLE_RATE):
                with _model_lock:
                    try:
                        full = decode_full(bytes(ses.buf))
                    except Exception as e:
                        self._send(500, {"error": "decode failed: %s" % str(e)[:200]})
                        return
                ses.last_decode_samples = len(ses.buf)
                if ses.prev_full:
                    ses.stable = common_prefix_stable(ses.prev_full, full)
                ses.prevprev_full = ses.prev_full
                ses.prev_full = full
            provisional = ses.prev_full[len(ses.stable):] if ses.prev_full.startswith(ses.stable) else ses.prev_full
            self._send(200, {"stable": ses.stable, "provisional": provisional})
            return
        self._send(404, {"error": "not found"})


def main():
    load_model()
    srv = ThreadingHTTPServer(("127.0.0.1", 8766), Handler)
    print("[%s] listening on http://127.0.0.1:8766" % _now(), flush=True)
    srv.serve_forever()


if __name__ == "__main__":
    main()
