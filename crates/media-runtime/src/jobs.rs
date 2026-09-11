//! Durable, single-job local encoding with owned staging and no-clobber publication.

use std::{
    collections::HashSet,
    ffi::OsString,
    fs::{self, File, OpenOptions},
    io::{Read, Write},
    path::{Path, PathBuf},
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, AtomicU64, Ordering},
    },
    time::{Duration, SystemTime, UNIX_EPOCH},
};

use fs2::FileExt;
use media_core::{AppError, EncodeJob, EncodeRequest, JobStatus};
use serde_json::Value;
use tokio::sync::{Notify, watch};

use crate::{
    discovery::find_executable,
    supervisor::{self, CommandSpec, RunError},
};

const MAX_JOBS: usize = 100;
const MAX_STORE_BYTES: u64 = 4 * 1024 * 1024;
static NEXT_ID: AtomicU64 = AtomicU64::new(0);

struct ActiveJob {
    id: String,
    cancel: watch::Sender<bool>,
}

struct State {
    jobs: Vec<EncodeJob>,
    active: Option<ActiveJob>,
}

pub struct JobManager {
    directory: PathBuf,
    // Held for the entire manager lifetime, including while background jobs run.
    _lock: File,
    state: Mutex<State>,
    closing: AtomicBool,
    idle: Notify,
}

fn error(code: &str, message: impl Into<String>) -> AppError {
    AppError::new(code, message, None)
}

fn io_error(context: &str, cause: impl std::fmt::Display) -> AppError {
    error("JOB_IO", format!("{context}: {cause}"))
}

fn now_ms() -> u128 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis()
}

fn active(status: &JobStatus) -> bool {
    matches!(
        status,
        JobStatus::Preparing
            | JobStatus::Encoding
            | JobStatus::Muxing
            | JobStatus::Validating
            | JobStatus::Publishing
    )
}

