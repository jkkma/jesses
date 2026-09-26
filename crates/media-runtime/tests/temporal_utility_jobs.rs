//! Opt-in native checks for the temporal export utilities.
use media_core::{
    CadenceRepairExportRequest, DeinterlaceExportMethod, DeinterlaceExportRequest, DeinterlaceMode,
    FieldOrder, QtgmcPreset, UtilityRequest, UtilityResult,
};
use std::{
    path::{Path, PathBuf},
    process::Command,
    time::{Duration, SystemTime, UNIX_EPOCH},
};

struct Fixture(PathBuf);
impl Fixture {
    fn new() -> Self {
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let path =
            std::env::temp_dir().join(format!("jesses-temporal-{}-{nonce}", std::process::id()));
        std::fs::create_dir(&path).unwrap();
        Self(path)
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        if std::env::var_os("JESSES_KEEP_TEST_FIXTURES").is_none() {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }
}
fn text(path: &Path) -> String {
    path.to_string_lossy().into_owned()
}
fn run(program: &str, args: &[&str]) {
    let output = Command::new(program).args(args).output().unwrap();
    assert!(
        output.status.success(),
        "{program}: {}",
        String::from_utf8_lossy(&output.stderr)
    );
}
fn probe(path: &Path) -> serde_json::Value {
    let output = Command::new("ffprobe")
        .args([
            "-v",
            "error",
            "-count_frames",
            "-show_streams",
            "-show_format",
            "-of",
            "json",
        ])
        .arg(path)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    serde_json::from_slice(&output.stdout).unwrap()
}
fn interlaced_source(path: &Path) {
    interlaced_source_with_sar(path, "1/1");
}

fn interlaced_source_with_sar(path: &Path, sar: &str) {
    let filter = format!(
        "format=yuv420p,setsar={sar},setfield=tff,setparams=range=limited:color_primaries=bt709:color_trc=bt709:colorspace=bt709"
    );
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
            "testsrc2=size=128x96:rate=30000/1001",
            "-f",
            "lavfi",
            "-i",
            "sine=frequency=440:sample_rate=48000",
            "-map",
            "0:v",
            "-map",
            "1:a",
            "-frames:v",
            "8",
            "-t",
            "0.267",
            "-vf",
            &filter,
            "-c:v",
            "libx264",
            "-preset",
            "ultrafast",
            "-x264-params",
            "tff=1",
            "-flags",
            "+ilme+ildct",
            "-c:a",
            "pcm_s16le",
            "-color_primaries",
            "bt709",
            "-color_trc",
            "bt709",
            "-colorspace",
            "bt709",
            &text(path),
        ],
    );
}

#[tokio::test]
#[ignore = "requires FFmpeg and libx264"]
async fn bwdif_export_preserves_non_square_sample_aspect_ratio() {
    let fixture = Fixture::new();
    let input = fixture.0.join("non-square-interlaced.mkv");
    let output = fixture.0.join("non-square-progressive.mkv");
    interlaced_source_with_sar(&input, "4/3");
    let original = std::fs::read(&input).unwrap();
    assert_eq!(probe(&input)["streams"][0]["sample_aspect_ratio"], "4:3");
    let (_owner, cancel) = tokio::sync::watch::channel(false);
    media_runtime::run_utility(
        UtilityRequest::DeinterlaceExport(DeinterlaceExportRequest {
            input_path: text(&input),
            output_path: text(&output),
            video_stream_index: 0,
            method: DeinterlaceExportMethod::Bwdif,
            mode: DeinterlaceMode::Frame,
            field_order: FieldOrder::TopFirst,
            qtgmc_preset: QtgmcPreset::Fast,
        }),
        cancel,
    )
    .await
    .unwrap();
    let encoded = probe(&output);
    assert_eq!(encoded["streams"][0]["sample_aspect_ratio"], "4:3");
    assert_eq!(encoded["streams"][0]["nb_read_frames"], "8");
    assert_eq!(std::fs::read(&input).unwrap(), original);
}

#[tokio::test]
#[ignore = "requires FFmpeg and libx264"]
async fn bwdif_export_keeps_source_and_audio_and_doubles_fields() {
    let fixture = Fixture::new();
    let input = fixture.0.join("interlaced.mkv");
    let output = fixture.0.join("progressive.mkv");
    interlaced_source(&input);
    let original = std::fs::read(&input).unwrap();
    let (_owner, cancel) = tokio::sync::watch::channel(false);
    let result = media_runtime::run_utility(
        UtilityRequest::DeinterlaceExport(DeinterlaceExportRequest {
            input_path: text(&input),
            output_path: text(&output),
            video_stream_index: 0,
            method: DeinterlaceExportMethod::Bwdif,
            mode: DeinterlaceMode::Bob,
            field_order: FieldOrder::TopFirst,
            qtgmc_preset: QtgmcPreset::Fast,
        }),
        cancel,
    )
    .await
    .unwrap();
    assert!(matches!(result, UtilityResult::Artifact(_)));
    let inspected = probe(&output);
    let video = &inspected["streams"][0];
    assert_eq!(video["codec_name"], "h264");
    assert_eq!(video["field_order"], "progressive");
    assert_eq!(video["nb_read_frames"], "16");
    assert_eq!(inspected["streams"][1]["codec_name"], "pcm_s16le");
    assert_eq!(std::fs::read(&input).unwrap(), original);
    assert!(!fixture.0.read_dir().unwrap().any(|item| {
        item.unwrap()
            .file_name()
            .to_string_lossy()
            .contains(".partial.")
    }));
}

