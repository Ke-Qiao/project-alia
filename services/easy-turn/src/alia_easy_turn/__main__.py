import argparse
import ipaddress
import math
from pathlib import Path

from .app import create_app
from .model import EasyTurnDetector


def main():
    parser = argparse.ArgumentParser(description="Resident official Easy-Turn inference; no startup downloads")
    parser.add_argument("--source-dir", required=True, type=Path)
    parser.add_argument("--model-dir", required=True, type=Path)
    parser.add_argument("--qwen-dir", required=True, type=Path)
    parser.add_argument("--device", choices=("cpu", "mps", "auto"), default="cpu")
    parser.add_argument("--host", default="127.0.0.1")
    parser.add_argument("--port", type=int, default=8091)
    parser.add_argument("--inference-timeout", type=float, default=60.0)
    args = parser.parse_args()
    if not ipaddress.ip_address(args.host).is_loopback:
        parser.error("This service must bind to a loopback IP on the Alia Server host")
    if not 1 <= args.port <= 65535 or not math.isfinite(args.inference_timeout) or args.inference_timeout <= 0:
        parser.error("port and inference-timeout are outside their valid range")
    detector = EasyTurnDetector(args.source_dir, args.model_dir, args.qwen_dir, args.device)
    import uvicorn
    uvicorn.run(create_app(detector, inference_timeout=args.inference_timeout), host=args.host,
                port=args.port, workers=1, limit_concurrency=32, timeout_keep_alive=5,
                access_log=False)


if __name__ == "__main__":
    main()
