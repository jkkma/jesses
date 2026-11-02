//! Native diagnostic windows. Actual tools retain their existing owned jobs,
//! pipes, pause state and exit status; this helper never runs their commands.
use super::*;
use base64::{Engine, engine::general_purpose::STANDARD};
use std::os::windows::io::{AsRawHandle, BorrowedHandle};
use std::{
    collections::BTreeMap,
    fs::File,
    io::{Read, Seek, SeekFrom, Write},
    sync::{Mutex as StdMutex, OnceLock, atomic::AtomicU64},
};
use windows_sys::Win32::System::Console::{
    AllocConsole, GetConsoleWindow, GetStdHandle, STD_OUTPUT_HANDLE, SetConsoleTitleW,
    WriteConsoleW,
};
const FLAG: &str = "--jesses-owned-console";
const LIMIT: u64 = 4 * 1024 * 1024;
static HOST: OnceLock<PathBuf> = OnceLock::new();
static NEXT: AtomicU64 = AtomicU64::new(0);
static ACTIVE: OnceLock<StdMutex<BTreeMap<u64, Active>>> = OnceLock::new();
fn active() -> &'static StdMutex<BTreeMap<u64, Active>> {
    ACTIVE.get_or_init(|| StdMutex::new(BTreeMap::new()))
}
#[cfg_attr(not(feature = "console-fixture"), allow(dead_code))]
struct Active {
    cancel: watch::Sender<bool>,
    task: tokio::task::JoinHandle<()>,
    title: String,
    directory: PathBuf,
}
struct Writer {
    file: Option<File>,
    length: u64,
}
impl Writer {
    fn emit(&mut self, key: u8, bytes: &[u8]) -> io::Result<()> {
        let file = self.file.as_mut().ok_or_else(|| {
            io::Error::new(io::ErrorKind::BrokenPipe, "The console spool is closed.")
        })?;
        for chunk in bytes.chunks(RECORD_BYTES) {
            let mut record = serde_json::to_vec(&(key, STANDARD.encode(chunk)))?;
            record.push(b'\n');
            if self.length + record.len() as u64 > LIMIT {
                file.set_len(0)?;
                file.seek(SeekFrom::Start(0))?;
                self.length = 0;
            }
            file.write_all(&record)?;
            self.length += record.len() as u64;
        }
        file.flush()
    }
}
struct Session {
    output: Output,
    state: Arc<StdMutex<Option<File>>>,
    ready: Option<tokio::fs::File>,
    cancel: watch::Sender<bool>,
    closed: watch::Receiver<bool>,
    done: watch::Receiver<Option<Result<(), String>>>,
}
impl Drop for Session {
    fn drop(&mut self) {
        self.cancel.send_replace(true);
    }
}
pub(super) fn initialize_host() -> io::Result<()> {
    let path = std::env::current_exe()?.canonicalize()?;
    let _ = HOST.set(path);
    Ok(())
}
impl Session {
    fn start(mode: Mode, specs: &[&CommandSpec]) -> io::Result<Option<Self>> {
        let Some(host) = HOST.get().filter(|_| mode != Mode::Hidden) else {
            return Ok(None);
        };
        let mut registry = active()
            .lock()
            .map_err(|_| io::Error::other("Console registry was poisoned."))?;
        registry.retain(|_, entry| !entry.task.is_finished());
        let id = NEXT.fetch_add(1, Ordering::Relaxed);
        let title = if mode == Mode::Av1anTail {
            "av1an".to_owned()
        } else {
            format!(
                "Jesses {} · {}-{}",
                specs
                    .first()
                    .and_then(|spec| spec.executable.file_stem())
                    .and_then(|stem| stem.to_str())
                    .unwrap_or("media"),
                std::process::id(),
                id
            )
        };
        let mut directory = crate::jobs::images::Directory::new(&std::env::temp_dir())
            .map_err(|error| io::Error::other(error.message))?;
        let spool = directory.path.join("diagnostics.jsonl");
        let state_path = directory.path.join("finished.json");
        let file = directory
            .create_file(spool.clone())
            .map_err(|error| io::Error::other(error.message))?;
        let state = directory
            .create_file(state_path.clone())
            .map_err(|error| io::Error::other(error.message))?;
        let state = Arc::new(StdMutex::new(Some(state)));
        let writer = Arc::new(StdMutex::new(Writer {
            file: Some(file),
            length: 0,
        }));
        let header = specs
            .iter()
            .map(|spec| {
                format!(
                    "{:?} {}\n",
                    spec.executable,
                    spec.args
                        .iter()
                        .map(|arg| format!("{arg:?}"))
                        .collect::<Vec<_>>()
                        .join(" ")
                )
            })
            .collect::<String>();
        writer
            .lock()
            .map_err(|_| io::Error::other("Console writer was poisoned."))?
            .emit(4, header.as_bytes())?;
        let command = CommandSpec {
            executable: host.clone(),
            args: vec![
                FLAG.into(),
                spool.as_os_str().to_owned(),
                state_path.as_os_str().to_owned(),
                match mode {
                    Mode::StayOpen => "stay",
                    Mode::Av1anTail => "av1an",
                    _ => "visible",
                }
                .into(),
                title.clone().into(),
            ],
            cwd: None,
        };
        let mut helper = platform::OwnedChild::spawn(&command)?;
        // Diagnostic helper has no meaningful redirected output. Its own
        // CONOUT$ is allocated before it reads this spool; it runs no tools.
        let (ready, _stderr) = helper.take_pipes();
        let (cancel, canceled) = watch::channel(false);
        let (closed_owner, closed) = watch::channel(false);
        let (done_owner, done) = watch::channel(None);
        let close_writer = writer.clone();
        let close_state = state.clone();
        let task = tokio::spawn(async move {
            tokio::select! {biased;_=cancelled(canceled)=>{},_=helper.wait_and_terminate_descendants()=>{}}
            let cleanup = helper
                .terminate_and_wait()
                .await
                .map_err(|error| error.to_string());
            drop(helper);
            close_writer
                .lock()
                .unwrap_or_else(|poison| poison.into_inner())
                .file
                .take();
            close_state
                .lock()
                .unwrap_or_else(|poison| poison.into_inner())
                .take();
            drop(directory);
            closed_owner.send_replace(true);
            done_owner.send_replace(Some(cleanup));
        });
        registry.insert(
            id,
            Active {
                cancel: cancel.clone(),
                task,
                title,
                directory: spool.parent().unwrap().to_owned(),
            },
        );
        let output: Output = Arc::new(move |key, bytes| {
            if let Ok(mut writer) = writer.lock() {
                let _ = writer.emit(key, bytes);
            }
        });
        Ok(Some(Self {
            output,
            state,
            ready: Some(ready),
            cancel,
            closed,
            done,
        }))
    }
    fn complete(&mut self, code: i32, message: &str) -> io::Result<()> {
        (self.output)(4, format!("\n{message}\n").as_bytes());
        let mut state = self
            .state
            .lock()
            .map_err(|_| io::Error::other("Console state was poisoned."))?;
        let state = state
            .as_mut()
            .ok_or_else(|| io::Error::new(io::ErrorKind::BrokenPipe, "The console is closed."))?;
        state.write_all(&serde_json::to_vec(&(code, message))?)?;
        state.flush()
    }
    async fn joined(&mut self) -> io::Result<()> {
        loop {
            if let Some(result) = self.done.borrow().clone() {
                return result.map_err(io::Error::other);
            }
            self.done
                .changed()
                .await
                .map_err(|_| io::Error::other("Console cleanup did not report completion."))?;
        }
    }
}
pub(super) async fn scope<T, F, Fut>(
    mode: Mode,
    specs: &[&CommandSpec],
    cancel: watch::Receiver<bool>,
    make: F,
) -> Result<T, SupervisorError>
where
    T: Outcome,
    F: FnOnce(watch::Receiver<bool>) -> Fut,
    Fut: std::future::Future<Output = Result<T, SupervisorError>>,
{
    if *cancel.borrow() {
        return Err(SupervisorError::Cancelled);
    }
    let Some(mut session) = Session::start(mode, specs)? else {
        return super::start(make, cancel).await;
    };
    let mut ready_pipe = session.ready.take().expect("owned console readiness pipe");
    let readiness = tokio::time::timeout(Duration::from_secs(20), async {
        let mut bytes = [0u8; 6];
        ready_pipe.read_exact(&mut bytes).await?;
        if &bytes != b"READY\n" {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "The diagnostic console sent an invalid readiness response.",
            ));
        }
        Ok(())
    });
    let ready = tokio::select! {biased;_=cancelled(cancel.clone())=>Err(SupervisorError::Cancelled),result=readiness=>match result{Ok(result)=>result.map_err(SupervisorError::Io),Err(_)=>Err(SupervisorError::Io(io::Error::new(io::ErrorKind::TimedOut,"The diagnostic console did not become ready.")))}};
    if let Err(error) = ready {
        session.cancel.send_replace(true);
        session.joined().await.map_err(SupervisorError::Cleanup)?;
        return Err(error);
    }
    let (owner, merged) = watch::channel(false);
    let mut closed = session.closed.clone();
    let running = super::start(make, merged);
    tokio::pin!(running);
    let result=OUTPUT.scope(session.output.clone(),async{
        tokio::select!{biased;
            _=cancelled(cancel.clone())=>{owner.send_replace(true);running.await},
            _=async{while !*closed.borrow(){if closed.changed().await.is_err(){break;}}}=>{owner.send_replace(true);let _=running.await;Err(SupervisorError::Cancelled)},
            result=&mut running=>result,
        }
    }).await;
    let completed = match &result {
        Ok(outcome) => Some(outcome.console_exit()),
        Err(
            SupervisorError::Cancelled | SupervisorError::Timeout | SupervisorError::Cleanup(_),
        ) => None,
        Err(error) => Some((1, format!("Supervisor error: {error}"))),
    };
    if let Some((code, message)) = completed {
        if let Err(error) = session.complete(code, &message) {
            session.cancel.send_replace(true);
            session.joined().await.map_err(SupervisorError::Cleanup)?;
            return Err(SupervisorError::Io(error));
        }
        // /K deliberately keeps the job/queue waiting. AV1AN has its separate
        // five-second tail. The actual tool status remains authoritative.
        tokio::select! {biased;_=cancelled(cancel)=>{session.cancel.send_replace(true);session.joined().await.map_err(SupervisorError::Cleanup)?;return Err(SupervisorError::Cancelled);},joined=session.joined()=>{joined.map_err(SupervisorError::Cleanup)?;}}
    } else {
        session.cancel.send_replace(true);
        session.joined().await.map_err(SupervisorError::Cleanup)?;
    }
    result
}
pub(super) async fn shutdown() {
    let entries = active()
        .lock()
        .map(|mut registry| {
            std::mem::take(&mut *registry)
                .into_values()
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();
    for entry in &entries {
        entry.cancel.send_replace(true);
    }
    for entry in entries {
        let _ = entry.task.await;
    }
}
#[cfg(feature = "console-fixture")]
pub(super) fn snapshots() -> Vec<(String, PathBuf)> {
    active()
        .lock()
        .map(|registry| {
            registry
                .values()
                .filter(|entry| !entry.task.is_finished())
                .map(|entry| (entry.title.clone(), entry.directory.clone()))
                .collect()
        })
        .unwrap_or_default()
}

fn write_console(file: &File, text: &str) -> io::Result<()> {
    let text = text.encode_utf16().collect::<Vec<_>>();
    for chunk in text.chunks(8192) {
        let mut offset = 0;
        while offset < chunk.len() {
            let mut written = 0;
            if unsafe {
                WriteConsoleW(
                    file.as_raw_handle(),
                    chunk[offset..].as_ptr().cast(),
                    (chunk.len() - offset) as u32,
                    &mut written,
                    std::ptr::null(),
                )
            } == 0
            {
                return Err(io::Error::last_os_error());
            }
            if written == 0 || written as usize > chunk.len() - offset {
                return Err(io::Error::new(
                    io::ErrorKind::WriteZero,
                    "The console did not accept its diagnostic text.",
                ));
            }
            offset += written as usize;
        }
    }
    Ok(())
}
fn decode_pending(pending: &mut Vec<u8>, file: &File, finished: bool) -> io::Result<()> {
    loop {
        match std::str::from_utf8(pending) {
            Ok(text) => {
                write_console(file, text)?;
                pending.clear();
                break;
            }
            Err(error) => {
                let valid = error.valid_up_to();
                if valid > 0 {
                    write_console(
                        file,
                        std::str::from_utf8(&pending[..valid]).expect("validated UTF-8"),
                    )?;
                    pending.drain(..valid);
                }
                if let Some(length) = error.error_len() {
                    write_console(file, "�")?;
                    pending.drain(..length);
                } else {
                    if finished {
                        write_console(file, &String::from_utf8_lossy(pending))?;
                        pending.clear();
                    }
                    break;
                }
            }
        }
    }
    Ok(())
}
fn helper(spool: &Path, state: &Path, mode: &str, title: &str) -> io::Result<i32> {
    let original_stdout = unsafe { GetStdHandle(STD_OUTPUT_HANDLE) };
    if original_stdout.is_null()
        || original_stdout == windows_sys::Win32::Foundation::INVALID_HANDLE_VALUE
    {
        return Err(io::Error::other(
            "The owned console readiness pipe is missing.",
        ));
    }
    // Duplicate the inherited pipe before AllocConsole can adjust std handles.
    // Readiness never reopens a writable filesystem pathname in the helper.
    let ready = unsafe { BorrowedHandle::borrow_raw(original_stdout) }.try_clone_to_owned()?;
    if unsafe { GetConsoleWindow() }.is_null() && unsafe { AllocConsole() } == 0 {
        return Err(io::Error::last_os_error());
    }
    let title = title.encode_utf16().chain([0]).collect::<Vec<_>>();
    if unsafe { SetConsoleTitleW(title.as_ptr()) } == 0 {
        return Err(io::Error::last_os_error());
    }
    let console = std::fs::OpenOptions::new().write(true).open("CONOUT$")?;
    let mut input = File::open(spool)?;
    File::from(ready).write_all(b"READY\n")?;
    let mut position = 0u64;
    let mut rows = Vec::new();
    let mut decoder: [Vec<u8>; 5] = std::array::from_fn(|_| Vec::new());
    let mut completed = None;
    let mut announced = false;
    loop {
        if input.metadata()?.len() < position {
            position = 0;
            rows.clear();
        }
        input.seek(SeekFrom::Start(position))?;
        let mut buffer = [0u8; 32768];
        let read = input.read(&mut buffer)?;
        position += read as u64;
        rows.extend_from_slice(&buffer[..read]);
        while let Some(end) = rows.iter().position(|byte| *byte == b'\n') {
            let line = rows.drain(..=end).collect::<Vec<_>>();
            if let Ok((key, encoded)) = serde_json::from_slice::<(usize, String)>(&line)
                && key < decoder.len()
                && let Ok(bytes) = STANDARD.decode(encoded)
            {
                decoder[key].extend(bytes);
                decode_pending(&mut decoder[key], &console, false)?;
            }
        }
        if rows.len() > 65536 {
            return Err(io::Error::other(
                "Console diagnostic record exceeded its limit.",
            ));
        }
        if completed.is_none()
            && let Ok(bytes) = std::fs::read(state)
            && let Ok((code, _)) = serde_json::from_slice::<(i32, String)>(&bytes)
        {
            completed = Some((code, std::time::Instant::now()));
        }
        if let Some((code, when)) = completed
            && read == 0
        {
            for pending in &mut decoder {
                decode_pending(pending, &console, true)?;
            }
            if mode == "stay" {
                if !announced {
                    write_console(
                        &console,
                        "\nFinished. Close this console to continue the queue, or Stop in Jesses.\n",
                    )?;
                    announced = true;
                }
            } else if mode != "av1an" || when.elapsed() >= Duration::from_secs(5) {
                return Ok(code);
            }
        }
        std::thread::sleep(Duration::from_millis(25));
    }
}
pub(super) fn helper_main() -> Option<i32> {
    let mut args = std::env::args_os();
    let _ = args.next();
    if args.next().as_deref() != Some(OsStr::new(FLAG)) {
        return None;
    }
    let args = args.collect::<Vec<_>>();
    if args.len() != 4 {
        return Some(2);
    }
    Some(
        helper(
            Path::new(&args[0]),
            Path::new(&args[1]),
            &args[2].to_string_lossy(),
            &args[3].to_string_lossy(),
        )
        .unwrap_or(1),
    )
}
