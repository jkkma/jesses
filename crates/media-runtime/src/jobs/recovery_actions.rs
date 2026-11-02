//! Source recovery actions operate only on inactive, identity-guarded workspaces.
use super::*;
use media_core::{Av1anRecoveryAction, Av1anRecoveryActionResult, Av1anRecoveryEntry};

fn eligible(entry: &Entry) -> bool {
    matches!(
        entry.snapshot.state,
        JobState::Stopped | JobState::Failed | JobState::Interrupted | JobState::Canceled
    ) && entry.snapshot.recovery.is_some()
        && entry
            .snapshot
            .encode_settings
            .as_ref()
            .is_some_and(|s| s.backend == media_core::EncodeBackend::Av1an)
        && entry.task.as_ref().is_none_or(|task| task.is_finished())
}
fn unavailable() -> AppError {
    AppError::new(
        "AV1AN_RECOVERY_UNAVAILABLE",
        "Choose an inactive AV1AN encode with verified saved progress.",
        None,
    )
}

async fn open_owned(
    snapshot: JobSnapshot,
    require_source: bool,
) -> Result<av1an::OwnedRecovery, AppError> {
    tokio::task::spawn_blocking(move || av1an::OwnedRecovery::open(&snapshot, require_source))
        .await
        .map_err(|error| AppError::new("RECOVERY_INVALID", error.to_string(), None))?
}

impl JobManager {
    pub async fn list_av1an_recoveries(&self) -> Result<Vec<Av1anRecoveryEntry>, AppError> {
        self.ready().await?;
        let state = self.state.lock().await;
        let mut result = Vec::new();
        for entry in state.entries.iter().rev().filter(|entry| eligible(entry)) {
            // Invalid entries do not hide other valid unfinished encodes. Direct
            // actions still return the precise ownership error for that job.
            if let Ok(owned) = open_owned(entry.snapshot.clone(), false).await {
                let (chunk_count, encoded_bytes) = owned.encoded_stats()?;
                let (created_at, last_run_at) = owned.times();
                result.push(Av1anRecoveryEntry {
                    job: {
                        let mut job = entry.snapshot.clone();
                        job.recovery = Some(owned.summary());
                        job
                    },
                    encoded_bytes: encoded_bytes.to_string(),
                    chunk_count,
                    created_at,
                    last_run_at,
                });
            }
        }
        result.sort_by_key(|entry| std::cmp::Reverse(entry.last_run_at));
        Ok(result)
    }

    pub async fn av1an_recovery_action(
        &self,
        request: Av1anRecoveryAction,
    ) -> Result<Av1anRecoveryActionResult, AppError> {
        self.av1an_recovery_action_with_review(request, None).await
    }

