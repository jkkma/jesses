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


def qualify_video_only(tools, scratch, log):
    scratch.mkdir()
    av1an = tools / "av1an/av1an.exe"
    ffmpeg = tools / "ffmpeg/ffmpeg.exe"
    ffprobe = tools / "ffmpeg/ffprobe.exe"
    frameserver = av1an.parent / "python/Lib/site-packages/vapoursynth"
    env = support.isolated_environment()
    env["PATH"] = os.pathsep.join(map(str, [ffmpeg.parent, tools / "mkvmerge",
                                           tools / "svt-av1", frameserver, support.system32()]))
    env["VSSCRIPT_PATH"] = str(frameserver / "vsscript.dll")

    def run(arguments):
        result = subprocess.run(list(map(str, arguments)), cwd=scratch, env=env,
                                capture_output=True, text=True, errors="replace", timeout=60)
        log.write(f"{arguments!r}\n{result.stdout}{result.stderr}")
        log.flush()
        result.check_returncode()
        return result

    def streams(output):
        return json.loads(run([ffprobe, "-v", "error", "-count_frames", "-show_entries",
                               "stream=codec_type,nb_read_frames", "-of", "json", output]).stdout)["streams"]

    def encode(method, name, audio_params, *, work=None, resume=False):
        output = scratch / f"{name}.mkv"
        detail_log = scratch / f"{name}.log"
        args = [av1an, "-i", source, "-o", output, "--temp", work or scratch / name,
                "--encoder", "svt-av1", "--passes", "1", "--no-defaults", "--concat", method,
                "--chunk-method", "select", "--split-method", "none", "--extra-split", "0",
                "--workers", "1", "--pix-format", "yuv420p", "--keep", "--quiet", "--log-level", "info",
                "--log-file", detail_log,
                "--video-params", "--preset 12 --crf 45 --lp 1", "--audio-params", audio_params]
        if resume:
            args.append("--resume")
        run(args)
        details = detail_log.read_text(encoding="utf-8")
        log.write(details)
        log.flush()
        return output, details

    source = scratch / "source.mkv"
    attachment = scratch / "attachment.txt"
    attachment.write_text("Fixture attachment", encoding="utf-8")
    run([ffmpeg, "-v", "error", "-f", "lavfi", "-i", "testsrc2=size=128x96:rate=8:duration=1",
         "-f", "lavfi", "-i", "sine=duration=1", "-c:v", "ffv1", "-c:a", "pcm_s16le",
         "-attach", attachment, "-metadata:s:t", "mimetype=text/plain", source])
    video_only = "-an -sn -dn -map -0:t?"
    warning = "FFmpeg failed to encode audio"
    for method in ("ffmpeg", "mkvmerge"):
        work = scratch / method
        output, details = encode(method, f"{method}-fresh", video_only, work=work)
        stale_audio = work / "audio.mkv"
        if warning in details or stale_audio.exists():
            raise ValueError(f"{method}: video-only encoding attempted to create audio.mkv.")
        if streams(output) != [{"codec_type": "video", "nb_read_frames": "8"}]:
            raise ValueError(f"{method}: the fresh stage must contain only eight video frames.")
        run([ffmpeg, "-v", "error", "-i", source, "-map", "0:a", "-c", "copy", stale_audio])
        stale_hash = support.digest(stale_audio)
        output, details = encode(method, f"{method}-resumed", video_only, work=work, resume=True)
        if warning in details or "0 remaining" not in details or support.digest(stale_audio) != stale_hash:
            raise ValueError(f"{method}: video-only resume must reuse the queue and ignore stale audio.")
        if streams(output) != [{"codec_type": "video", "nb_read_frames": "8"}]:
            raise ValueError(f"{method}: the resumed stage must contain only eight video frames.")

    output, details = encode("ffmpeg", "requested-audio", "-sn -dn -map -0:t?")
    if warning in details or sorted(stream["codec_type"] for stream in streams(output)) != ["audio", "video"]:
        raise ValueError("Requested audio must still be encoded and concatenated.")
    _, details = encode("ffmpeg", "broken-audio", "-c:a no_such_audio_encoder -sn -dn -map -0:t?")
    if warning not in details or "no_such_audio_encoder" not in details:
        raise ValueError("A genuine audio failure must remain visible.")


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--runner", type=Path, required=True)
    parser.add_argument("--resources", type=Path, required=True)
    parser.add_argument("--receipt", type=Path, required=True)
    parser.add_argument("--skip-test", action="append", default=[])
    parser.add_argument("--verify-video-only", action="store_true")
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
            if args.verify_video_only:
                qualify_video_only(tools, Path(temporary) / "video-only", log)
            command = [str(runner), "--ignored", "--test-threads=1", "--nocapture"]
            for name in args.skip_test:
                command.extend(("--skip", name))
            result = subprocess.run(command, cwd=temporary, env=env, stdout=log, stderr=subprocess.STDOUT, timeout=1800)
    unchanged = before == support.inventory(runtime)
    receipt.write_text(json.dumps({"schemaVersion": 1, "runnerSha256": support.digest(runner), "manifestSha256": support.digest(tools / "manifest.json"), "exitCode": result.returncode, "runtimeUnchanged": unchanged, "runtimeFiles": len(before), "externalPathToolsUnavailable": True, "conflictingFrameserverEnvironment": True, "videoOnlyVerified": args.verify_video_only, "skippedTests": args.skip_test}, indent=2) + "\n", encoding="utf-8")
    if result.returncode or not unchanged:
        raise SystemExit("The packaged av1an gate failed; see the retained receipt and log.")
    print(f"Portable av1an native jobs passed with unchanged runtime: {receipt}")


if __name__ == "__main__":
    main()
