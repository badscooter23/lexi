#!/usr/bin/env python3
import json
import random
import sys
import time
from datetime import datetime


def now_iso() -> str:
    return datetime.now().isoformat()


def main() -> None:
    start_time = time.time()
    received_at = now_iso()

    prompt = " ".join(sys.argv[1:]).strip()
    if not prompt:
        prompt = sys.stdin.read().strip()
    if not prompt:
        prompt = "(no prompt provided)"

    number = random.randint(0, 1000)
    response_at = now_iso()
    processing_ms = int((time.time() - start_time) * 1000)

    payload = {
        "input": {"prompt": prompt, "timestamp": received_at},
        "response": {
            "value": number,
            "timestamp": response_at,
            "processing_ms": processing_ms,
        },
    }

    print(json.dumps(payload))


if __name__ == "__main__":
    main()
