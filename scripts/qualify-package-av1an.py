"""Exercise owned av1an jobs with external tools and frameserver settings absent."""

import argparse
import importlib.util
import json
import os
from pathlib import Path
import subprocess
import sys
import tempfile

sys.dont_write_bytecode = True
spec = importlib.util.spec_from_file_location("av1an_support", Path(__file__).with_name("package-av1an-support.py"))
support = importlib.util.module_from_spec(spec)
spec.loader.exec_module(support)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--runner", type=Path, required=True)
    parser.add_argument("--resources", type=Path, required=True)
    parser.add_argument("--receipt", type=Path, required=True)
    parser.add_argument("--skip-test", action="append", default=[])
    args = parser.parse_args()
    if sys.platform != "win32":
        raise SystemExit("This gate currently qualifies the Windows portable frameserver.")
    runner, resources = args.runner.resolve(strict=True), args.resources.resolve(strict=True)
    receipt = args.receipt.absolute()
    if receipt.exists() or receipt.with_suffix(".log").exists():
        raise ValueError("The gate receipt must have a fresh destination.")
    tools = resources / "resources/tools"
    runtime = tools / "av1an/python"
    before = support.inventory(runtime)
    with tempfile.TemporaryDirectory(prefix="jesses-portable-av1an-gate-") as temporary:
        env = support.isolated_environment()
        env.update(LOCALAPPDATA=temporary, APPDATA=temporary, PATH=str(support.system32()), JESSES_TEST_TOOL_RESOURCES=str(resources))
        for identifier, relative in {"FFMPEG": "ffmpeg/ffmpeg.exe", "FFPROBE": "ffmpeg/ffprobe.exe", "X264": "x264/x264.exe", "SVT_AV1": "svt-av1/SvtAv1EncApp.exe", "SVT_AV1_5FISH": "svt-av1-5fish/SvtAv1EncApp.exe", "SVT_AV1_HDR": "svt-av1-hdr/SvtAv1EncApp.exe"}.items():
            env["JESSES_" + identifier] = str(tools / relative)
        # These settings must be replaced/removed only for the bundled child.
        # A path-only launcher would try to load the nonexistent frameserver.
        env.update(VSSCRIPT_PATH=str(Path(temporary) / "unavailable-vsscript.dll"), PYTHONHOME=str(Path(temporary) / "unavailable-python"), PYTHONPATH=str(Path(temporary) / "unavailable-modules"), VAPOURSYNTH_EXTRA_PLUGIN_PATH=str(Path(temporary) / "unavailable-plugins"))
        with receipt.with_suffix(".log").open("x", encoding="utf-8") as log:
            command = [str(runner), "--ignored", "--test-threads=1", "--nocapture"]
            for name in args.skip_test:
                command.extend(("--skip", name))
            result = subprocess.run(command, cwd=temporary, env=env, stdout=log, stderr=subprocess.STDOUT, timeout=1800)
    unchanged = before == support.inventory(runtime)
    receipt.write_text(json.dumps({"schemaVersion": 1, "runnerSha256": support.digest(runner), "manifestSha256": support.digest(tools / "manifest.json"), "exitCode": result.returncode, "runtimeUnchanged": unchanged, "runtimeFiles": len(before), "externalPathToolsUnavailable": True, "conflictingFrameserverEnvironment": True, "skippedTests": args.skip_test}, indent=2) + "\n", encoding="utf-8")
    if result.returncode or not unchanged:
        raise SystemExit("The packaged av1an gate failed; see the retained receipt and log.")
    print(f"Portable av1an native jobs passed with unchanged runtime: {receipt}")


if __name__ == "__main__":
    main()
