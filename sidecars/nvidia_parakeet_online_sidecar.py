import os
import re
import struct
import threading
import time
import uuid
from dataclasses import dataclass, field
from pathlib import Path

import riva.client
from fastapi import FastAPI, HTTPException, Request
from pydantic import BaseModel


MODEL = "nvidia/parakeet-ctc-0_6b-zh-cn"
SAMPLE_RATE = int(os.environ.get("PARAKEET_SAMPLE_RATE_HZ", "16000"))
LANGUAGE_CODE = os.environ.get("PARAKEET_LANGUAGE_CODE", "zh-CN")
URI = os.environ.get("PARAKEET_GRPC_URI", "grpc.nvcf.nvidia.com:443")
FUNCTION_ID = os.environ.get("PARAKEET_FUNCTION_ID", "9add5ef7-322e-47e0-ad7a-5653fb8d259b")
CONFIG_PATH = Path(
    os.environ.get(
        "PARAKEET_CLIPROXY_CONFIG",
        "/opt/cliproxyapi-host-v6937/configs/config.prod.yaml",
    )
)
TIMEOUT_SEC = float(os.environ.get("PARAKEET_TIMEOUT_SEC", "30"))
MAX_AUDIO_SEC = float(os.environ.get("PARAKEET_MAX_AUDIO_SEC", "90"))


@dataclass
class AsrSession:
    chunks: list[bytes] = field(default_factory=list)
    created_at: float = field(default_factory=time.time)
    updated_at: float = field(default_factory=time.time)
    audio_samples: int = 0
    calls: int = 0
    lock: threading.Lock = field(default_factory=threading.Lock)


class StartRequest(BaseModel):
    context: str = ""
    language: str | None = None
    unfixed_chunk_num: int | None = None
    unfixed_token_num: int | None = None
    chunk_size_sec: float | None = None


class StartResponse(BaseModel):
    session_id: str
    sample_rate_hz: int


class ChunkResponse(BaseModel):
    session_id: str
    text: str
    language: str | None
    audio_ms: int
    elapsed_ms: float


class FinishResponse(ChunkResponse):
    finished: bool


app = FastAPI(title="ainput NVIDIA Parakeet online ASR sidecar", version="0.1")
sessions: dict[str, AsrSession] = {}
keys_lock = threading.Lock()
keys_cache: list[str] = []
keys_cache_mtime: float | None = None
key_cursor = 0


def load_keys() -> list[str]:
    global keys_cache, keys_cache_mtime
    env_keys = [
        value.strip()
        for value in re.split(r"[\s,]+", os.environ.get("PARAKEET_NVIDIA_API_KEYS", ""))
        if value.strip()
    ]
    if env_keys:
        return env_keys

    stat = CONFIG_PATH.stat()
    with keys_lock:
        if keys_cache and keys_cache_mtime == stat.st_mtime:
            return list(keys_cache)
        text = CONFIG_PATH.read_text(encoding="utf-8", errors="ignore")
        keys = list(dict.fromkeys(re.findall(r"nvapi-[A-Za-z0-9_-]+", text)))
        keys_cache = keys
        keys_cache_mtime = stat.st_mtime
        return list(keys)


def next_key() -> tuple[int, str]:
    global key_cursor
    keys = load_keys()
    if not keys:
        raise RuntimeError("no NVIDIA API keys found")
    with keys_lock:
        index = key_cursor % len(keys)
        key_cursor += 1
    return index, keys[index]


def make_service(api_key: str) -> riva.client.ASRService:
    auth = riva.client.Auth(
        uri=URI,
        use_ssl=True,
        metadata_args=[
            ["authorization", f"Bearer {api_key}"],
            ["function-id", FUNCTION_ID],
        ],
    )
    return riva.client.ASRService(auth)


def f32_chunks_to_pcm16(chunks: list[bytes]) -> bytes:
    output = bytearray()
    for chunk in chunks:
        if len(chunk) % 4 != 0:
            raise ValueError("chunk body must be little-endian f32 PCM")
        for (sample,) in struct.iter_unpack("<f", chunk):
            if sample > 1.0:
                sample = 1.0
            elif sample < -1.0:
                sample = -1.0
            output.extend(struct.pack("<h", int(round(sample * 32767.0))))
    return bytes(output)


def normalize_transcript(text: str) -> str:
    text = re.sub(r"\s+", " ", text).strip()
    text = re.sub(r"([\u4e00-\u9fff])\s+([\u4e00-\u9fff])", r"\1\2", text)
    text = re.sub(r"([\u4e00-\u9fff])\s+([，。！？；：、])", r"\1\2", text)
    return text


