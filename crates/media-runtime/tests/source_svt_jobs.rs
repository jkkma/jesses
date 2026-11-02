//! The single source SVT identity binds a qualified provider, never a UI fork choice.
use media_core::*;
use media_runtime::{
    jobs::JobManager,
    supervisor::{CommandSpec, run_capture},
};
use std::{
    ffi::OsString,
    path::{Path, PathBuf},
    sync::atomic::{AtomicU64, Ordering},
    time::Duration,
};
use tokio::sync::watch;
static NEXT: AtomicU64 = AtomicU64::new(1);
struct Fixture(PathBuf);
impl Fixture {
    fn new() -> Self {
        let bundle = PathBuf::from(
            std::env::var_os("JESSES_TEST_TOOL_RESOURCES")
                .expect("Supply the qualified SVT package root"),
        );
        static CONFIGURED: std::sync::OnceLock<()> = std::sync::OnceLock::new();
        CONFIGURED.get_or_init(|| media_runtime::configure_bundled_tools(bundle).unwrap());
        let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../../target/test-artifacts/source-svt");
        std::fs::create_dir_all(&root).unwrap();
        let path = root.join(format!(
            "{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        std::fs::create_dir(&path).unwrap();
        Self(std::fs::canonicalize(path).unwrap())
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        if std::thread::panicking() || std::env::var_os("JESSES_KEEP_TEST_ARTIFACTS").is_some() {
            eprintln!("Retained {}", self.0.display());
        } else {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }
}

#[tokio::test]
#[ignore = "requires pinned HDR provider, FFmpeg/FFprobe and AV1AN"]
async fn canonical_source_managed_grain_table_is_consumed_by_both_backends() {
    const TABLE: &str = "filmgrn1\nE 0 18446744073709551615 1 10956 1\n\tp 0 6 0 8 1 1 0 0 0 0 0 0\n\tsY 14  0 5 20 4 39 4 59 4 78 4 98 4 118 4 137 4 157 4 177 4 196 4 216 5 235 5 255 5\n\tsCb 0\n\tsCr 0\n\tcY\n\tcCb 0\n\tcCr 0\n";
    let f = Fixture::new();
    let input = source(&f.0, false).await;
    let manager = JobManager::open(f.0.join("logs"), f.0.join("history")).await;
    for backend in [EncodeBackend::Standalone, EncodeBackend::Av1an] {
        let output = f.0.join(format!("table-{backend:?}.mkv"));
        let mut s = settings(backend);
        s.av1an_grain = Some(Av1anGrainSettings {
            table: Some(TABLE.into()),
            denoise: false,
            denoise_strength: 4,
        });
        let job = manager
            .start_encode(request(&input, &output, s))
            .await
            .unwrap();
        let done = finish(&manager, &job.id).await;
        assert_eq!(done.state, JobState::Succeeded, "{done:#?}");
        assert!(output.exists());
        let mut a = args(&["-v", "trace", "-i"]);
        a.push(output.as_os_str().to_owned());
        a.extend(args(&[
            "-map",
            "0:v:0",
            "-c",
            "copy",
            "-bsf:v",
            "trace_headers",
            "-f",
            "null",
            "-",
        ]));
        let trace = String::from_utf8(ffmpeg_capture(a).await.stderr).unwrap();
        for (field, value) in [
            ("film_grain_params_present", "1"),
            ("apply_grain", "1"),
            ("num_y_points", "14"),
            ("point_y_scaling[0]", "5"),
        ] {
            assert!(
                trace
                    .lines()
                    .any(|line| line.contains(field) && line.ends_with(&format!("= {value}"))),
                "missing authored grain field {field}={value}"
            );
        }
        eprintln!(
            "{backend:?}: canonical source managed fgs-table accepted and output fully verified"
        );
    }
    manager.shutdown().await;
}
fn args(v: &[&str]) -> Vec<OsString> {
    v.iter().map(OsString::from).collect()
}
async fn ffmpeg(a: Vec<OsString>) -> Vec<u8> {
    ffmpeg_capture(a).await.stdout
}
async fn ffmpeg_capture(a: Vec<OsString>) -> media_runtime::supervisor::CapturedOutput {
    let (_owner, cancel) = watch::channel(false);
    let result = run_capture(
        &CommandSpec {
            executable: std::env::var_os("JESSES_FFMPEG")
                .unwrap_or_else(|| "ffmpeg".into())
                .into(),
            args: a,
            cwd: None,
        },
        cancel,
        8 * 1024 * 1024,
        Duration::from_secs(60),
    )
    .await
    .unwrap();
    assert!(
        result.status.success(),
        "{}",
        String::from_utf8_lossy(&result.stderr)
    );
    result
}
async fn source(root: &Path, hdr: bool) -> PathBuf {
    source_frames(root, hdr, 4).await
}
async fn source_frames(root: &Path, hdr: bool, frames: u32) -> PathBuf {
    let path = root.join(if hdr { "pq.mkv" } else { "sdr.mkv" });
    let pattern = format!(
        "testsrc2=size=192x128:rate=4:duration={}",
        f64::from(frames) / 4.0
    );
    let frame_count = frames.to_string();
    let mut a = args(&[
        "-v",
        "error",
        "-f",
        "lavfi",
        "-i",
        &pattern,
        "-frames:v",
        &frame_count,
        "-pix_fmt",
        "yuv420p10le",
        "-chroma_sample_location",
        "left",
        "-c:v",
        "ffv1",
        "-level",
        "3",
        "-threads",
        "2",
    ]);
    if hdr {
        a.extend(args(&["-vf","setparams=field_mode=prog:range=tv:color_primaries=bt2020:color_trc=smpte2084:colorspace=bt2020nc"]));
        a.extend(args(&[
            "-color_primaries",
            "bt2020",
            "-color_trc",
            "smpte2084",
            "-colorspace",
            "bt2020nc",
        ]));
    } else {
        a.extend(args(&[
            "-color_primaries",
            "bt709",
            "-color_trc",
            "bt709",
            "-colorspace",
            "bt709",
        ]));
    }
    a.push(path.as_os_str().to_owned());
    ffmpeg(a).await;
    path
}
fn settings(backend: EncodeBackend) -> EncodeSettings {
    EncodeSettings {
        encoder: VideoEncoder::SvtAv1,
        backend,
        preset: 12,
        workers: 1,
        av1an_options: (backend == EncodeBackend::Av1an).then_some(Av1anOptions {
            chunk_method: Av1anChunkMethod::Select,
            split_method: Av1anSplitMethod::FixedChunks,
            maximum_chunk_frames: 0,
            minimum_scene_frames: 1,
            concat_method: Av1anConcatMethod::Ffmpeg,
            encoder_threads: Some(1),
            ..Default::default()
        }),
        ..Default::default()
    }
}
fn request(input: &Path, output: &Path, settings: EncodeSettings) -> EncodeRequest {
    EncodeRequest {
        source: RemuxRequest {
            input_path: input.to_string_lossy().into_owned(),
            output_path: output.to_string_lossy().into_owned(),
            stream_indices: vec![0],
        },
        settings,
    }
}
async fn finish(manager: &JobManager, id: &str) -> JobSnapshot {
    tokio::time::timeout(Duration::from_secs(90), async {
        loop {
            let job = manager
                .list_jobs()
                .await
                .into_iter()
                .find(|job| job.id == id)
                .unwrap();
            if job.state.is_terminal() {
                return job;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .unwrap()
}
async fn pixels(input: &Path) -> Vec<u8> {
    let mut a = args(&["-v", "error", "-i"]);
    a.push(input.as_os_str().to_owned());
    a.extend(args(&[
        "-map",
        "0:v:0",
        "-pix_fmt",
        "yuv420p10le",
        "-f",
        "rawvideo",
        "-",
    ]));
    ffmpeg(a).await
}
fn authored() -> HdrMetadataSettings {
    HdrMetadataSettings {
        mastering_display: Some(HdrMasteringDisplay {
            red: HdrChromaticity { x: 34000, y: 16000 },
            green: HdrChromaticity { x: 13250, y: 34500 },
            blue: HdrChromaticity { x: 7500, y: 3000 },
            white_point: HdrChromaticity { x: 15635, y: 16450 },
            max_luminance_ten_thousandths: 20_000_000,
            min_luminance_ten_thousandths: 1,
        }),
        content_light: Some(HdrContentLight {
            max_content: 500,
            max_average: 250,
        }),
        ..Default::default()
    }
}
fn dynamic(root: &Path, frames: usize) -> (PathBuf, PathBuf) {
    use dolby_vision::rpu::{dovi_rpu::DoviRpu, generate::GenerateConfig};
    use hdr10plus::{
        metadata::{DistributionMaxRgb, Hdr10PlusMetadata},
        metadata_json::generate_json,
    };
    let mut rpus = Vec::new();
    let mut metadata = Vec::new();
    for index in 0..frames {
        let rpu = DoviRpu::profile81_config(&GenerateConfig {
            source_max_pq: Some(3000 + index as u16),
            ..Default::default()
        })
        .unwrap();
        rpus.extend([0, 0, 0, 1]);
        rpus.extend(&rpu.write_hevc_unspec62_nalu().unwrap()[2..]);
        metadata.push(Hdr10PlusMetadata {
            profile: "A".into(),
            itu_t_t35_country_code: 0xB5,
            itu_t_t35_terminal_provider_code: 0x3C,
            itu_t_t35_terminal_provider_oriented_code: 1,
            application_identifier: 4,
            application_version: 1,
            num_windows: 1,
            maxscl: [1000 + index as u32, 900, 800],
            average_maxrgb: 500 + index as u32,
            num_distribution_maxrgb_percentiles: 9,
            distribution_maxrgb: [1, 5, 10, 25, 50, 75, 90, 95, 99]
                .into_iter()
                .enumerate()
                .map(|(i, percentage)| DistributionMaxRgb {
                    percentage,
                    percentile: 100 + i as u32 * 100,
                })
                .collect(),
            ..Default::default()
        });
    }
    let rpu = root.join("source metadata.rpu");
    let json = root.join("source hdr10plus.json");
    std::fs::write(&rpu, rpus).unwrap();
    std::fs::write(
        &json,
        serde_json::to_vec_pretty(&generate_json(
            &metadata.iter().collect::<Vec<_>>(),
            "Jesses fixture",
            "1",
        ))
        .unwrap(),
    )
    .unwrap();
    (rpu, json)
}

#[tokio::test]
#[ignore = "requires pinned SVT package, qualified FFmpeg/FFprobe and AV1AN"]
async fn canonical_source_zero_is_valid_lossy_crf_for_quick_and_av1an() {
    let f = Fixture::new();
    let input = source(&f.0, false).await;
    let original = pixels(&input).await;
    let manager = JobManager::open(f.0.join("logs"), f.0.join("history")).await;
    let capabilities = media_runtime::get_capabilities().await;
    let canonical = capabilities
        .iter()
        .find(|tool| tool.id == "svt-av1")
        .unwrap();
    assert!(canonical.available, "{canonical:?}");
    assert!(canonical.path.as_ref().unwrap().contains("svt-av1-hdr"));
    assert!(canonical.version.as_ref().unwrap().contains("SVT-AV1-HDR"));
    for backend in [EncodeBackend::Standalone, EncodeBackend::Av1an] {
        let output = f.0.join(format!("zero-{backend:?}.mkv"));
        let mut s = settings(backend);
        s.crf = 0;
        s.lossless = false;
        let job = manager
            .start_encode(request(&input, &output, s))
            .await
            .unwrap();
        let done = finish(&manager, &job.id).await;
        assert_eq!(done.state, JobState::Succeeded, "{done:#?}");
        assert!(!done.encode_settings.as_ref().unwrap().lossless);
        let decoded = pixels(&output).await;
        assert_eq!(decoded.len(), original.len());
        assert_ne!(
            decoded, original,
            "source CRF0 must not be given a false lossless guarantee"
        );
        eprintln!(
            "{backend:?} source CRF0: {} decoded bytes, valid lossy output, saved lossless=false",
            decoded.len()
        );
    }
    manager.shutdown().await;
}

#[tokio::test]
#[ignore = "requires metadata-linked pinned HDR provider, FFmpeg/FFprobe and AV1AN"]
async fn canonical_source_manual_hdr_resources_and_source_only_scalars_are_verified() {
    let f = Fixture::new();
    let input = source(&f.0, true).await;
    let (rpu, json) = dynamic(&f.0, 4);
    let original_rpu = std::fs::read(&rpu).unwrap();
    let original_json = std::fs::read(&json).unwrap();
    let manager = JobManager::open(f.0.join("logs"), f.0.join("history")).await;
    for backend in [EncodeBackend::Standalone, EncodeBackend::Av1an] {
        let output = f.0.join(format!("metadata-{backend:?}.mkv"));
        let mut s = settings(backend);
        let mut metadata = authored();
        metadata.dolby_vision_rpu_path = Some(rpu.to_string_lossy().into_owned());
        metadata.hdr10_plus_json_path = Some(json.to_string_lossy().into_owned());
        s.hdr_metadata = Some(metadata);
        s.parameters = vec![
            EncoderParameter {
                name: "tx-bias".into(),
                value: "1".into(),
            },
            EncoderParameter {
                name: "noise-adaptive-filtering".into(),
                value: "0".into(),
            },
            EncoderParameter {
                name: "hbd-mds".into(),
                value: "1".into(),
            },
        ];
        let job = manager
            .start_encode(request(&input, &output, s))
            .await
            .unwrap();
        let done = finish(&manager, &job.id).await;
        assert_eq!(done.state, JobState::Succeeded, "{done:#?}");
        assert!(output.exists());
        eprintln!(
            "{backend:?}: canonical HDR/static+DOVI+HDR10plus+source-only scalars passed every-frame native verifier"
        );
    }
    assert_eq!(std::fs::read(rpu).unwrap(), original_rpu);
    assert_eq!(std::fs::read(json).unwrap(), original_json);
    manager.shutdown().await;
}

#[tokio::test]
#[ignore = "requires pinned HDR provider, qualified FFmpeg/FFprobe XPSNR and AV1AN"]
async fn source_target_uses_actual_engine_bounds_and_input_resolution_instead_of_hidden_draft() {
    let f = Fixture::new();
    let input = source(&f.0, false).await;
    let output = f.0.join("target.mkv");
    let manager = JobManager::open(f.0.join("logs"), f.0.join("history")).await;
    let mut s = settings(EncodeBackend::Av1an);
    let options = s.av1an_options.as_mut().unwrap();
    options.source_target_defaults = true;
    options.target_quality = Some(Av1anTargetQuality {
        metric: Av1anTargetMetric::XpsnrWeighted,
        minimum_score_tenths: 350,
        maximum_score_tenths: 0,
        minimum_crf: 0,
        maximum_crf: 0,
        probes: 0,
        probing_rate: 255,
        probe_width: 1,
        probe_height: 3,
    });
    let job = manager
        .start_encode(request(&input, &output, s))
        .await
        .unwrap();
    let queue_path =
        f.0.join(format!(".jesses-{}.av1an/chunks/chunks.json", job.id));
    let capture = async {
        tokio::time::timeout(Duration::from_secs(90), async {
            loop {
                if let Ok(bytes) = std::fs::read(&queue_path)
                    && let Ok(queue) = serde_json::from_slice::<serde_json::Value>(&bytes)
                {
                    std::fs::write(f.0.join("target-queue.json"), &bytes).unwrap();
                    return queue;
                }
                tokio::time::sleep(Duration::from_millis(2)).await;
            }
        })
        .await
        .expect("engine queue must be captured before owned-workspace cleanup")
    };
    let (done, queue) = tokio::join!(finish(&manager, &job.id), capture);
    assert_eq!(done.state, JobState::Succeeded, "{done:#?}");
    let target = &queue[0]["target_quality"];
    assert_eq!(target["min_q"], 15);
    assert_eq!(target["max_q"], 50);
    assert_eq!(target["probes"], 4);
    assert_eq!(target["probing_rate"], 1);
    assert_eq!(target["target"], serde_json::json!([34.65, 35.35]));
    assert_eq!(target["vmaf_res"], "1920x1080");
    assert!(target["probe_res"].is_null());
    assert!(target["video_params"].is_null());
    assert_eq!(target["params_copied"], false);
    assert_eq!(target["probing_statistic"]["name"], "Automatic");
    assert_eq!(target["source_unfiltered_probes"], true);
    assert!(
        done.encode_settings
            .as_ref()
            .unwrap()
            .av1an_options
            .unwrap()
            .source_target_defaults
    );
    assert!(
        done.logs
            .iter()
            .any(|line| line.contains("encoder search 15–50")
                && line.contains("probe resolution=input 192×128")),
        "{:#?}",
        done.logs
    );
    assert!(output.exists());
    eprintln!(
        "source target completed with effective SVT15–50, probes4, sampling1, input192x128 and Automatic/null probe receipt"
    );
    manager.shutdown().await;
}

fn source_target_settings() -> EncodeSettings {
    let mut selected = settings(EncodeBackend::Av1an);
    let options = selected.av1an_options.as_mut().unwrap();
    options.source_target_defaults = true;
    options.maximum_chunk_frames = 16;
    options.target_quality = Some(Av1anTargetQuality {
        metric: Av1anTargetMetric::XpsnrWeighted,
        minimum_score_tenths: 350,
        maximum_score_tenths: 0,
        minimum_crf: 0,
        maximum_crf: 0,
        probes: 0,
        probing_rate: 255,
        probe_width: 1,
        probe_height: 3,
    });
    selected
}

#[tokio::test]
#[ignore = "requires pinned HDR provider, qualified XPSNR engine and FFmpeg/FFprobe"]
async fn canceled_source_target_saved_and_new_resume_verify_and_regenerate_only_owned_caches() {
    for new_settings in [false, true] {
        let f = Fixture::new();
        let input = source_frames(&f.0, false, 32).await;
        let original = std::fs::read(&input).unwrap();
        let output = f.0.join("resumed-target.mkv");
        let history = f.0.join("history");
        let logs = f.0.join("logs");
        let manager = JobManager::open(logs.clone(), history.clone()).await;
        let s = source_target_settings();
        let job = manager
            .start_encode(request(&input, &output, s.clone()))
            .await
            .unwrap();
        let workspace = f.0.join(format!(".jesses-{}.av1an", job.id));
        let split = workspace.join("chunks/split");
        tokio::time::timeout(Duration::from_secs(60), async {
            loop {
                if std::fs::metadata(split.join("0.json")).is_ok_and(|metadata| metadata.len() > 0)
                {
                    break;
                }
                tokio::time::sleep(Duration::from_millis(2)).await;
            }
        })
        .await
        .expect("an actual target metric must be active before cancellation");
        manager.stop_job(job.id.clone()).await.unwrap();
        assert_eq!(finish(&manager, &job.id).await.state, JobState::Stopped);
        manager.shutdown().await;
        drop(manager);
        assert!(!output.exists());
        let manifest_path = workspace.join("manifest.json");
        let manifest: serde_json::Value =
            serde_json::from_slice(&std::fs::read(&manifest_path).unwrap()).unwrap();
        let artifacts = manifest["target_artifacts"]
            .as_array()
            .expect("quiescent target files must be sealed");
        assert!(!artifacts.is_empty());
        let cache = PathBuf::from(artifacts[0]["path"].as_str().unwrap());
        let cache_bytes = std::fs::read(&cache).unwrap();
        let manager = JobManager::open(logs, history).await;
        manager.ready().await.unwrap();
        // Changed existing bytes and foreign files fail before any cleanup.
        std::fs::write(&cache, b"changed metric cache").unwrap();
        let error = manager.resume_job(job.id.clone()).await.unwrap_err();
        assert_eq!(error.code, "RECOVERY_INVALID");
        assert!(
            error.message.contains("metric target cache changed"),
            "{error:?}"
        );
        assert!(cache.exists());
        std::fs::write(&cache, &cache_bytes).unwrap();
        let foreign = split.join("not-an-engine-cache.json");
        std::fs::write(&foreign, b"foreign must survive").unwrap();
        assert_eq!(
            manager.resume_job(job.id.clone()).await.unwrap_err().code,
            "RECOVERY_INVALID"
        );
        assert_eq!(std::fs::read(&foreign).unwrap(), b"foreign must survive");
        std::fs::remove_file(foreign).unwrap();
        if new_settings {
            let mut changed = s;
            changed
                .av1an_options
                .as_mut()
                .unwrap()
                .target_quality
                .as_mut()
                .unwrap()
                .minimum_score_tenths = 360;
            manager
                .av1an_recovery_action(Av1anRecoveryAction::ResumeNew {
                    id: job.id.clone(),
                    settings: Box::new(changed),
                })
                .await
                .unwrap();
        } else {
            // A missing disposable cache is allowed; saved queue/source/tool
            // receipts remain authoritative and probes are regenerated.
            std::fs::remove_file(&cache).unwrap();
            manager.resume_job(job.id.clone()).await.unwrap();
        }
        let done = finish(&manager, &job.id).await;
        assert_eq!(done.state, JobState::Succeeded, "{done:#?}");
        assert!(
            done.encode_settings
                .as_ref()
                .unwrap()
                .av1an_options
                .unwrap()
                .source_target_defaults
        );
        manager.shutdown().await;
        assert!(output.exists());
        assert!(!workspace.exists());
        assert_eq!(std::fs::read(&input).unwrap(), original);
        eprintln!(
            "source target {} resume: real active-metric cancellation, sealed-cache mismatch/foreign rejection, owned cache regeneration and32-frame output verified",
            if new_settings { "New" } else { "Saved" }
        );
    }
}

#[tokio::test]
#[ignore = "requires qualified source-probe engine, SVT HDR package and FFmpeg/FFprobe"]
async fn source_target_keeps_original_probes_while_final_crop_runs_per_chunk() {
    let f = Fixture::new();
    let input = source(&f.0, false).await;
    let output = f.0.join("cropped-target.mkv");
    let manager = JobManager::open(f.0.join("logs"), f.0.join("history")).await;
    let mut s = source_target_settings();
    s.av1an_options.as_mut().unwrap().maximum_chunk_frames = 0;
    s.framing.crop.left = 16;
    s.framing.crop.right = 16;
    let job = manager
        .start_encode(request(&input, &output, s))
        .await
        .unwrap();
    let done = finish(&manager, &job.id).await;
    assert_eq!(done.state, JobState::Succeeded, "{done:#?}");
    assert!(
        !done
            .logs
            .iter()
            .any(|line| line.contains("Preparing one verified lossless processed source")),
        "source probes must keep the original source"
    );
    assert_eq!(pixels(&output).await.len(), 160 * 128 * 3 * 4);
    let engine = std::fs::read_to_string(
        Path::new(done.log_path.as_ref().unwrap()).with_extension("av1an.log"),
    )
    .unwrap();
    assert!(engine.contains("source_unfiltered_probes: true"));
    assert!(engine.contains("vmaf_filter: None, ffmpeg_filter_args: []"));
    assert!(engine.contains("Input: 192x128"));
    manager.shutdown().await;
    eprintln!(
        "source target finalcrop160x128 verified; original192x128 probes/reference retained with explicit source-probe receipt flag"
    );
}

async fn direct_source_svt_reference(input: &Path, output: &Path, graph: &str, root: &Path) {
    let bundle = PathBuf::from(std::env::var_os("JESSES_TEST_TOOL_RESOURCES").unwrap());
    let mut producer = args(&["-v", "warning", "-nostdin", "-i"]);
    producer.push(input.as_os_str().to_owned());
    producer.extend(args(&[
        "-map",
        "0:0",
        "-vf",
        graph,
        "-pix_fmt",
        "yuv420p10le",
        "-an",
        "-sn",
        "-dn",
        "-f",
        "yuv4mpegpipe",
        "-strict",
        "-1",
        "-",
    ]));
    let consumer = args(&[
        "-i",
        "stdin",
        "-b",
        "stdout",
        "--preset",
        "12",
        "--crf",
        "30",
        "--color-primaries",
        "1",
        "--transfer-characteristics",
        "1",
        "--matrix-coefficients",
        "1",
        "--color-range",
        "0",
        "--chroma-sample-position",
        "left",
        "--film-grain",
        "0",
        "--film-grain-denoise",
        "0",
        "--lp",
        "1",
    ]);
    let (_owner, cancel) = watch::channel(false);
    let (events, _receiver) = tokio::sync::mpsc::channel(256);
    let file = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(output)
        .unwrap();
    let result = media_runtime::supervisor::run_pipeline_to_file(
        &CommandSpec {
            executable: std::env::var_os("JESSES_FFMPEG").unwrap().into(),
            args: producer,
            cwd: None,
        },
        &CommandSpec {
            executable: bundle.join("resources/tools/svt-av1-hdr/SvtAv1EncApp"),
            args: consumer,
            cwd: None,
        },
        cancel,
        events,
        &root.join("source-reference.log"),
        Duration::from_secs(60),
        file,
    )
    .await
    .unwrap();
    assert!(result.producer_status.success() && result.consumer_status.success());
}
async fn video_summary(path: &Path) -> serde_json::Value {
    let (_owner, cancel) = watch::channel(false);
    let mut a = args(&[
        "-v",
        "error",
        "-count_frames",
        "-select_streams",
        "v:0",
        "-show_entries",
        "stream=codec_name,width,height,pix_fmt,avg_frame_rate,r_frame_rate,nb_read_frames,sample_aspect_ratio",
        "-of",
        "json",
        "-i",
    ]);
    a.push(path.as_os_str().to_owned());
    let result = run_capture(
        &CommandSpec {
            executable: std::env::var_os("JESSES_FFPROBE").unwrap().into(),
            args: a,
            cwd: None,
        },
        cancel,
        1024 * 1024,
        Duration::from_secs(60),
    )
    .await
    .unwrap();
    assert!(result.status.success());
    serde_json::from_slice::<serde_json::Value>(&result.stdout).unwrap()["streams"][0].clone()
}
async fn decoded_clock(path: &Path) -> Vec<f64> {
    let (_owner, cancel) = watch::channel(false);
    let mut a = args(&[
        "-v",
        "error",
        "-select_streams",
        "v:0",
        "-show_frames",
        "-show_entries",
        "frame=best_effort_timestamp_time",
        "-of",
        "json",
        "-i",
    ]);
    a.push(path.as_os_str().to_owned());
    let result = run_capture(
        &CommandSpec {
            executable: std::env::var_os("JESSES_FFPROBE").unwrap().into(),
            args: a,
            cwd: None,
        },
        cancel,
        1024 * 1024,
        Duration::from_secs(60),
    )
    .await
    .unwrap();
    assert!(result.status.success());
    serde_json::from_slice::<serde_json::Value>(&result.stdout).unwrap()["frames"]
        .as_array()
        .unwrap()
        .iter()
        .map(|frame| {
            frame["best_effort_timestamp_time"]
                .as_str()
                .unwrap()
                .parse()
                .unwrap()
        })
        .collect()
}
fn effective_rate(stream: &serde_json::Value) -> f64 {
    for key in ["avg_frame_rate", "r_frame_rate"] {
        if let Some((n, d)) = stream[key].as_str().and_then(|s| s.split_once('/')) {
            let (n, d) = (n.parse::<f64>().unwrap(), d.parse::<f64>().unwrap());
            if n > 0.0 && d > 0.0 {
                return n / d;
            }
        }
    }
    panic!("No valid nominal rate: {stream}")
}
#[tokio::test]
#[ignore = "requires qualified CPU FFmpeg and pinned SVT HDR provider"]
async fn source_cpu_svt_raw_geometry_fps_and_pts_match_the_actual_fork_pipe() {
    for (name, graph) in [
        ("geometry", "scale=128:96,setsar=1,negate"),
        ("fps", "fps=2"),
        ("pts", "setpts=2*PTS"),
    ] {
        let f = Fixture::new();
        let input = source(&f.0, false).await;
        let original = std::fs::read(&input).unwrap();
        let reference = f.0.join("source-reference.ivf");
        direct_source_svt_reference(&input, &reference, graph, &f.0).await;
        let output = f.0.join("native.mkv");
        let manager = JobManager::open(f.0.join("logs"), f.0.join("history")).await;
        let mut s = settings(EncodeBackend::Standalone);
        s.source_filters = true;
        s.av1an_filters = vec![graph.into()];
        let job = manager
            .start_encode(request(&input, &output, s))
            .await
            .unwrap();
        let done = finish(&manager, &job.id).await;
        assert_eq!(done.state, JobState::Succeeded, "{name}: {done:#?}");
        let expected = video_summary(&reference).await;
        let actual = video_summary(&output).await;
        for field in ["codec_name", "width", "height", "pix_fmt", "nb_read_frames"] {
            assert_eq!(
                actual[field], expected[field],
                "{name} {field}: source={expected:?}, native={actual:?}"
            );
        }
        assert!(
            (effective_rate(&actual) - effective_rate(&expected)).abs() < 0.0001,
            "{name}: source={expected}, native={actual}"
        );
        let expected_clock = decoded_clock(&reference).await;
        let actual_clock = decoded_clock(&output).await;
        assert_eq!(actual_clock.len(), expected_clock.len());
        for (index, (a, b)) in actual_clock.iter().zip(&expected_clock).enumerate() {
            assert!(
                ((a - actual_clock[0]) - (b - expected_clock[0])).abs() <= 0.002,
                "{name} frame{index}: {actual_clock:?} vs{expected_clock:?}"
            );
        }
        assert_eq!(std::fs::read(&input).unwrap(), original);
        manager.shutdown().await;
        eprintln!(
            "Direct SVT raw{name}: {}frames at{}fps, {}x{} matches source's literal Y4M pipe and full decoded clock (filter PTS are not compared to coded-clock ticks)",
            actual["nb_read_frames"],
            effective_rate(&actual),
            actual["width"],
            actual["height"]
        );
    }
}

async fn direct_annex_b_reference(
    input: &Path,
    output: &Path,
    graph: &str,
    root: &Path,
    hevc: bool,
) {
    let raw = root.join(if hevc {
        "reference.265"
    } else {
        "reference.264"
    });
    let mut producer = args(&["-v", "warning", "-nostdin", "-i"]);
    producer.push(input.as_os_str().to_owned());
    producer.extend(args(&[
        "-map",
        "0:0",
        "-vf",
        graph,
        "-pix_fmt",
        "yuv420p10le",
        "-an",
        "-sn",
        "-dn",
        "-f",
        "yuv4mpegpipe",
        "-strict",
        "-1",
        "-",
    ]));
    let consumer = if hevc {
        args(&[
            "--y4m",
            "--input",
            "-",
            "--output",
            "-",
            "--preset",
            "ultrafast",
            "--crf",
            "30",
            "--output-depth",
            "10",
            "--colorprim",
            "1",
            "--transfer",
            "1",
            "--colormatrix",
            "1",
            "--range",
            "limited",
            "--pools",
            "1",
            "--frame-threads",
            "1",
        ])
    } else {
        args(&[
            "--demuxer",
            "y4m",
            "--muxer",
            "raw",
            "-o",
            "-",
            "-",
            "--preset",
            "ultrafast",
            "--crf",
            "30",
            "--output-depth",
            "10",
            "--colorprim",
            "bt709",
            "--transfer",
            "bt709",
            "--colormatrix",
            "bt709",
            "--range",
            "tv",
            "--threads",
            "1",
        ])
    };
    let (_owner, cancel) = watch::channel(false);
    let (events, _receiver) = tokio::sync::mpsc::channel(256);
    let file = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&raw)
        .unwrap();
    let result = media_runtime::supervisor::run_pipeline_to_file(
        &CommandSpec {
            executable: std::env::var_os("JESSES_FFMPEG").unwrap().into(),
            args: producer,
            cwd: None,
        },
        &CommandSpec {
            executable: PathBuf::from(if hevc {
                "/usr/bin/x265"
            } else {
                "/usr/bin/x264"
            }),
            args: consumer,
            cwd: None,
        },
        cancel.clone(),
        events,
        &root.join("source-reference.log"),
        Duration::from_secs(60),
        file,
    )
    .await
    .unwrap();
    assert!(
        result.producer_status.success() && result.consumer_status.success(),
        "producer={:?}, consumer={:?}",
        result.producer_status,
        result.consumer_status
    );
    // Source GetPostFilterRate is derived from the original/UI cadence, not
    // arbitrary raw fps rows. Annex B is wrapped at that explicit source rate.
    let result = run_capture(
        &CommandSpec {
            executable: "/usr/bin/mkvmerge".into(),
            args: args(&["--disable-track-statistics-tags", "-o"])
                .into_iter()
                .chain([output.as_os_str().to_owned()])
                .chain(args(&["--default-duration", "0:4fps"]))
                .chain([raw.as_os_str().to_owned()])
                .collect(),
            cwd: None,
        },
        cancel,
        1024 * 1024,
        Duration::from_secs(30),
    )
    .await
    .unwrap();
    assert!(
        matches!(result.status.code(), Some(0 | 1)),
        "{}",
        String::from_utf8_lossy(&result.stderr)
    );
}
#[tokio::test]
#[ignore = "requires qualified CPU FFmpeg, installed x264/x265 and mkvmerge"]
async fn source_cpu_annex_b_raw_sar_and_clock_match_source_wrapper_precedence() {
    for hevc in [false, true] {
        for (name, graph) in [
            ("geometry-sar", "scale=128:96,setsar=2,negate"),
            ("fps", "fps=2"),
            ("pts", "setpts=2*PTS"),
        ] {
            let f = Fixture::new();
            let input = source_frames(&f.0, false, 8).await;
            let original = std::fs::read(&input).unwrap();
            let reference = f.0.join("source-reference.mkv");
            direct_annex_b_reference(&input, &reference, graph, &f.0, hevc).await;
            let output = f.0.join("native.mkv");
            let manager = JobManager::open(f.0.join("logs"), f.0.join("history")).await;
            let mut s = settings(EncodeBackend::Standalone);
            s.encoder = if hevc {
                VideoEncoder::X265Standalone
            } else {
                VideoEncoder::X264
            };
            s.preset = 0;
            s.source_filters = true;
            s.av1an_filters = vec![graph.into()];
            s.output_pixel_format = Some(Av1anPixelFormat::Yuv420p10le);
            let job = manager
                .start_encode(request(&input, &output, s))
                .await
                .unwrap();
            let done = finish(&manager, &job.id).await;
            assert_eq!(done.state, JobState::Succeeded, "{hevc}/{name}: {done:#?}");
            let expected = video_summary(&reference).await;
            let actual = video_summary(&output).await;
            for field in [
                "codec_name",
                "width",
                "height",
                "pix_fmt",
                "nb_read_frames",
                "sample_aspect_ratio",
            ] {
                assert_eq!(
                    actual[field], expected[field],
                    "{hevc}/{name}/{field}: source={expected:?}, native={actual:?}"
                );
            }
            assert!(
                (effective_rate(&actual) - effective_rate(&expected)).abs() < 0.0001,
                "{hevc}/{name}: source={expected}, native={actual}"
            );
            let expected_clock = decoded_clock(&reference).await;
            let actual_clock = decoded_clock(&output).await;
            assert_eq!(actual_clock.len(), expected_clock.len());
            for (a, b) in actual_clock.iter().zip(&expected_clock) {
                assert!(
                    ((a - actual_clock[0]) - (b - expected_clock[0])).abs() <= 0.002,
                    "{hevc}/{name}: {actual_clock:?} vs{expected_clock:?}"
                );
            }
            assert_eq!(std::fs::read(&input).unwrap(), original);
            manager.shutdown().await;
            eprintln!(
                "{} raw{name}: {}frames at{}fps SAR{} matches source AnnexB+explicit4fps wrapper",
                if hevc { "x265" } else { "x264" },
                actual["nb_read_frames"],
                effective_rate(&actual),
                actual["sample_aspect_ratio"]
            );
        }
    }
}

#[tokio::test]
#[ignore = "requires qualified CPU FFmpeg and pinned SVT HDR provider"]
async fn source_empty_grid_vfr_clock_matches_literal_y4m_default() {
    let f = Fixture::new();
    let input = f.0.join("vfr.mkv");
    let mut a = args(&[
        "-v",
        "error",
        "-f",
        "lavfi",
        "-i",
        "testsrc2=size=192x128:rate=4:duration=2",
        "-vf",
        "select='not(eq(n,2))'",
        "-fps_mode",
        "passthrough",
        "-c:v",
        "ffv1",
        "-level",
        "3",
        "-pix_fmt",
        "yuv420p10le",
        "-color_primaries",
        "bt709",
        "-color_trc",
        "bt709",
        "-colorspace",
        "bt709",
        "-color_range",
        "tv",
        "-chroma_sample_location",
        "left",
    ]);
    a.push(input.as_os_str().to_owned());
    ffmpeg(a).await;
    let original = std::fs::read(&input).unwrap();
    let reference = f.0.join("source.ivf");
    direct_source_svt_reference(&input, &reference, "null", &f.0).await;
    let manager = JobManager::open(f.0.join("logs"), f.0.join("history")).await;
    let output = f.0.join("native.mkv");
    let job = manager
        .start_encode(request(
            &input,
            &output,
            settings(EncodeBackend::Standalone),
        ))
        .await
        .unwrap();
    let done = finish(&manager, &job.id).await;
    assert_eq!(done.state, JobState::Succeeded, "{done:#?}");
    let expected = video_summary(&reference).await;
    let actual = video_summary(&output).await;
    assert_eq!(
        actual["nb_read_frames"], expected["nb_read_frames"],
        "source={expected}, native={actual}"
    );
    assert!(
        (effective_rate(&actual) - effective_rate(&expected)).abs() < 0.0001,
        "source={expected}, native={actual}"
    );
    let expected_clock = decoded_clock(&reference).await;
    let actual_clock = decoded_clock(&output).await;
    for (a, b) in actual_clock.iter().zip(&expected_clock) {
        assert!(((a - actual_clock[0]) - (b - expected_clock[0])).abs() <= 0.002);
    }
    assert_eq!(std::fs::read(&input).unwrap(), original);
    manager.shutdown().await;
    eprintln!(
        "Empty-grid VFR: source/native {}frames at{}fps and every decoded clock agree",
        actual["nb_read_frames"],
        effective_rate(&actual)
    );
}

#[tokio::test]
#[ignore = "requires qualified source AV1AN/SVT/CPU FFmpeg providers"]
async fn source_av1an_raw_geometry_stateful_chunks_and_real_frame_mismatch() {
    for (name, graph, success) in [
        ("geometry", "scale=128:96,setsar=1,negate", true),
        ("stateful", "negate=enable='eq(n,0)'", true),
        ("qtgmc-stateful", "negate=enable='eq(n,0)'", true),
        ("frame-mismatch", "select='not(eq(n,0))'", false),
    ] {
        let f = Fixture::new();
        let mut input = source_frames(&f.0, false, 8).await;
        let qtgmc = name == "qtgmc-stateful";
        if qtgmc {
            let interlaced = f.0.join("interlaced.mkv");
            let mut a = args(&["-v", "error", "-i"]);
            a.push(input.as_os_str().to_owned());
            a.extend(args(&[
                "-vf",
                "setfield=tff",
                "-c:v",
                "ffv1",
                "-level",
                "3",
                "-threads",
                "2",
                "-chroma_sample_location",
                "left",
            ]));
            a.push(interlaced.as_os_str().to_owned());
            ffmpeg(a).await;
            input = interlaced;
        }
        let frames = if qtgmc { 16 } else { 8 };
        let original = std::fs::read(&input).unwrap();
        let output = f.0.join("native.mkv");
        let manager = JobManager::open(f.0.join("logs"), f.0.join("history")).await;
        let mut s = settings(EncodeBackend::Av1an);
        if qtgmc {
            s.temporal = Some(TemporalSettings {
                qtgmc: Some(QtgmcSettings {
                    mode: DeinterlaceMode::Bob,
                    field_order: FieldOrder::TopFirst,
                    preset: QtgmcPreset::Fast,
                }),
                ..Default::default()
            });
        }
        s.source_filters = true;
        s.av1an_filters = vec![graph.into()];
        s.av1an_options.as_mut().unwrap().maximum_chunk_frames = 4;
        let job = manager
            .start_encode(request(&input, &output, s))
            .await
            .unwrap();
        let queue =
            f.0.join(format!(".jesses-{}.av1an/chunks/chunks.json", job.id));
        let saved_queue = f.0.join("actual-queue.json");
        let (stop, captured_cancel) = watch::channel(false);
        let saved_capture = saved_queue.clone();
        let capture = tokio::spawn(async move {
            while !*captured_cancel.borrow() {
                if let Ok(bytes) = std::fs::read(&queue)
                    && serde_json::from_slice::<serde_json::Value>(&bytes).is_ok()
                {
                    std::fs::write(&saved_capture, bytes).unwrap();
                }
                tokio::time::sleep(Duration::from_millis(2)).await;
            }
        });
        let done = finish(&manager, &job.id).await;
        stop.send_replace(true);
        capture.await.unwrap();
        if success {
            assert_eq!(done.state, JobState::Succeeded, "{name}: {done:#?}");
            let actual = video_summary(&output).await;
            assert_eq!(actual["nb_read_frames"], frames.to_string());
            assert_eq!(actual["width"], if name == "geometry" { 128 } else { 192 });
            if name.ends_with("stateful") {
                let samples = pixels(&output).await;
                let frame = 192 * 128 * 3;
                assert_eq!(samples.len(), frame * frames);
                let y = 192 * 128 * 2;
                let average = |n: usize| {
                    samples[n * frame..n * frame + y]
                        .as_chunks::<2>()
                        .0
                        .iter()
                        .map(|p| u16::from_le_bytes([p[0], p[1]]) as f64)
                        .sum::<f64>()
                        / (192 * 128) as f64
                };
                let means = (0..frames).map(average).collect::<Vec<_>>();
                let queue: serde_json::Value =
                    serde_json::from_slice(&std::fs::read(&saved_queue).unwrap()).unwrap();
                let starts = queue
                    .as_array()
                    .unwrap()
                    .iter()
                    .map(|chunk| chunk["start_frame"].as_u64().unwrap() as usize)
                    .collect::<Vec<_>>();
                assert!(starts.len() > 1);
                for (index, mean) in means.iter().enumerate() {
                    assert_eq!(
                        *mean > 520.0,
                        starts.contains(&index),
                        "only the first frame of each actual queued chunk is negated: starts={starts:?}, means={means:?}"
                    );
                }
                eprintln!("Actual AV1AN stateful chunk starts{starts:?}, luma means{means:?}");
            }
        } else {
            assert_eq!(done.state, JobState::Failed, "{name}: {done:#?}");
            assert!(!output.exists());
            let engine = std::fs::read_to_string(
                Path::new(done.log_path.as_ref().unwrap()).with_extension("av1an.log"),
            )
            .unwrap_or_default();
            assert!(
                engine.contains("FRAME MISMATCH"),
                "actual source engine must reject changed count: {engine}"
            );
        }
        assert_eq!(std::fs::read(&input).unwrap(), original);
        manager.shutdown().await;
        eprintln!("Actual AV1AN source raw{name}: {:?}", done.state);
    }
}

#[tokio::test]
#[ignore = "requires qualified source AV1AN/SVT/CPU FFmpeg providers"]
async fn source_raw_filters_saved_and_new_resume_keep_sealed_chunk_authority() {
    for new_settings in [false, true] {
        let f = Fixture::new();
        let input = source_frames(&f.0, false, 24).await;
        let original = std::fs::read(&input).unwrap();
        let output = f.0.join("resumed.mkv");
        let logs = f.0.join("logs");
        let history = f.0.join("history");
        let manager = JobManager::open(logs.clone(), history.clone()).await;
        let mut settings = settings(EncodeBackend::Av1an);
        settings.source_filters = true;
        settings.av1an_filters = vec!["scale=128:96,realtime=speed=1,negate".into()];
        settings
            .av1an_options
            .as_mut()
            .unwrap()
            .maximum_chunk_frames = 6;
        let job = manager
            .start_encode(request(&input, &output, settings.clone()))
            .await
            .unwrap();
        let workspace = f.0.join(format!(".jesses-{}.av1an", job.id));
        let done_path = workspace.join("chunks/done.json");
        tokio::time::timeout(Duration::from_secs(30), async {
            loop {
                if let Ok(bytes) = std::fs::read(&done_path)
                    && let Ok(value) = serde_json::from_slice::<serde_json::Value>(&bytes)
                    && value["done"]
                        .as_object()
                        .is_some_and(|done| !done.is_empty())
                {
                    break;
                }
                tokio::time::sleep(Duration::from_millis(2)).await;
            }
        })
        .await
        .unwrap();
        manager.stop_job(job.id.clone()).await.unwrap();
        let stopped = finish(&manager, &job.id).await;
        assert_eq!(stopped.state, JobState::Stopped, "{stopped:#?}");
        manager.shutdown().await;
        drop(manager);
        assert!(!output.exists());
        let manifest: serde_json::Value =
            serde_json::from_slice(&std::fs::read(workspace.join("manifest.json")).unwrap())
                .unwrap();
        assert_eq!(manifest["settings"]["sourceFilters"], true);
        let completed = manifest["completed"].as_object().unwrap();
        assert!(!completed.is_empty());
        let sealed = PathBuf::from(
            completed.values().next().unwrap()["file"]["path"]
                .as_str()
                .unwrap(),
        );
        let sealed_bytes = std::fs::read(&sealed).unwrap();
        let queue = workspace.join("chunks/chunks.json");
        let queue_bytes = std::fs::read(&queue).unwrap();
        let manager = JobManager::open(logs, history).await;
        manager.ready().await.unwrap();
        std::fs::write(&queue, b"[]").unwrap();
        assert_eq!(
            manager.resume_job(job.id.clone()).await.unwrap_err().code,
            "RECOVERY_INVALID"
        );
        assert!(queue.exists());
        std::fs::write(&queue, &queue_bytes).unwrap();
        let hidden = f.0.join("retained-queue.json");
        std::fs::rename(&queue, &hidden).unwrap();
        assert_eq!(
            manager.resume_job(job.id.clone()).await.unwrap_err().code,
            "RECOVERY_INVALID"
        );
        std::fs::rename(&hidden, &queue).unwrap();
        let mut changed = sealed_bytes.clone();
        changed[40] ^= 1;
        std::fs::write(&sealed, changed).unwrap();
        assert_eq!(
            manager.resume_job(job.id.clone()).await.unwrap_err().code,
            "RECOVERY_INVALID"
        );
        std::fs::write(&sealed, &sealed_bytes).unwrap();
        let foreign = workspace.join("chunks/encode/foreign.ivf");
        std::fs::write(&foreign, b"foreign").unwrap();
        assert_eq!(
            manager.resume_job(job.id.clone()).await.unwrap_err().code,
            "RECOVERY_INVALID"
        );
        assert_eq!(std::fs::read(&foreign).unwrap(), b"foreign");
        std::fs::remove_file(foreign).unwrap();
        // Mutable engine done state is never authority: rebuild it exclusively
        // from sealed manifest receipts instead of adopting a forged chunk.
        std::fs::write(
            &done_path,
            br#"{"frames":24,"done":{"99999":{"frames":24,"size_bytes":64}},"audio_done":true}"#,
        )
        .unwrap();
        if new_settings {
            settings.crf = 32;
            manager
                .av1an_recovery_action(Av1anRecoveryAction::ResumeNew {
                    id: job.id.clone(),
                    settings: Box::new(settings),
                })
                .await
                .unwrap();
        } else {
            manager.resume_job(job.id.clone()).await.unwrap();
        }
        let done = finish(&manager, &job.id).await;
        assert_eq!(done.state, JobState::Succeeded, "{done:#?}");
        let actual = video_summary(&output).await;
        assert_eq!(actual["nb_read_frames"], "24");
        assert_eq!(actual["width"], 128);
        assert_eq!(actual["height"], 96);
        assert_eq!(std::fs::read(&input).unwrap(), original);
        assert!(!workspace.exists());
        manager.shutdown().await;
        eprintln!(
            "Raw-filter {} resume: actual cancellation after sealed chunk, altered/missing queue + sealed-byte + foreign-file rejection; forged done discarded,24valid128x96frames",
            if new_settings { "New" } else { "Saved" }
        );
    }
}

#[tokio::test]
#[ignore = "requires qualified CPU FFmpeg and installed x264/mkvmerge"]
async fn source_anamorphic_raw_sar_correction_preserves_actual_display_and_legacy_isolation() {
    let f = Fixture::new();
    let original = source_frames(&f.0, false, 8).await;
    let input = f.0.join("anamorphic.mkv");
    let mut a = args(&["-v", "error", "-i"]);
    a.push(original.as_os_str().to_owned());
    a.extend(args(&[
        "-vf",
        "setsar=8/9",
        "-c:v",
        "ffv1",
        "-level",
        "3",
        "-threads",
        "2",
        "-chroma_sample_location",
        "left",
    ]));
    a.push(input.as_os_str().to_owned());
    ffmpeg(a).await;
    let bytes = std::fs::read(&input).unwrap();
    let encoded = f.0.join("source-encoded.mkv");
    direct_annex_b_reference(&input, &encoded, "scale=128:96,setsar=2", &f.0, false).await;
    let stale = f.0.join("source-stale-aspect.mkv");
    let mut a = args(&["-v", "error", "-i"]);
    a.push(encoded.as_os_str().to_owned());
    a.extend(args(&["-map", "0:v:0", "-c:v", "copy", "-aspect", "4:3"]));
    a.push(stale.as_os_str().to_owned());
    ffmpeg(a).await;
    let stale_summary = video_summary(&stale).await;
    assert_eq!(
        stale_summary["sample_aspect_ratio"], "1:1",
        "the source's pre-filter4:3 DAR overrides custom2:1 SAR on128x96"
    );
    let manager = JobManager::open(f.0.join("logs"), f.0.join("history")).await;
    for raw in [true, false] {
        let output = f.0.join(if raw { "corrected.mkv" } else { "legacy.mkv" });
        let mut settings = settings(EncodeBackend::Standalone);
        settings.encoder = VideoEncoder::X264;
        settings.preset = 0;
        settings.output_pixel_format = Some(Av1anPixelFormat::Yuv420p10le);
        settings.source_filters = raw;
        settings.av1an_filters = vec![
            if raw {
                "scale=128:96,setsar=2"
            } else {
                "negate"
            }
            .into(),
        ];
        let job = manager
            .start_encode(request(&input, &output, settings))
            .await
            .unwrap();
        let done = finish(&manager, &job.id).await;
        assert_eq!(done.state, JobState::Succeeded, "{done:#?}");
        let summary = video_summary(&output).await;
        assert_eq!(summary["width"], if raw { 128 } else { 192 });
        assert_eq!(summary["height"], if raw { 96 } else { 128 });
        assert_eq!(
            summary["sample_aspect_ratio"],
            if raw { "2:1" } else { "8:9" }
        );
        assert_eq!(summary["nb_read_frames"], "8");
        eprintln!(
            "Anamorphic raw{raw}: {}x{},SAR{},DAR{} (source stale mux4:3; corrected raw display8:3; legacy4:3 unchanged)",
            summary["width"],
            summary["height"],
            summary["sample_aspect_ratio"],
            if raw { "8:3" } else { "4:3" }
        );
    }
    assert_eq!(std::fs::read(&input).unwrap(), bytes);
    manager.shutdown().await;
    eprintln!(
        "Retained literal stale-aspect reference and corrected/legacy outputs {}",
        f.0.display()
    );
}
