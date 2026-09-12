//! Run one immutable request: cargo run -p media-runtime --example encode_request -- REQUEST.json
//! The source is read-only; output publication keeps the normal no-overwrite guard.
use media_runtime::{EncodeRequest, JobManager, JobState};
use std::{path::PathBuf, time::Duration};

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut args = std::env::args_os().skip(1);
    let file = args.next().ok_or("Usage: encode_request REQUEST.json")?;
    if args.next().is_some() {
        return Err("Usage: encode_request REQUEST.json".into());
    }
    let request: EncodeRequest = serde_json::from_slice(&std::fs::read(file)?)?;
    let directory = PathBuf::from(&request.source.output_path)
        .parent()
        .ok_or("Output requires a parent directory")?
        .join("jesses-job-logs");
    let manager = JobManager::new(directory);
    let job = manager.start_encode(request).await?;
    let mut previous = None;
    let mut interrupt = std::pin::pin!(tokio::signal::ctrl_c());
    loop {
        tokio::select! {
            _ = &mut interrupt => { manager.cancel_job(job.id.clone()).await?; manager.shutdown().await; },
            _ = tokio::time::sleep(Duration::from_millis(200)) => {},
        }
        let snapshot = manager
            .list_jobs()
            .await
            .into_iter()
            .find(|item| item.id == job.id)
            .ok_or("Job disappeared")?;
        if previous != Some(snapshot.state) || snapshot.state.is_terminal() {
            println!("{}", serde_json::to_string(&snapshot)?);
            previous = Some(snapshot.state);
        }
        if snapshot.state.is_terminal() {
            manager.shutdown().await;
            if snapshot.state != JobState::Succeeded {
                return Err(
                    format!("Encoding ended: {:?}: {:?}", snapshot.state, snapshot.error).into(),
                );
            }
            return Ok(());
        }
    }
}
