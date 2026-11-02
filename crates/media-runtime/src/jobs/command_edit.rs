//! Held native preparation for the source Shift+Run argument window.
use super::*;
use media_core::{
    CommandEditConfirmation, CommandEditRequest, CommandEditResult, CommandEditReview,
    EditedCommand,
};
use std::{
    collections::HashMap,
    sync::{Mutex as StdMutex, atomic::AtomicBool},
};
use tokio::sync::oneshot;
mod images;
pub(super) mod rebind;
mod shell;
pub(crate) use shell::PreparedShell;

type ReviewSender = oneshot::Sender<Result<CommandEditReview, AppError>>;
type AckSender = oneshot::Sender<Result<(), AppError>>;

pub(crate) struct Session {
    pub id: String,
    job_id: StdMutex<Option<String>>,
    review: StdMutex<Option<ReviewSender>>,
    decision: StdMutex<Option<oneshot::Sender<String>>>,
    decision_receiver: Mutex<Option<oneshot::Receiver<String>>>,
    acknowledged: StdMutex<Option<AckSender>>,
    acknowledgment: Mutex<Option<oneshot::Receiver<Result<(), AppError>>>>,
    image: Mutex<Option<images::ImageState>>,
    image_pause: supervisor::PauseControl,
    cancel: watch::Sender<bool>,
    pub running: AtomicBool,
    fresh: bool,
    confirmed: AtomicBool,
    resume_backup: StdMutex<Option<(JobSnapshot, av1an::CommandRecipe)>>,
}

#[derive(Clone, Default)]
pub(crate) struct Sessions(Arc<StdMutex<HashMap<String, Arc<Session>>>>);

fn expired() -> AppError {
    AppError::new(
        "COMMAND_EDIT_EXPIRED",
        "This command review is no longer available. Prepare the task again.",
        None,
    )
}
fn canceled_review() -> AppError {
    AppError::new(
        "COMMAND_EDIT_CANCELED",
        "The command was cleared in the edit window, so nothing was run.",
        None,
    )
}

impl Session {
    fn new(
        fresh: bool,
    ) -> (
        Arc<Self>,
        oneshot::Receiver<Result<CommandEditReview, AppError>>,
    ) {
        let id = format!(
            "command-{}-{}",
            std::process::id(),
            NEXT_ID.fetch_add(1, Ordering::Relaxed)
        );
        let (review, receiver) = oneshot::channel();
        let (decision, decision_receiver) = oneshot::channel();
        let (acknowledged, acknowledgment) = oneshot::channel();
        let (cancel, _) = watch::channel(false);
        (
            Arc::new(Self {
                id,
                job_id: StdMutex::new(None),
                review: StdMutex::new(Some(review)),
                decision: StdMutex::new(Some(decision)),
                decision_receiver: Mutex::new(Some(decision_receiver)),
                acknowledged: StdMutex::new(Some(acknowledged)),
                acknowledgment: Mutex::new(Some(acknowledgment)),
                image: Mutex::new(None),
                image_pause: Default::default(),
                cancel,
                running: AtomicBool::new(true),
                fresh,
                confirmed: AtomicBool::new(false),
                resume_backup: StdMutex::new(None),
            }),
            receiver,
        )
    }
    pub(super) fn bind_job(&self, id: &str) {
        *self
            .job_id
            .lock()
            .unwrap_or_else(|cause| cause.into_inner()) = Some(id.to_owned());
    }
    pub(in crate::jobs) fn save_resume_backup(
        &self,
        snapshot: JobSnapshot,
        recipe: av1an::CommandRecipe,
    ) {
        *self
            .resume_backup
            .lock()
            .unwrap_or_else(|cause| cause.into_inner()) = Some((snapshot, recipe));
    }
    fn job_id(&self) -> Option<String> {
        self.job_id
            .lock()
            .unwrap_or_else(|cause| cause.into_inner())
            .clone()
    }
    fn cancel(&self) {
        self.cancel.send_replace(true);
        self.decision
            .lock()
            .unwrap_or_else(|cause| cause.into_inner())
            .take();
    }
    pub(super) fn finish(&self, error: Option<AppError>) {
        self.running.store(false, Ordering::Relaxed);
        let failure = error.unwrap_or_else(expired);
        if let Some(sender) = self
            .review
            .lock()
            .unwrap_or_else(|cause| cause.into_inner())
            .take()
        {
            let _ = sender.send(Err(failure.clone()));
        }
        if let Some(sender) = self
            .acknowledged
            .lock()
            .unwrap_or_else(|cause| cause.into_inner())
            .take()
        {
            let _ = sender.send(Err(failure));
        }
    }
    pub(super) async fn ask(
        &self,
        label: &str,
        generated: &str,
        output: &Path,
        cancel: &watch::Receiver<bool>,
    ) -> Result<EditedCommand, AppError> {
        self.ask_initial(label, generated, generated, output, cancel)
            .await
    }
    async fn ask_initial(
        &self,
        label: &str,
        generated: &str,
        initial: &str,
        output: &Path,
        cancel: &watch::Receiver<bool>,
    ) -> Result<EditedCommand, AppError> {
        check_cancel(cancel)?;
        let receiver = self
            .decision_receiver
            .lock()
            .await
            .take()
            .ok_or_else(expired)?;
        let review = CommandEditReview {
            id: self.id.clone(),
            executable_label: label.into(),
            arguments: initial.into(),
            output_path: output.to_string_lossy().into_owned(),
            job: None,
        };
        self.review
            .lock()
            .unwrap_or_else(|cause| cause.into_inner())
            .take()
            .ok_or_else(expired)?
            .send(Ok(review))
            .map_err(|_| canceled())?;
        let arguments = tokio::select! {
            biased;
            _ = wait_cancel(cancel.clone()) => return Err(canceled()),
            _ = wait_cancel(self.cancel.subscribe()) => return Err(canceled()),
            value = receiver => value.map_err(|_| canceled())?,
        };
        if arguments.trim().is_empty() {
            return Err(canceled());
        }
        self.confirmed.store(true, Ordering::Relaxed);
        Ok(EditedCommand {
            generated: generated.to_owned(),
            arguments: arguments.trim().to_owned(),
        })
    }
    pub(super) fn acknowledge(&self, result: Result<(), AppError>) {
        if let Some(sender) = self
            .acknowledged
            .lock()
            .unwrap_or_else(|cause| cause.into_inner())
            .take()
        {
            let _ = sender.send(result);
        }
    }
}

