#!/usr/bin/env python3
"""Minimal WildRig-shaped HTTP API for the e2e/screenshot harness.

Usage: fake-miner-api.py PORT GPU_STATE [MODE_FILE]

It reads the same GPU state file as fake-nvidia-smi and reports one thread per
card with a stable busID. MODE_FILE may contain `low` or `zero` to simulate a
health regression for the watchdog tests.
"""

import json
import math
import sys
import time
from http.server import BaseHTTPRequestHandler, HTTPServer


PORT = int(sys.argv[1])
STATE_PATH = sys.argv[2]
MODE_PATH = sys.argv[3] if len(sys.argv) > 3 else ""


def bus_number(pci_bus_id):
    try:
        return int(pci_bus_id.split(":")[1], 16)
    except (IndexError, ValueError):
        return 0


def mode():
    try:
        with open(MODE_PATH, encoding="utf-8") as handle:
            return handle.read().strip()
    except OSError:
        return ""


class Handler(BaseHTTPRequestHandler):
    def do_GET(self):
        try:
            with open(STATE_PATH, encoding="utf-8") as handle:
                gpus = json.load(handle)["gpus"]
        except Exception:
            gpus = []
        now = time.time()
        current = mode()
        threads = []
        buses = []
        accepted = []
        rejected = []
        for index, gpu in enumerate(gpus):
            if current == "zero":
                value = 0.0
            elif current == "low":
                value = 1.0e12
            else:
                base = 100.0e12 if index == 0 else 25.0e12
                value = base * (1.0 + 0.01 * math.sin(now / 5.0 + index))
            threads.append([value, value, value * 0.99])
            buses.append(bus_number(gpu.get("pci_bus_id", "00000000:00:00.0")))
            accepted.append(500 + index)
            rejected.append(1 + index)
        total = [sum(row[0] for row in threads), sum(row[1] for row in threads), sum(row[2] for row in threads)]
        payload = {
            "version": "fake",
            "hashrate": {"total": total, "threads": threads},
            "hwmon": {"busID": buses},
            "results": {
                "shares_good": sum(accepted),
                "shares_total": sum(accepted) + sum(rejected),
                "shares_accepted": accepted,
                "shares_rejected": rejected,
            },
        }
        body = json.dumps(payload).encode()
        self.send_response(200)
        self.send_header("Content-Type", "application/json")
        self.send_header("Content-Length", str(len(body)))
        self.end_headers()
        self.wfile.write(body)

    def log_message(self, *_args):
        pass


HTTPServer(("127.0.0.1", PORT), Handler).serve_forever()
