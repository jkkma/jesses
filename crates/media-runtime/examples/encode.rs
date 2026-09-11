//! Exercise the same durable job manager used by the desktop.
//! cargo run -p media-runtime --example encode -- INPUT OUTPUT [PRESET] [CANCEL_AFTER_MS]
//! Set JESSES_JOB_DIRECTORY to isolate the job store. Use --list to recover/list it.

use std::{path::PathBuf, sync::Arc, time::Duration};

use media_core::{EncodeRequest, JobStatus};
use media_runtime::JobManager;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let directory = std::env::var_os("JESSES_JOB_DIRECTORY")
        .map(PathBuf::from)
        .unwrap_or_else(|| std::env::temp_dir().join("jesses-example-jobs"));
    let manager = Arc::new(JobManager::open(directory)?);
    if args.first().is_some_and(|arg| arg == "--list") {
        println!("{}", serde_json::to_string_pretty(&manager.list()?)?);
        return Ok(());
    }
    if args.len() < 2 || args.len() > 4 {
        return Err("Usage: encode INPUT OUTPUT [PRESET] [CANCEL_AFTER_MS] (or --list)".into());
    }
    let job = manager
        .start(EncodeRequest {
            input_path: args[0].clone(),
            output_path: args[1].clone(),
            crf: 30,
            preset: args
                .get(2)
                .map(|text| text.parse())
                .transpose()?
                .unwrap_or(4),
            audio_bitrate_kbps: 128,
            audio_channels: None,
        })
        .await?;
    if let Some(after) = args.get(3) {
        let after = Duration::from_millis(after.parse()?);
        let manager = Arc::clone(&manager);
        let id = job.id.clone();
        tokio::spawn(async move {
            tokio::time::sleep(after).await;
            let _ = manager.cancel(&id);
        });
    }
    let mut last = String::new();
    loop {
        let current = manager
            .list()?
            .into_iter()
            .find(|entry| entry.id == job.id)
            .ok_or("Job disappeared from the store")?;
        let json = serde_json::to_string(&current)?;
        if json != last {
            println!("{json}");
            last = json;
        }
        if !current.status.is_active() {
            return match current.status {
                JobStatus::Completed | JobStatus::Cancelled => Ok(()),
                _ => Err(current.message.into()),
            };
        }
        tokio::time::sleep(Duration::from_millis(500)).await;
    }
}
