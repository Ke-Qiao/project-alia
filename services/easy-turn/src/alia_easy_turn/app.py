"""Bounded HTTP contract; model work stays off the HTTP event loop."""

import asyncio
import base64
import binascii
import logging
import math
import re
import threading
import time
from typing import Annotated, Literal, Protocol

from fastapi import FastAPI, Request
from fastapi.exceptions import RequestValidationError
from fastapi.responses import JSONResponse
from pydantic import BaseModel, ConfigDict, Field, StrictInt, StrictStr

SAMPLE_RATE = 16000
MIN_AUDIO_BYTES = 3200  # 100 ms, enough for the upstream STFT and subsampling.
MAX_AUDIO_BYTES = 30 * SAMPLE_RATE * 2
MAX_BASE64_CHARS = 4 * ((MAX_AUDIO_BYTES + 2) // 3)
MAX_BODY_BYTES = MAX_BASE64_CHARS + 4096
STATES = ("complete", "incomplete", "backchannel", "wait")
IDENTIFIER = r"^[A-Za-z0-9._:-]{1,128}$"
U64_MAX = (1 << 64) - 1


class AudioWindow(BaseModel):
    model_config = ConfigDict(extra="forbid")
    encoding: Literal["pcm_s16le"]
    sample_rate_hz: Annotated[StrictInt, Field(ge=16000, le=16000)]
    channels: Annotated[StrictInt, Field(ge=1, le=1)]
    data_base64: StrictStr  # The pre-JSON body bound limits allocation.


class DetectRequest(BaseModel):
    model_config = ConfigDict(extra="forbid")
    request_id: Annotated[StrictStr, Field(pattern=IDENTIFIER)]
    generation_epoch: Annotated[StrictInt, Field(ge=0, le=U64_MAX)]
    audio: AudioWindow


class Detector(Protocol):
    """The single real model implementation and explicitly injected test doubles."""

    metadata: dict

    def detect(self, pcm: bytes) -> tuple[str, str]: ...


def error(status: int, code: str, message: str, retryable: bool = False, **ids):
    return JSONResponse(
        status_code=status,
        content={**ids, "error": {"code": code, "message": message, "retryable": retryable}},
        headers={"Cache-Control": "no-store"},
    )


def correlation(body) -> dict:
    """Recover only bounded validated correlation fields, never echo audio/errors."""
    ids = {}
    if isinstance(body, dict):
        request_id = body.get("request_id")
        epoch = body.get("generation_epoch")
        if isinstance(request_id, str) and re.fullmatch(IDENTIFIER, request_id):
            ids["request_id"] = request_id
        if type(epoch) is int and 0 <= epoch <= U64_MAX:
            ids["generation_epoch"] = epoch
    return ids


class BodyLimit:
    """Reject oversized/chunked bodies before framework JSON parsing allocates them."""

    def __init__(self, app):
        self.app = app

    async def __call__(self, scope, receive, send):
        if scope["type"] != "http":
            return await self.app(scope, receive, send)
        body = bytearray()
        while True:
            message = await receive()
            if message["type"] == "http.disconnect":
                return
            chunk = message.get("body", b"")
            if len(body) + len(chunk) > MAX_BODY_BYTES:
                return await error(413, "audio_too_large", "Request body exceeds the audio window limit.")(
                    scope, receive, send
                )
            body.extend(chunk)
            if not message.get("more_body", False):
                break
        delivered = False

        async def replay():
            nonlocal delivered
            if not delivered:
                delivered = True
                return {"type": "http.request", "body": bytes(body), "more_body": False}
            return await receive()

        await self.app(scope, replay, send)


def create_app(detector: Detector, *, inference_timeout: float = 60.0) -> FastAPI:
    if not math.isfinite(inference_timeout) or inference_timeout <= 0:
        raise ValueError("inference_timeout must be positive")
    app = FastAPI(title="Alia Easy-Turn", version="1", docs_url=None, redoc_url=None)
    app.add_middleware(BodyLimit)
    # One resident model, one inference at a time. No unbounded request queue.
    slot = threading.BoundedSemaphore(1)
    running: set[asyncio.Task] = set()

    @app.exception_handler(RequestValidationError)
    async def invalid_request(_request: Request, exc: RequestValidationError):
        return error(422, "invalid_request", "Request does not match the v1 audio contract.",
                     **correlation(exc.body))

    @app.get("/health")
    async def health():
        return {"status": "ready", "model_loaded": True, "model": detector.metadata}

    @app.get("/v1/capabilities")
    async def capabilities():
        return {
            "api_version": "1", "model": detector.metadata,
            "audio": {"encoding": "pcm_s16le", "sample_rate_hz": SAMPLE_RATE,
                      "channels": 1, "min_duration_ms": 100, "max_duration_ms": 30000},
            "states": STATES, "max_concurrent_requests": 1,
            "inference_timeout_ms": inference_timeout * 1000,
            "streaming": False, "cancellation": "caller_discards_stale_result",
        }

    def infer(pcm: bytes):
        try:
            start = time.perf_counter()
            transcript, state = detector.detect(pcm)
            if not isinstance(transcript, str) or state not in STATES:
                raise ValueError("invalid model output")
            return transcript, state, (time.perf_counter() - start) * 1000
        finally:
            # A timed-out/cancelled HTTP caller cannot free a still-running model.
            slot.release()

    def completed(task):
        running.discard(task)
        if not task.cancelled():
            task.exception()  # Retrieve late failures without logging transcripts.

    @app.post("/v1/detect")
    async def detect(request: DetectRequest):
        ids = {"request_id": request.request_id, "generation_epoch": request.generation_epoch}
        try:
            pcm = base64.b64decode(request.audio.data_base64, validate=True)
        except (binascii.Error, ValueError):
            return error(422, "invalid_audio", "Audio must be valid base64 PCM s16le.", **ids)
        if len(pcm) > MAX_AUDIO_BYTES:
            return error(413, "audio_too_large", "Audio must not exceed 30 seconds.", **ids)
        if len(pcm) < MIN_AUDIO_BYTES or len(pcm) % 2:
            return error(422, "invalid_audio", "Audio must contain complete PCM16 samples and at least 100 ms.", **ids)
        if not slot.acquire(blocking=False):
            return error(429, "busy", "The model is still processing an audio window.", True, **ids)
        task = asyncio.create_task(asyncio.to_thread(infer, pcm))
        running.add(task)
        task.add_done_callback(completed)
        try:
            transcript, state, elapsed = await asyncio.wait_for(asyncio.shield(task), inference_timeout)
        except asyncio.TimeoutError:
            return error(504, "inference_timeout", "Inference exceeded its response deadline.", True, **ids)
        except Exception:
            logging.getLogger(__name__).error("Easy-Turn inference failed; request_id=%s", request.request_id)
            return error(500, "inference_failed", "The model did not produce a valid turn result.", False, **ids)
        return JSONResponse(
            content={**ids, "transcript": transcript, "state": state,
                     "model": detector.metadata, "inference_ms": elapsed},
            headers={"Cache-Control": "no-store"},
        )

    return app
