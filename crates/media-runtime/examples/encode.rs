//! cargo run -p media-runtime --example encode -- INPUT OUTPUT [CRF] [PRESET] [GRAIN] [HDR10_FALLBACK] [BACKEND] [WORKERS] [ENCODER] [LINEART_BIAS] [TEXTURE_BIAS] [HDR_TUNE] [LOSSLESS]
use media_runtime::{
    EncodeBackend, EncodeRequest, EncodeSettings, HdrTune, JobManager, JobState, RemuxRequest,
    VideoEncoder, probe_media,
};
use std::{path::PathBuf, time::Duration};

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args: Vec<_> = std::env::args().skip(1).collect();
    if !(2..=13).contains(&args.len()) {
        return Err("Usage: encode INPUT_ABSOLUTE OUTPUT_ABSOLUTE.mkv [CRF] [PRESET] [GRAIN_0_50] [HDR10_FALLBACK_true_false] [standalone|av1an] [WORKERS_1_32] [svtAv1|svtAv1FiveFish|svtAv1Hdr|x264|x265|vp9|aomAv1|x265Standalone|vpxStandalone|h264Nvenc|hevcNvenc] [LINEART_0_7] [TEXTURE_0_7] [visualQuality|filmGrain] [LOSSLESS_true_false]".into());
    }
    let encoder = match args.get(8).map(String::as_str) {
        None | Some("svtAv1") => VideoEncoder::SvtAv1,
        Some("svtAv1FiveFish") => VideoEncoder::SvtAv1FiveFish,
        Some("svtAv1Hdr") => VideoEncoder::SvtAv1Hdr,
        Some("x264") => VideoEncoder::X264,
        Some("x265") => VideoEncoder::X265,
        Some("vp9") => VideoEncoder::Vp9,
        Some("aomAv1") => VideoEncoder::AomAv1,
        Some("x265Standalone") => VideoEncoder::X265Standalone,
        Some("vpxStandalone") => VideoEncoder::VpxStandalone,
        Some("h264Nvenc") => VideoEncoder::H264Nvenc,
        Some("hevcNvenc") => VideoEncoder::HevcNvenc,
        _ => {
            return Err("Encoder name is not supported by this example.".into());
        }
    };
    let media = probe_media(args[0].clone()).await?;
    let video = media
        .streams
        .iter()
        .find(|s| s.kind == "video")
        .ok_or("No video stream")?;
    let settings = EncodeSettings {
        temporal: None,
        parameters: Vec::new(),
        av1an_options: None,
        rate_control: None,
        lossless: args
            .get(12)
            .map(|v| v.parse())
            .transpose()?
            .unwrap_or(false),
        svt_crf_quarter_steps: None,
        svt_preset: None,
        tone_map: None,
        trim: None,
        subtitles: Vec::new(),
        framing: Default::default(),
        audio: Vec::new(),
        video_stream_index: video.index,
        encoder,
        crf: args.get(2).map(|v| v.parse()).transpose()?.unwrap_or(
            if encoder == VideoEncoder::X264 {
                23
            } else if matches!(encoder, VideoEncoder::X265 | VideoEncoder::X265Standalone) {
                28
            } else if matches!(encoder, VideoEncoder::Vp9 | VideoEncoder::VpxStandalone) {
                32
            } else if encoder == VideoEncoder::H264Nvenc {
                18
            } else if encoder == VideoEncoder::HevcNvenc {
                22
            } else if encoder == VideoEncoder::SvtAv1FiveFish {
                18
            } else {
                30
            },
        ),
        preset: args
            .get(3)
            .map(|v| v.parse())
            .transpose()?
            .unwrap_or(match encoder {
                VideoEncoder::X264 | VideoEncoder::X265 | VideoEncoder::X265Standalone => 5,
                VideoEncoder::SvtAv1 => 4,
                VideoEncoder::AomAv1 => 6,
                VideoEncoder::H264Nvenc | VideoEncoder::HevcNvenc => 4,
                _ => 2,
            }),
        film_grain: args.get(4).map(|v| v.parse()).transpose()?.unwrap_or(0),
        hdr10_fallback: args.get(5).map(|v| v.parse()).transpose()?.unwrap_or(false),
        backend: match args.get(6).map(String::as_str) {
            None | Some("standalone" | "svtAv1") => EncodeBackend::Standalone,
            Some("av1an") => EncodeBackend::Av1an,
            _ => return Err("Backend must be standalone or av1an".into()),
        },
        workers: args.get(7).map(|v| v.parse()).transpose()?.unwrap_or(2),
        lineart_psy_bias: args.get(9).map(|v| v.parse()).transpose()?.unwrap_or(
            if encoder == VideoEncoder::SvtAv1FiveFish {
                5
            } else {
                0
            },
        ),
        texture_psy_bias: args.get(10).map(|v| v.parse()).transpose()?.unwrap_or(
            if encoder == VideoEncoder::SvtAv1FiveFish {
                4
            } else {
                0
            },
        ),
        hdr_tune: match args.get(11).map(String::as_str) {
            Some("visualQuality") => HdrTune::VisualQuality,
            Some("filmGrain") => HdrTune::FilmGrain,
            None if encoder == VideoEncoder::SvtAv1Hdr => HdrTune::FilmGrain,
            None => HdrTune::VisualQuality,
            _ => return Err("HDR tune must be visualQuality or filmGrain".into()),
        },
    };
    let stream_indices = media
        .streams
        .iter()
        .filter(|s| s.kind != "video" || s.index == video.index)
        .map(|s| s.index)
        .collect();
    let manager = JobManager::new(
        PathBuf::from(&args[1])
            .parent()
            .ok_or("Output requires parent")?
            .join("jesses-job-logs"),
    );
    let started = manager
        .start_encode(EncodeRequest {
            source: RemuxRequest {
                input_path: args[0].clone(),
                output_path: args[1].clone(),
                stream_indices,
            },
            settings,
        })
        .await?;
    println!("{}", serde_json::to_string(&started)?);
    let mut previous = None;
    let mut interrupt = std::pin::pin!(tokio::signal::ctrl_c());
    loop {
        tokio::select! { _ = &mut interrupt => { manager.cancel_job(started.id.clone()).await?; manager.shutdown().await; }, _ = tokio::time::sleep(Duration::from_millis(200)) => {} }
        let job = manager.list_jobs().await.remove(0);
        if previous != Some(job.state) || job.state.is_terminal() {
            println!("{}", serde_json::to_string(&job)?);
            previous = Some(job.state);
        }
        if job.state.is_terminal() {
            manager.shutdown().await;
            if job.state != JobState::Succeeded {
                return Err(format!("Encoding ended: {:?}: {:?}", job.state, job.error).into());
            }
            break;
        }
    }
    Ok(())
}