def response_text(response) -> str:
    parts: list[str] = []
    for result in response.results:
        if result.alternatives:
            parts.append(result.alternatives[0].transcript)
    return normalize_transcript(" ".join(parts))


def transcribe_pcm16(pcm16: bytes) -> tuple[str, int]:
    config = riva.client.RecognitionConfig(
        encoding=riva.client.AudioEncoding.LINEAR_PCM,
        sample_rate_hertz=SAMPLE_RATE,
        language_code=LANGUAGE_CODE,
        max_alternatives=1,
        audio_channel_count=1,
        enable_automatic_punctuation=True,
    )
    streaming_config = riva.client.StreamingRecognitionConfig(
        config=config,
        interim_results=False,
    )
    attempts = max(len(load_keys()), 1)
    last_error = "unknown error"
    chunk_bytes = max((SAMPLE_RATE // 10) * 2, 3200)
    audio_chunks = [pcm16[offset : offset + chunk_bytes] for offset in range(0, len(pcm16), chunk_bytes)]
    for _ in range(attempts):
        key_index, api_key = next_key()
        try:
            service = make_service(api_key)
            parts: list[str] = []
            deadline = time.monotonic() + TIMEOUT_SEC
            for response in service.streaming_response_generator(audio_chunks, streaming_config):
                if time.monotonic() > deadline:
                    raise TimeoutError(f"streaming recognition exceeded {TIMEOUT_SEC}s")
                text = response_text(response)
                if text:
                    parts.append(text)
            return normalize_transcript(" ".join(parts)), key_index
        except Exception as error:
            last_error = f"{type(error).__name__}: {error}"
    raise RuntimeError(f"Parakeet transcription failed after key rotation: {last_error}")


@app.get("/health")
def health() -> dict:
    try:
        key_count = len(load_keys())
    except Exception:
        key_count = 0
    return {
        "ok": key_count > 0,
        "model": MODEL,
        "sessions": len(sessions),
        "sample_rate_hz": SAMPLE_RATE,
        "language": LANGUAGE_CODE,
        "key_count": key_count,
        "uri": URI,
        "function_id": FUNCTION_ID,
    }


@app.post("/v1/sessions", response_model=StartResponse)
def start_session(request: StartRequest | None = None) -> StartResponse:
    _ = request
    session_id = uuid.uuid4().hex
    sessions[session_id] = AsrSession()
    return StartResponse(session_id=session_id, sample_rate_hz=SAMPLE_RATE)


@app.post("/v1/sessions/{session_id}/chunk", response_model=ChunkResponse)
async def accept_chunk(session_id: str, request: Request) -> ChunkResponse:
    session = sessions.get(session_id)
    if session is None:
        raise HTTPException(status_code=404, detail="unknown session")
    body = await request.body()
    if len(body) % 4 != 0:
        raise HTTPException(status_code=400, detail="chunk body must be little-endian f32 PCM")
    samples = len(body) // 4
    with session.lock:
        if (session.audio_samples + samples) / SAMPLE_RATE > MAX_AUDIO_SEC:
            raise HTTPException(status_code=413, detail="audio session too long")
        session.chunks.append(body)
        session.audio_samples += samples
        session.calls += 1
        session.updated_at = time.time()
        audio_ms = round(session.audio_samples / SAMPLE_RATE * 1000)
    return ChunkResponse(
        session_id=session_id,
        text="",
        language=LANGUAGE_CODE,
        audio_ms=audio_ms,
        elapsed_ms=0.0,
    )


@app.post("/v1/sessions/{session_id}/finish", response_model=FinishResponse)
def finish_session(session_id: str) -> FinishResponse:
    session = sessions.pop(session_id, None)
    if session is None:
        raise HTTPException(status_code=404, detail="unknown session")
    started = time.perf_counter()
    with session.lock:
        pcm16 = f32_chunks_to_pcm16(session.chunks)
        audio_ms = round(session.audio_samples / SAMPLE_RATE * 1000)
    if not pcm16:
        text = ""
        key_index = -1
    else:
        try:
            text, key_index = transcribe_pcm16(pcm16)
        except Exception as error:
            raise HTTPException(status_code=502, detail=str(error)) from error
    elapsed_ms = round((time.perf_counter() - started) * 1000, 1)
    print(
        f"[parakeet-sidecar] finish session={session_id} audio_ms={audio_ms} "
        f"elapsed_ms={elapsed_ms} key_index={key_index} text_chars={len(text)}",
        flush=True,
    )
    return FinishResponse(
        session_id=session_id,
        text=text,
        language=LANGUAGE_CODE,
        audio_ms=audio_ms,
        elapsed_ms=elapsed_ms,
        finished=True,
    )