impl JobManager {
    pub fn open(directory: PathBuf) -> Result<Self, AppError> {
        fs::create_dir_all(&directory)
            .map_err(|e| io_error("Could not create the job store", e))?;
        let directory = directory
            .canonicalize()
            .map_err(|e| io_error("Could not resolve the job store", e))?;
        let lock = OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .open(directory.join("jobs.lock"))
            .map_err(|e| io_error("Could not open the job lock", e))?;
        FileExt::try_lock_exclusive(&lock).map_err(|_| {
            error(
                "JOB_STORE_BUSY",
                "Another jesses process is using this job store.",
            )
        })?;
        let mut jobs: Vec<EncodeJob> = match File::open(directory.join("jobs.json")) {
            Ok(file) => {
                let mut bytes = Vec::new();
                file.take(MAX_STORE_BYTES + 1)
                    .read_to_end(&mut bytes)
                    .map_err(|e| io_error("Could not read saved jobs", e))?;
                if bytes.len() as u64 > MAX_STORE_BYTES {
                    return Err(error(
                        "JOB_STORE_INVALID",
                        "The saved job file exceeds its size limit.",
                    ));
                }
                serde_json::from_slice(&bytes).map_err(|e| {
                    error(
                        "JOB_STORE_INVALID",
                        format!("Saved jobs could not be read; the file was preserved: {e}"),
                    )
                })?
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Vec::new(),
            Err(e) => return Err(io_error("Could not read saved jobs", e)),
        };
        let mut ids = HashSet::new();
        if jobs.len() > MAX_JOBS
            || jobs
                .iter()
                .any(|job| !valid_id(&job.id) || !ids.insert(job.id.clone()))
        {
            return Err(error(
                "JOB_STORE_INVALID",
                "Saved jobs contain invalid identifiers or exceed the history limit.",
            ));
        }
        for job in &mut jobs {
            if active(&job.status) {
                job.status = JobStatus::Interrupted;
                job.progress = None;
                job.updated_at_ms = now_ms().to_string();
                job.message = "Interrupted when jesses stopped. Any published destination was preserved; choose a new destination before retrying.".into();
            }
            // Only exact, marked staging directories and fixed leaf names are eligible.
            // Never recursively delete a path obtained from a saved record.
            cleanup_stage(job);
        }
        let manager = Self {
            directory,
            _lock: lock,
            state: Mutex::new(State { jobs, active: None }),
            closing: AtomicBool::new(false),
            idle: Notify::new(),
        };
        {
            let state = manager
                .state
                .lock()
                .map_err(|_| error("JOB_STATE", "The job store is unavailable."))?;
            manager.save(&state.jobs)?;
        }
        Ok(manager)
    }

    pub fn list(&self) -> Result<Vec<EncodeJob>, AppError> {
        let state = self
            .state
            .lock()
            .map_err(|_| error("JOB_STATE", "The job store is unavailable."))?;
        Ok(state.jobs.iter().rev().cloned().collect())
    }

    pub async fn start(self: &Arc<Self>, request: EncodeRequest) -> Result<EncodeJob, AppError> {
        if self.closing.load(Ordering::Acquire) {
            return Err(error("APP_CLOSING", "jesses is shutting down."));
        }
        let request = validate_paths(request).await?;
        let id = format!(
            "job-{}-{}-{}",
            now_ms(),
            std::process::id(),
            NEXT_ID.fetch_add(1, Ordering::Relaxed)
        );
        let timestamp = now_ms().to_string();
        let job = EncodeJob {
            id: id.clone(),
            request,
            status: JobStatus::Preparing,
            progress: None,
            message: "Checking source streams, timing, and installed encoders.".into(),
            created_at_ms: timestamp.clone(),
            updated_at_ms: timestamp,
        };
        let (sender, receiver) = watch::channel(false);
        {
            let mut state = self
                .state
                .lock()
                .map_err(|_| error("JOB_STATE", "The job store is unavailable."))?;
            if self.closing.load(Ordering::Acquire) {
                return Err(error("APP_CLOSING", "jesses is shutting down."));
            }
            if state.active.is_some() {
                return Err(error(
                    "JOB_BUSY",
                    "An encode is already running. Cancel it or wait for it to finish.",
                ));
            }
            let previous_jobs = state.jobs.clone();
            if state.jobs.len() == MAX_JOBS {
                state.jobs.remove(0);
            }
            state.jobs.push(job.clone());
            if let Err(e) = self.save(&state.jobs) {
                state.jobs = previous_jobs;
                return Err(e);
            }
            state.active = Some(ActiveJob { id, cancel: sender });
        }
        let manager = Arc::clone(self);
        let running = job.clone();
        tokio::spawn(async move {
            let outcome = manager.execute(&running, receiver.clone()).await;
            cleanup_stage(&running);
            let (status, message) = match outcome {
                Ok(()) => (
                    JobStatus::Completed,
                    "Encoding complete. The MKV was verified and saved.".into(),
                ),
                Err(RunError::Cancelled) => (
                    JobStatus::Cancelled,
                    "Encoding cancelled. No partial output was published.".into(),
                ),
                Err(RunError::Failed(message)) => (JobStatus::Failed, message),
            };
            manager.finish(&running.id, status, message);
        });
        Ok(job)
    }

    pub fn cancel(&self, id: &str) -> Result<(), AppError> {
        let state = self
            .state
            .lock()
            .map_err(|_| error("JOB_STATE", "The job store is unavailable."))?;
        match &state.active {
            Some(job) if job.id == id => {
                let _ = job.cancel.send(true);
                Ok(())
            }
            _ if state.jobs.iter().any(|job| job.id == id) => Ok(()),
            _ => Err(error(
                "JOB_NOT_FOUND",
                "This job is no longer in the history.",
            )),
        }
    }

    pub fn shutdown(&self) {
        self.closing.store(true, Ordering::Release);
        if let Ok(state) = self.state.lock()
            && let Some(job) = &state.active
        {
            let _ = job.cancel.send(true);
        }
    }

    pub async fn wait_idle(&self) {
        loop {
            let notified = self.idle.notified();
            tokio::pin!(notified);
            notified.as_mut().enable();
            if self
                .state
                .lock()
                .map_or(true, |state| state.active.is_none())
            {
                return;
            }
            notified.await;
        }
    }

    fn save(&self, jobs: &[EncodeJob]) -> Result<(), AppError> {
        let mut temporary = tempfile::NamedTempFile::new_in(&self.directory)
            .map_err(|e| io_error("Could not stage saved jobs", e))?;
        serde_json::to_writer(&mut temporary, jobs)
            .map_err(|e| io_error("Could not serialize jobs", e))?;
        temporary
            .flush()
            .map_err(|e| io_error("Could not flush jobs", e))?;
        temporary
            .as_file()
            .sync_all()
            .map_err(|e| io_error("Could not sync jobs", e))?;
        temporary
            .persist(self.directory.join("jobs.json"))
            .map_err(|e| io_error("Could not publish saved jobs", e.error))?;
        #[cfg(unix)]
        File::open(&self.directory)
            .and_then(|file| file.sync_all())
            .map_err(|e| io_error("Could not sync the job directory", e))?;
        Ok(())
    }

    fn transition(
        &self,
        id: &str,
        status: JobStatus,
        progress: Option<f64>,
        message: &str,
    ) -> Result<(), RunError> {
        let mut state = self
            .state
            .lock()
            .map_err(|_| RunError::Failed("The job store is unavailable.".into()))?;
        let job = state
            .jobs
            .iter_mut()
            .find(|job| job.id == id)
            .ok_or_else(|| RunError::Failed("The active job disappeared.".into()))?;
        job.status = status;
        job.progress = progress;
        job.message = message.into();
        job.updated_at_ms = now_ms().to_string();
        self.save(&state.jobs)
            .map_err(|e| RunError::Failed(e.message))
    }

    fn progress(&self, id: &str, percentage: f64) {
        if let Ok(mut state) = self.state.lock()
            && let Some(job) = state.jobs.iter_mut().find(|job| job.id == id)
        {
            // Progress is ephemeral; durable phase transitions are written separately.
            job.progress = Some(percentage.clamp(0.0, 99.0));
            job.updated_at_ms = now_ms().to_string();
        }
    }

    fn finish(&self, id: &str, status: JobStatus, message: String) {
        if let Ok(mut state) = self.state.lock() {
            if let Some(job) = state.jobs.iter_mut().find(|job| job.id == id) {
                job.progress = matches!(status, JobStatus::Completed).then_some(100.0);
                job.status = status;
                job.message = message;
                job.updated_at_ms = now_ms().to_string();
            }
            if let Err(e) = self.save(&state.jobs)
                && let Some(job) = state.jobs.iter_mut().find(|job| job.id == id)
            {
                job.message
                    .push_str(&format!(" Job history could not be saved: {}", e.message));
            }
            state.active = None;
        }
        self.idle.notify_waiters();
    }

    async fn execute(
        self: &Arc<Self>,
        job: &EncodeJob,
        cancel: watch::Receiver<bool>,
    ) -> Result<(), RunError> {
        check_cancel(&cancel)?;
        let plan = preflight(&job.request, cancel.clone()).await?;
        check_cancel(&cancel)?;
        let stage = create_stage(job).map_err(|e| RunError::Failed(e.message))?;
        self.transition(
            &job.id,
            JobStatus::Encoding,
            Some(0.0),
            "Encoding video with standalone SVT-AV1.",
        )?;
        supervisor::run(
            encode_commands(&plan, &job.request, &stage),
            cancel.clone(),
            self.progress_callback(job, plan.duration, 0.0, 80.0),
        )
        .await?;
        check_cancel(&cancel)?;
        verify_source_unchanged(&plan)?;
        self.transition(&job.id, JobStatus::Muxing, Some(80.0), "Encoding all audio to Opus and preserving subtitles, attachments, chapters, and metadata.")?;
        supervisor::run(
            vec![mux_command(&plan, &job.request, &stage)],
            cancel.clone(),
            self.progress_callback(job, plan.duration, 80.0, 15.0),
        )
        .await?;
        check_cancel(&cancel)?;
        verify_source_unchanged(&plan)?;
        self.transition(
            &job.id,
            JobStatus::Validating,
            Some(95.0),
            "Verifying the staged MKV before saving it.",
        )?;
        let output =
            probe_document(&plan.ffprobe, &stage.join("output.mkv"), cancel.clone()).await?;
        validate_output(&plan, &output)?;
        let output_frames =
            scan_timing(&plan, &stage.join("output.mkv"), 0, cancel.clone()).await?;
        if output_frames != plan.frame_count {
            return Err(RunError::Failed(format!(
                "Output verification failed: expected {} video frames, found {output_frames}.",
                plan.frame_count
            )));
        }
        check_cancel(&cancel)?;
        self.transition(
            &job.id,
            JobStatus::Publishing,
            Some(99.0),
            "Saving the verified MKV without replacing existing files.",
        )?;
        check_cancel(&cancel)?;
        publish(
            &stage.join("output.mkv"),
            Path::new(&job.request.output_path),
        )
        .map_err(|e| RunError::Failed(e.message))?;
        // Publication is the commit point. A late cancellation cannot undo completion.
        Ok(())
    }

    fn progress_callback(
        self: &Arc<Self>,
        job: &EncodeJob,
        duration: f64,
        base: f64,
        span: f64,
    ) -> Arc<dyn Fn(String) + Send + Sync> {
        let manager = Arc::clone(self);
        let id = job.id.clone();
        Arc::new(move |line| {
            if let Some(value) = line
                .trim()
                .strip_prefix("out_time_us=")
                .and_then(|value| value.parse::<f64>().ok())
                .filter(|value| value.is_finite())
            {
                manager.progress(
                    &id,
                    base + span * (value / 1_000_000.0 / duration).clamp(0.0, 1.0),
                );
            }
        })
    }
}

fn check_cancel(cancel: &watch::Receiver<bool>) -> Result<(), RunError> {
    if *cancel.borrow() {
        Err(RunError::Cancelled)
    } else {
        Ok(())
    }
}

fn valid_id(id: &str) -> bool {
    id.starts_with("job-")
        && id.len() <= 100
        && id
            .bytes()
            .all(|byte| byte.is_ascii_digit() || byte == b'-' || b"job".contains(&byte))
}

fn stage_path(job: &EncodeJob) -> Option<PathBuf> {
    valid_id(&job.id)
        .then(|| {
            Path::new(&job.request.output_path)
                .parent()
                .map(|parent| parent.join(format!(".jesses-{}", job.id)))
        })
        .flatten()
}

fn create_stage(job: &EncodeJob) -> Result<PathBuf, AppError> {
    let stage = stage_path(job)
        .ok_or_else(|| error("OUTPUT_INVALID", "The output directory is invalid."))?;
    fs::create_dir(&stage)
        .map_err(|e| io_error("Could not create an exclusive output staging directory", e))?;
    let mut owner = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(stage.join(".jesses-owner"))
        .map_err(|e| io_error("Could not mark the output staging directory", e))?;
    owner
        .write_all(job.id.as_bytes())
        .and_then(|()| owner.sync_all())
        .map_err(|e| io_error("Could not save staging ownership", e))?;
    Ok(stage)
}

fn cleanup_stage(job: &EncodeJob) {
    let Some(stage) = stage_path(job) else {
        return;
    };
    let Ok(metadata) = fs::symlink_metadata(&stage) else {
        return;
    };
    if !metadata.is_dir() || metadata.file_type().is_symlink() {
        return;
    }
    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt;
        if metadata.file_attributes() & 0x400 != 0 {
            return;
        }
    }
    let marker = stage.join(".jesses-owner");
    let Ok(owner) = fs::symlink_metadata(&marker) else {
        return;
    };
    if !owner.is_file() || owner.file_type().is_symlink() || owner.len() > 100 {
        return;
    }
    if fs::read_to_string(&marker).ok().as_deref() != Some(&job.id) {
        return;
    }
    for name in ["video.ivf", "output.mkv"] {
        let path = stage.join(name);
        if fs::symlink_metadata(&path)
            .is_ok_and(|metadata| metadata.is_file() && !metadata.file_type().is_symlink())
            && fs::remove_file(path).is_err()
        {
            return;
        }
    }
    if fs::read_dir(&stage).map_or(true, |mut entries| {
        entries.any(|entry| entry.map_or(true, |entry| entry.file_name() != ".jesses-owner"))
    }) {
        return;
    }
    let _ = fs::remove_file(marker);
    let _ = fs::remove_dir(stage); // Nonempty or foreign contents are preserved.
}

