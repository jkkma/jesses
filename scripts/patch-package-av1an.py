"""Apply pinned timestamp, segment, scorer, probe, metric and video-only fixes."""

import argparse
import difflib
import hashlib
from pathlib import Path
import tarfile

SOURCE_SHA256 = "7f570da8fe0ba5970cbf04d882b48e442e04d1df5ec180e12d9ff974450dfca2"
SOURCE_ROOT = "Av1an-805dad69143fa0a81cfe2fb89c0b9e90a828ea72"
MARKER = "ffmpeg9-passthrough-v1"
PROBE_FILTER_MARKER = "target-probe-filter-v1"
SEGMENT_MARKER = "segment-ffmpeg9-v1"
SIGNED_XPSNR_MARKER = "xpsnr-signed-stats-v1"
SOURCE_PROBE_MARKER = "source-unfiltered-probes-v1"


def patch_source(source, archive_path, patch_path):
    with archive_path.open("rb") as stream:
        if hashlib.file_digest(stream, "sha256").hexdigest() != SOURCE_SHA256:
            raise ValueError("The av1an patch requires the exact pinned upstream archive.")
    concat_path = "av1an-core/src/concat/mod.rs"
    paths = ["av1an-core/src/context.rs", "av1an/src/main.rs", "av1an-core/src/encoder/mod.rs", "av1an-core/src/encoder/tests.rs", "av1an-core/src/vapoursynth.rs", "av1an-core/src/metrics/xpsnr.rs", "av1an-core/src/metrics/vmaf.rs", "av1an-core/src/target_quality.rs", "av1an-core/src/settings.rs", "av1an-core/src/split/mod.rs", concat_path]
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
    version = version.replace(before, '"{}-unstable (rev {}) ({}) [' + MARKER + '] [' + SEGMENT_MARKER + '] [julek-butteraugli-v1] [lsmash-software-probes-v1] [ffmpeg-metric-matrix-v1] [' + PROBE_FILTER_MARKER + '] [' + SIGNED_XPSNR_MARKER + '] [' + SOURCE_PROBE_MARKER + ']\n')
    before = "    pub vmaf_filter: Option<String>,\n"
    after = before + '''
    /// Preserve source-app target probes and references before final filters.
    #[clap(long, help_heading = "Target Quality")]
    pub source_unfiltered_probes: bool,
'''
    if version.count(before) != 1:
        raise ValueError("The pinned CLI reference filter option changed.")
    version = version.replace(before, after)
    before = "            vmaf_filter: self.vmaf_filter.clone(),\n"
    after = before + "            ffmpeg_filter_args: vec![],\n            source_unfiltered_probes: self.source_unfiltered_probes,\n"
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
    # FFmpeg's XPSNR scores can be negative. The pinned engine's unsigned
    # expressions reject a valid negative average and silently skip frames.
    before = r"([0-9\.]+|inf)"
    after = r"(-?[0-9\.]+|-?inf)"
    if xpsnr.count(before) != 7:
        raise ValueError("The pinned XPSNR score parser changed.")
    xpsnr = xpsnr.replace(before, after)
    xpsnr += '''

#[cfg(test)]
mod signed_stats_tests {
    use super::*;
    use std::io::Write;

    #[test]
    fn signed_frame_and_average_scores_are_not_dropped() {
        let mut file = tempfile::NamedTempFile::new().unwrap();
        writeln!(file, "n: 1 XPSNR y: -1.4500 XPSNR u: 0.0152 XPSNR v: -0.8051").unwrap();
        writeln!(file, "n: 2 XPSNR y: 2.0000 XPSNR u: 1.0000 XPSNR v: 0.5000").unwrap();
        writeln!(file, "XPSNR average, 2 frames  y: 0.7021  u: 0.0152  v: -0.8051  (minimum: -0.8051)").unwrap();
        let (minimum, frames) = read_xpsnr_file(file.path(), XPSNRSubMetric::Minimum).unwrap();
        assert_eq!(minimum, -0.8051);
        assert_eq!(frames, [-1.45, 0.5]);
        let (weighted, frames) = read_xpsnr_file(file.path(), XPSNRSubMetric::Weighted).unwrap();
        assert_eq!(weighted, weight_xpsnr(0.7021, 0.0152, -0.8051));
        assert_eq!(frames.len(), 2);
        assert!((frames[0] - (-1.45 * 4.0 + 0.0152 - 0.8051) / 6.0).abs() < 1e-12);
    }
}
'''
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
    after = before + "    #[serde(default)]\n    pub ffmpeg_filter_args:    Vec<String>,\n    #[serde(default)]\n    pub source_unfiltered_probes: bool,\n"
    if target_quality.count(before) != 1:
        raise ValueError("The pinned target-quality filter fields changed.")
    target_quality = target_quality.replace(before, after)
    before = "            vmaf_filter: None,\n"
    after = before + "            ffmpeg_filter_args: vec![],\n            source_unfiltered_probes: false,\n"
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
    target_quality = target_quality.replace(before, after)
    before = "impl TargetQuality {\n"
    after = before + '''    /// Source-app compatibility: final filters must not leak into probes.
    pub fn set_final_filters(&mut self, filters: &[String]) {
        if self.source_unfiltered_probes {
            self.ffmpeg_filter_args.clear();
            self.vmaf_filter = None;
        } else {
            self.ffmpeg_filter_args.clone_from(&filters.to_vec());
        }
    }

'''
    if target_quality.count(before) != 1:
        raise ValueError("The pinned target-quality implementation changed.")
    target_quality = target_quality.replace(before, after)
    target_quality += '''

#[cfg(test)]
mod source_probe_filter_tests {
    use super::*;

    #[test]
    fn source_mode_omits_final_transforms_and_legacy_mode_retains_them() {
        let filters = vec!["-vf".to_owned(), "crop=160:96,hue=s=0".to_owned()];
        let mut quality = TargetQuality::default("temp", crate::encoder::Encoder::svt_av1);
        quality.vmaf_filter = Some("crop=160:96,hue=s=0".to_owned());
        quality.set_final_filters(&filters);
        assert_eq!(quality.ffmpeg_filter_args, filters);
        assert!(quality.vmaf_filter.is_some());
        quality.source_unfiltered_probes = true;
        quality.set_final_filters(&filters);
        assert!(quality.ffmpeg_filter_args.is_empty());
        assert!(quality.vmaf_filter.is_none());
        assert_eq!(serde_json::to_value(&quality).unwrap()["source_unfiltered_probes"], true);
    }
}
'''
    changed[paths[7]] = target_quality
    settings = original[paths[8]]
    before = """        if self.target_quality.target.is_some() {
            match self.target_quality.metric {
"""
    after = """        if self.target_quality.target.is_some() {
            // Persist the actual final-chunk FFmpeg input transforms with the
            // target-quality plan so fresh and resumed probes use them once.
            self.target_quality.set_final_filters(&self.ffmpeg_filter_args);
            match self.target_quality.metric {
"""
    if settings.count(before) != 1:
        raise ValueError("The pinned target-quality validation changed.")
    changed[paths[8]] = settings.replace(before, after)
    # This pinned source already fixed Segment's FFmpeg 9 invocation. The
    # marker promises that the build retains it; fail if a future pin drifts.
    segment = original[paths[9]]
    if '"-vsync"' in segment or segment.count('        "-fps_mode",\n        "passthrough",') != 1:
        raise ValueError("The pinned source segment FFmpeg 9 behavior is absent.")
    context = changed[paths[0]]
    for before, after in [
        ("""        crossbeam_utils::thread::scope(|s| -> anyhow::Result<()> {
            // vapoursynth audio is currently unsupported
            let audio_thread = (self.args.input.is_video()
""", """        // Jesses applies the audio and subtitle recipe after the video join.
        // Its existing flags remove every stream that could enter audio.mkv.
        let include_audio_sidecar = !self.args.audio_params.iter().map(String::as_str)
            .eq(["-an", "-sn", "-dn", "-map", "-0:t?"]);
        crossbeam_utils::thread::scope(|s| -> anyhow::Result<()> {
            // vapoursynth audio is currently unsupported
            let audio_thread = (include_audio_sidecar
                && self.args.input.is_video()
"""),
        ("""            // TODO add explicit parameter to concatenation functions to control whether
            // audio is also muxed in
""", ""),
        ("""                        self.args.encoder,
                        total_chunks,
                        if self.args.ignore_frame_mismatch {
""", """                        self.args.encoder,
                        total_chunks,
                        include_audio_sidecar,
                        if self.args.ignore_frame_mismatch {
"""),
        ("""                    concat::ffmpeg(self.args.temp.as_ref(), self.args.output_file.as_ref())?;
""", """                    concat::ffmpeg(
                        self.args.temp.as_ref(),
                        self.args.output_file.as_ref(),
                        include_audio_sidecar,
                    )?;
"""),
    ]:
        if context.count(before) != 1:
            raise ValueError("The pinned video-only encode and concat call sites changed.")
        context = context.replace(before, after)
    changed[paths[0]] = context

    concat = original[concat_path]
    for before, after in [
        ("""    num_chunks: usize,
    output_fps: Option<Rational64>,
""", """    num_chunks: usize,
    include_audio_sidecar: bool,
    output_fps: Option<Rational64>,
"""),
        ("""    let audio_file = PathBuf::from(&temp_dir).join("audio.mkv");
    let audio_file = PathAbs::new(&audio_file)?;
    let audio_file = audio_file.as_path().exists().then(|| fix_path(audio_file));
""", """    let audio_file = if include_audio_sidecar {
        let file = PathAbs::new(PathBuf::from(&temp_dir).join("audio.mkv"))?;
        file.as_path().exists().then(|| fix_path(file))
    } else {
        None
    };
"""),
        ("""pub fn ffmpeg(temp: &Path, output: &Path) -> anyhow::Result<()> {
""", """pub fn ffmpeg(temp: &Path, output: &Path, include_audio_sidecar: bool) -> anyhow::Result<()> {
"""),
        ("""    let audio_file = {
        let file = temp.join("audio.mkv");
        (file.exists() && file.metadata().expect("file should have metadata").len() > 1000)
            .then_some(file)
    };
""", """    let audio_file = if include_audio_sidecar {
        let file = temp.join("audio.mkv");
        (file.exists() && file.metadata().expect("file should have metadata").len() > 1000)
            .then_some(file)
    } else {
        None
    };
"""),
    ]:
        if concat.count(before) != 1:
            raise ValueError("The pinned FFmpeg or mkvmerge audio sidecar handling changed.")
        concat = concat.replace(before, after)
    changed[concat_path] = concat
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
