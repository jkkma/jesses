use media_core::{
    ColorMetadataTransferRequest, ConcatRequest, CrfLadderRequest, GrainRequest, GrainSource,
    KeyframeCutRequest, LadderEncoder, LadderMetric, SubtitleOcrRequest, UtilityRequest,
    UtilityResult,
};
use std::{
    fs,
    path::{Path, PathBuf},
    process::Command,
    time::{SystemTime, UNIX_EPOCH},
};

struct Fixture(PathBuf);

impl Fixture {
    fn new() -> Self {
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let path = std::env::temp_dir().join(format!(
            "jesses-utility-validation-{}-{nonce}",
            std::process::id()
        ));
        fs::create_dir(&path).unwrap();
        Self(path)
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        let resolved = self.0.canonicalize().ok();
        if resolved.as_ref().is_some_and(|path| {
            path.starts_with(std::env::temp_dir())
                && path.file_name().is_some_and(|name| {
                    name.to_string_lossy()
                        .starts_with("jesses-utility-validation-")
                })
        }) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }
}

fn run(program: &str, args: &[&str]) {
    let executable = if cfg!(windows) && program == "seconv" {
        std::env::var_os("LOCALAPPDATA")
            .map(PathBuf::from)
            .map(|root| root.join("jesses/tools/subtitle-ocr/seconv/seconv.exe"))
            .filter(|path| path.is_file())
            .unwrap_or_else(|| PathBuf::from(program))
    } else {
        PathBuf::from(program)
    };
    let output = Command::new(&executable).args(args).output().unwrap();
    assert!(
        output.status.success(),
        "{program} failed: {}\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}

#[tokio::test]
#[ignore = "requires FFmpeg xpsnr, libx264, and the installed CPU VapourSynth vszip/julek scorers"]
async fn crf_ladder_scores_all_new_metrics_with_expected_direction() {
    let fixture = Fixture::new();
    let source = fixture.0.join("ladder-scorer-source.mkv");
    run(
        "ffmpeg",
        &[
            "-hide_banner",
            "-v",
            "error",
            "-nostdin",
            "-n",
            "-f",
            "lavfi",
            "-i",
            "testsrc2=s=128x96:r=24",
            "-frames:v",
            "8",
            "-vf",
            "format=yuv420p10le,setparams=field_mode=prog:range=tv:color_primaries=bt709:color_trc=bt709:colorspace=bt709",
            "-c:v",
            "ffv1",
            "-level",
            "3",
            "-color_range",
            "tv",
            "-colorspace",
            "bt709",
            "-color_primaries",
            "bt709",
            "-color_trc",
            "bt709",
            "-chroma_sample_location",
            "left",
            &path(&source),
        ],
    );
    let original = fs::read(&source).unwrap();
    let (_owner, cancel) = tokio::sync::watch::channel(false);
    for (metric, lower_is_better) in [
        (LadderMetric::Ssimulacra2, false),
        (LadderMetric::ButteraugliInf, true),
        (LadderMetric::XpsnrWeighted, false),
    ] {
        let result = media_runtime::run_utility(
            UtilityRequest::CrfLadder(CrfLadderRequest {
                content_preset: None,
                parameters: Vec::new(),
                vmaf_model: None,
                keep_samples_directory: None,
                input_path: path(&source),
                video_stream_index: 0,
                encoder: LadderEncoder::H264,
                preset: "ultrafast".into(),
                pixel_format: "yuv420p10le".into(),
                crfs: vec![18, 42],
                sample_count: 1,
                sample_seconds: 1.0,
                metric,
                recommendation_threshold: None,
            }),
            cancel.clone(),
        )
        .await
        .unwrap();
        let UtilityResult::CrfLadder(result) = result else {
            panic!("expected ladder result")
        };
        let first = result.rungs[0].score.expect("low CRF metric score");
        let second = result.rungs[1].score.expect("high CRF metric score");
        assert!(
            first.is_finite() && second.is_finite(),
            "{metric:?}: {first} {second}"
        );
        assert!(
            if lower_is_better {
                first < second
            } else {
                first > second
            },
            "{metric:?}: {first} {second}"
        );
        assert_eq!(
            result.recommendation_threshold,
            match metric {
                LadderMetric::Ssimulacra2 => Some(80.0),
                LadderMetric::ButteraugliInf => Some(4.0),
                LadderMetric::XpsnrWeighted => None,
                _ => unreachable!(),
            }
        );
        if metric == LadderMetric::XpsnrWeighted {
            assert_eq!(result.recommended_crf, None);
        }
        assert_eq!(fs::read(&source).unwrap(), original);
    }
}

fn capture(program: &str, args: &[&str]) -> String {
    let output = Command::new(program).args(args).output().unwrap();
    assert!(
        output.status.success(),
        "{program} failed: {}\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8(output.stdout).unwrap()
}

fn first_packet_pts(path_value: &Path, selector: &str) -> f64 {
    capture(
        "ffprobe",
        &[
            "-v",
            "error",
            "-select_streams",
            selector,
            "-read_intervals",
            "%+#1",
            "-show_packets",
            "-show_entries",
            "packet=pts_time",
            "-of",
            "csv=p=0",
            &path(path_value),
        ],
    )
    .lines()
    .find_map(|line| line.trim().trim_end_matches(',').parse().ok())
    .unwrap()
}

fn packet_payload_hash(path_value: &Path, stream_index: u32) -> String {
    capture(
        "ffmpeg",
        &[
            "-hide_banner",
            "-v",
            "error",
            "-nostdin",
            "-i",
            &path(path_value),
            "-map",
            &format!("0:{stream_index}"),
            "-c",
            "copy",
            "-f",
            "hash",
            "-hash",
            "sha256",
            "-",
        ],
    )
    .trim()
    .to_owned()
}

fn decoded_video_frames(path_value: &Path) -> u64 {
    capture(
        "ffprobe",
        &[
            "-v",
            "error",
            "-select_streams",
            "v:0",
            "-count_frames",
            "-show_entries",
            "stream=nb_read_frames",
            "-of",
            "default=nw=1:nk=1",
            &path(path_value),
        ],
    )
    .trim()
    .parse()
    .unwrap()
}

fn stream_count(path_value: &Path) -> usize {
    capture(
        "ffprobe",
        &[
            "-v",
            "error",
            "-show_entries",
            "stream=index",
            "-of",
            "csv=p=0",
            &path(path_value),
        ],
    )
    .lines()
    .filter(|line| !line.trim().is_empty())
    .count()
}

fn path(path: &Path) -> String {
    path.to_string_lossy().into_owned()
}

fn make_ffv1(path_value: &Path, matrix: &str, range: &str, seconds: &str) {
    run(
        "ffmpeg",
        &[
            "-hide_banner",
            "-v",
            "error",
            "-y",
            "-f",
            "lavfi",
            "-i",
            &format!("testsrc2=size=128x96:rate=24:duration={seconds}"),
            "-f",
            "lavfi",
            "-i",
            &format!("sine=frequency=440:sample_rate=48000:duration={seconds}"),
            "-map",
            "0:v",
            "-map",
            "1:a",
            "-c:v",
            "ffv1",
            "-level",
            "3",
            "-pix_fmt",
            "yuv420p",
            "-vf",
            &format!(
                "setsar=1,setparams=field_mode=prog:range={range}:color_primaries={matrix}:color_trc={matrix}:colorspace={matrix}"
            ),
            "-chroma_sample_location",
            "left",
            "-c:a",
            "flac",
            &path(path_value),
        ],
    );
}

#[tokio::test]
#[ignore = "requires FFmpeg, FFprobe, MKVToolNix, grav1synth, seconv, Tesseract, and eng.traineddata"]
async fn real_utility_routes_publish_validated_outputs_without_touching_sources() {
    let fixture = Fixture::new();
    let first = fixture.0.join("first.mkv");
    let second = fixture.0.join("second.mkv");
    let color_source = fixture.0.join("color-source.mkv");
    make_ffv1(&first, "bt709", "tv", "1");
    make_ffv1(&second, "bt709", "tv", "1");
    make_ffv1(&color_source, "smpte170m", "pc", "1");
    let original_first = fs::read(&first).unwrap();
    let original_second = fs::read(&second).unwrap();
    let (_owner, cancel) = tokio::sync::watch::channel(false);
    let capabilities = media_runtime::inspect_utility_capabilities(cancel.clone())
        .await
        .unwrap();
    for id in ["seconv", "tesseract"] {
        let dependency = capabilities
            .dependencies
            .iter()
            .find(|dependency| dependency.id == id)
            .unwrap();
        assert!(dependency.available, "{id}: {}", dependency.detail);
        eprintln!(
            "{id}: {}",
            dependency.path.as_deref().unwrap_or("<missing>")
        );
    }

    let concatenated = fixture.0.join("concatenated.mkv");
    let result = media_runtime::run_utility(
        UtilityRequest::Concat(ConcatRequest {
            input_paths: vec![path(&first), path(&second)],
            output_path: path(&concatenated),
        }),
        cancel.clone(),
    )
    .await
    .unwrap();
    assert!(matches!(result, UtilityResult::Artifact(_)));

    let b_frame_source = fixture.0.join("b-frame-source.mkv");
    run(
        "ffmpeg",
        &[
            "-hide_banner",
            "-v",
            "error",
            "-y",
            "-f",
            "lavfi",
            "-i",
            "testsrc2=size=128x96:rate=24:duration=2",
            "-itsoffset",
            "0.125",
            "-f",
            "lavfi",
            "-i",
            "sine=frequency=880:sample_rate=48000:duration=1.875",
            "-map",
            "0:v",
            "-map",
            "1:a",
            "-c:v",
            "libx264",
            "-preset",
            "fast",
            "-g",
            "24",
            "-bf",
            "3",
            "-c:a",
            "flac",
            &path(&b_frame_source),
        ],
    );
    let original_b_frame_source = fs::read(&b_frame_source).unwrap();
    let b_frame_cut = fixture.0.join("b-frame-cut.mkv");
    let result = media_runtime::run_utility(
        UtilityRequest::KeyframeCut(KeyframeCutRequest {
            input_path: path(&b_frame_source),
            output_path: path(&b_frame_cut),
            start_seconds: 0.0,
            end_seconds: 1.5,
        }),
        cancel.clone(),
    )
    .await
    .unwrap();
    let UtilityResult::Artifact(b_frame_artifact) = result else {
        panic!("expected B-frame cut artifact")
    };
    assert!(b_frame_artifact.duration_seconds.unwrap() >= 1.45);
    let source_offset =
        first_packet_pts(&b_frame_source, "a:0") - first_packet_pts(&b_frame_source, "v:0");
    let output_offset =
        first_packet_pts(&b_frame_cut, "a:0") - first_packet_pts(&b_frame_cut, "v:0");
    assert!((source_offset - output_offset).abs() <= 0.03);

    let cut = fixture.0.join("cut.mkv");
    let result = media_runtime::run_utility(
        UtilityRequest::KeyframeCut(KeyframeCutRequest {
            input_path: path(&concatenated),
            output_path: path(&cut),
            start_seconds: 0.3,
            end_seconds: 1.5,
        }),
        cancel.clone(),
    )
    .await
    .unwrap();
    assert!(matches!(result, UtilityResult::Artifact(_)));

    let color_output = fixture.0.join("color-transfer.mkv");
    let result = media_runtime::run_utility(
        UtilityRequest::ColorMetadataTransfer(ColorMetadataTransferRequest {
            copy_color: true,
            copy_hdr: true,
            in_place: false,
            metadata_source_path: path(&color_source),
            metadata_source_video_stream_index: 0,
            input_path: path(&first),
            input_video_stream_index: 0,
            output_path: path(&color_output),
        }),
        cancel.clone(),
    )
    .await
    .unwrap();
    assert!(matches!(result, UtilityResult::Artifact(_)));

    let ladder = media_runtime::run_utility(
        UtilityRequest::CrfLadder(CrfLadderRequest {
            content_preset: None,
            parameters: Vec::new(),
            vmaf_model: None,
            keep_samples_directory: None,
            input_path: path(&first),
            video_stream_index: 0,
            encoder: LadderEncoder::H264,
            preset: "ultrafast".into(),
            pixel_format: "yuv420p".into(),
            crfs: vec![24, 32],
            sample_count: 1,
            sample_seconds: 1.0,
            metric: LadderMetric::Ssim,
            recommendation_threshold: Some(0.9),
        }),
        cancel.clone(),
    )
    .await
    .unwrap();
    let UtilityResult::CrfLadder(ladder) = ladder else {
        panic!("expected ladder result")
    };
    assert_eq!(ladder.rungs.len(), 2);
    assert!(ladder.rungs.iter().all(|rung| rung.score.is_some()));

    let noisy = fixture.0.join("grain-measure-source.mkv");
    run(
        "ffmpeg",
        &[
            "-hide_banner",
            "-v",
            "error",
            "-y",
            "-f",
            "lavfi",
            "-i",
            "testsrc2=size=128x96:rate=24:duration=1,noise=alls=8:allf=t+u",
            "-c:v",
            "ffv1",
            "-pix_fmt",
            "yuv420p",
            &path(&noisy),
        ],
    );
    let denoised = fixture.0.join("grain-measure-denoised.mkv");
    run(
        "ffmpeg",
        &[
            "-hide_banner",
            "-v",
            "error",
            "-y",
            "-i",
            &path(&noisy),
            "-vf",
            "hqdn3d=4:3:6:4.5",
            "-c:v",
            "ffv1",
            "-pix_fmt",
            "yuv420p",
            &path(&denoised),
        ],
    );
    let measured_table = fixture.0.join("measured-grain.txt");
    let measured = media_runtime::run_utility(
        UtilityRequest::Grain(GrainRequest::Measure {
            source_path: path(&noisy),
            denoised_path: path(&denoised),
            output_table_path: path(&measured_table),
        }),
        cancel.clone(),
    )
    .await
    .unwrap();
    assert!(matches!(measured, UtilityResult::GrainTable(_)));

    let av1 = fixture.0.join("grain-source.mkv");
    let grain_captions = fixture.0.join("grain-captions.srt");
    fs::write(
        &grain_captions,
        "1\r\n00:00:00,200 --> 00:00:00,800\r\nGRAIN TEST\r\n",
    )
    .unwrap();
    run(
        "ffmpeg",
        &[
            "-hide_banner",
            "-v",
            "error",
            "-y",
            "-f",
            "lavfi",
            "-i",
            "testsrc2=size=128x96:rate=24:duration=1",
            "-itsoffset",
            "0.125",
            "-f",
            "lavfi",
            "-i",
            "sine=frequency=600:sample_rate=48000:duration=0.8",
            "-i",
            &path(&grain_captions),
            "-map",
            "0:v:0",
            "-map",
            "1:a:0",
            "-map",
            "2:s:0",
            "-c:v",
            "libsvtav1",
            "-preset",
            "12",
            "-crf",
            "45",
            "-pix_fmt",
            "yuv420p",
            "-c:a",
            "flac",
            "-c:s",
            "srt",
            "-metadata",
            "title=Grain multitrack fixture",
            "-metadata:s:a:0",
            "language=eng",
            "-metadata:s:s:0",
            "language=eng",
            "-disposition:s:0",
            "default",
            &path(&av1),
        ],
    );
    let original_av1 = fs::read(&av1).unwrap();
    let original_video_frames = decoded_video_frames(&av1);
    let original_audio_hash = packet_payload_hash(&av1, 1);
    let original_subtitle_hash = packet_payload_hash(&av1, 2);
    let original_audio_start = first_packet_pts(&av1, "a:0");
    let original_subtitle_start = first_packet_pts(&av1, "s:0");
    let grained = fixture.0.join("grained.mkv");
    media_runtime::run_utility(
        UtilityRequest::Grain(GrainRequest::Apply {
            input_path: path(&av1),
            output_path: path(&grained),
            source: GrainSource::PhotonNoise {
                iso: 400,
                chroma: true,
            },
        }),
        cancel.clone(),
    )
    .await
    .unwrap();
    let table = fixture.0.join("grain-table.txt");
    let extracted = media_runtime::run_utility(
        UtilityRequest::Grain(GrainRequest::Extract {
            input_path: path(&grained),
            output_table_path: path(&table),
        }),
        cancel.clone(),
    )
    .await
    .unwrap();
    assert!(matches!(extracted, UtilityResult::GrainTable(_)));
    let rewritten = fixture.0.join("grain-rewritten.mkv");
    media_runtime::run_utility(
        UtilityRequest::Grain(GrainRequest::RewriteHeaders {
            input_path: path(&grained),
            output_path: path(&rewritten),
            source: GrainSource::Table {
                table_path: path(&table),
            },
        }),
        cancel.clone(),
    )
    .await
    .unwrap();
    let clean = fixture.0.join("grain-removed.mkv");
    media_runtime::run_utility(
        UtilityRequest::Grain(GrainRequest::Remove {
            input_path: path(&rewritten),
            output_path: path(&clean),
        }),
        cancel.clone(),
    )
    .await
    .unwrap();
    for output in [&grained, &rewritten, &clean] {
        assert_eq!(stream_count(output), 3);
        assert_eq!(decoded_video_frames(output), original_video_frames);
        assert_eq!(packet_payload_hash(output, 1), original_audio_hash);
        assert_eq!(packet_payload_hash(output, 2), original_subtitle_hash);
        assert!((first_packet_pts(output, "a:0") - original_audio_start).abs() < 0.000_001);
        assert!((first_packet_pts(output, "s:0") - original_subtitle_start).abs() < 0.000_001);
    }

    let captions = fixture.0.join("captions.srt");
    fs::write(
        &captions,
        "1\r\n00:00:00,100 --> 00:00:00,900\r\nHELLO WORLD\r\n",
    )
    .unwrap();
    run(
        "seconv",
        &[
            &path(&captions),
            "bluraysup",
            "--resolution:1280x720",
            "--font-name:Arial",
            "--font-size:72",
            &format!("--output-folder:{}", fixture.0.to_string_lossy()),
            "--output-filename:captions.sup",
            "--overwrite",
        ],
    );
    let video = fixture.0.join("ocr-video.mkv");
    run(
        "ffmpeg",
        &[
            "-hide_banner",
            "-v",
            "error",
            "-y",
            "-f",
            "lavfi",
            "-i",
            "color=black:size=1280x720:rate=24:duration=1",
            "-c:v",
            "ffv1",
            &path(&video),
        ],
    );
    let ocr_source = fixture.0.join("ocr-source.mkv");
    run(
        "mkvmerge",
        &[
            "--ui-language",
            "en",
            "-o",
            &path(&ocr_source),
            &path(&video),
            &path(&fixture.0.join("captions.sup")),
        ],
    );
    let ocr_output = fixture.0.join("recognized.srt");
    let ocr = media_runtime::run_utility(
        UtilityRequest::SubtitleOcr(SubtitleOcrRequest {
            input_path: path(&ocr_source),
            subtitle_stream_index: 1,
            language: "eng".into(),
            output_path: path(&ocr_output),
        }),
        cancel,
    )
    .await
    .unwrap();
    let UtilityResult::SubtitleOcr(ocr) = ocr else {
        panic!("expected OCR result")
    };
    assert_eq!(ocr.cue_count, 1);
    assert!(
        fs::read_to_string(ocr_output)
            .unwrap()
            .to_ascii_uppercase()
            .contains("HELLO")
    );

    assert_eq!(fs::read(first).unwrap(), original_first);
    assert_eq!(fs::read(second).unwrap(), original_second);
    assert_eq!(fs::read(b_frame_source).unwrap(), original_b_frame_source);
    assert_eq!(fs::read(av1).unwrap(), original_av1);
}

#[tokio::test]
#[ignore = "requires real FFmpeg/FFprobe with libx264 and grav1synth"]
async fn fork_presets_kept_ladder_samples_and_source_grain_measurement() {
    let fixture = Fixture::new();
    let source = fixture.0.join("grainy source.mkv");
    make_ffv1(&source, "bt709", "tv", "1");
    let original = fs::read(&source).unwrap();
    let (_owner, cancel) = tokio::sync::watch::channel(false);
    let samples = fixture.0.join("reviewable ladder samples");
    let result = media_runtime::run_utility(
        UtilityRequest::CrfLadder(CrfLadderRequest {
            input_path: path(&source),
            video_stream_index: 0,
            encoder: LadderEncoder::H264,
            preset: "ultrafast".into(),
            pixel_format: "yuv420p10le".into(),
            crfs: vec![18, 35],
            sample_count: 1,
            sample_seconds: 1.0,
            metric: LadderMetric::Ssim,
            recommendation_threshold: None,
            content_preset: Some("Anime / Cel Animation".into()),
            parameters: Vec::new(),
            vmaf_model: Some(media_core::QualityVmafModel::Negative),
            keep_samples_directory: Some(path(&samples)),
        }),
        cancel.clone(),
    )
    .await
    .unwrap();
    let UtilityResult::CrfLadder(ladder) = result else {
        panic!("expected ladder");
    };
    assert_eq!(ladder.parameters.len(), 8, "{:?}", ladder.diagnostics);
    assert_eq!(ladder.rungs.len(), 2);
    assert_eq!(
        ladder.content_preset.as_deref(),
        Some("Anime / Cel Animation")
    );
    assert_eq!(ladder.vmaf_model, media_core::QualityVmafModel::Negative);
    assert_eq!(
        ladder.samples_directory.as_deref(),
        Some(path(&samples).as_str())
    );
    assert!(
        ladder
            .rungs
            .iter()
            .all(|rung| rung.score.is_some_and(|score| score.is_finite()))
    );
    let files = fs::read_dir(&samples)
        .unwrap()
        .map(|entry| entry.unwrap().path())
        .collect::<Vec<_>>();
    assert_eq!(files.len(), 3);
    for sample in &files {
        assert_eq!(decoded_video_frames(sample), 24);
    }
    for keep in [false, true] {
        let table = fixture.0.join(if keep {
            "kept-reference.tbl"
        } else {
            "private-reference.tbl"
        });
        let denoised = fixture.0.join("retained denoised.mkv");
        let result = media_runtime::run_utility(
            UtilityRequest::Grain(GrainRequest::MeasureSource {
                source_path: path(&source),
                output_table_path: path(&table),
                denoise_strength: 4,
                denoised_output_path: keep.then(|| path(&denoised)),
            }),
            cancel.clone(),
        )
        .await
        .unwrap();
        let UtilityResult::GrainTable(measured) = result else {
            panic!("expected grain table");
        };
        assert!(measured.segment_count > 0);
        assert!(table.is_file());
        assert_eq!(measured.denoised_output_path.is_some(), keep);
        if keep {
            assert_eq!(decoded_video_frames(&denoised), 24);
        }
    }
    assert_eq!(fs::read(&source).unwrap(), original);
}

#[tokio::test]
#[ignore = "requires real FFmpeg and FFprobe"]
async fn utility_batch_preserves_successes_and_continues_after_a_failed_source() {
    let fixture = Fixture::new();
    let source = fixture.0.join("batch source.mkv");
    make_ffv1(&source, "bt709", "tv", "1");
    let original = fs::read(&source).unwrap();
    let output = fixture.0.join("batch output.mkv");
    let cut = |input: &Path, output: &Path| {
        UtilityRequest::KeyframeCut(KeyframeCutRequest {
            input_path: path(input),
            output_path: path(output),
            start_seconds: 0.0,
            end_seconds: 1.0,
        })
    };
    let (_owner, cancel) = tokio::sync::watch::channel(false);
    let request = media_core::UtilityBatchRequest {
        requests: vec![
            cut(
                &fixture.0.join("missing.mkv"),
                &fixture.0.join("failed.mkv"),
            ),
            cut(&source, &output),
        ],
    };
    let events = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
    let sink = events.clone();
    let observed_output = output.clone();
    let result = media_runtime::with_utility_progress(
        move |value| {
            let existed = observed_output.exists();
            sink.lock().unwrap().push((value, existed));
        },
        media_runtime::run_utility_batch(request.clone(), cancel.clone()),
    )
    .await
    .unwrap();
    {
        let events = events.lock().unwrap();
        assert!(
            events
                .iter()
                .any(|(value, _)| value.row_index == Some(0) && value.status == "failed")
        );
        assert!(
            events
                .iter()
                .any(|(value, existed)| value.row_index == Some(1)
                    && value.status == "running"
                    && !existed)
        );
        assert!(events.iter().any(|(value, _)| value.row_index == Some(1)
            && value.status == "succeeded"
            && value.completed_rows == 2));
    }
    assert_eq!(result.completed, 1);
    assert!(!result.canceled);
    assert_eq!(result.entries.len(), 2);
    assert!(result.entries[0].error.is_some());
    assert!(result.entries[1].result.is_some());
    assert_eq!(decoded_video_frames(&output), 24);
    let published = fs::read(&output).unwrap();
    let repeated = media_runtime::run_utility_batch(request, cancel)
        .await
        .unwrap_err();
    assert_eq!(repeated.code, "OUTPUT_EXISTS");
    assert_eq!(fs::read(&output).unwrap(), published);
    assert_eq!(fs::read(&source).unwrap(), original);
}

#[cfg(not(windows))]
#[tokio::test]
#[ignore = "requires an actual external VapourSynth Python with L-SMASH, vszip and julek plugins"]
async fn portable_cpu_ladder_metrics_score_every_retained_frame() {
    let fixture = Fixture::new();
    let source = fixture.0.join("portable metric source.mkv");
    make_ffv1(&source, "bt709", "tv", "1");
    let original = fs::read(&source).unwrap();
    for metric in [LadderMetric::Ssimulacra2, LadderMetric::ButteraugliInf] {
        let samples = fixture.0.join(format!("samples-{metric:?}"));
        let (_owner, cancel) = tokio::sync::watch::channel(false);
        let result = media_runtime::run_utility(
            UtilityRequest::CrfLadder(CrfLadderRequest {
                input_path: path(&source),
                video_stream_index: 0,
                encoder: LadderEncoder::H264,
                preset: "ultrafast".into(),
                pixel_format: "yuv420p10le".into(),
                crfs: vec![23],
                sample_count: 1,
                sample_seconds: 1.0,
                metric,
                recommendation_threshold: None,
                content_preset: None,
                parameters: Vec::new(),
                vmaf_model: None,
                keep_samples_directory: Some(path(&samples)),
            }),
            cancel,
        )
        .await
        .unwrap();
        let UtilityResult::CrfLadder(ladder) = result else {
            panic!("expected ladder")
        };
        assert_eq!(ladder.rungs.len(), 1);
        assert!(ladder.rungs[0].score.is_some_and(f64::is_finite));
        for entry in fs::read_dir(samples).unwrap() {
            assert_eq!(decoded_video_frames(&entry.unwrap().path()), 24);
        }
        assert_eq!(fs::read(&source).unwrap(), original);
    }
}

#[tokio::test]
#[ignore = "requires FFmpeg/FFprobe with libaom, SVT-AV1 and all three built-in libvmaf models"]
async fn av1_ladders_use_all_vmaf_models_presets_and_no_clobber_sample_publication() {
    use media_core::QualityVmafModel;
    let fixture = Fixture::new();
    let source = fixture.0.join("AV1 model source.mkv");
    make_ffv1(&source, "bt709", "tv", "1");
    let original = fs::read(&source).unwrap();
    for encoder in [LadderEncoder::AomAv1, LadderEncoder::Av1] {
        for model in [
            QualityVmafModel::Standard,
            QualityVmafModel::Negative,
            QualityVmafModel::FourK,
        ] {
            let samples = fixture.0.join(format!("{encoder:?}-{model:?}-samples"));
            let request = UtilityRequest::CrfLadder(CrfLadderRequest {
                input_path: path(&source),
                video_stream_index: 0,
                encoder,
                preset: "8".into(),
                pixel_format: "yuv420p10le".into(),
                crfs: vec![28],
                sample_count: 1,
                sample_seconds: 1.0,
                metric: LadderMetric::Vmaf,
                recommendation_threshold: None,
                content_preset: (encoder == LadderEncoder::Av1)
                    .then(|| "Game Capture / Gameplay".into()),
                parameters: Vec::new(),
                vmaf_model: Some(model),
                keep_samples_directory: Some(path(&samples)),
            });
            let (_owner, cancel) = tokio::sync::watch::channel(false);
            let result = media_runtime::run_utility(request.clone(), cancel.clone())
                .await
                .unwrap();
            let UtilityResult::CrfLadder(ladder) = result else {
                panic!("expected ladder")
            };
            assert_eq!(ladder.encoder, encoder);
            assert_eq!(ladder.vmaf_model, model);
            assert_eq!(ladder.rungs.len(), 1);
            let score = ladder.rungs[0]
                .score
                .expect("VMAF must score the complete sample");
            assert!(score.is_finite() && (0.0..=100.0).contains(&score));
            assert_eq!(
                ladder.samples_directory.as_deref(),
                Some(path(&samples).as_str())
            );
            if encoder == LadderEncoder::Av1 {
                assert_eq!(
                    ladder.content_preset.as_deref(),
                    Some("Game Capture / Gameplay")
                );
                let source_presets: serde_json::Value =
                    serde_json::from_str(include_str!("../../../src/lib/content-presets.json"))
                        .unwrap();
                let source_rows = source_presets["svtAv1"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .find(|item| item["name"] == "Game Capture / Gameplay")
                    .unwrap()["parameters"]
                    .as_array()
                    .unwrap();
                for row in source_rows {
                    let name = row["name"].as_str().unwrap();
                    assert!(
                        ladder.parameters.iter().any(|item| item.name == name)
                            || ladder
                                .diagnostics
                                .iter()
                                .any(|text| text.contains(&format!("control {name}="))
                                    && text.contains("omitted")),
                        "missing preset disposition {name}: {ladder:?}"
                    );
                }
            }
            let mut retained = fs::read_dir(&samples)
                .unwrap()
                .map(|entry| {
                    let path = entry.unwrap().path();
                    assert_eq!(decoded_video_frames(&path), 24);
                    (path.clone(), fs::read(path).unwrap())
                })
                .collect::<Vec<_>>();
            retained.sort_by(|left, right| left.0.cmp(&right.0));
            assert_eq!(retained.len(), 2, "one reference plus one encoded rung");
            let repeated = media_runtime::run_utility(request, cancel)
                .await
                .unwrap_err();
            assert_eq!(repeated.code, "OUTPUT_EXISTS");
            for (path, bytes) in retained {
                assert_eq!(fs::read(path).unwrap(), bytes);
            }
            assert_eq!(fs::read(&source).unwrap(), original);
            eprintln!(
                "{encoder:?} {model:?}: VMAF={score:.6}, applied preset controls={}, retained2×24frames; existing destination preserved",
                ladder.parameters.len()
            );
        }
    }
}

#[tokio::test]
#[ignore = "requires FFmpeg/FFprobe with SVT-AV1 and libvmaf"]
async fn eight_bit_svt_preset_reports_high_bit_depth_control_omission() {
    let fixture = Fixture::new();
    let source = fixture.0.join("eight bit gameplay.mkv");
    make_ffv1(&source, "bt709", "tv", "1");
    let original = fs::read(&source).unwrap();
    let (_owner, cancel) = tokio::sync::watch::channel(false);
    let result = media_runtime::run_utility(
        UtilityRequest::CrfLadder(CrfLadderRequest {
            input_path: path(&source),
            video_stream_index: 0,
            encoder: LadderEncoder::Av1,
            preset: "8".into(),
            pixel_format: "yuv420p".into(),
            crfs: vec![28],
            sample_count: 1,
            sample_seconds: 1.0,
            metric: LadderMetric::Vmaf,
            recommendation_threshold: None,
            content_preset: Some("Game Capture / Gameplay".into()),
            parameters: Vec::new(),
            vmaf_model: None,
            keep_samples_directory: None,
        }),
        cancel,
    )
    .await
    .unwrap();
    let UtilityResult::CrfLadder(ladder) = result else {
        panic!("expected ladder")
    };
    assert!(
        ladder
            .parameters
            .iter()
            .all(|value| value.name != "hbd-mds")
    );
    assert!(
        ladder
            .diagnostics
            .iter()
            .any(|text| text.contains("control hbd-mds=1") && text.contains("omitted"))
    );
    assert!(ladder.rungs[0].score.is_some_and(f64::is_finite));
    assert_eq!(fs::read(source).unwrap(), original);
}
