//! Run the same job manager as the desktop app; outputs never replace files.
//! cargo run -p media-runtime --example remux -- INPUT OUTPUT [0,1,2]
use std::{path::PathBuf, time::Duration};

use media_runtime::{JobManager, JobState, RemuxRequest, probe_media};

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args: Vec<_> = std::env::args().skip(1).collect();
    if !(2..=3).contains(&args.len()) {
        return Err("Usage: remux INPUT_ABSOLUTE OUTPUT_ABSOLUTE.mkv [stream,indices]".into());
    }
    let input_path = args[0].clone();
    let output_path = args[1].clone();
    let stream_indices = if let Some(indices) = args.get(2) {
        indices
            .split(',')
            .map(str::parse)
            .collect::<Result<Vec<u32>, _>>()?
    } else {
        probe_media(input_path.clone())
            .await?
            .streams
            .into_iter()
            .map(|s| s.index)
            .collect()
    };
    let log_dir = PathBuf::from(&output_path)
        .parent()
        .ok_or("Output needs a parent folder")?
        .join("jesses-job-logs");
    let manager = JobManager::new(log_dir);
    let started = manager
        .start_remux(RemuxRequest {
            input_path,
            output_path,
            stream_indices,
        })
        .await?;
    println!("{}", serde_json::to_string(&started)?);
    let mut previous = None;
    let mut interrupt = std::pin::pin!(tokio::signal::ctrl_c());
    loop {
        tokio::select! {
            _ = &mut interrupt => { manager.cancel_job(started.id.clone()).await?; manager.shutdown().await; },
            _ = tokio::time::sleep(Duration::from_millis(200)) => {},
        }
        let job = manager.list_jobs().await.remove(0);
        if previous != Some(job.state) || job.state.is_terminal() {
            println!("{}", serde_json::to_string(&job)?);
            previous = Some(job.state);
        }
        if job.state.is_terminal() {
            manager.shutdown().await;
            if job.state != JobState::Succeeded {
                return Err(format!("Job ended: {:?}: {:?}", job.state, job.error).into());
            }
            break;
        }
    }
    Ok(())
}