impl Sessions {
    fn insert(&self, session: Arc<Session>) {
        self.0
            .lock()
            .unwrap_or_else(|cause| cause.into_inner())
            .insert(session.id.clone(), session);
    }
    fn get(&self, id: &str) -> Result<Arc<Session>, AppError> {
        self.0
            .lock()
            .unwrap_or_else(|cause| cause.into_inner())
            .get(id)
            .cloned()
            .ok_or_else(expired)
    }
    fn remove(&self, id: &str) {
        self.0
            .lock()
            .unwrap_or_else(|cause| cause.into_inner())
            .remove(id);
    }
    pub(super) fn cancel_all(&self) {
        for session in self
            .0
            .lock()
            .unwrap_or_else(|cause| cause.into_inner())
            .values()
        {
            session.cancel();
        }
    }
    pub(super) async fn shutdown(&self) {
        let sessions: Vec<_> = self
            .0
            .lock()
            .unwrap_or_else(|cause| cause.into_inner())
            .drain()
            .map(|(_, session)| session)
            .collect();
        for session in &sessions {
            session.cancel();
        }
        for session in sessions {
            images::join(&session).await;
        }
    }
}

impl JobManager {
    /// The image task outlives preparation's IPC call. Keep pause ownership on
    /// that exact session instead of borrowing a transient analysis identifier.
    pub async fn set_command_edit_paused(&self, id: &str, paused: bool) -> Result<(), AppError> {
        let session = self.command_edits.get(id)?;
        if session.job_id().is_some() || session.image.lock().await.is_none() {
            return Err(AppError::new(
                "COMMAND_EDIT_PAUSE_UNSUPPORTED",
                "Use the native job's Pause control for a queued encode.",
                None,
            ));
        }
        if !session.running.load(Ordering::Relaxed) || *session.cancel.borrow() {
            return Err(expired());
        }
        let control = session.image_pause.clone();
        tokio::task::spawn_blocking(move || control.set_paused(paused))
            .await
            .map_err(|cause| AppError::new("COMMAND_EDIT_PAUSE_FAILED", cause.to_string(), None))?
            .map_err(|cause| AppError::new("COMMAND_EDIT_PAUSE_FAILED", cause.to_string(), None))
    }
    /// Reload/navigation cancels held sessions without imposing a user timer.
    pub async fn cancel_command_reviews(&self) -> Result<(), AppError> {
        let ids: Vec<_> = self
            .command_edits
            .0
            .lock()
            .unwrap_or_else(|cause| cause.into_inner())
            .keys()
            .cloned()
            .collect();
        for id in ids {
            self.cancel_command_edit(&id).await?;
        }
        Ok(())
    }
    pub async fn prepare_command_edit(
        &self,
        request: CommandEditRequest,
        cancel: watch::Receiver<bool>,
    ) -> Result<CommandEditReview, AppError> {
        check_cancel(&cancel)?;
        let (session, receiver) =
            Session::new(matches!(&request, CommandEditRequest::Encode { .. }));
        self.command_edits.insert(session.clone());
        let admitted = match request {
            CommandEditRequest::Encode { request } => {
                let request = *request;
                match encode::validate_settings(&request.settings) {
                    Ok(()) => self
                        .admit_job_with_review(
                            request.source,
                            Some(request.settings),
                            None,
                            true,
                            Some(session.clone()),
                        )
                        .await
                        .map(Some),
                    Err(error) => Err(error),
                }
            }
            CommandEditRequest::Image { request } => {
                images::start(self, &session, *request).await.map(|()| None)
            }
            CommandEditRequest::ResumeSaved { id } => self
                .resume_job_with_review(id, Some(session.clone()))
                .await
                .map(Some),
            CommandEditRequest::ResumeNew { id, settings } => self
                .av1an_recovery_action_with_review(
                    media_core::Av1anRecoveryAction::ResumeNew { id, settings },
                    Some(session.clone()),
                )
                .await
                .map(|result| result.jobs.into_iter().next()),
        };
        if let Err(error) = admitted {
            self.command_edits.remove(&session.id);
            return Err(error);
        }
        let result = tokio::select! {
            biased;
            _ = wait_cancel(cancel.clone()) => { let _ = self.cancel_command_edit(&session.id).await; Err(canceled_review()) },
            result = receiver => result.map_err(|_| expired())?,
        };
        match result {
            Ok(mut review) => {
                if *cancel.borrow() {
                    self.cancel_command_edit(&session.id).await?;
                    return Err(canceled_review());
                }
                if let Some(id) = session.job_id() {
                    review.job = self.list_jobs().await.into_iter().find(|job| job.id == id);
                }
                Ok(review)
            }
            Err(error) => {
                self.command_edits.remove(&session.id);
                Err(error)
            }
        }
    }

