"""Verify compiled Vulkan/libplacebo Spline support; optionally render on a device.

Package creation requires compiled processing support, not a local GPU. Device
qualification is explicit and records whether the device is physical or CPU.
"""

import argparse
import hashlib
import json
from pathlib import Path
import re
import subprocess


def digest(path):
    with path.open("rb") as stream:
        return hashlib.file_digest(stream, "sha256").hexdigest()


def qualify(ffmpeg, exercise=False, require_physical=False):
    ffmpeg = Path(ffmpeg).resolve(strict=True)

    def run(arguments):
        result = subprocess.run([str(ffmpeg), "-hide_banner", "-nostdin", *arguments], capture_output=True, timeout=60)
        if result.returncode:
            raise ValueError("The Vulkan processing probe failed: " + result.stderr.decode(errors="replace")[-8192:])
        return result

    version = run(["-version"]).stdout.decode(errors="replace")
    help_text = run(["-h", "filter=libplacebo"]).stdout.decode(errors="replace")
    devices = run(["-hwaccels"]).stdout.decode(errors="replace").splitlines()
    if not all(flag in version for flag in ("--enable-vulkan", "--enable-libplacebo")):
        raise ValueError("FFmpeg must explicitly enable Vulkan and libplacebo under the pinned recipe.")
    if "vulkan" not in [line.strip() for line in devices] or not all(name in help_text for name in ("tonemapping", "spline", "peak_detect", "apply_dolbyvision")):
        raise ValueError("The compiled Vulkan/libplacebo Spline/peak/Dolby processing capability is incomplete.")
    report = {"schemaVersion": 1, "processingPolicy": "vulkan-libplacebo", "ffmpegSha256": digest(ffmpeg),
              "version": version.splitlines()[0], "vulkan": True, "libplacebo": True,
              "spline": True, "peakDetection": True, "applyDolbyVision": True,
              "runtimeExercised": False}
    if exercise or require_physical:
        filter_graph = ("format=yuv420p10le,setparams=field_mode=prog:range=tv:color_primaries=bt2020:"
                        "color_trc=smpte2084:colorspace=bt2020nc,libplacebo=tonemapping=spline:peak_detect=1:"
                        "apply_dolbyvision=0:format=yuv420p10le:colorspace=bt709:color_primaries=bt709:color_trc=bt709:range=tv")
        result = run(["-v", "verbose", "-init_hw_device", "vulkan", "-f", "lavfi", "-i", "color=black:s=64x64:r=1",
                      "-vf", filter_graph, "-frames:v", "1", "-pix_fmt", "yuv420p10le", "-f", "rawvideo", "pipe:1"])
        selected = [line for line in result.stderr.decode(errors="replace").splitlines() if " selected: " in line]
        if len(selected) != 1 or len(result.stdout) != 64 * 64 * 3:
            raise ValueError("Vulkan did not identify one device and render one complete 10-bit frame.")
        physical = bool(re.search(r"\((?:integrated|discrete)\)", selected[0]))
        if require_physical and not physical:
            raise ValueError("A physical Vulkan GPU was required; selected " + selected[0])
        report.update(runtimeExercised=True, physicalGpu=physical, selectedDevice=selected[0],
                      outputBytes=len(result.stdout), pixelsSha256=hashlib.sha256(result.stdout).hexdigest())
    return report


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--ffmpeg", type=Path, required=True)
    parser.add_argument("--output", type=Path, required=True)
    parser.add_argument("--exercise", action="store_true")
    parser.add_argument("--require-physical", action="store_true")
    args = parser.parse_args()
    args.output.write_text(json.dumps(qualify(args.ffmpeg, args.exercise, args.require_physical), indent=2) + "\n")


if __name__ == "__main__":
    main()
