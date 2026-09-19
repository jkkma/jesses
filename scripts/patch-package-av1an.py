"""Apply retained timestamp, scorer API, software-probe and metric-tag fixes."""

import argparse
import difflib
import hashlib
from pathlib import Path
import tarfile

SOURCE_SHA256 = "7f570da8fe0ba5970cbf04d882b48e442e04d1df5ec180e12d9ff974450dfca2"
SOURCE_ROOT = "Av1an-805dad69143fa0a81cfe2fb89c0b9e90a828ea72"
MARKER = "ffmpeg9-passthrough-v1"
PROBE_FILTER_MARKER = "target-probe-filter-v1"


def patch_source(source, archive_path, patch_path):
    with archive_path.open("rb") as stream:
        if hashlib.file_digest(stream, "sha256").hexdigest() != SOURCE_SHA256:
            raise ValueError("The av1an patch requires the exact pinned upstream archive.")
    paths = ["av1an-core/src/context.rs", "av1an/src/main.rs", "av1an-core/src/encoder/mod.rs", "av1an-core/src/encoder/tests.rs", "av1an-core/src/vapoursynth.rs", "av1an-core/src/metrics/xpsnr.rs", "av1an-core/src/metrics/vmaf.rs", "av1an-core/src/target_quality.rs", "av1an-core/src/settings.rs"]
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
    version = version.replace(before, '"{}-unstable (rev {}) ({}) [' + MARKER + '] [julek-butteraugli-v1] [lsmash-software-probes-v1] [ffmpeg-metric-matrix-v1] [' + PROBE_FILTER_MARKER + ']\n')
    before = "            vmaf_filter: self.vmaf_filter.clone(),\n"
    after = before + "            ffmpeg_filter_args: vec![],\n"
    if version.count(before) != 1:
        raise ValueError("The pinned CLI target-quality initializer changed.")
    changed[paths[1]] = version.replace(before, after)
    encoder = original[paths[2]]
    before = """        custom_video_params: Option<Vec<String>>,
    ) -> (Option<Vec<String>>, Vec<Cow<'static, str>>) {
        let filters = if probing_rate > 1 {
            vec![
                \"-vf\".to_string(),
                format!(\"select=not(mod(n\\\\,{probing_rate}))\"),
                \"-fps_mode\".to_string(),
                \"passthrough\".to_string(),
            ]
        } else {
            Vec::new()
        };
"""
    after = """        custom_video_params: Option<Vec<String>>,
        ffmpeg_filter_args: &[String],
    ) -> (Option<Vec<String>>, Vec<Cow<'static, str>>) {
        // Target probes must see the same encoder-input transforms as final
        // chunks. The scoring-only vmaf_filter remains a separate reference
        // transform and must not be substituted for these actual FFmpeg args.
        let mut filters = ffmpeg_filter_args.to_vec();
        if probing_rate > 1 {
            let sampling = format!(\"select=not(mod(n\\\\,{probing_rate}))\");
            if let Some(position) = filters.iter().position(|arg| arg == \"-vf\") {
                if let Some(graph) = filters.get_mut(position + 1) {
                    graph.push_str(&format!(\",{sampling}\"));
                } else {
                    filters.extend([\"-vf\".to_string(), sampling]);
                }
            } else {
                filters.extend([\"-vf\".to_string(), sampling]);
            }
            filters.extend([\"-fps_mode\".to_string(), \"passthrough\".to_string()]);
        }
"""
    if encoder.count(before) != 1:
        raise ValueError("The pinned target-quality probe pipe changed.")
    changed[paths[2]] = encoder.replace(before, after)
    encoder_tests = original[paths[3]]
    before = """fn ffmpeg_probe_uses_fps_mode_passthrough() {
    let (pipe, _) = Encoder::svt_av1.probe_cmd(
        \"temp\".to_owned(),
        0,
        30.0,
        FFPixelFormat::YUV420P10LE,
        2,
        1,
        None,
    );
    let pipe = pipe.expect(\"FFmpeg pipe should be present\");

    assert!(pipe.windows(2).any(|args| args[0] == \"-fps_mode\" && args[1] == \"passthrough\"));
    assert!(!pipe.iter().any(|arg| arg == \"-vsync\"));
}
"""
    after = """fn ffmpeg_probe_uses_fps_mode_passthrough() {
    let transform = vec![\"-vf\".to_string(), \"crop=304:168:8:8,scale=160:88\".to_string()];
    let (pipe, _) = Encoder::svt_av1.probe_cmd(
        \"temp\".to_owned(),
        0,
        30.0,
        FFPixelFormat::YUV420P10LE,
        2,
        1,
        None,
        &transform,
    );
    let pipe = pipe.expect(\"FFmpeg pipe should be present\");

    assert!(pipe.windows(2).any(|args| args[0] == \"-fps_mode\" && args[1] == \"passthrough\"));
    assert!(!pipe.iter().any(|arg| arg == \"-vsync\"));
    assert_eq!(pipe.iter().filter(|arg| arg.as_str() == \"-vf\").count(), 1);
    assert!(pipe.windows(2).any(|args| {
        args[0] == \"-vf\"
            && args[1]
                == \"crop=304:168:8:8,scale=160:88,select=not(mod(n\\\\,2))\"
    }));

    let (unsampled, _) = Encoder::svt_av1.probe_cmd(
        \"temp\".to_owned(),
        0,
        30.0,
        FFPixelFormat::YUV420P10LE,
        1,
        1,
        None,
        &transform,
    );
    let unsampled = unsampled.expect(\"FFmpeg pipe should be present\");
    assert_eq!(unsampled.iter().filter(|arg| arg.as_str() == \"-vf\").count(), 1);
    assert!(unsampled.windows(2).any(|args| {
        args[0] == \"-vf\" && args[1] == \"crop=304:168:8:8,scale=160:88\"
    }));
}
"""
    if encoder_tests.count(before) != 1:
        raise ValueError("The pinned FFmpeg target-probe test changed.")
    changed[paths[3]] = encoder_tests.replace(before, after)
    scorer = original[paths[4]]
    start = scorer.index("fn compare_butteraugli")
    end = scorer.index("fn compare_xpsnr", start)
    region = scorer[start:end]
    if region.count('"butteraugli"') != 1:
        raise ValueError("The pinned Julek Butteraugli invocation changed.")
    changed[paths[4]] = scorer[:start] + region.replace('"butteraugli"', '"Butteraugli"') + scorer[end:]
    scorer = changed[paths[4]]
    start = scorer.index("fn import_lsmash")
    end = scorer.index("fn import_ffms2", start)
    region = scorer[start:end]
    before = '// Allow hardware acceleration, falls back to software decoding.\n    arguments.set_int("prefer_hw", 3)?;'
    if region.count(before) != 1:
        raise ValueError("The pinned L-SMASH probe import changed.")
    after = '// Decode metric probes in software for deterministic portable operation.\n    arguments.set_int("prefer_hw", 0)?;'
    changed[paths[4]] = scorer[:start] + region.replace(before, after) + scorer[end:]
    xpsnr = original[paths[5]]
    for before, after in [
        ('[0:v]scale=', '[0:v]setparams=colorspace=unknown,scale='),
        ('[1:v]{filter}scale=', '[1:v]{filter}setparams=colorspace=unknown,scale='),
    ]:
        if xpsnr.count(before) != 1:
            raise ValueError("The pinned XPSNR metric scale inputs changed.")
        xpsnr = xpsnr.replace(before, after)
    changed[paths[5]] = xpsnr
    vmaf = original[paths[6]]
    start = vmaf.index("pub fn run_vmaf(")
    end = vmaf.index("pub fn run_vmaf_weighted(", start)
    region = vmaf[start:end]
    before = """    let mut filter = if sample_rate > 1 {
        format!(
            \"select=not(mod(n\\\\,{})),setpts={:.4}*PTS,\",
            sample_rate,
            1.0 / sample_rate as f64,
        )
    } else {
        String::new()
    };

    if let Some(vmaf_filter) = vmaf_filter {
        filter.reserve(1 + vmaf_filter.len());
        filter.push_str(vmaf_filter);
        filter.push(',');
    }
"""
    after = """    let mut filter = String::new();
    if let Some(vmaf_filter) = vmaf_filter {
        filter.reserve(1 + vmaf_filter.len());
        filter.push_str(vmaf_filter);
        filter.push(',');
    }
    if sample_rate > 1 {
        filter.push_str(&format!(
            \"select=not(mod(n\\\\,{})),setpts={:.4}*PTS,\",
            sample_rate,
            1.0 / sample_rate as f64,
        ));
    }
"""
    if region.count(before) != 1:
        raise ValueError("The pinned VMAF reference sampling order changed.")
    region = region.replace(before, after)
    for before, after in [
        ('[0:v]scale=', '[0:v]setparams=colorspace=unknown,scale='),
        ('[1:v]{}scale=', '[1:v]{}setparams=colorspace=unknown,scale='),
    ]:
        if region.count(before) != 1:
            raise ValueError("The pinned VMAF metric scale inputs changed.")
        region = region.replace(before, after)
    changed[paths[6]] = vmaf[:start] + region + vmaf[end:]
    target_quality = original[paths[7]]
    before = "    pub vmaf_filter:           Option<String>,\n"
    after = before + "    #[serde(default)]\n    pub ffmpeg_filter_args:    Vec<String>,\n"
    if target_quality.count(before) != 1:
        raise ValueError("The pinned target-quality filter fields changed.")
    target_quality = target_quality.replace(before, after)
    before = "            vmaf_filter: None,\n"
    after = before + "            ffmpeg_filter_args: vec![],\n"
    if target_quality.count(before) != 1:
        raise ValueError("The pinned target-quality defaults changed.")
    target_quality = target_quality.replace(before, after)
    before = """            self.video_params.clone(),
        );
"""
    after = """            self.video_params.clone(),
            &self.ffmpeg_filter_args,
        );
"""
    if target_quality.count(before) != 1:
        raise ValueError("The pinned target-quality probe call changed.")
    changed[paths[7]] = target_quality.replace(before, after)
    settings = original[paths[8]]
    before = """        if self.target_quality.target.is_some() {
            match self.target_quality.metric {
"""
    after = """        if self.target_quality.target.is_some() {
            // Persist the actual final-chunk FFmpeg input transforms with the
            // target-quality plan so fresh and resumed probes use them once.
            self.target_quality
                .ffmpeg_filter_args
                .clone_from(&self.ffmpeg_filter_args);
            match self.target_quality.metric {
"""
    if settings.count(before) != 1:
        raise ValueError("The pinned target-quality validation changed.")
    changed[paths[8]] = settings.replace(before, after)
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