fn publish(staged: &Path, destination: &Path) -> Result<(), AppError> {
    OpenOptions::new()
        .read(true)
        .write(true)
        .open(staged)
        .and_then(|file| file.sync_all())
        .map_err(|e| io_error("Could not flush the verified output", e))?;
    // A same-filesystem hard link atomically fails if the destination exists.
    // The staging link is removed only after the destination has been committed.
    fs::hard_link(staged, destination).map_err(|e| {
        if e.kind() == std::io::ErrorKind::AlreadyExists { error("OUTPUT_EXISTS", "The destination appeared while encoding. It was preserved; choose another output filename.") }
        else { io_error("Could not publish output; use a destination filesystem that supports hard links", e) }
    })?;
    #[cfg(unix)]
    if let Some(parent) = destination.parent() {
        let _ = File::open(parent).and_then(|file| file.sync_all());
    }
    Ok(())
}

async fn validate_paths(mut request: EncodeRequest) -> Result<EncodeRequest, AppError> {
    if request.crf > 63
        || request.preset > 13
        || !(32..=512).contains(&request.audio_bitrate_kbps)
        || !matches!(request.audio_channels, None | Some(1) | Some(2))
    {
        return Err(error(
            "ENCODE_SETTINGS_INVALID",
            "Use CRF 0–63, preset 0–13, audio 32–512 kb/s, and preserved, mono, or stereo audio channels.",
        ));
    }
    for path in [&request.input_path, &request.output_path] {
        if path.trim().is_empty()
            || path.contains('\0')
            || path.contains("://")
            || !Path::new(path).is_absolute()
        {
            return Err(error(
                "PATH_INVALID",
                "Choose absolute local input and output paths.",
            ));
        }
    }
    let source = tokio::fs::canonicalize(&request.input_path)
        .await
        .map_err(|e| io_error("Could not access the input file", e))?;
    if !tokio::fs::metadata(&source)
        .await
        .map_err(|e| io_error("Could not inspect the input file", e))?
        .is_file()
    {
        return Err(error(
            "NOT_A_FILE",
            "The source must be an individual media file.",
        ));
    }
    let output = Path::new(&request.output_path);
    if !output
        .extension()
        .is_some_and(|extension| extension.eq_ignore_ascii_case("mkv"))
    {
        return Err(error("OUTPUT_INVALID", "Choose an MKV output filename."));
    }
    let name = output
        .file_name()
        .ok_or_else(|| error("OUTPUT_INVALID", "Choose an output filename."))?;
    if name.to_string_lossy().contains([':', '\0']) {
        return Err(error("OUTPUT_INVALID", "The output filename is invalid."));
    }
    let parent = output
        .parent()
        .ok_or_else(|| error("OUTPUT_INVALID", "Choose an output directory."))?;
    let parent = tokio::fs::canonicalize(parent)
        .await
        .map_err(|e| io_error("Could not access the output directory", e))?;
    if !tokio::fs::metadata(&parent)
        .await
        .map_err(|e| io_error("Could not inspect the output directory", e))?
        .is_dir()
    {
        return Err(error(
            "OUTPUT_INVALID",
            "The output parent must be a directory.",
        ));
    }
    let output = parent.join(name);
    if source == output
        || (cfg!(windows)
            && source
                .to_string_lossy()
                .eq_ignore_ascii_case(&output.to_string_lossy()))
    {
        return Err(error(
            "OUTPUT_IS_SOURCE",
            "The output must differ from the source file.",
        ));
    }
    match tokio::fs::symlink_metadata(&output).await {
        Ok(_) => {
            return Err(error(
                "OUTPUT_EXISTS",
                "The output already exists. Choose another filename.",
            ));
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => (),
        Err(e) => return Err(io_error("Could not inspect the output path", e)),
    }
    request.input_path = source
        .to_str()
        .ok_or_else(|| error("PATH_INVALID", "The input path must be valid Unicode."))?
        .into();
    request.output_path = output
        .to_str()
        .ok_or_else(|| error("PATH_INVALID", "The output path must be valid Unicode."))?
        .into();
    Ok(request)
}

struct Plan {
    input: PathBuf,
    ffmpeg: PathBuf,
    ffprobe: PathBuf,
    svt: PathBuf,
    duration: f64,
    fps: f64,
    time_base: f64,
    frame_count: u64,
    source_size: u64,
    source_modified: SystemTime,
    video_index: u64,
    pixel_format: String,
    document: Value,
}

async fn tool(names: &[&str]) -> Result<PathBuf, RunError> {
    find_executable(names)
        .await
        .map_err(RunError::Failed)?
        .ok_or_else(|| {
            RunError::Failed(format!(
                "{} was not found on PATH. Install the standalone tool and restart jesses.",
                names[0]
            ))
        })
}

async fn probe_document(
    executable: &Path,
    input: &Path,
    cancel: watch::Receiver<bool>,
) -> Result<Value, RunError> {
    let mut args = os_args(&[
        "-v",
        "error",
        "-protocol_whitelist",
        "file",
        "-print_format",
        "json",
        "-show_format",
        "-show_streams",
        "-show_chapters",
        "-i",
    ]);
    args.push(input.as_os_str().into());
    let contents = capture_tool(
        executable,
        args,
        cancel,
        4 * 1024 * 1024,
        "Media inspection",
    )
    .await?;
    serde_json::from_slice(&contents)
        .map_err(|e| RunError::Failed(format!("Could not read media metadata: {e}")))
}

async fn capture_tool(
    executable: &Path,
    args: Vec<OsString>,
    mut cancel: watch::Receiver<bool>,
    max_bytes: usize,
    description: &str,
) -> Result<Vec<u8>, RunError> {
    check_cancel(&cancel)?;
    let (stop, receiver) = watch::channel(false);
    let contents = Arc::new(Mutex::new((Vec::<u8>::new(), false)));
    let accumulator = Arc::clone(&contents);
    let stop_overflow = stop.clone();
    let callback = Arc::new(move |line: String| {
        if let Ok(mut contents) = accumulator.lock() {
            if contents.0.len().saturating_add(line.len() + 1) > max_bytes {
                contents.1 = true;
                let _ = stop_overflow.send(true);
            } else if !contents.1 {
                contents.0.extend_from_slice(line.as_bytes());
                contents.0.push(b'\n');
            }
        }
    });
    let execution = supervisor::run(
        vec![CommandSpec {
            executable: executable.to_owned(),
            args,
        }],
        receiver,
        callback,
    );
    tokio::pin!(execution);
    let result = tokio::select! {
        result = &mut execution => result,
        _ = cancel.changed() => {
            let _ = stop.send(true);
            let _ = execution.await;
            return Err(RunError::Cancelled);
        }
        _ = tokio::time::sleep(Duration::from_secs(30)) => {
            let _ = stop.send(true);
            let _ = execution.await;
            return Err(RunError::Failed(format!("{description} exceeded its 30-second time limit.")));
        }
    };
    let mut contents = contents
        .lock()
        .map_err(|_| RunError::Failed(format!("{description} failed.")))?;
    if contents.1 {
        return Err(RunError::Failed(format!(
            "{description} exceeded its output limit."
        )));
    }
    result?;
    Ok(std::mem::take(&mut contents.0))
}

async fn preflight(
    request: &EncodeRequest,
    cancel: watch::Receiver<bool>,
) -> Result<Plan, RunError> {
    let ffmpeg = tool(&["ffmpeg"]).await?;
    check_cancel(&cancel)?;
    let ffprobe = tool(&["ffprobe"]).await?;
    check_cancel(&cancel)?;
    let svt = tool(&["SvtAv1EncApp", "svtav1encapp"]).await?;
    let input = PathBuf::from(&request.input_path);
    let source_metadata = fs::metadata(&input)
        .map_err(|e| RunError::Failed(format!("Could not inspect the source file: {e}")))?;
    let source_size = source_metadata.len();
    let source_modified = source_metadata.modified().map_err(|e| {
        RunError::Failed(format!("Could not read the source modification time: {e}"))
    })?;
    let document = probe_document(&ffprobe, &input, cancel.clone()).await?;
    let (video_index, pixel_format, duration, fps, time_base) = inspect_source(&document)?;
    if streams(&document)
        .iter()
        .any(|stream| stream["codec_type"] == "audio")
    {
        let encoders = capture_tool(
            &ffmpeg,
            os_args(&["-hide_banner", "-encoders"]),
            cancel.clone(),
            256 * 1024,
            "FFmpeg audio capability check",
        )
        .await?;
        if !has_libopus_encoder(&String::from_utf8_lossy(&encoders)) {
            return Err(RunError::Failed("This FFmpeg build does not include the libopus audio encoder required by Quick Convert. Install an FFmpeg build with libopus and restart jesses.".into()));
        }
    }
    let mut plan = Plan {
        input,
        ffmpeg,
        ffprobe,
        svt,
        duration,
        fps,
        time_base,
        frame_count: 0,
        source_size,
        source_modified,
        video_index,
        pixel_format,
        document,
    };
    plan.frame_count = scan_timing(&plan, &plan.input, plan.video_index, cancel).await?;
    verify_source_unchanged(&plan)?;
    Ok(plan)
}

fn has_libopus_encoder(listing: &str) -> bool {
    listing.lines().any(|line| {
        let mut columns = line.split_whitespace();
        matches!((columns.next(), columns.next()), (Some(flags), Some("libopus")) if flags.len() == 6 && flags.starts_with('A'))
    })
}

fn verify_source_unchanged(plan: &Plan) -> Result<(), RunError> {
    let metadata = fs::metadata(&plan.input)
        .map_err(|e| RunError::Failed(format!("The source file became unavailable: {e}")))?;
    if metadata.len() != plan.source_size || metadata.modified().ok() != Some(plan.source_modified)
    {
        return Err(RunError::Failed("The source file changed during encoding. No output was published; restart with the updated source.".into()));
    }
    Ok(())
}

fn number(value: &Value) -> Option<f64> {
    value
        .as_f64()
        .or_else(|| value.as_str()?.parse().ok())
        .filter(|value| value.is_finite())
}

fn rational(value: &Value) -> Option<f64> {
    let (a, b) = value.as_str()?.split_once('/')?;
    let (a, b) = (a.parse::<f64>().ok()?, b.parse::<f64>().ok()?);
    (a.is_finite() && b.is_finite() && a > 0.0 && b > 0.0).then_some(a / b)
}

fn streams(document: &Value) -> &[Value] {
    document["streams"]
        .as_array()
        .map(Vec::as_slice)
        .unwrap_or(&[])
}

fn inspect_source(document: &Value) -> Result<(u64, String, f64, f64, f64), RunError> {
    let fail = |message: &str| RunError::Failed(message.into());
    let videos: Vec<&Value> = streams(document)
        .iter()
        .filter(|stream| stream["codec_type"] == "video")
        .collect();
    if videos.len() != 1 {
        return Err(fail(
            "Quick Convert currently requires exactly one video stream; multiple angles and cover-art video need explicit stream selection.",
        ));
    }
    if streams(document).iter().any(|stream| {
        !matches!(
            stream["codec_type"].as_str(),
            Some("video" | "audio" | "subtitle" | "attachment")
        )
    }) {
        return Err(fail(
            "The source contains a stream type this workflow cannot preserve.",
        ));
    }
    let video = videos[0];
    if video["disposition"]["attached_pic"] == 1
        || !matches!(
            video["field_order"].as_str(),
            None | Some("unknown" | "progressive")
        )
    {
        return Err(fail(
            "Quick Convert requires progressive video without an attached-picture video stream.",
        ));
    }
    let pixel_format = video["pix_fmt"]
        .as_str()
        .ok_or_else(|| fail("The video pixel format is unknown."))?;
    if !matches!(pixel_format, "yuv420p" | "yuv420p10le") {
        return Err(fail(
            "Quick Convert currently supports only 8-bit or 10-bit YUV 4:2:0 SDR video.",
        ));
    }
    for key in ["color_transfer", "color_primaries", "color_space"] {
        if !matches!(
            video[key].as_str(),
            None | Some("unknown" | "unspecified" | "bt709")
        ) {
            return Err(fail(
                "HDR and non-BT.709 color conversions are not supported yet; the source was preserved.",
            ));
        }
    }
    if !matches!(video["color_range"].as_str(), None | Some("unknown" | "tv")) {
        return Err(fail(
            "Full-range video is not supported by this encoding preset.",
        ));
    }
    if video["side_data_list"].as_array().is_some_and(|items| {
        items.iter().any(|item| {
            let kind = item["side_data_type"]
                .as_str()
                .unwrap_or("")
                .to_ascii_lowercase();
            kind.contains("mastering")
                || kind.contains("content light")
                || kind.contains("dovi")
                || kind.contains("hdr")
                || kind.contains("display matrix")
        })
    }) {
        return Err(fail(
            "HDR or rotated video needs a dedicated conversion workflow.",
        ));
    }
    let (width, height) = (
        video["width"].as_u64().unwrap_or(0),
        video["height"].as_u64().unwrap_or(0),
    );
    if width < 64
        || height < 64
        || width > 8192
        || height > 8192
        || width % 2 != 0
        || height % 2 != 0
    {
        return Err(fail(
            "SVT-AV1 requires supported, even video dimensions between 64 and 8192 pixels.",
        ));
    }
    if !matches!(
        video["sample_aspect_ratio"].as_str(),
        None | Some("1:1" | "N/A")
    ) {
        return Err(fail(
            "Anamorphic video needs explicit aspect-ratio handling and is not supported yet.",
        ));
    }
    let fps = rational(&video["avg_frame_rate"])
        .ok_or_else(|| fail("A reliable constant frame rate is required."))?;
    let nominal = rational(&video["r_frame_rate"])
        .ok_or_else(|| fail("A reliable constant frame rate is required."))?;
    if !(1.0..=120.0).contains(&fps) || (nominal - fps).abs() / fps > 0.001 {
        return Err(fail(
            "Variable or unsupported frame-rate video is not supported yet.",
        ));
    }
    let duration = number(&document["format"]["duration"])
        .filter(|duration| *duration > 0.0)
        .ok_or_else(|| fail("The source has no reliable finite duration."))?;
    let time_base = rational(&video["time_base"])
        .ok_or_else(|| fail("The source video has no reliable timestamp time base."))?;
    if time_base > 0.02 {
        return Err(fail(
            "The source timestamp precision is too low to verify constant frame rate.",
        ));
    }
    let index = video["index"]
        .as_u64()
        .ok_or_else(|| fail("The source video stream index is missing."))?;
    Ok((index, pixel_format.into(), duration, fps, time_base))
}

#[derive(Default)]
struct Timing {
    count: u64,
    packets: u64,
    first: Option<f64>,
    error: Option<String>,
}

impl Timing {
    fn observe(&mut self, line: &str, fps: f64, time_base: f64) {
        if self.error.is_some() || !line.starts_with("dts_time=") {
            return;
        }
        self.packets += 1;
        let mut timestamp = None;
        let mut duration = None;
        for pair in line.split('|') {
            if let Some(value) = pair.strip_prefix("dts_time=") {
                timestamp = value.parse::<f64>().ok().filter(|value| value.is_finite());
            }
            if let Some(value) = pair.strip_prefix("duration_time=") {
                duration = value.parse::<f64>().ok().filter(|value| value.is_finite());
            }
        }
        let tolerance = (time_base * 2.01).max(0.002);
        let expected = 1.0 / fps;
        if duration
            .is_some_and(|duration| duration > 0.0 && (duration - expected).abs() > tolerance)
        {
            self.error = Some("Variable frame durations were detected. Quick Convert does not support VFR sources yet.".into());
            return;
        }
        // A few leading N/A DTS values are normal for codecs with frame reordering.
        let Some(timestamp) = timestamp else {
            if self.first.is_some() {
                self.error = Some("The source has missing video timestamps.".into());
            }
            return;
        };
        let first = *self.first.get_or_insert(timestamp);
        if (timestamp - first - self.count as f64 * expected).abs() > tolerance {
            self.error = Some("Variable or discontinuous frame timing was detected. Quick Convert requires a constant frame rate.".into());
            return;
        }
        self.count += 1;
    }
}

async fn scan_timing(
    plan: &Plan,
    path: &Path,
    video_index: u64,
    cancel: watch::Receiver<bool>,
) -> Result<u64, RunError> {
    let timing = Arc::new(Mutex::new(Timing::default()));
    let result = Arc::clone(&timing);
    let (fps, time_base) = (plan.fps, plan.time_base);
    let callback = Arc::new(move |line: String| {
        if let Ok(mut timing) = result.lock() {
            timing.observe(&line, fps, time_base);
        }
    });
    let mut args = os_args(&[
        "-v",
        "error",
        "-protocol_whitelist",
        "file",
        "-select_streams",
    ]);
    args.push(video_index.to_string().into());
    args.extend(os_args(&[
        "-show_packets",
        "-show_entries",
        "packet=dts_time,duration_time",
        "-of",
        "compact=p=0:nk=0",
        "-i",
    ]));
    args.push(path.as_os_str().into());
    supervisor::run(
        vec![CommandSpec {
            executable: plan.ffprobe.clone(),
            args,
        }],
        cancel,
        callback,
    )
    .await?;
    let timing = timing
        .lock()
        .map_err(|_| RunError::Failed("The timing check failed.".into()))?;
    if let Some(message) = &timing.error {
        return Err(RunError::Failed(message.clone()));
    }
    if timing.count < 2 {
        return Err(RunError::Failed(
            "There are not enough reliable frame timestamps to encode this source.".into(),
        ));
    }
    Ok(timing.packets)
}

fn os_args(args: &[&str]) -> Vec<OsString> {
    args.iter().map(OsString::from).collect()
}

fn encode_commands(plan: &Plan, request: &EncodeRequest, stage: &Path) -> Vec<CommandSpec> {
    let mut decode = os_args(&[
        "-hide_banner",
        "-xerror",
        "-nostdin",
        "-v",
        "warning",
        "-nostats",
        "-progress",
        "pipe:2",
        "-protocol_whitelist",
        "file",
        "-noautorotate",
        "-i",
    ]);
    decode.push(plan.input.as_os_str().into());
    decode.extend(os_args(&[
        "-map",
        &format!("0:{}", plan.video_index),
        "-an",
        "-sn",
        "-dn",
        "-pix_fmt",
        &plan.pixel_format,
        "-fps_mode",
        "passthrough",
        "-strict",
        "-1",
        "-f",
        "yuv4mpegpipe",
        "pipe:1",
    ]));
    let mut encode = os_args(&[
        "-i",
        "stdin",
        "--crf",
        &request.crf.to_string(),
        "--preset",
        &request.preset.to_string(),
        "-b",
    ]);
    encode.push(stage.join("video.ivf").into_os_string());
    let video = streams(&plan.document)
        .iter()
        .find(|stream| stream["codec_type"] == "video")
        .expect("preflight guarantees video");
    for (key, flag) in [
        ("color_primaries", "--color-primaries"),
        ("color_transfer", "--transfer-characteristics"),
        ("color_space", "--matrix-coefficients"),
    ] {
        encode.extend(os_args(&[
            flag,
            if video[key] == "bt709" { "1" } else { "2" },
        ]));
    }
    encode.extend(os_args(&["--color-range", "0"]));
    vec![
        CommandSpec {
            executable: plan.ffmpeg.clone(),
            args: decode,
        },
        CommandSpec {
            executable: plan.svt.clone(),
            args: encode,
        },
    ]
}

fn mux_command(plan: &Plan, request: &EncodeRequest, stage: &Path) -> CommandSpec {
    let video = streams(&plan.document)
        .iter()
        .find(|stream| stream["codec_type"] == "video")
        .expect("preflight guarantees video");
    let video_start = number(&video["start_time"]).unwrap_or(0.0);
    let mut args = os_args(&[
        "-hide_banner",
        "-xerror",
        "-nostdin",
        "-v",
        "warning",
        "-nostats",
        "-progress",
        "pipe:1",
        "-n",
        "-copyts",
        "-itsoffset",
        &video_start.to_string(),
        "-i",
    ]);
    args.push(stage.join("video.ivf").into_os_string());
    args.extend(os_args(&["-protocol_whitelist", "file", "-i"]));
    args.push(plan.input.as_os_str().into());
    args.extend(os_args(&[
        "-map",
        "0:v:0",
        "-map",
        "1:a?",
        "-map",
        "1:s?",
        "-map",
        "1:t?",
        "-map_metadata",
        "1",
        "-map_chapters",
        "1",
        "-c:v",
        "copy",
        "-c:a",
        "libopus",
        "-b:a",
        &format!("{}k", request.audio_bitrate_kbps),
        "-c:s",
        "copy",
        "-c:t",
        "copy",
        "-avoid_negative_ts",
        "disabled",
        "-map_metadata:s:v:0",
        &format!("1:s:{}", plan.video_index),
    ]));
    if let Some(channels) = request.audio_channels {
        args.extend(os_args(&["-ac:a", &channels.to_string()]));
    }
    for kind in ["video", "audio", "subtitle", "attachment"] {
        let specifier = match kind {
            "video" => "v",
            "audio" => "a",
            "subtitle" => "s",
            _ => "t",
        };
        for (index, stream) in streams(&plan.document)
            .iter()
            .filter(|stream| stream["codec_type"] == kind)
            .enumerate()
        {
            if let Some(source_index) = stream["index"].as_u64() {
                args.extend(os_args(&[
                    &format!("-map_metadata:s:{specifier}:{index}"),
                    &format!("1:s:{source_index}"),
                ]));
            }
            if matches!(kind, "video" | "audio") {
                for tag in [
                    "BPS",
                    "BPS-eng",
                    "NUMBER_OF_FRAMES",
                    "NUMBER_OF_FRAMES-eng",
                    "NUMBER_OF_BYTES",
                    "NUMBER_OF_BYTES-eng",
                    "DURATION",
                    "DURATION-eng",
                    "_STATISTICS_WRITING_APP",
                    "_STATISTICS_WRITING_DATE_UTC",
                    "_STATISTICS_TAGS",
                    "_STATISTICS_TAGS-eng",
                ] {
                    args.extend(os_args(&[
                        &format!("-metadata:s:{specifier}:{index}"),
                        &format!("{tag}="),
                    ]));
                }
            }
            let dispositions: Vec<&str> = stream["disposition"]
                .as_object()
                .map(|object| {
                    object
                        .iter()
                        .filter_map(|(key, value)| {
                            (value.as_i64() == Some(1)).then_some(key.as_str())
                        })
                        .collect()
                })
                .unwrap_or_default();
            args.extend(os_args(&[
                &format!("-disposition:{specifier}:{index}"),
                &if dispositions.is_empty() {
                    "0".into()
                } else {
                    dispositions.join("+")
                },
            ]));
        }
    }
    args.extend(os_args(&["-f", "matroska"]));
    args.push(stage.join("output.mkv").into_os_string());
    CommandSpec {
        executable: plan.ffmpeg.clone(),
        args,
    }
}

fn validate_output(plan: &Plan, document: &Value) -> Result<(), RunError> {
    let fail = |message: &str| RunError::Failed(format!("Output verification failed: {message}"));
    let output = streams(document);
    let input = streams(&plan.document);
    for kind in ["video", "audio", "subtitle", "attachment"] {
        let source: Vec<&Value> = input
            .iter()
            .filter(|stream| stream["codec_type"] == kind)
            .collect();
        let produced: Vec<&Value> = output
            .iter()
            .filter(|stream| stream["codec_type"] == kind)
            .collect();
        if source.len() != produced.len() {
            return Err(fail("the output stream counts do not match the source."));
        }
        for (source, produced) in source.iter().zip(produced.iter()) {
            let expected_codec = match kind {
                "video" => Some("av1"),
                "audio" => Some("opus"),
                _ => source["codec_name"].as_str(),
            };
            if produced["codec_name"].as_str() != expected_codec {
                return Err(fail("a stream has an unexpected codec."));
            }
            if kind == "video"
                && (source["width"] != produced["width"] || source["height"] != produced["height"])
            {
                return Err(fail("video dimensions changed unexpectedly."));
            }
            if kind == "video" && source["pix_fmt"] != produced["pix_fmt"] {
                return Err(fail(
                    "video bit depth or chroma subsampling changed unexpectedly.",
                ));
            }
            if kind == "video" {
                for key in ["color_transfer", "color_primaries", "color_space"] {
                    if source[key] == "bt709" && produced[key] != "bt709" {
                        return Err(fail("video color metadata was not preserved."));
                    }
                }
            }
            if let (Some(source_start), Some(output_start)) = (
                number(&source["start_time"]),
                number(&produced["start_time"]),
            ) && (source_start - output_start).abs() > 0.1
            {
                return Err(fail("a stream's start timestamp changed unexpectedly."));
            }
            for key in ["language", "title"] {
                if source["tags"][key].is_string() && source["tags"][key] != produced["tags"][key] {
                    return Err(fail("stream language or title metadata was not preserved."));
                }
            }
            for key in ["default", "forced"] {
                if source["disposition"][key].as_i64().unwrap_or(0)
                    != produced["disposition"][key].as_i64().unwrap_or(0)
                {
                    return Err(fail("stream default or forced dispositions changed."));
                }
            }
        }
    }
    let source_chapters = plan.document["chapters"].as_array().map_or(0, Vec::len);
    if document["chapters"].as_array().map_or(0, Vec::len) != source_chapters {
        return Err(fail("chapters were not preserved."));
    }
    if let (Some(source), Some(produced)) = (
        plan.document["chapters"].as_array(),
        document["chapters"].as_array(),
    ) {
        for (source, produced) in source.iter().zip(produced) {
            for key in ["start_time", "end_time"] {
                if let (Some(a), Some(b)) = (number(&source[key]), number(&produced[key]))
                    && (a - b).abs() > 0.002
                {
                    return Err(fail("chapter timestamps were not preserved."));
                }
            }
        }
    }
    let duration = number(&document["format"]["duration"])
        .ok_or_else(|| fail("the output duration is missing."))?;
    if (duration - plan.duration).abs() > (2.0 / plan.fps).max(0.15) {
        return Err(fail("the output duration differs from the source."));
    }
    if number(&document["format"]["size"]).unwrap_or(0.0) <= 0.0 {
        return Err(fail("the output is empty."));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn request(input: &Path, output: &Path) -> EncodeRequest {
        EncodeRequest {
            input_path: input.to_string_lossy().into(),
            output_path: output.to_string_lossy().into(),
            crf: 30,
            preset: 8,
            audio_bitrate_kbps: 128,
            audio_channels: None,
        }
    }

    fn job(directory: &Path, status: JobStatus) -> EncodeJob {
        EncodeJob {
            id: "job-123-1-0".into(),
            request: request(&directory.join("source.mkv"), &directory.join("result.mkv")),
            status,
            progress: Some(90.0),
            message: "working".into(),
            created_at_ms: "1".into(),
            updated_at_ms: "2".into(),
        }
    }

    #[test]
    fn publication_never_replaces_existing_output() {
        let directory = tempfile::tempdir().unwrap();
        let staged = directory.path().join("staged");
        let destination = directory.path().join("destination");
        fs::write(&staged, b"new").unwrap();
        fs::write(&destination, b"original").unwrap();
        assert!(publish(&staged, &destination).is_err());
        assert_eq!(fs::read(&destination).unwrap(), b"original");
        fs::remove_file(&destination).unwrap();
        publish(&staged, &destination).unwrap();
        fs::remove_file(staged).unwrap();
        assert_eq!(fs::read(destination).unwrap(), b"new");
    }

    #[test]
    fn recovery_marks_interrupted_and_only_cleans_marked_staging_leaves() {
        let directory = tempfile::tempdir().unwrap();
        let history = directory.path().join("history");
        fs::create_dir(&history).unwrap();
        let saved = job(directory.path(), JobStatus::Publishing);
        let stage = create_stage(&saved).unwrap();
        fs::write(stage.join("output.mkv"), b"partial").unwrap();
        fs::write(stage.join("unrelated.txt"), b"keep").unwrap();
        fs::write(&saved.request.output_path, b"published").unwrap();
        fs::write(
            history.join("jobs.json"),
            serde_json::to_vec(&vec![saved.clone()]).unwrap(),
        )
        .unwrap();
        let manager = JobManager::open(history.clone()).unwrap();
        assert!(matches!(
            manager.list().unwrap()[0].status,
            JobStatus::Interrupted
        ));
        assert!(!stage.join("output.mkv").exists());
        assert_eq!(fs::read(stage.join("unrelated.txt")).unwrap(), b"keep");
        assert_eq!(fs::read(&saved.request.output_path).unwrap(), b"published");
        assert_eq!(
            JobManager::open(history).err().unwrap().code,
            "JOB_STORE_BUSY"
        );
    }

    #[test]
    fn cleanup_rejects_unmarked_or_mismatched_directory() {
        let directory = tempfile::tempdir().unwrap();
        let saved = job(directory.path(), JobStatus::Encoding);
        let stage = stage_path(&saved).unwrap();
        fs::create_dir(&stage).unwrap();
        fs::write(stage.join("output.mkv"), b"keep").unwrap();
        cleanup_stage(&saved);
        assert!(stage.join("output.mkv").exists());
        fs::write(stage.join(".jesses-owner"), b"another-job").unwrap();
        cleanup_stage(&saved);
        assert!(stage.join("output.mkv").exists());
    }

    #[tokio::test]
    async fn preflight_rejects_source_overwrite_existing_output_and_bad_settings() {
        let directory = tempfile::tempdir().unwrap();
        let source = directory.path().join("source.mkv");
        fs::write(&source, b"source").unwrap();
        assert_eq!(
            validate_paths(request(&source, &source))
                .await
                .unwrap_err()
                .code,
            "OUTPUT_IS_SOURCE"
        );
        let destination = directory.path().join("output.mkv");
        fs::write(&destination, b"existing").unwrap();
        assert_eq!(
            validate_paths(request(&source, &destination))
                .await
                .unwrap_err()
                .code,
            "OUTPUT_EXISTS"
        );
        let mut invalid = request(&source, &directory.path().join("new.mkv"));
        invalid.crf = 64;
        assert_eq!(
            validate_paths(invalid).await.unwrap_err().code,
            "ENCODE_SETTINGS_INVALID"
        );
    }

    #[test]
    fn opus_capability_requires_the_exact_audio_encoder_name() {
        assert!(has_libopus_encoder(
            "Encoders:\n A....D libopus libopus Opus\n V....D libaom-av1 AV1"
        ));
        for listing in [
            " A....D libopus_custom A different encoder",
            " A....D opus Native Opus encoder (not libopus)",
            " V....D libopus Invalid video entry",
            "libopus appears only in explanatory text",
        ] {
            assert!(
                !has_libopus_encoder(listing),
                "unexpected capability in {listing}"
            );
        }
    }

    #[test]
    fn timing_accepts_container_tick_rounding_but_rejects_vfr_and_discontinuities() {
        let mut timing = Timing::default();
        for frame in 0..1000 {
            let timestamp = (f64::from(frame) * 1001.0 / 24000.0 * 1000.0).round() / 1000.0;
            timing.observe(
                &format!("dts_time={timestamp:.3}|duration_time=0.041"),
                24000.0 / 1001.0,
                0.001,
            );
        }
        assert_eq!(timing.count, 1000);
        assert!(timing.error.is_none());
        timing.observe(
            "dts_time=42.000|duration_time=0.083",
            24000.0 / 1001.0,
            0.001,
        );
        assert!(timing.error.is_some());
    }
}
