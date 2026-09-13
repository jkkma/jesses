"""Apply retained timestamp, scorer API, software-probe and metric-tag fixes."""

import argparse
import difflib
import hashlib
from pathlib import Path
import tarfile

SOURCE_SHA256 = "7f570da8fe0ba5970cbf04d882b48e442e04d1df5ec180e12d9ff974450dfca2"
SOURCE_ROOT = "Av1an-805dad69143fa0a81cfe2fb89c0b9e90a828ea72"
MARKER = "ffmpeg9-passthrough-v1"


def patch_source(source, archive_path, patch_path):
    with archive_path.open("rb") as stream:
        if hashlib.file_digest(stream, "sha256").hexdigest() != SOURCE_SHA256:
            raise ValueError("The av1an patch requires the exact pinned upstream archive.")
    paths = ["av1an-core/src/context.rs", "av1an/src/main.rs", "av1an-core/src/encoder/mod.rs", "av1an-core/src/vapoursynth.rs", "av1an-core/src/metrics/xpsnr.rs", "av1an-core/src/metrics/vmaf.rs"]
    original = {}
    with tarfile.open(archive_path) as archive:
        for path in paths:
            original[path] = archive.extractfile(SOURCE_ROOT + "/" + path).read().decode("utf-8")
            if (source / path).read_text(encoding="utf-8") != original[path]:
                raise ValueError(f"The av1an source changed before the compatibility patch: {path}")
    # The pinned revision already corrected sampled quality probes. Keep this
    # condition part of the marker contract rather than advertising an old build.
    if '"-vsync"' in original[paths[2]] or '"-fps_mode".to_string(),\n                "passthrough".to_string(),' not in original[paths[2]]:
        raise ValueError("The pinned sampled-probe timestamp behavior is absent.")
    context = original[paths[0]]
    start = context.index("    fn create_select_chunk(")
    end = context.index("        let output_ext", start)
    region = context[start:end]
    before = 'r"select=between(n\\,{start}\\,{end})"'
    after = 'r"select=between(n\\,{start}\\,{end}),setpts=PTS-STARTPTS"'
    if region.count(before) != 1 or region.count('            "-f",') != 1:
        raise ValueError("The select generator changed; inspect the patch before applying it.")
    region = region.replace(before, after).replace('            "-f",', '            "-fps_mode",\n            "passthrough",\n            "-f",')
    changed = {paths[0]: context[:start] + region + context[end:]}
    version = original[paths[1]]
    before = '"{}-unstable (rev {}) ({})\n'
    if version.count(before) != 1:
        raise ValueError("The pinned av1an version banner changed.")
    changed[paths[1]] = version.replace(before, '"{}-unstable (rev {}) ({}) [' + MARKER + '] [julek-butteraugli-v1] [lsmash-software-probes-v1] [ffmpeg-metric-matrix-v1]\n')
    scorer = original[paths[3]]
    start = scorer.index("fn compare_butteraugli")
    end = scorer.index("fn compare_xpsnr", start)
    region = scorer[start:end]
    if region.count('"butteraugli"') != 1:
        raise ValueError("The pinned Julek Butteraugli invocation changed.")
    changed[paths[3]] = scorer[:start] + region.replace('"butteraugli"', '"Butteraugli"') + scorer[end:]
    scorer = changed[paths[3]]
    start = scorer.index("fn import_lsmash")
    end = scorer.index("fn import_ffms2", start)
    region = scorer[start:end]
    before = '// Allow hardware acceleration, falls back to software decoding.\n    arguments.set_int("prefer_hw", 3)?;'
    if region.count(before) != 1:
        raise ValueError("The pinned L-SMASH probe import changed.")
    after = '// Decode metric probes in software for deterministic portable operation.\n    arguments.set_int("prefer_hw", 0)?;'
    changed[paths[3]] = scorer[:start] + region.replace(before, after) + scorer[end:]
    xpsnr = original[paths[4]]
    for before, after in [
        ('[0:v]scale=', '[0:v]setparams=colorspace=unknown,scale='),
        ('[1:v]{filter}scale=', '[1:v]{filter}setparams=colorspace=unknown,scale='),
    ]:
        if xpsnr.count(before) != 1:
            raise ValueError("The pinned XPSNR metric scale inputs changed.")
        xpsnr = xpsnr.replace(before, after)
    changed[paths[4]] = xpsnr
    vmaf = original[paths[5]]
    start = vmaf.index("pub fn run_vmaf(")
    end = vmaf.index("pub fn run_vmaf_weighted(", start)
    region = vmaf[start:end]
    for before, after in [
        ('[0:v]scale=', '[0:v]setparams=colorspace=unknown,scale='),
        ('[1:v]{}scale=', '[1:v]{}setparams=colorspace=unknown,scale='),
    ]:
        if region.count(before) != 1:
            raise ValueError("The pinned VMAF metric scale inputs changed.")
        region = region.replace(before, after)
    changed[paths[5]] = vmaf[:start] + region + vmaf[end:]
    patch_text = "".join("".join(difflib.unified_diff(original[path].splitlines(keepends=True), value.splitlines(keepends=True), fromfile="a/" + path, tofile="b/" + path)) for path, value in changed.items())
    with patch_path.open("x", encoding="utf-8", newline="\n") as output:
        output.write(patch_text)
    for path, value in changed.items():
        (source / path).write_text(value, encoding="utf-8", newline="\n")


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--source", type=Path, required=True)
    parser.add_argument("--archive", type=Path, required=True)
    parser.add_argument("--patch-output", type=Path, required=True)
    args = parser.parse_args()
    patch_source(args.source.resolve(strict=True), args.archive.resolve(strict=True), args.patch_output.absolute())


if __name__ == "__main__":
    main()