#[tokio::test]
#[ignore = "requires packaged VSPipe, havsfunc, L-SMASH, FFmpeg and libx264"]
async fn qtgmc_export_keeps_source_and_audio_and_doubles_fields() {
    let fixture = Fixture::new();
    let input = fixture.0.join("interlaced.mkv");
    let output = fixture.0.join("qtgmc-progressive.mkv");
    interlaced_source(&input);
    let original = std::fs::read(&input).unwrap();
    let (_owner, cancel) = tokio::sync::watch::channel(false);
    let result = media_runtime::run_utility(
        UtilityRequest::DeinterlaceExport(DeinterlaceExportRequest {
            input_path: text(&input),
            output_path: text(&output),
            video_stream_index: 0,
            method: DeinterlaceExportMethod::Qtgmc,
            mode: DeinterlaceMode::Bob,
            field_order: FieldOrder::TopFirst,
            qtgmc_preset: QtgmcPreset::VerySlow,
        }),
        cancel,
    )
    .await
    .unwrap();
    assert!(matches!(result, UtilityResult::Artifact(_)));
    let inspected = probe(&output);
    assert_eq!(inspected["streams"][0]["field_order"], "progressive");
    assert_eq!(inspected["streams"][0]["nb_read_frames"], "16");
    assert_eq!(inspected["streams"][1]["codec_name"], "pcm_s16le");
    assert_eq!(std::fs::read(&input).unwrap(), original);
}

#[tokio::test]
#[ignore = "requires FFmpeg, MKVToolNix, VSPipe, BestSource and libx264"]
async fn cadence_export_removes_padded_duplicates_but_keeps_interlace_and_audio() {
    let fixture = Fixture::new();
    let padded = fixture.0.join("padded-untimed.mkv");
    let input = fixture.0.join("padded-capture.mkv");
    let output = fixture.0.join("repaired.mkv");
    let times = fixture.0.join("timestamps.txt");
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
            "testsrc2=size=128x96:rate=24:duration=1",
            "-f",
            "lavfi",
            "-i",
            "sine=frequency=440:sample_rate=48000:duration=1",
            "-map",
            "0:v",
            "-map",
            "1:a",
            "-vf",
            "fps=36,format=yuv420p,setfield=tff,setparams=range=limited:color_primaries=bt709:color_trc=bt709:colorspace=bt709",
            "-frames:v",
            "36",
            "-c:v",
            "libx264",
            "-preset",
            "ultrafast",
            "-x264-params",
            "tff=1",
            "-flags",
            "+ilme+ildct",
            "-c:a",
            "pcm_s16le",
            &text(&padded),
        ],
    );
    let mut text_times = String::from("# timestamp format v2\n");
    for index in 0..36 {
        let slot = index * 24 / 36;
        text_times.push_str(&format!("{:.6}\n", f64::from(slot) * 1000.0 / 24.0));
    }
    std::fs::write(&times, text_times).unwrap();
    run(
        "mkvmerge",
        &[
            "-q",
            "-o",
            &text(&input),
            "--default-duration",
            "0:24fps",
            "--timestamps",
            &format!("0:{}", text(&times)),
            &text(&padded),
        ],
    );
    let source_probe = probe(&input);
    assert_eq!(source_probe["streams"][0]["nb_read_frames"], "36");
    assert_eq!(source_probe["streams"][0]["r_frame_rate"], "24/1");
    let original = std::fs::read(&input).unwrap();
    let (_owner, cancel) = tokio::sync::watch::channel(false);
    let result = media_runtime::run_utility(
        UtilityRequest::CadenceRepairExport(CadenceRepairExportRequest {
            input_path: text(&input),
            output_path: text(&output),
            video_stream_index: 0,
        }),
        cancel,
    )
    .await
    .unwrap();
    assert!(matches!(result, UtilityResult::Artifact(_)));
    let repaired = probe(&output);
    assert_eq!(repaired["streams"][0]["nb_read_frames"], "24");
    assert!(matches!(
        repaired["streams"][0]["field_order"].as_str(),
        Some("tt" | "tb")
    ));
    assert_eq!(repaired["streams"][1]["codec_name"], "pcm_s16le");
    assert_eq!(std::fs::read(&input).unwrap(), original);
}

