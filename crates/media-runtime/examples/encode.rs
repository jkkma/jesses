//! cargo run -p media-runtime --example encode -- INPUT OUTPUT [CRF] [PRESET] [GRAIN] [HDR10_FALLBACK] [BACKEND] [WORKERS] [ENCODER]
use media_runtime::{
    EncodeBackend, EncodeRequest, EncodeSettings, JobManager, JobState, RemuxRequest, VideoEncoder,
    probe_media,
};
use std::{path::PathBuf, time::Duration};

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args: Vec<_> = std::env::args().skip(1).collect();
    if !(2..=9).contains(&args.len()) {
        return Err("Usage: encode INPUT_ABSOLUTE OUTPUT_ABSOLUTE.mkv [CRF] [PRESET] [GRAIN_0_50] [HDR10_FALLBACK_true_false] [standalone|av1an] [WORKERS_1_32] [svtAv1|x264]".into());
    }
    let encoder = match args.get(8).map(String::as_str) {
        None | Some("svtAv1") => VideoEncoder::SvtAv1,
        Some("x264") => VideoEncoder::X264,
        _ => return Err("Encoder must be svtAv1 or x264".into()),
    };
    let media = probe_media(args[0].clone()).await?;
    let video = media
        .streams
        .iter()
        .find(|s| s.kind == "video")
        .ok_or("No video stream")?;
    let settings = EncodeSettings {
        video_stream_index: video.index,
        encoder,
        crf: args.get(2).map(|v| v.parse()).transpose()?.unwrap_or(
            if encoder == VideoEncoder::X264 {
                23
            } else {
                30
            },
        ),
        preset: args
            .get(3)
            .map(|v| v.parse())
            .transpose()?
            .unwrap_or(if encoder == VideoEncoder::X264 { 5 } else { 4 }),
        film_grain: args.get(4).map(|v| v.parse()).transpose()?.unwrap_or(0),
        hdr10_fallback: args.get(5).map(|v| v.parse()).transpose()?.unwrap_or(false),
        backend: match args.get(6).map(String::as_str) {
            None | Some("standalone" | "svtAv1") => EncodeBackend::Standalone,
            Some("av1an") => EncodeBackend::Av1an,
            _ => return Err("Backend must be standalone or av1an".into()),
        },
        workers: args.get(7).map(|v| v.parse()).transpose()?.unwrap_or(2),
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
