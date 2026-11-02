//! Source encode contracts qualified with actual FFmpeg output and owned fixtures.
use media_core::{
    AudioChannelLayoutPolicy, AudioChannels, AudioCodec, AudioNormalization, AudioTrackSettings,
    EncodeRequest, EncodeSettings, LoudnessRequest, LoudnessTarget, RemuxRequest, SubtitleMode,
    SubtitleTrackSettings, VideoEncoder, VideoRateControl, VideoTrim,
};
use media_runtime::{
    JobManager, JobState, measure_loudness,
    supervisor::{CommandSpec, run_capture},
};
use sha2::{Digest, Sha256};
use std::{
    ffi::OsString,
    path::{Path, PathBuf},
    time::{Duration, SystemTime, UNIX_EPOCH},
};

struct Fixture(PathBuf);
impl Fixture {
    fn new() -> Self {
        let p = std::env::temp_dir().join(format!(
            "jesses-source-audio-{}-{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir(&p).unwrap();
        Self(p)
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}
async fn tool(id: &str) -> PathBuf {
    let tools = media_runtime::get_capabilities().await;
    let t = tools.iter().find(|t| t.id == id).unwrap();
    assert!(t.available, "{t:?}");
    t.path.as_ref().unwrap().into()
}
async fn run(ffmpeg: &Path, args: Vec<OsString>) -> Vec<u8> {
    let (_owner, cancel) = tokio::sync::watch::channel(false);
    let r = run_capture(
        &CommandSpec {
            executable: ffmpeg.into(),
            args,
            cwd: None,
        },
        cancel,
        16 * 1024 * 1024,
        Duration::from_secs(90),
    )
    .await
    .unwrap();
    assert!(r.status.success(), "{}", String::from_utf8_lossy(&r.stderr));
    r.stdout
}
fn args(s: &[&str]) -> Vec<OsString> {
    s.iter().map(Into::into).collect()
}
async fn finish(manager: &JobManager, request: EncodeRequest) -> media_runtime::JobSnapshot {
    let id = manager.start_encode(request).await.unwrap().id;
    tokio::time::timeout(Duration::from_secs(120), async {
        loop {
            let job = manager
                .list_jobs()
                .await
                .into_iter()
                .find(|j| j.id == id)
                .unwrap();
            if job.state.is_terminal() {
                return job;
            }
            tokio::time::sleep(Duration::from_millis(25)).await;
        }
    })
    .await
    .unwrap()
}
async fn measure(path: &Path, index: u32) -> media_core::LoudnessResult {
    let (_owner, cancel) = tokio::sync::watch::channel(false);
    measure_loudness(
        LoudnessRequest {
            input_path: path.to_string_lossy().into_owned(),
            stream_index: index,
            channels: AudioChannels::Preserve,
            target_lufs: -16.0,
            peak_limit_dbfs: -1.0,
        },
        cancel,
    )
    .await
    .unwrap()
}

#[tokio::test]
#[ignore = "requires actual FFmpeg 9, FFprobe, x264"]
async fn normalizes_after_explicit_downmix_over_only_trimmed_samples() {
    let f = Fixture::new();
    let ffmpeg = tool("ffmpeg").await;
    let input = f.0.join("source's 5.1.mkv");
    let output = f.0.join("normalized.mkv");
    let wave = "aevalsrc='if(lt(t,6),0.002,0.035)*(0.7+0.3*sin(2*PI*t/3))*sin(2*PI*440*t)|if(lt(t,6),0.003,0.045)*(0.7+0.3*sin(2*PI*t/3))*sin(2*PI*660*t)|0.02*sin(2*PI*880*t)|0|0.01*sin(2*PI*330*t)|0.01*sin(2*PI*550*t)':s=48000:d=12:c=5.1";
    let mut a = args(&[
        "-v",
        "error",
        "-nostdin",
        "-n",
        "-f",
        "lavfi",
        "-i",
        "testsrc2=s=64x64:r=24:d=12",
        "-f",
        "lavfi",
        "-i",
        wave,
        "-map",
        "0:v",
        "-map",
        "1:a",
        "-vf",
        "setparams=field_mode=prog:range=tv:color_primaries=bt709:color_trc=bt709:colorspace=bt709",
        "-c:v",
        "ffv1",
        "-chroma_sample_location",
        "left",
        "-c:a",
        "pcm_s24le",
    ]);
    a.push(input.as_os_str().into());
    run(&ffmpeg, a).await;
    let before = Sha256::digest(std::fs::read(&input).unwrap());
    let manager = JobManager::open(f.0.join("logs"), f.0.join("history")).await;
    let request = EncodeRequest {
        source: RemuxRequest {
            input_path: input.to_string_lossy().into_owned(),
            output_path: output.to_string_lossy().into_owned(),
            stream_indices: vec![0, 1],
        },
        settings: EncodeSettings {
            encoder: VideoEncoder::X264,
            crf: 23,
            preset: 0,
            trim: Some(VideoTrim {
                start_frame: 144,
                end_frame_exclusive: 288,
                time: None,
            }),
            audio: vec![AudioTrackSettings {
                stream_index: 1,
                codec: AudioCodec::Flac,
                bitrate_kbps: 8192,
                channels: AudioChannels::Explicit {
                    count: 2,
                    layout: AudioChannelLayoutPolicy::Standard,
                },
                gain: None,
                normalization: Some(AudioNormalization {
                    target: LoudnessTarget::Lufs16,
                }),
            }],
            ..Default::default()
        },
    };
    let job = finish(&manager, request).await;
    assert_eq!(
        job.state,
        JobState::Succeeded,
        "{:?}\n{:?}",
        job.error,
        job.logs
    );
    let m = measure(&output, 1).await;
    eprintln!(
        "{}: {:?} LUFS, {:?} dBTP, {:?} LU LRA",
        output.display(),
        m.integrated_lufs,
        m.true_peak_dbfs,
        m.loudness_range_lu
    );
    assert!((m.integrated_lufs.unwrap() + 16.0).abs() < 0.2, "{m:?}");
    assert!(m.true_peak_dbfs.unwrap() <= -0.9, "{m:?}");
    let probe = tool("ffprobe").await;
    let mut a = args(&["-v", "error", "-show_streams", "-of", "json"]);
    a.push(output.as_os_str().into());
    let d: serde_json::Value = serde_json::from_slice(&run(&probe, a).await).unwrap();
    assert_eq!(d["streams"][1]["channels"], 2);
    assert_eq!(
        d["streams"][0]["nb_frames"].as_str().unwrap_or("144"),
        "144"
    );
    assert!(
        job.logs
            .iter()
            .any(|line| line.contains("atrim=start_sample=288000:end_sample=576000")),
        "{:?}",
        job.logs
    );
    assert_eq!(Sha256::digest(std::fs::read(&input).unwrap()), before);
    manager.shutdown().await;
}

#[tokio::test]
#[ignore = "requires actual FFmpeg 9, FFprobe, x264"]
async fn independent_burn_retains_mov_text_tracks_and_fractional_size_is_not_rounded() {
    let f = Fixture::new();
    let ffmpeg = tool("ffmpeg").await;
    let input = f.0.join("source.mkv");
    let output = f.0.join("burned.mp4");
    let captions = f.0.join("captions.srt");
    std::fs::write(
        &captions,
        "1\n00:00:00,250 --> 00:00:02,750\nVisible caption\n\n",
    )
    .unwrap();
    let mut a = args(&[
        "-v",
        "error",
        "-nostdin",
        "-n",
        "-f",
        "lavfi",
        "-i",
        "color=black:s=192x128:r=24:d=3",
        "-f",
        "srt",
        "-i",
    ]);
    a.push(captions.as_os_str().into());
    a.extend(args(&[
        "-map",
        "0:v",
        "-map",
        "1:s",
        "-map",
        "1:s",
        "-vf",
        "setparams=field_mode=prog:range=tv:color_primaries=bt709:color_trc=bt709:colorspace=bt709",
        "-c:v",
        "ffv1",
        "-chroma_sample_location",
        "left",
        "-c:s",
        "srt",
    ]));
    a.push(input.as_os_str().into());
    run(&ffmpeg, a).await;
    let before = Sha256::digest(std::fs::read(&input).unwrap());
    let manager = JobManager::open(f.0.join("logs"), f.0.join("history")).await;
    let request = EncodeRequest {
        source: RemuxRequest {
            input_path: input.to_string_lossy().into_owned(),
            output_path: output.to_string_lossy().into_owned(),
            stream_indices: vec![0, 1, 2],
        },
        settings: EncodeSettings {
            encoder: VideoEncoder::X264,
            crf: 23,
            preset: 0,
            rate_control: Some(VideoRateControl::TargetSizeDecimal {
                target_size_tenths_mb: 11,
            }),
            subtitles: vec![
                SubtitleTrackSettings {
                    stream_index: 1,
                    mode: SubtitleMode::MovText,
                    burn_in: true,
                },
                SubtitleTrackSettings {
                    stream_index: 2,
                    mode: SubtitleMode::MovText,
                    burn_in: false,
                },
            ],
            ..Default::default()
        },
    };
    let job = finish(&manager, request).await;
    assert_eq!(
        job.state,
        JobState::Succeeded,
        "{:?}\n{:?}",
        job.error,
        job.logs
    );
    let probe = tool("ffprobe").await;
    let mut a = args(&["-v", "error", "-show_streams", "-of", "json"]);
    a.push(output.as_os_str().into());
    let d: serde_json::Value = serde_json::from_slice(&run(&probe, a).await).unwrap();
    assert_eq!(d["streams"].as_array().unwrap().len(), 3);
    for i in [1, 2] {
        assert_eq!(d["streams"][i]["codec_name"], "mov_text");
    }
    let mut a = args(&["-v", "error", "-i"]);
    a.push(output.as_os_str().into());
    a.extend(args(&[
        "-ss",
        "1",
        "-frames:v",
        "1",
        "-pix_fmt",
        "gray",
        "-f",
        "rawvideo",
        "-",
    ]));
    let pixels = run(&ffmpeg, a).await;
    assert!(
        pixels.iter().filter(|v| **v > 100).count() > 30,
        "burned caption absent"
    );
    assert!(
        job.logs
            .iter()
            .any(|line| line.contains("requested 1153434 bytes")),
        "{:?}",
        job.logs
    );
    assert_eq!(Sha256::digest(std::fs::read(&input).unwrap()), before);
    let copied = f.0.join("copied-video-subrip.mkv");
    let copied_job = finish(
        &manager,
        EncodeRequest {
            source: RemuxRequest {
                input_path: output.to_string_lossy().into_owned(),
                output_path: copied.to_string_lossy().into_owned(),
                stream_indices: vec![0, 1, 2],
            },
            settings: EncodeSettings {
                video_mode: media_core::VideoMode::Copy,
                subtitles: vec![
                    SubtitleTrackSettings {
                        stream_index: 1,
                        mode: SubtitleMode::SubRip,
                        burn_in: false,
                    },
                    SubtitleTrackSettings {
                        stream_index: 2,
                        mode: SubtitleMode::SubRip,
                        burn_in: false,
                    },
                ],
                ..Default::default()
            },
        },
    )
    .await;
    assert_eq!(
        copied_job.state,
        JobState::Succeeded,
        "{:?}\n{:?}",
        copied_job.error,
        copied_job.logs
    );
    manager.shutdown().await;
}

#[tokio::test]
#[ignore = "requires actual FFmpeg 9 and FFprobe"]
async fn qualifies_ffmpeg_numeric_count_defaults_and_rejects_incorrect_layout_warnings() {
    let f = Fixture::new();
    let ffmpeg = tool("ffmpeg").await;
    let input = f.0.join("layoutless-pcm.mkv");
    let mut a = args(&[
        "-v",
        "error",
        "-nostdin",
        "-n",
        "-f",
        "lavfi",
        "-i",
        "anullsrc=r=48000:cl=5.1:d=2",
        "-c:a",
        "pcm_s24le",
    ]);
    a.push(input.as_os_str().into());
    run(&ffmpeg, a).await;
    let before = Sha256::digest(std::fs::read(&input).unwrap());
    let manager = JobManager::open(f.0.join("logs"), f.0.join("history")).await;
    let probe = tool("ffprobe").await;
    for (count, codec, layout) in [
        (3, AudioCodec::Aac, "2.1"),
        (4, AudioCodec::Aac, "4.0"),
        (5, AudioCodec::Flac, "5.0"),
        (7, AudioCodec::Flac, "6.1"),
        (6, AudioCodec::Opus, "5.1"),
    ] {
        let output = f.0.join(format!("count-{count}.mkv"));
        let job = finish(
            &manager,
            EncodeRequest {
                source: RemuxRequest {
                    input_path: input.to_string_lossy().into_owned(),
                    output_path: output.to_string_lossy().into_owned(),
                    stream_indices: vec![0],
                },
                settings: EncodeSettings {
                    video_mode: media_core::VideoMode::Drop,
                    audio: vec![AudioTrackSettings {
                        stream_index: 0,
                        codec,
                        bitrate_kbps: match codec {
                            AudioCodec::Aac => 768,
                            AudioCodec::Opus => 1024,
                            _ => 8192,
                        },
                        channels: if codec == AudioCodec::Opus {
                            AudioChannels::Preserve
                        } else {
                            AudioChannels::Explicit {
                                count,
                                layout: AudioChannelLayoutPolicy::FfmpegDefault,
                            }
                        },
                        gain: None,
                        normalization: None,
                    }],
                    ..Default::default()
                },
            },
        )
        .await;
        assert_eq!(
            job.state,
            JobState::Succeeded,
            "count {count}: {:?}\n{:?}",
            job.error,
            job.logs
        );
        let mut a = args(&["-v", "error", "-show_streams", "-of", "json"]);
        a.push(output.as_os_str().into());
        let d: serde_json::Value = serde_json::from_slice(&run(&probe, a).await).unwrap();
        assert_eq!(d["streams"][0]["channels"], count);
        assert_eq!(d["streams"][0]["channel_layout"], layout);
    }
    for count in [3, 4] {
        let output = f.0.join(format!("unsafe-flac-{count}.mkv"));
        let job = finish(
            &manager,
            EncodeRequest {
                source: RemuxRequest {
                    input_path: input.to_string_lossy().into_owned(),
                    output_path: output.to_string_lossy().into_owned(),
                    stream_indices: vec![0],
                },
                settings: EncodeSettings {
                    video_mode: media_core::VideoMode::Drop,
                    audio: vec![AudioTrackSettings {
                        stream_index: 0,
                        codec: AudioCodec::Flac,
                        bitrate_kbps: 128,
                        channels: AudioChannels::Explicit {
                            count,
                            layout: AudioChannelLayoutPolicy::FfmpegDefault,
                        },
                        gain: None,
                        normalization: None,
                    }],
                    ..Default::default()
                },
            },
        )
        .await;
        assert_eq!(job.state, JobState::Failed);
        assert_eq!(job.error.unwrap().code, "AUDIO_TOOL_UNSUPPORTED");
        assert!(!output.exists());
    }
    assert_eq!(Sha256::digest(std::fs::read(&input).unwrap()), before);
    manager.shutdown().await;
}

#[tokio::test]
#[ignore = "requires actual FFmpeg 9, FFprobe, x264"]
async fn external_track_measured_normalization_enforces_true_peak_when_linear_gain_cannot() {
    let f = Fixture::new();
    let ffmpeg = tool("ffmpeg").await;
    let input = f.0.join("video.mkv");
    let audio = f.0.join("external.mka");
    let output = f.0.join("peak-limited.mkv");
    let mut a = args(&[
        "-v",
        "error",
        "-nostdin",
        "-n",
        "-f",
        "lavfi",
        "-i",
        "testsrc2=s=64x64:r=24:d=8",
        "-vf",
        "setparams=field_mode=prog:range=tv:color_primaries=bt709:color_trc=bt709:colorspace=bt709",
        "-c:v",
        "ffv1",
        "-chroma_sample_location",
        "left",
    ]);
    a.push(input.as_os_str().into());
    run(&ffmpeg, a).await;
    let wave = "aevalsrc='0.018*sin(2*PI*997*t)+if(lt(mod(t,1),0.004),0.85*sin(2*PI*8000*t),0)':s=48000:d=8:c=mono";
    let mut a = args(&[
        "-v", "error", "-nostdin", "-n", "-f", "lavfi", "-i", wave, "-c:a", "flac",
    ]);
    a.push(audio.as_os_str().into());
    run(&ffmpeg, a).await;
    let before = Sha256::digest(std::fs::read(&audio).unwrap());
    let manager = JobManager::open(f.0.join("logs"), f.0.join("history")).await;
    let job = finish(
        &manager,
        EncodeRequest {
            source: RemuxRequest {
                input_path: input.to_string_lossy().into_owned(),
                output_path: output.to_string_lossy().into_owned(),
                stream_indices: vec![0],
            },
            settings: EncodeSettings {
                encoder: VideoEncoder::X264,
                crf: 23,
                preset: 0,
                external_tracks: vec![media_core::ExternalTrack {
                    input_path: audio.to_string_lossy().into_owned(),
                    stream_index: 0,
                    audio: Some(media_core::ExternalAudioSettings {
                        codec: AudioCodec::Flac,
                        bitrate_kbps: 8,
                        channels: AudioChannels::Explicit {
                            count: 1,
                            layout: AudioChannelLayoutPolicy::FfmpegDefault,
                        },
                        gain: None,
                        normalization: Some(AudioNormalization {
                            target: LoudnessTarget::Lufs14,
                        }),
                    }),
                    offset_milliseconds: 0,
                    subtitle_mode: None,
                    subtitle_burn_in: false,
                    title: None,
                    language: None,
                    default: None,
                    forced: None,
                }],
                ..Default::default()
            },
        },
    )
    .await;
    assert_eq!(
        job.state,
        JobState::Succeeded,
        "{:?}\n{:?}",
        job.error,
        job.logs
    );
    assert!(
        job.logs
            .iter()
            .any(|line| line.contains("Peak requires dynamic limiting: true")),
        "{:?}",
        job.logs
    );
    let m = measure(&output, 1).await;
    eprintln!(
        "{}: {:?} LUFS, {:?} dBTP, {:?} LU LRA",
        output.display(),
        m.integrated_lufs,
        m.true_peak_dbfs,
        m.loudness_range_lu
    );
    assert!(m.true_peak_dbfs.unwrap() <= -0.9, "{m:?}");
    assert_eq!(Sha256::digest(std::fs::read(&audio).unwrap()), before);
    manager.shutdown().await;
}
