#!/usr/bin/env python3
"""Stateful fake `nvidia-smi` for the Trommel e2e/development harness.

The daemon verifies the power limit after every apply/restore and checks the
clock locks once the miner is under load, so a static CSV fixture is no longer
enough. This fake keeps a small JSON state file and honours `-pl`, `-lgc`,
`-lmc`, `-rgc` and `-rmc`.

Environment:
  TROMMEL_FAKE_GPU_STATE  path to the JSON state (required)
  TROMMEL_FAKE_MODE       optional file with one switch per line:
                          ignore_pl, fail_pl, ignore_lgc, fail_lmc,
                          ignore_lmc, fail_rgc, fail_rmc
"""

import json
import os
import sys


STATE_PATH = os.environ["TROMMEL_FAKE_GPU_STATE"]
MODE_PATH = os.environ.get("TROMMEL_FAKE_MODE", "")


def load_state():
    with open(STATE_PATH, encoding="utf-8") as handle:
        return json.load(handle)


def load_modes():
    if not MODE_PATH or not os.path.exists(MODE_PATH):
        return set()
    with open(MODE_PATH, encoding="utf-8") as handle:
        return {line.strip() for line in handle if line.strip()}


def fail(message):
    print(f"nvidia-smi: {message}", file=sys.stderr)
    sys.exit(1)


def main():
    state = load_state()
    modes = load_modes()
    args = sys.argv[1:]
    query = None
    uuid = None
    mutation = None
    value = None
    index = 0
    while index < len(args):
        arg = args[index]
        if arg in ("-i", "--id"):
            uuid = args[index + 1]
            index += 2
        elif arg in ("-pl", "--power-limit"):
            mutation, value = "pl", args[index + 1]
            index += 2
        elif arg in ("-lgc", "--lock-gpu-clocks"):
            mutation, value = "lgc", args[index + 1]
            index += 2
        elif arg in ("-lmc", "--lock-memory-clocks"):
            mutation, value = "lmc", args[index + 1]
            index += 2
        elif arg in ("-rgc", "--reset-gpu-clocks"):
            mutation = "rgc"
            index += 1
        elif arg in ("-rmc", "--reset-memory-clocks"):
            mutation = "rmc"
            index += 1
        elif arg.startswith("--query-gpu="):
            query = arg.split("=", 1)[1]
            index += 1
        elif arg == "--query-gpu":
            query = args[index + 1]
            index += 2
        elif arg == "--format":
            index += 2
        else:
            index += 1

    selected = [
        gpu for gpu in state["gpus"] if uuid is None or gpu["uuid"] == uuid
    ]
    if uuid is not None and not selected:
        print("No devices were found", file=sys.stderr)
        sys.exit(2)

    if mutation == "pl":
        if "fail_pl" in modes:
            fail("power limit rejected")
        if "ignore_pl" not in modes:
            for gpu in selected:
                gpu["power"] = round(float(value), 2)
    elif mutation == "lgc":
        if "ignore_lgc" not in modes:
            clock = float(value.split(",")[0])
            for gpu in selected:
                gpu["core"] = int(clock)
    elif mutation == "lmc":
        if "fail_lmc" in modes:
            fail("memory clock lock rejected")
        if "ignore_lmc" not in modes:
            for gpu in selected:
                gpu["mem"] = int(value)
    elif mutation == "rgc":
        if "fail_rgc" in modes:
            fail("GPU clock reset rejected")
        for gpu in selected:
            gpu["core"] = gpu["core_default"]
    elif mutation == "rmc":
        if "fail_rmc" in modes:
            fail("memory clock reset rejected")
        for gpu in selected:
            gpu["mem"] = gpu["mem_default"]

    if mutation is not None:
        with open(STATE_PATH, "w", encoding="utf-8") as handle:
            json.dump(state, handle)
    if query is None:
        return

    fields = [field.strip() for field in query.split(",")]
    values = {
        "index": lambda gpu: gpu["index"],
        "uuid": lambda gpu: gpu["uuid"],
        "name": lambda gpu: gpu["name"],
        "temperature.gpu": lambda gpu: gpu["temp"],
        "fan.speed": lambda gpu: gpu["fan"],
        "power.draw": lambda gpu: f"{gpu['draw']:.2f}",
        "power.limit": lambda gpu: f"{gpu['power']:.2f}",
        "power.default_limit": lambda gpu: f"{gpu['power_default']:.2f}",
        "power.min_limit": lambda gpu: f"{gpu['power_min']:.2f}",
        "power.max_limit": lambda gpu: f"{gpu['power_max']:.2f}",
        "clocks.sm": lambda gpu: gpu["core"],
        "clocks.mem": lambda gpu: gpu["mem"],
        "clocks.current.sm": lambda gpu: gpu["core"],
        "clocks.current.memory": lambda gpu: gpu["mem"],
        "utilization.gpu": lambda gpu: gpu["util"],
        "memory.used": lambda gpu: gpu["vram"],
        "pci.bus_id": lambda gpu: gpu.get("pci_bus_id", "N/A"),
    }
    for gpu in selected:
        print(
            ", ".join(
                str(values[field](gpu)) if field in values else "N/A"
                for field in fields
            )
        )


if __name__ == "__main__":
    main()