    pub async fn confirm_command_edit(
        &self,
        request: CommandEditConfirmation,
        cancel: watch::Receiver<bool>,
    ) -> Result<CommandEditResult, AppError> {
        if request.arguments.contains('\0') || request.arguments.len() > 1024 * 1024 {
            return Err(AppError::new(
                "COMMAND_EDIT_INVALID",
                "The command must contain no NUL characters and fit within 1 MiB.",
                None,
            ));
        }
        let session = self.command_edits.get(&request.id)?;
        if request.arguments.trim().is_empty() {
            self.cancel_command_edit(&request.id).await?;
            return Err(canceled_review());
        }
        check_cancel(&cancel)?;
        let sender = session
            .decision
            .lock()
            .unwrap_or_else(|cause| cause.into_inner())
            .take()
            .ok_or_else(expired)?;
        sender.send(request.arguments).map_err(|_| expired())?;
        let receiver = session
            .acknowledgment
            .lock()
            .await
            .take()
            .ok_or_else(expired)?;
        let result = tokio::select! {
            biased;
            _ = wait_cancel(cancel.clone()) => { let _ = self.cancel_command_edit(&request.id).await; Err(canceled_review()) },
            result = receiver => result.map_err(|_| expired())?,
        };
        result?;
        if let Some(id) = session.job_id() {
            self.command_edits.remove(&request.id);
            let job = self
                .list_jobs()
                .await
                .into_iter()
                .find(|job| job.id == id)
                .ok_or_else(expired)?;
            Ok(CommandEditResult::Encode { job: Box::new(job) })
        } else {
            let result = images::finish(&session, cancel).await;
            self.command_edits.remove(&request.id);
            result.map(|result| CommandEditResult::Image { result })
        }
    }

    pub async fn cancel_command_edit(&self, id: &str) -> Result<(), AppError> {
        let session = match self.command_edits.get(id) {
            Ok(session) => session,
            Err(_) => return Ok(()),
        };
        session.cancel();
        if let Some(id) = session.job_id() {
            let _ = self.cancel_job(id).await?;
        } else {
            images::join(&session).await;
        }
        self.command_edits.remove(&session.id);
        Ok(())
    }

