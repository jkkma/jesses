//! Durable av1an recovery qualification through the public manager API.
//! Usage: av1an_recovery HISTORY_DIR status
//!        av1an_recovery HISTORY_DIR start REQUEST.json [--stop-after-frames N | --crash-after-frames N | --stop-at-finalizing]
//!        av1an_recovery HISTORY_DIR resume JOB_ID [same optional action]
//! The explicit crash action is Windows-only: owned Job Objects terminate tools.
use std::{io::Write, path::PathBuf, time::Duration};

use media_runtime::{EncodeBackend, EncodeRequest, JobManager, JobState, RecoveryPhase};

#[derive(Clone, Copy, PartialEq, Eq)]
enum Action {
    None,
    StopAfter(u64),
    CrashAfter(u64),
    StopFinalizing,
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut args = std::env::args_os().skip(1);
    let history = PathBuf::from(
        args.next()
            .ok_or("Expected HISTORY_DIR and start/resume/status")?,
    );
    let mode = args.next().ok_or("Expected start/resume/status")?;
    let subject = if mode == "status" {
        None
    } else {
        Some(args.next().ok_or("Expected request file or saved job id")?)
    };
    let action = match args.next().as_deref().and_then(|arg| arg.to_str()) {
        None => Action::None,
        Some("--stop-at-finalizing") => Action::StopFinalizing,
        Some(flag @ ("--stop-after-frames" | "--crash-after-frames")) => {
            let frames: u64 = args.next().and_then(|value| value.into_string().ok()).ok_or("Expected a positive frame count")?.parse()?;
            if frames == 0 { return Err("Frame count must be positive".into()); }
            if flag == "--crash-after-frames" {
                if !cfg!(windows) { return Err("Crash qualification is only enabled on Windows with owned Job Objects".into()); }
                Action::CrashAfter(frames)
            } else { Action::StopAfter(frames) }
        }
        Some(_) => return Err("Unknown action; use --stop-after-frames N, --crash-after-frames N, or --stop-at-finalizing".into()),
    };
    if args.next().is_some() {
        return Err("Unexpected extra arguments".into());
    }
    let manager = JobManager::open(history.join("logs"), history).await;
    manager.ready().await?;
    if mode == "status" {
        println!(
            "{}",
            serde_json::to_string_pretty(&manager.list_jobs().await)?
        );
        manager.shutdown().await;
        return Ok(());
    }
    let subject = subject.expect("validated command subject");
    let submitted = if mode == "start" {
        let request: EncodeRequest = serde_json::from_slice(&std::fs::read(subject)?)?;
        if request.settings.backend != EncodeBackend::Av1an {
            return Err("Recovery qualification requires an av1an request".into());
        }
        manager.start_encode(request).await?
    } else if mode == "resume" {
        manager
            .resume_job(
                subject
                    .into_string()
                    .map_err(|_| "Job id must be Unicode")?,
            )
            .await?
    } else {
        return Err("Expected start/resume/status".into());
    };
    let mut previous = String::new();
    let mut acted = false;
    let mut interrupted = std::pin::pin!(tokio::signal::ctrl_c());
    loop {
        let job = manager
            .list_jobs()
            .await
            .into_iter()
            .find(|job| job.id == submitted.id)
            .ok_or("Saved job disappeared")?;
        let projection = format!("{:?}:{:?}", job.state, job.recovery);
        if projection != previous || job.state.is_terminal() {
            println!("{}", serde_json::to_string(&job)?);
            std::io::stdout().flush()?;
            previous = projection;
        }
        if job.state.is_terminal() {
            manager.shutdown().await;
            if action != Action::None && !acted && job.state == JobState::Succeeded {
                return Err(
                    "The job finished before the requested recovery checkpoint action was observed"
                        .into(),
                );
            }
            return if matches!(job.state, JobState::Succeeded | JobState::Stopped) {
                Ok(())
            } else {
                Err(format!("Job ended {:?}: {:?}", job.state, job.error).into())
            };
        }
        if !acted && let Some(recovery) = &job.recovery {
            let reached = match action {
                Action::StopAfter(frames) | Action::CrashAfter(frames) => {
                    recovery.completed_frames >= frames
                }
                Action::StopFinalizing => recovery.phase == RecoveryPhase::Finalizing,
                Action::None => false,
            };
            if reached {
                acted = true;
                if matches!(action, Action::CrashAfter(_)) {
                    eprintln!(
                        "Explicit Windows crash qualification after a retained checkpoint; exiting 86."
                    );
                    std::io::stderr().flush()?;
                    std::process::exit(86);
                }
                manager.stop_job(job.id.clone()).await?;
            }
        }
        tokio::select! {
            _ = &mut interrupted, if !acted => { acted = true; manager.stop_job(job.id.clone()).await?; },
            _ = tokio::time::sleep(Duration::from_millis(20)) => {},
        }
    }
}