#[tokio::test]
#[ignore = "requires FFmpeg and FFprobe"]
async fn cadence_export_refuses_an_unpadded_capture_without_creating_output() {
    let fixture = Fixture::new();
    let input = fixture.0.join("ordinary-interlaced.mkv");
    let output = fixture.0.join("should-not-exist.mkv");
    interlaced_source(&input);
    let original = std::fs::read(&input).unwrap();
    let (_owner, cancel) = tokio::sync::watch::channel(false);
    let failure = media_runtime::run_utility(
        UtilityRequest::CadenceRepairExport(CadenceRepairExportRequest {
            input_path: text(&input),
            output_path: text(&output),
            video_stream_index: 0,
        }),
        cancel,
    )
    .await
    .unwrap_err();
    assert_eq!(failure.code, "UTILITY_NOT_NEEDED");
    assert!(!output.exists());
    assert_eq!(std::fs::read(&input).unwrap(), original);
}

#[tokio::test]
#[ignore = "requires FFmpeg and FFprobe"]
async fn deinterlace_export_refuses_hdr_that_would_lose_static_metadata() {
    let fixture = Fixture::new();
    let input = fixture.0.join("hdr.mkv");
    let output = fixture.0.join("should-not-exist.mkv");
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
            "testsrc2=size=128x96:rate=24:duration=0.5",
            "-frames:v",
            "12",
            "-vf",
            "format=yuv420p10le,setparams=range=limited:color_primaries=bt2020:color_trc=smpte2084:colorspace=bt2020nc",
            "-c:v",
            "ffv1",
            "-level",
            "3",
            "-color_primaries",
            "bt2020",
            "-color_trc",
            "smpte2084",
            "-colorspace",
            "bt2020nc",
            &text(&input),
        ],
    );
    let original = std::fs::read(&input).unwrap();
    let (_owner, cancel) = tokio::sync::watch::channel(false);
    let failure = media_runtime::run_utility(
        UtilityRequest::DeinterlaceExport(DeinterlaceExportRequest {
            input_path: text(&input),
            output_path: text(&output),
            video_stream_index: 0,
            method: DeinterlaceExportMethod::Bwdif,
            mode: DeinterlaceMode::Frame,
            field_order: FieldOrder::TopFirst,
            qtgmc_preset: QtgmcPreset::Fast,
        }),
        cancel,
    )
    .await
    .unwrap_err();
    assert_eq!(failure.code, "UTILITY_HDR_UNSUPPORTED");
    assert!(!output.exists());
    assert_eq!(std::fs::read(&input).unwrap(), original);
}

#[tokio::test]
#[ignore = "requires packaged VSPipe, havsfunc, L-SMASH, FFmpeg and libx264"]
async fn cancellation_cleans_temporal_export_without_publishing_output() {
    let fixture = Fixture::new();
    let input = fixture.0.join("interlaced.mkv");
    let output = fixture.0.join("canceled-progressive.mkv");
    interlaced_source(&input);
    let original = std::fs::read(&input).unwrap();
    let (owner, cancel) = tokio::sync::watch::channel(false);
    let request = UtilityRequest::DeinterlaceExport(DeinterlaceExportRequest {
        input_path: text(&input),
        output_path: text(&output),
        video_stream_index: 0,
        method: DeinterlaceExportMethod::Qtgmc,
        mode: DeinterlaceMode::Bob,
        field_order: FieldOrder::TopFirst,
        qtgmc_preset: QtgmcPreset::VerySlow,
    });
    let running = tokio::spawn(media_runtime::run_utility(request, cancel));
    tokio::time::timeout(Duration::from_secs(15), async {
        loop {
            if fixture.0.read_dir().unwrap().any(|entry| {
                entry
                    .unwrap()
                    .file_name()
                    .to_string_lossy()
                    .contains(".partial.")
            }) {
                break;
            }
            assert!(
                !running.is_finished(),
                "temporal export finished before creating its owned output"
            );
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    })
    .await
    .expect("temporal export did not create its owned temporary output");
    owner.send(true).unwrap();
    let failure = tokio::time::timeout(Duration::from_secs(15), running)
        .await
        .expect("canceled export did not stop")
        .unwrap()
        .unwrap_err();
    assert!(
        matches!(failure.code.as_str(), "JOB_CANCELED" | "UTILITY_CANCELED"),
        "{failure:?}"
    );
    assert!(!output.exists());
    assert_eq!(std::fs::read(&input).unwrap(), original);
    assert!(!fixture.0.read_dir().unwrap().any(|entry| {
        entry
            .unwrap()
            .file_name()
            .to_string_lossy()
            .contains(".partial.")
    }));
}
