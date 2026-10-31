"""HTTP/service contract checks with an injected double; not model acceptance."""

import asyncio
import base64
import threading
import unittest

import httpx

from alia_easy_turn.app import MAX_AUDIO_BYTES, MAX_BODY_BYTES, create_app
from alia_easy_turn.model import parse_output


class FakeDetector:
    metadata = {"source": "test-double", "revision": "test-only", "device": "cpu"}

    def __init__(self):
        self.calls = []

    def detect(self, pcm):
        self.calls.append(pcm)
        return "请等一下", "wait"


def payload(pcm=b"\x01\x00" * 1600):
    return {
        "request_id": "request-17", "generation_epoch": 42,
        "audio": {"encoding": "pcm_s16le", "sample_rate_hz": 16000, "channels": 1,
                  "data_base64": base64.b64encode(pcm).decode()},
    }


class HttpContractTests(unittest.IsolatedAsyncioTestCase):
    async def asyncSetUp(self):
        self.detector = FakeDetector()
        self.client = httpx.AsyncClient(transport=httpx.ASGITransport(app=create_app(self.detector)),
                                        base_url="http://127.0.0.1")

    async def asyncTearDown(self):
        await self.client.aclose()

    async def test_health_capabilities_and_correlated_result(self):
        health = await self.client.get("/health")
        self.assertEqual(health.json()["status"], "ready")
        capabilities = (await self.client.get("/v1/capabilities")).json()
        self.assertEqual(capabilities["audio"]["max_duration_ms"], 30000)
        self.assertEqual(capabilities["max_concurrent_requests"], 1)
        self.assertFalse(capabilities["streaming"])
        with self.assertLogs("uvicorn.error", level="INFO") as logs:
            result = await self.client.post("/v1/detect", json=payload())
        self.assertEqual(result.status_code, 200)
        self.assertEqual(result.json()["request_id"], "request-17")
        self.assertEqual(result.json()["generation_epoch"], 42)
        self.assertEqual(result.json()["transcript"], "请等一下")
        self.assertEqual(result.json()["state"], "wait")
        self.assertEqual(result.headers["cache-control"], "no-store")
        self.assertEqual(self.detector.calls, [b"\x01\x00" * 1600])
        self.assertEqual(len(logs.records), 3)
        self.assertIn("accepted request_id=request-17 generation_epoch=42", logs.output[0])
        self.assertIn("pcm_bytes=3200 audio_ms=100.000", logs.output[0])
        self.assertIn("native_started", logs.output[1])
        self.assertRegex(logs.output[1], r"queue_ms=\d+\.\d+")
        self.assertIn("native_finished", logs.output[2])
        self.assertIn("status=success state=wait", logs.output[2])
        self.assertRegex(logs.output[2], r"native_ms=\d+\.\d+")
        for record in logs.output:
            self.assertRegex(record, r"at_unix_ms=\d+")
            self.assertNotIn(result.json()["transcript"], record)
            self.assertNotIn(payload()["audio"]["data_base64"], record)
            self.assertNotIn(repr(self.detector.calls[0]), record)

    async def test_invalid_audio_and_format_do_not_reach_model(self):
        invalid = [payload(b""), payload(b"\x00" * 3199), payload(b"\x00" * 3201)]
        for field, value in [("sample_rate_hz", 48000), ("channels", 2),
                             ("encoding", "wav"), ("sample_rate_hz", "16000"),
                             ("data_base64", "@invalid!"), ("data_base64", "你好")]:
            item = payload()
            item["audio"][field] = value
            invalid.append(item)
        for item in invalid:
            with self.subTest(item=str(item)[:100]):
                result = await self.client.post("/v1/detect", json=item)
                self.assertEqual(result.status_code, 422)
                self.assertEqual(result.json()["request_id"], "request-17")
                self.assertNotIn("data_base64", result.text)
        self.assertEqual(self.detector.calls, [])

    async def test_ids_are_strict_and_bounded(self):
        for key, value in [("generation_epoch", True), ("generation_epoch", -1),
                           ("generation_epoch", 2**64), ("generation_epoch", 1.0),
                           ("request_id", ""), ("request_id", "x" * 129)]:
            item = payload()
            item[key] = value
            result = await self.client.post("/v1/detect", json=item)
            self.assertEqual(result.status_code, 422)
        item = payload()
        item["generation_epoch"] = 2**64 - 1
        result = await self.client.post("/v1/detect", json=item)
        self.assertEqual(result.json()["generation_epoch"], 2**64 - 1)

    async def test_audio_duration_boundary_is_enforced_after_base64_decoding(self):
        accepted = await self.client.post("/v1/detect", json=payload(b"\x00" * MAX_AUDIO_BYTES))
        self.assertEqual(accepted.status_code, 200)
        rejected = await self.client.post("/v1/detect", json=payload(b"\x00" * (MAX_AUDIO_BYTES + 2)))
        self.assertEqual(rejected.status_code, 413)
        self.assertEqual(rejected.json()["error"]["code"], "audio_too_large")
        self.assertEqual(len(self.detector.calls), 1)

    async def test_invalid_json_and_unknown_fields_do_not_echo_private_input(self):
        result = await self.client.post("/v1/detect", content=b'{"private":"secret",broken',
                                        headers={"content-type": "application/json"})
        self.assertEqual(result.status_code, 422)
        self.assertNotIn("secret", result.text)
        item = payload()
        item["prompt"] = "arbitrary caller instruction"
        result = await self.client.post("/v1/detect", json=item)
        self.assertEqual(result.status_code, 422)
        self.assertNotIn("arbitrary", result.text)

    async def test_chunked_body_has_a_pre_json_limit(self):
        async def chunks():
            yield b" " * (MAX_BODY_BYTES // 2)
            yield b" " * (MAX_BODY_BYTES // 2 + 1)

        result = await self.client.post("/v1/detect", content=chunks(),
                                        headers={"content-type": "application/json"})
        self.assertEqual(result.status_code, 413)
        self.assertEqual(self.detector.calls, [])

    async def test_model_error_is_sanitized_and_releases_slot(self):
        class BrokenDetector(FakeDetector):
            def detect(self, pcm):
                raise RuntimeError("private transcript /secret/path prompt=private-prompt api_key=private-key")

        async with httpx.AsyncClient(transport=httpx.ASGITransport(app=create_app(BrokenDetector())),
                                     base_url="http://127.0.0.1") as client:
            with self.assertLogs("uvicorn.error", level="INFO") as logs:
                for _ in range(2):
                    result = await client.post("/v1/detect", json=payload())
                    self.assertEqual(result.status_code, 500)
                    self.assertEqual(result.json()["error"]["code"], "inference_failed")
                    self.assertNotIn("secret", result.text)
            failures = [record for record in logs.records if record.levelname == "ERROR"]
            self.assertEqual(len(failures), 2)
            for record in failures:
                self.assertIn("status=error error_type=RuntimeError", record.getMessage())
                self.assertIsNone(record.exc_info)
            for private in ("private transcript", "/secret/path", "private-prompt", "private-key",
                            payload()["audio"]["data_base64"]):
                self.assertNotIn(private, "\n".join(logs.output))

    async def test_busy_and_timeout_keep_slot_until_inference_actually_ends(self):
        entered, release = threading.Event(), threading.Event()

        class SlowDetector(FakeDetector):
            def detect(self, pcm):
                entered.set()
                release.wait(timeout=2)
                return "done", "complete"

        app = create_app(SlowDetector(), inference_timeout=0.1)
        with self.assertLogs("uvicorn.error", level="INFO") as logs:
            async with httpx.AsyncClient(transport=httpx.ASGITransport(app=app),
                                         base_url="http://127.0.0.1") as client:
                first = asyncio.create_task(client.post("/v1/detect", json=payload()))
                try:
                    self.assertTrue(await asyncio.to_thread(entered.wait, 1))
                    busy = await client.post("/v1/detect", json=payload())
                    self.assertEqual(busy.status_code, 429)
                    self.assertEqual((await client.get("/health")).status_code, 200)
                    timed_out = await first
                    self.assertEqual(timed_out.status_code, 504)
                    still_busy = await client.post("/v1/detect", json=payload())
                    self.assertEqual(still_busy.status_code, 429)
                    self.assertEqual(sum("accepted" in line for line in logs.output), 1)
                    self.assertTrue(any("response_timeout" in line for line in logs.output))
                    self.assertFalse(any("native_finished" in line for line in logs.output))
                finally:
                    release.set()
                    await first
                for _ in range(100):
                    await asyncio.sleep(0.01)
                    recovered = await client.post("/v1/detect", json=payload())
                    if recovered.status_code != 429:
                        break
                self.assertEqual(recovered.status_code, 200)
                finishes = [line for line in logs.output if "native_finished" in line]
                self.assertEqual(len(finishes), 2)
                self.assertTrue(all("status=success state=complete" in line for line in finishes))

    async def test_malformed_model_output_is_not_a_fabricated_turn_state(self):
        class InvalidDetector(FakeDetector):
            def detect(self, pcm):
                return "no state", "success"

        async with httpx.AsyncClient(transport=httpx.ASGITransport(app=create_app(InvalidDetector())),
                                     base_url="http://127.0.0.1") as client:
            self.assertEqual((await client.post("/v1/detect", json=payload())).status_code, 500)

    async def test_caller_cancellation_does_not_release_the_model_slot(self):
        entered, release = threading.Event(), threading.Event()

        class SlowDetector(FakeDetector):
            def detect(self, pcm):
                entered.set()
                release.wait(timeout=2)
                return "done", "complete"

        async with httpx.AsyncClient(transport=httpx.ASGITransport(app=create_app(SlowDetector())),
                                     base_url="http://127.0.0.1") as client:
            first = asyncio.create_task(client.post("/v1/detect", json=payload()))
            try:
                self.assertTrue(await asyncio.to_thread(entered.wait, 1))
                first.cancel()
                with self.assertRaises(asyncio.CancelledError):
                    await first
                self.assertEqual((await client.post("/v1/detect", json=payload())).status_code, 429)
            finally:
                release.set()

    async def test_timeout_configuration_must_be_finite_and_positive(self):
        for timeout in (0, -1, float("inf"), float("nan")):
            with self.assertRaises(ValueError):
                create_app(self.detector, inference_timeout=timeout)


class OutputContractTests(unittest.TestCase):
    def test_each_official_label_is_parsed_without_inventing_transcript(self):
        for label in ("complete", "incomplete", "backchannel", "wait"):
            self.assertEqual(parse_output([f"用户原文<{label.upper()}>  "]), ("用户原文", label))

    def test_missing_or_multiple_labels_fail(self):
        for output in (["missing"], ["x<wait> trailing"], ["x<wait><complete>"],
                       ["one<wait>", "two<wait>"], "x<wait>", []):
            with self.subTest(output=output), self.assertRaises(ValueError):
                parse_output(output)

    def test_real_checkpoint_literal_eos_suffix_is_not_part_of_transcript(self):
        self.assertEqual(parse_output(["嗯对对对<backchannel><|endoftext|>"]), ("嗯对对对", "backchannel"))
        with self.assertRaises(ValueError):
            parse_output(["嗯对对对<backchannel><|endoftext|>unexpected"])


if __name__ == "__main__":
    unittest.main()
