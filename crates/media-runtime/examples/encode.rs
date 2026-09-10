//! cargo run -p media-runtime --example encode -- INPUT OUTPUT [CRF] [PRESET]
use media_runtime::{
    EncodeRequest, EncodeSettings, JobManager, JobState, RemuxRequest, probe_media,
};
use std::{path::PathBuf, time::Duration};

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args: Vec<_> = std::env::args().skip(1).collect();
    if !(2..=4).contains(&args.len()) {
        return Err("Usage: encode INPUT_ABSOLUTE OUTPUT_ABSOLUTE.mkv [CRF] [PRESET]".into());
    }
    let media = probe_media(args[0].clone()).await?;
    let video = media
        .streams
        .iter()
        .find(|s| s.kind == "video")
        .ok_or("No video stream")?;
    let settings = EncodeSettings {
        video_stream_index: video.index,
        crf: args.get(2).map(|v| v.parse()).transpose()?.unwrap_or(30),
        preset: args.get(3).map(|v| v.parse()).transpose()?.unwrap_or(4),
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
