//! Authored HDR metadata is verified from every decoded output frame.
use media_core::{
    EncodeRequest, EncodeSettings, HdrChromaticity, HdrContentLight, HdrMasteringDisplay,
    HdrMetadataSettings, JobState, RemuxRequest, VideoEncoder,
};
use media_runtime::{
    jobs::JobManager,
    supervisor::{self, CommandSpec},
};
use std::{
    path::PathBuf,
    time::{Duration, SystemTime, UNIX_EPOCH},
};
use tokio::sync::watch;
struct Fixture(PathBuf);
impl Fixture {
    fn new() -> Self {
        let path = std::env::temp_dir().join(format!(
            "jesses-authored-hdr-{}-{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir(&path).unwrap();
        Self(path)
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        if std::env::var_os("JESSES_KEEP_HDR_FIXTURE").is_some() {
            eprintln!("HDR fixture: {}", self.0.display());
        } else {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }
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
async fn finish(manager: &JobManager, id: &str) -> media_core::JobSnapshot {
    tokio::time::timeout(Duration::from_secs(120), async {
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
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await
    .expect("HDR job completed")
}
#[tokio::test]
#[ignore = "requires qualified FFmpeg/FFprobe, SVT4.2, x265, AOM and mkvmerge"]
async fn manual_mastering_and_content_light_are_written_by_all_hdr_routes() {
    let fixture = Fixture::new();
    let input = fixture.0.join("pq-without-static-metadata.mkv");
    let (_owner, cancel) = watch::channel(false);
    let mut args:Vec<std::ffi::OsString>=["-v","error","-nostdin","-n","-f","lavfi","-i","testsrc2=size=128x96:rate=24:duration=1","-vf","setparams=field_mode=prog:range=tv:color_primaries=bt2020:color_trc=smpte2084:colorspace=bt2020nc","-pix_fmt","yuv420p10le","-chroma_sample_location","left","-c:v","ffv1","-level","3"].into_iter().map(Into::into).collect();
    args.push(input.as_os_str().to_owned());
    let generated = supervisor::run_capture(
        &CommandSpec {
            executable: "ffmpeg".into(),
            args,
            cwd: None,
        },
        cancel,
        1024 * 1024,
        Duration::from_secs(20),
    )
    .await
    .unwrap();
    assert!(
        generated.status.success(),
        "{}",
        String::from_utf8_lossy(&generated.stderr)
    );
    let source = std::fs::read(&input).unwrap();
    let manager = JobManager::new(fixture.0.join("logs"));
    for (index, encoder, preset) in [
        (0, VideoEncoder::X265, 0),
        (1, VideoEncoder::X265Standalone, 0),
        (2, VideoEncoder::SvtAv1, 10),
        (3, VideoEncoder::AomAv1, 8),
        (4, VideoEncoder::X265Standalone, 0),
    ] {
        if index == 4 && std::env::var_os("JESSES_AV1AN").is_none() {
            continue;
        }
        let output = fixture.0.join(format!("authored-{index}.mkv"));
        let job = manager
            .start_encode(EncodeRequest {
                source: RemuxRequest {
                    input_path: input.to_string_lossy().into_owned(),
                    output_path: output.to_string_lossy().into_owned(),
                    stream_indices: vec![0],
                },
                settings: EncodeSettings {
                    encoder,
                    preset,
                    backend: if index == 4 {
                        media_core::EncodeBackend::Av1an
                    } else {
                        media_core::EncodeBackend::Standalone
                    },
                    av1an_options: (index == 4).then_some(media_core::Av1anOptions {
                        chunk_method: media_core::Av1anChunkMethod::Select,
                        split_method: media_core::Av1anSplitMethod::FixedChunks,
                        maximum_chunk_frames: 12,
                        minimum_scene_frames: 12,
                        ..Default::default()
                    }),
                    hdr_metadata: Some(authored()),
                    ..Default::default()
                },
            })
            .await
            .unwrap();
        let completed = finish(&manager, &job.id).await;
        assert_eq!(
            completed.state,
            JobState::Succeeded,
            "{encoder:?}: {completed:#?}"
        );
        let (_owner, cancel) = watch::channel(false);
        let probe = supervisor::run_capture(
            &CommandSpec {
                executable: "ffprobe".into(),
                args: vec![
                    "-v".into(),
                    "error".into(),
                    "-select_streams".into(),
                    "v:0".into(),
                    "-show_frames".into(),
                    "-show_entries".into(),
                    "frame=best_effort_timestamp_time:frame_side_data".into(),
                    "-of".into(),
                    "json".into(),
                    output.as_os_str().to_owned(),
                ],
                cwd: None,
            },
            cancel,
            2 * 1024 * 1024,
            Duration::from_secs(30),
        )
        .await
        .unwrap();
        assert!(probe.status.success());
        let document: serde_json::Value = serde_json::from_slice(&probe.stdout).unwrap();
        let frames = document["frames"].as_array().unwrap();
        assert_eq!(frames.len(), 24);
        for frame in frames {
            let sides = frame["side_data_list"].as_array().unwrap();
            let mastering = sides
                .iter()
                .find(|side| side["side_data_type"] == "Mastering display metadata")
                .unwrap();
            let light = sides
                .iter()
                .find(|side| side["side_data_type"] == "Content light level metadata")
                .unwrap();
            let (n, d) = mastering["max_luminance"]
                .as_str()
                .unwrap()
                .split_once('/')
                .unwrap();
            assert!(
                (n.parse::<f64>().unwrap() / d.parse::<f64>().unwrap() - 2000.0).abs() < 0.01,
                "{encoder:?}: {frame}"
            );
            assert_eq!(light["max_content"], 500);
            assert_eq!(light["max_average"], 250);
        }
    }
    manager.shutdown().await;
    assert_eq!(std::fs::read(input).unwrap(), source);
}

fn dynamic_resources(directory: &std::path::Path, frames: usize) -> (PathBuf, PathBuf) {
    std::fs::create_dir_all(directory).unwrap();
    use dolby_vision::rpu::{dovi_rpu::DoviRpu, generate::GenerateConfig};
    use hdr10plus::{
        metadata::{DistributionMaxRgb, Hdr10PlusMetadata},
        metadata_json::generate_json,
    };
    let mut rpus = Vec::new();
    let mut metadata = Vec::new();
    for index in 0..frames {
        let config = GenerateConfig {
            source_max_pq: Some(3000 + index as u16),
            ..Default::default()
        };
        let rpu = DoviRpu::profile81_config(&config).unwrap();
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
    let rpu = directory.join("Dolby Vision imported metadata.rpu");
    let json = directory.join("HDR10 plus imported metadata.json");
    std::fs::write(&rpu, rpus).unwrap();
    std::fs::write(
        &json,
        serde_json::to_vec_pretty(&generate_json(
            &metadata.iter().collect::<Vec<_>>(),
            "Jesses generated fixture",
            "1",
        ))
        .unwrap(),
    )
    .unwrap();
    (rpu, json)
}

#[tokio::test]
#[ignore = "requires metadata-linked SVT5fish/HDR and qualified FFmpeg/FFprobe; AV1AN Select optional via JESSES_AV1AN"]
async fn dynamic_imports_match_every_frame_and_reject_wrong_lengths() {
    use media_core::{Av1anChunkMethod, Av1anOptions, Av1anSplitMethod, EncodeBackend};
    let fixture = Fixture::new();
    let input = fixture.0.join("pq.mkv");
    let (rpu, json) = dynamic_resources(&fixture.0, 6);
    let (_owner, cancel) = watch::channel(false);
    let mut args:Vec<std::ffi::OsString>=["-v","error","-nostdin","-n","-f","lavfi","-i","testsrc2=size=128x96:rate=24:duration=0.25","-vf","setparams=field_mode=prog:range=tv:color_primaries=bt2020:color_trc=smpte2084:colorspace=bt2020nc","-pix_fmt","yuv420p10le","-chroma_sample_location","left","-c:v","ffv1","-level","3"].into_iter().map(Into::into).collect();
    args.push(input.as_os_str().to_owned());
    let generated = supervisor::run_capture(
        &CommandSpec {
            executable: "ffmpeg".into(),
            args,
            cwd: None,
        },
        cancel,
        1024 * 1024,
        Duration::from_secs(20),
    )
    .await
    .unwrap();
    assert!(
        generated.status.success(),
        "{}",
        String::from_utf8_lossy(&generated.stderr)
    );
    let manager = JobManager::new(fixture.0.join("logs"));
    let mut routes = vec![
        (VideoEncoder::SvtAv1FiveFish, EncodeBackend::Standalone),
        (VideoEncoder::SvtAv1Hdr, EncodeBackend::Standalone),
    ];
    if std::env::var_os("JESSES_AV1AN").is_some() {
        routes.push((VideoEncoder::SvtAv1FiveFish, EncodeBackend::Av1an));
    }
    for (index, (encoder, backend)) in routes.into_iter().enumerate() {
        let mut metadata = authored();
        metadata.dolby_vision_rpu_path = Some(rpu.to_string_lossy().into_owned());
        metadata.hdr10_plus_json_path = Some(json.to_string_lossy().into_owned());
        let output = fixture.0.join(format!(
            "dynamic-{index}.{}",
            if index == 1 { "mp4" } else { "mkv" }
        ));
        let job = manager
            .start_encode(EncodeRequest {
                source: RemuxRequest {
                    input_path: input.to_string_lossy().into_owned(),
                    output_path: output.to_string_lossy().into_owned(),
                    stream_indices: vec![0],
                },
                settings: EncodeSettings {
                    encoder,
                    backend,
                    preset: 10,
                    hdr_metadata: Some(metadata),
                    av1an_options: (backend == EncodeBackend::Av1an).then_some(Av1anOptions {
                        split_method: Av1anSplitMethod::FixedChunks,
                        maximum_chunk_frames: 0,
                        chunk_method: Av1anChunkMethod::Select,
                        ..Default::default()
                    }),
                    ..Default::default()
                },
            })
            .await
            .unwrap();
        let completed = finish(&manager, &job.id).await;
        assert_eq!(
            completed.state,
            JobState::Succeeded,
            "{encoder:?}/{backend:?}: {completed:#?}"
        );
        assert!(output.exists());
    }
    let (short_rpu, _) = dynamic_resources(&fixture.0.join("short"), 5);
    let mut metadata = authored();
    metadata.dolby_vision_rpu_path = Some(short_rpu.to_string_lossy().into_owned());
    let output = fixture.0.join("bad-length.mkv");
    let job = manager
        .start_encode(EncodeRequest {
            source: RemuxRequest {
                input_path: input.to_string_lossy().into_owned(),
                output_path: output.to_string_lossy().into_owned(),
                stream_indices: vec![0],
            },
            settings: EncodeSettings {
                encoder: VideoEncoder::SvtAv1FiveFish,
                preset: 10,
                hdr_metadata: Some(metadata),
                ..Default::default()
            },
        })
        .await
        .unwrap();
    let completed = finish(&manager, &job.id).await;
    assert_eq!(completed.error.unwrap().code, "HDR_METADATA_INVALID");
    assert!(!output.exists());
    manager.shutdown().await;
}

#[tokio::test]
#[ignore = "requires qualified FFmpeg/FFprobe libx265, x264 and CPU zscale/tonemap"]
async fn pq_and_hlg_canonicalization_preserve_static_metadata_pixels_and_audio() {
    let fixture = Fixture::new();
    let manager = JobManager::new(fixture.0.join("logs"));
    async fn run(args: Vec<std::ffi::OsString>) {
        let (_owner, cancel) = watch::channel(false);
        let result = supervisor::run_capture(
            &CommandSpec {
                executable: "ffmpeg".into(),
                args,
                cwd: None,
            },
            cancel,
            1024 * 1024,
            Duration::from_secs(30),
        )
        .await
        .unwrap();
        assert!(
            result.status.success(),
            "{}",
            String::from_utf8_lossy(&result.stderr)
        );
    }
    for transfer in ["smpte2084", "arib-std-b67"] {
        let base = fixture.0.join(format!("{transfer}-base.mkv"));
        let mut args:Vec<std::ffi::OsString>=["-v","error","-nostdin","-n","-f","lavfi","-i","testsrc2=size=128x96:rate=24:duration=0.5","-f","lavfi","-i","sine=frequency=440:sample_rate=48000:duration=0.5","-pix_fmt","yuv420p10le","-color_primaries","bt2020","-colorspace","bt2020nc","-color_trc",transfer,"-chroma_sample_location","left","-c:v","libx265","-preset","ultrafast","-x265-params","pools=none:frame-threads=1:master-display=G(13250,34500)B(7500,3000)R(34000,16000)WP(15635,16450)L(10000000,1):max-cll=500,250","-c:a","pcm_s16le"].into_iter().map(Into::into).collect();
        let options = args.iter().position(|arg| arg == "-x265-params").unwrap() + 1;
        args[options]=format!("pools=none:frame-threads=1:colorprim=9:transfer={}:colormatrix=9:range=limited:master-display=G(13250,34500)B(7500,3000)R(34000,16000)WP(15635,16450)L(10000000,1):max-cll=500,250",if transfer=="smpte2084" {16}else{18}).into();
        args.push(base.as_os_str().to_owned());
        run(args).await;
        for kind in ["shifted", "vfr", "rotated"] {
            let input = fixture.0.join(format!(
                "{transfer}-{kind}.{}",
                if kind == "rotated" { "mp4" } else { "mkv" }
            ));
            let mut args: Vec<std::ffi::OsString> = ["-v", "error", "-nostdin", "-n"]
                .into_iter()
                .map(Into::into)
                .collect();
            if kind == "shifted" {
                args.extend([
                    "-copyts".into(),
                    "-itsoffset".into(),
                    "2.5".into(),
                    "-i".into(),
                    base.as_os_str().to_owned(),
                    "-map".into(),
                    "0".into(),
                    "-c".into(),
                    "copy".into(),
                    "-avoid_negative_ts".into(),
                    "disabled".into(),
                ]);
            } else if kind == "vfr" {
                args.extend([
                    "-i".into(),
                    base.as_os_str().to_owned(),
                    "-map".into(),
                    "0".into(),
                    "-vf".into(),
                    "setpts=PTS+if(gte(N\\,6)\\,0.25/TB\\,0)".into(),
                    "-fps_mode".into(),
                    "vfr".into(),
                    "-c:v".into(),
                    "ffv1".into(),
                    "-c:a".into(),
                    "copy".into(),
                ]);
            } else {
                let plain = fixture.0.join(format!("{transfer}-plain.mp4"));
                run(vec![
                    "-v".into(),
                    "error".into(),
                    "-i".into(),
                    base.as_os_str().to_owned(),
                    "-map".into(),
                    "0".into(),
                    "-c:v".into(),
                    "copy".into(),
                    "-c:a".into(),
                    "alac".into(),
                    plain.as_os_str().to_owned(),
                ])
                .await;
                args.extend([
                    "-display_rotation:v:0".into(),
                    "90".into(),
                    "-i".into(),
                    plain.as_os_str().to_owned(),
                    "-map".into(),
                    "0".into(),
                    "-c".into(),
                    "copy".into(),
                ]);
            }
            args.push(input.as_os_str().to_owned());
            run(args).await;
            let original = std::fs::read(&input).unwrap();
            let output = fixture.0.join(format!("{transfer}-{kind}-output.mkv"));
            let tone_map = (transfer == "arib-std-b67").then_some(media_core::ToneMapSettings {
                source_workflow: false,
                algorithm: Some(media_core::ToneMapAlgorithm::Hable),
                source_peak_nits: 1000,
                backend: media_core::ToneMapBackend::Cpu,
                peak_mode: media_core::ToneMapPeakMode::Manual,
                hdr10_base_layer: false,
            });
            let job = manager
                .start_encode(EncodeRequest {
                    source: RemuxRequest {
                        input_path: input.to_string_lossy().into_owned(),
                        output_path: output.to_string_lossy().into_owned(),
                        stream_indices: vec![0, 1],
                    },
                    settings: EncodeSettings {
                        encoder: if tone_map.is_some() {
                            VideoEncoder::X264
                        } else {
                            VideoEncoder::X265
                        },
                        preset: 0,
                        tone_map,
                        ..Default::default()
                    },
                })
                .await
                .unwrap();
            let completed = finish(&manager, &job.id).await;
            assert_eq!(
                completed.state,
                JobState::Succeeded,
                "{transfer}/{kind}: {completed:#?}"
            );
            assert!(
                completed
                    .logs
                    .iter()
                    .any(|line| line.contains("Verified lossless canonical source"))
            );
            assert_eq!(std::fs::read(&input).unwrap(), original);
            let media = media_runtime::probe_media(output.to_string_lossy().into_owned())
                .await
                .unwrap();
            let video = media
                .streams
                .iter()
                .find(|stream| stream.kind == "video")
                .unwrap();
            assert_eq!(
                (video.width, video.height),
                if kind == "rotated" {
                    (Some(96), Some(128))
                } else {
                    (Some(128), Some(96))
                }
            );
            assert!(
                (media.duration_seconds.unwrap() - if kind == "vfr" { 0.75 } else { 0.5 }).abs()
                    < 0.05
            );
        }
    }
    manager.shutdown().await;
    assert!(!std::fs::read_dir(&fixture.0).unwrap().any(|entry| {
        entry
            .unwrap()
            .file_name()
            .to_string_lossy()
            .ends_with(".source")
    }));
}