    pub(super) async fn command_review(&self, id: &str) -> Option<Arc<Session>> {
        self.state
            .lock()
            .await
            .entries
            .iter()
            .find(|entry| entry.snapshot.id == id)
            .and_then(|entry| entry.command_review.clone())
    }
    pub(super) async fn discard_unconfirmed_command(
        &self,
        id: &str,
        session: &Session,
    ) -> Result<(), AppError> {
        if session.confirmed.load(Ordering::Relaxed) {
            return Ok(());
        }
        let snapshot = self
            .list_jobs()
            .await
            .into_iter()
            .find(|job| job.id == id)
            .ok_or_else(expired)?;
        let backup = session
            .resume_backup
            .lock()
            .unwrap_or_else(|cause| cause.into_inner())
            .take();
        if let Some((original, recipe)) = backup {
            let saved = snapshot.clone();
            let (summary, keep) = tokio::task::spawn_blocking(move || {
                let mut owned = av1an::OwnedRecovery::open(&saved, false)?;
                let summary = owned.restore_command_recipe(recipe)?;
                Ok::<_, AppError>((summary, owned.source_variant_settings()))
            })
            .await
            .map_err(|cause| AppError::new("RECOVERY_INVALID", cause.to_string(), None))??;
            if let Some(cleanup) = source_normalization::prepare_recovery_action(
                id,
                &snapshot.request,
                snapshot.encode_settings.as_ref().expect("saved settings"),
                snapshot.encode_settings.as_ref().expect("saved settings"),
            )? {
                cleanup.rebind(
                    original
                        .encode_settings
                        .as_ref()
                        .expect("original settings"),
                )?;
            }
            source_normalization::av1an_trim::discard_variants_except(
                id,
                &snapshot.request,
                &keep,
            )?;
            self.change(id, |snapshot| { snapshot.encode_settings = original.encode_settings; snapshot.edited_command = original.edited_command; snapshot.recovery = Some(summary); append_log(snapshot, "Command review canceled; the previously confirmed saved command, settings and remaining chunk receipts were restored.".into()); }).await;
            return Ok(());
        }
        if !session.fresh {
            return Ok(());
        }
        if snapshot.recovery.is_some() {
            let saved = snapshot.clone();
            tokio::task::spawn_blocking(move || {
                av1an::OwnedRecovery::open(&saved, false)?.discard()
            })
            .await
            .map_err(|cause| AppError::new("RECOVERY_INVALID", cause.to_string(), None))??;
        }
        if let Some(settings) = &snapshot.encode_settings {
            source_normalization::discard(id, &snapshot.request, settings)?;
            source_normalization::av1an_trim::discard(id, &snapshot.request, settings)?;
        }
        self.change(id, |snapshot| {
            snapshot.recovery = None;
            snapshot.standalone_recovery = None;
        })
        .await;
        Ok(())
    }

    pub(super) async fn review_command(
        &self,
        id: &str,
        label: &str,
        generated: &str,
        output: &Path,
        cancel: &watch::Receiver<bool>,
    ) -> Result<Option<EditedCommand>, AppError> {
        self.review_command_initial(id, label, generated, generated, output, cancel)
            .await
    }
    pub(super) async fn review_command_initial(
        &self,
        id: &str,
        label: &str,
        generated: &str,
        initial: &str,
        output: &Path,
        cancel: &watch::Receiver<bool>,
    ) -> Result<Option<EditedCommand>, AppError> {
        let Some(session) = self.command_review(id).await else {
            return Ok(None);
        };
        self.phase(
            id,
            JobState::Preparing,
            "Reviewing the held command before execution.",
        )
        .await;
        let decision = session
            .ask_initial(label, generated, initial, output, cancel)
            .await?;
        // Only confirmed text enters durable history. A close/empty decision
        // never writes an argument override or starts a media encoder.
        self.change(id, |snapshot| {
            snapshot.edited_command =
                (decision.arguments != decision.generated.trim()).then(|| decision.clone());
            snapshot.state = JobState::Running;
            append_log(
                snapshot,
                "Command review confirmed; running the reviewed task.".into(),
            );
        })
        .await;
        let ready = self.ready().await;
        session.acknowledge(ready.clone());
        ready?;
        Ok(Some(decision))
    }

    pub(super) async fn run_edited_program(
        &self,
        id: &str,
        command: &EditedCommand,
        cwd: Option<PathBuf>,
        source: &Path,
        cancel: &watch::Receiver<bool>,
        log: &Path,
    ) -> Result<(), AppError> {
        self.run_edited_program_with_environment(id, command, cwd, source, cancel, log, None)
            .await
    }