    pub(super) async fn av1an_recovery_action_with_review(
        &self,
        request: Av1anRecoveryAction,
        command_review: Option<Arc<command_edit::Session>>,
    ) -> Result<Av1anRecoveryActionResult, AppError> {
        self.ready().await?;
        let (ids, next_settings, resume, delete_chunks) = match request {
            Av1anRecoveryAction::DeleteEncode { id } => (vec![id], None, false, false),
            Av1anRecoveryAction::DeleteChunks { id } => (vec![id], None, false, true),
            Av1anRecoveryAction::ResumeSaved { id } => (vec![id], None, true, false),
            Av1anRecoveryAction::ResumeNew { id, settings } => {
                if settings.backend != media_core::EncodeBackend::Av1an {
                    return Err(unavailable());
                }
                encode_plan::validate_settings(&settings)?;
                (vec![id], Some(*settings), true, false)
            }
            Av1anRecoveryAction::ClearAll { ids } => (ids, None, false, false),
        };
        if ids.len() > MAX_HISTORY || ids.iter().collect::<HashSet<_>>().len() != ids.len() {
            return Err(unavailable());
        }
        let mut state = self.state.lock().await;
        if let Some(error) = &state.storage_error {
            return Err(error.clone());
        }
        if state.shutting_down {
            return Err(AppError::new(
                "APP_CLOSING",
                "Recovery actions are unavailable while closing.",
                None,
            ));
        }
        let mut owned = Vec::new();
        for id in &ids {
            let entry = state
                .entries
                .iter()
                .find(|entry| entry.snapshot.id == *id)
                .ok_or_else(unavailable)?;
            if resume && let Some(settings) = &entry.snapshot.encode_settings {
                // Never silently reinterpret a saved hardware encode as CPU.
                encode_plan::validate_settings(settings)?;
            }
            if !eligible(entry) {
                return Err(unavailable());
            }
            owned.push((
                entry.snapshot.clone(),
                open_owned(entry.snapshot.clone(), resume).await?,
            ));
        }
        // Acquire every ownership guard before clear-all changes any workspace.
        if next_settings.is_some() {
            let (_owner, cancel) = watch::channel(false);
            for (_, recovery) in &mut owned {
                encode::boxed(|| recovery.seal_source_chunks(&cancel)).await?;
            }
        }
        let mut canonical = Vec::new();
        let mut source_cuts = Vec::new();
        if !delete_chunks {
            for (snapshot, recovery) in &owned {
                let cleanup = source_normalization::prepare_recovery_action(
                    &snapshot.id,
                    &snapshot.request,
                    snapshot.encode_settings.as_ref().expect("saved settings"),
                    recovery.settings(),
                )?;
                if resume && let Some(cleanup) = &cleanup {
                    cleanup.verify_source()?;
                }
                let cut = source_normalization::av1an_trim::prepare_recovery_action(
                    &snapshot.id,
                    &snapshot.request,
                    snapshot.encode_settings.as_ref().expect("saved settings"),
                    recovery.settings(),
                )?;
                if resume && let Some(cut) = &cut {
                    cut.verify_source()?;
                }
                source_cuts.push((
                    cut,
                    next_settings.clone().unwrap_or_else(|| {
                        snapshot.encode_settings.clone().expect("saved settings")
                    }),
                ));
                canonical.push((
                    cleanup,
                    next_settings.clone().unwrap_or_else(|| {
                        snapshot.encode_settings.clone().expect("saved settings")
                    }),
                ));
            }
        }
        let mut updated = Vec::new();
        for (snapshot, mut recovery) in owned {
            let index = state
                .entries
                .iter()
                .position(|entry| entry.snapshot.id == snapshot.id)
                .expect("held state");
            if let Some(settings) = &next_settings {
                if let Some(review) = &command_review {
                    review.save_resume_backup(snapshot.clone(), recovery.command_recipe());
                }
                let locator = recovery.new_settings(settings)?;
                state.entries[index].snapshot.encode_settings = Some(settings.clone());
                state.entries[index].snapshot.edited_command = None;
                state.entries[index].snapshot.recovery = Some(locator);
                append_log(&mut state.entries[index].snapshot,if settings.av1an_options.is_some_and(|options|options.source_workflow){"Source New Settings retains the sealed scene/encoder queue and completed chunks; new global processing arguments apply only to unfinished chunks."}else{"New-settings resume invalidated all encoded chunks and incompatible planning receipts; verified compatible prepared source bytes remain guarded."}.into());
            } else if delete_chunks {
                state.entries[index].snapshot.recovery = Some(recovery.reset_chunks()?);
                append_log(&mut state.entries[index].snapshot,"Encoded chunks and completion progress were deleted; verified source preparation and scene/index receipts were retained.".into());
            } else if !resume {
                recovery.discard()?;
                state.entries[index].snapshot.recovery = None;
                append_log(
                    &mut state.entries[index].snapshot,
                    "Owned AV1AN encode workspace was deleted; original media was preserved."
                        .into(),
                );
            }
            updated.push(state.entries[index].snapshot.clone());
        }
        for (cleanup, settings) in canonical {
            if let Some(cleanup) = cleanup {
                if resume {
                    cleanup.rebind(&settings)?;
                } else {
                    cleanup.finish()?;
                }
            }
        }
        for (cleanup, settings) in source_cuts {
            if let Some(cleanup) = cleanup {
                if resume {
                    cleanup.rebind(&settings)?;
                } else {
                    cleanup.finish()?;
                }
            }
        }
        if !resume || next_settings.is_some() {
            self.persist(&mut state).await?;
        }
        drop(state);
        if resume {
            let id = ids.into_iter().next().expect("one resume id");
            updated = vec![self.resume_job_with_review(id, command_review).await?];
        }
        self.queue_changed.notify_waiters();
        Ok(Av1anRecoveryActionResult { jobs: updated })
    }
}
