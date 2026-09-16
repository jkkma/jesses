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