    #[allow(clippy::too_many_arguments)]
    pub(super) async fn run_edited_program_with_environment(
        &self,
        id: &str,
        command: &EditedCommand,
        cwd: Option<PathBuf>,
        source: &Path,
        cancel: &watch::Receiver<bool>,
        log: &Path,
        environment: Option<&crate::supervisor::ChildEnvironment>,
    ) -> Result<(), AppError> {
        check_cancel(cancel)?;
        self.phase(
            id,
            JobState::Running,
            "Running the confirmed edited command.",
        )
        .await;
        tokio::fs::create_dir_all(self.log_dir.as_ref())
            .await
            .map_err(|cause| {
                files::error(
                    "LOG_CREATE_FAILED",
                    cause.to_string(),
                    self.log_dir.as_ref(),
                )
            })?;
        let (sender, events) = mpsc::channel(256);
        let observer = self.observe_encode(id, events, None);
        let shell = PreparedShell::new(&command.arguments, cwd, log, environment)?;
        let result = supervisor::with_console_profile(supervisor::ConsoleProfile::General, || {
            supervisor::run_with_environment(
                &shell.command,
                cancel.clone(),
                sender,
                log,
                Duration::from_secs(24 * 60 * 60),
                Some(&shell.environment),
                None,
            )
        })
        .await;
        let _ = observer.await;
        let result = result.map_err(|cause| process_error(cause, source))?;
        if result.status.success() {
            shell.verify()?;
        }
        if !result.status.success() {
            return Err(files::error(
                "EDITED_COMMAND_FAILED",
                format!(
                    "The confirmed command failed ({}). Inspect the job log.",
                    result.status
                ),
                source,
            ));
        }
        check_cancel(cancel)
    }
}

pub(super) async fn verify_actual_output(
    ffmpeg: &Path,
    ffprobe: &Path,
    temporary: &Temporary,
    cancel: &watch::Receiver<bool>,
) -> Result<Document, AppError> {
    temporary.flush_nonempty_async().await?;
    let document = probe(ffprobe, &temporary.path, cancel, None).await?;
    if document.streams.is_empty()
        || !document.streams.iter().any(|stream| {
            stream
                .nb_read_packets
                .as_deref()
                .and_then(|value| value.parse::<u64>().ok())
                .is_some_and(|count| count > 0)
        })
    {
        return Err(files::error(
            "EDITED_COMMAND_OUTPUT_INVALID",
            "The edited command did not produce any media packets in its held output.",
            &temporary.path,
        ));
    }
    if document
        .streams
        .iter()
        .any(|stream| matches!(stream.codec_type.as_deref(), Some("video" | "audio")))
    {
        let mut args: Vec<OsString> = [
            "-v",
            "error",
            "-nostdin",
            "-xerror",
            "-err_detect",
            "explode",
            "-protocol_whitelist",
            "file,pipe",
            "-i",
        ]
        .into_iter()
        .map(Into::into)
        .collect();
        args.push(temporary.path.as_os_str().to_owned());
        args.extend(
            ["-map", "0:v?", "-map", "0:a?", "-f", "null", "-"]
                .into_iter()
                .map(OsString::from),
        );
        let result = supervisor::run_capture(
            &CommandSpec {
                executable: ffmpeg.to_owned(),
                args,
                cwd: None,
            },
            cancel.clone(),
            4 * 1024 * 1024,
            Duration::from_secs(24 * 60 * 60),
        )
        .await
        .map_err(|cause| process_error(cause, &temporary.path))?;
        if !result.status.success() {
            return Err(files::error(
                "EDITED_COMMAND_OUTPUT_INVALID",
                format!(
                    "The edited output could not be fully decoded: {}",
                    String::from_utf8_lossy(&result.stderr).trim()
                ),
                &temporary.path,
            ));
        }
    }
    check_cancel(cancel)?;
    Ok(document)
}

pub(crate) fn quote(value: &str) -> String {
    #[cfg(unix)]
    {
        format!("'{}'", value.replace('\'', "'\"'\"'"))
    }
    #[cfg(windows)]
    {
        shell::quote_windows(value)
    }
}

pub(crate) fn format_command(command: &CommandSpec) -> String {
    let mut text = std::iter::once(command.executable.to_string_lossy().into_owned())
        .chain(
            command
                .args
                .iter()
                .map(|arg| arg.to_string_lossy().into_owned()),
        )
        .map(|arg| quote(&arg))
        .collect::<Vec<_>>()
        .join(" ");
    if let Some(directory) = &command.cwd {
        let cd = if cfg!(windows) { "cd /d" } else { "cd" };
        text = format!("({cd} {} && {text})", quote(&directory.to_string_lossy()));
    }
    text
}

#[cfg(unix)]
pub(crate) fn shell_command(text: &str, cwd: Option<PathBuf>) -> CommandSpec {
    CommandSpec {
        executable: PathBuf::from("/bin/bash"),
        args: vec!["-o".into(), "pipefail".into(), "-c".into(), text.into()],
        cwd,
    }
}
