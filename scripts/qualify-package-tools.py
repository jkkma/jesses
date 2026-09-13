"""Run the native discovery probe with PATH tools and managed installs unavailable."""

import argparse
import json
import os
from pathlib import Path
import subprocess
import sys
import tempfile


def comparable_path(path):
    value = str(path.resolve(strict=True))
    if sys.platform == "win32":
        if value.startswith("\\\\?\\UNC\\"):
            value = "\\\\" + value[8:]
        elif value.startswith("\\\\?\\"):
            value = value[4:]
    return Path(value)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--probe", type=Path, required=True)
    parser.add_argument("--resources", type=Path, required=True)
    parser.add_argument("--require-media", action="store_true")
    parser.add_argument("--require-tool", action="append", default=[], choices=["x264", "svt-av1", "av1an"])
    args = parser.parse_args()
    probe = args.probe.resolve(strict=True)
    resources = comparable_path(args.resources)
    with tempfile.TemporaryDirectory(prefix="jesses-package-probe-") as temporary:
        env = {name: value for name, value in os.environ.items() if not name.upper().startswith("JESSES_")}
        env.update(LOCALAPPDATA=temporary, APPDATA=temporary, XDG_DATA_HOME=temporary,
                   XDG_CONFIG_HOME=temporary, XDG_CACHE_HOME=temporary)
        env["PATH"] = str(Path(os.environ["SystemRoot"]) / "System32") if sys.platform == "win32" else temporary
        command = [str(probe), str(resources)]
        if args.require_media:
            command.append("--require-media")
        result = subprocess.run(command, cwd=temporary, env=env, capture_output=True, text=True, timeout=120)
        print(result.stdout, end="")
        if result.returncode:
            print(result.stderr, file=sys.stderr, end="")
            raise SystemExit(result.returncode)
        tools = json.loads(result.stdout)
        expected = {"svt-av1-5fish", "svt-av1-hdr"}
        expected.update(args.require_tool)
        if args.require_media:
            expected.update(("ffmpeg", "ffprobe"))
        found = set()
        for tool in tools:
            if tool["id"] in expected:
                if not tool["available"] or not comparable_path(Path(tool["path"])).is_relative_to(resources):
                    raise ValueError(f"The {tool['id']} capability did not come from this package.")
                found.add(tool["id"])
        if found != expected:
            raise ValueError("The native probe did not report every required package tool.")


if __name__ == "__main__":
    main()
