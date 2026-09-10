//! Windows 10+ atomic Job Object assignment. No child instruction can run
//! outside the owned job, and the child never inherits the job handle.
use super::CommandSpec;
use std::{
    ffi::OsStr,
    io,
    mem::{size_of, zeroed},
    os::windows::{
        ffi::OsStrExt,
        io::{AsRawHandle, FromRawHandle, OwnedHandle},
        process::ExitStatusExt,
    },
    process::ExitStatus,
    ptr::{null, null_mut},
    time::Duration,
};
use windows_sys::Win32::{
    Foundation::{HANDLE, HANDLE_FLAG_INHERIT, SetHandleInformation, WAIT_OBJECT_0, WAIT_TIMEOUT},
    Security::SECURITY_ATTRIBUTES,
    System::{
        JobObjects::{
            CreateJobObjectW, JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE,
            JOBOBJECT_BASIC_ACCOUNTING_INFORMATION, JOBOBJECT_EXTENDED_LIMIT_INFORMATION,
            JobObjectBasicAccountingInformation, JobObjectExtendedLimitInformation,
            QueryInformationJobObject, SetInformationJobObject, TerminateJobObject,
        },
        Pipes::CreatePipe,
        Threading::{
            CREATE_NO_WINDOW, CreateProcessW, DeleteProcThreadAttributeList,
            EXTENDED_STARTUPINFO_PRESENT, GetExitCodeProcess, InitializeProcThreadAttributeList,
            LPPROC_THREAD_ATTRIBUTE_LIST, PROC_THREAD_ATTRIBUTE_HANDLE_LIST,
            PROC_THREAD_ATTRIBUTE_JOB_LIST, PROCESS_INFORMATION, STARTF_USESTDHANDLES,
            STARTUPINFOEXW, UpdateProcThreadAttribute, WaitForSingleObject,
        },
    },
};

pub(super) struct OwnedChild {
    job: OwnedHandle,
    process: OwnedHandle,
    stdin: Option<tokio::fs::File>,
    stdout: Option<tokio::fs::File>,
    stderr: Option<tokio::fs::File>,
}

impl OwnedChild {
    pub(super) fn spawn(spec: &CommandSpec) -> io::Result<Self> {
        Self::spawn_with_input(spec, false)
    }

    pub(super) fn spawn_with_stdin(spec: &CommandSpec) -> io::Result<Self> {
        Self::spawn_with_input(spec, true)
    }

    fn spawn_with_input(spec: &CommandSpec, pipe_stdin: bool) -> io::Result<Self> {
        if spec
            .executable
            .extension()
            .is_some_and(|ext| ext.eq_ignore_ascii_case("cmd") || ext.eq_ignore_ascii_case("bat"))
        {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "Shell scripts are not supported media executables.",
            ));
        }
        let executable = wide(spec.executable.as_os_str())?;
        let mut command_line = Vec::new();
        quote(spec.executable.as_os_str(), &mut command_line)?;
        for arg in &spec.args {
            command_line.push(b' ' as u16);
            quote(arg, &mut command_line)?;
        }
        command_line.push(0);
        let cwd = spec
            .cwd
            .as_ref()
            .map(|path| wide(path.as_os_str()))
            .transpose()?;
        let (stdout_read, stdout_write) = pipe()?;
        let (stderr_read, stderr_write) = pipe()?;
        let (stdin_read, stdin_write) = pipe()?;
        // The parent writer must never be inherited: an inherited writer would
        // keep stdin open forever even after the producer completes.
        check(unsafe {
            SetHandleInformation(stdin_write.as_raw_handle(), HANDLE_FLAG_INHERIT, 0)
        })?;
        let stdin_write = if pipe_stdin {
            Some(stdin_write)
        } else {
            // Non-pipeline tools receive EOF rather than waiting for UI input.
            drop(stdin_write);
            None
        };
        // pipe() clears inheritance on the read end; stdin is the exception.
        check(unsafe {
            SetHandleInformation(
                stdin_read.as_raw_handle(),
                HANDLE_FLAG_INHERIT,
                HANDLE_FLAG_INHERIT,
            )
        })?;

        // SAFETY: null security attributes create a non-inheritable unnamed job.
        let job = owned(unsafe { CreateJobObjectW(null(), null()) })?;
        let mut limits: JOBOBJECT_EXTENDED_LIMIT_INFORMATION = unsafe { zeroed() };
        limits.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;
        // No breakaway flags: descendants remain in the owned job.
        check(unsafe {
            SetInformationJobObject(
                job.as_raw_handle(),
                JobObjectExtendedLimitInformation,
                (&limits as *const JOBOBJECT_EXTENDED_LIMIT_INFORMATION).cast(),
                size_of::<JOBOBJECT_EXTENDED_LIMIT_INFORMATION>() as u32,
            )
        })?;

        let mut jobs = [job.as_raw_handle()];
        let mut inherited = [
            stdin_read.as_raw_handle(),
            stdout_write.as_raw_handle(),
            stderr_write.as_raw_handle(),
        ];
        let mut attributes = Attributes::new(2)?;
        // SAFETY: both arrays and their handles remain alive through CreateProcessW
        // and until Attributes is destroyed. Only the three stdio handles inherit.
        check(unsafe {
            UpdateProcThreadAttribute(
                attributes.pointer(),
                0,
                PROC_THREAD_ATTRIBUTE_JOB_LIST as usize,
                jobs.as_mut_ptr().cast(),
                size_of_val(&jobs),
                null_mut(),
                null(),
            )
        })?;
        check(unsafe {
            UpdateProcThreadAttribute(
                attributes.pointer(),
                0,
                PROC_THREAD_ATTRIBUTE_HANDLE_LIST as usize,
                inherited.as_mut_ptr().cast(),
                size_of_val(&inherited),
                null_mut(),
                null(),
            )
        })?;

        let mut startup: STARTUPINFOEXW = unsafe { zeroed() };
        startup.StartupInfo.cb = size_of::<STARTUPINFOEXW>() as u32;
        startup.StartupInfo.dwFlags = STARTF_USESTDHANDLES;
        startup.StartupInfo.hStdInput = stdin_read.as_raw_handle();
        startup.StartupInfo.hStdOutput = stdout_write.as_raw_handle();
        startup.StartupInfo.hStdError = stderr_write.as_raw_handle();
        startup.lpAttributeList = attributes.pointer();
        let mut info: PROCESS_INFORMATION = unsafe { zeroed() };
        // JOB_LIST assigns the job before the initial thread can execute, avoiding
        // both the child-spawn race and the suspended-child crash window.
        // https://learn.microsoft.com/windows/win32/api/processthreadsapi/nf-processthreadsapi-updateprocthreadattribute
        check(unsafe {
            CreateProcessW(
                executable.as_ptr(),
                command_line.as_mut_ptr(),
                null(),
                null(),
                1,
                CREATE_NO_WINDOW | EXTENDED_STARTUPINFO_PRESENT,
                null(),
                cwd.as_ref().map_or(null(), |value| value.as_ptr()),
                &startup.StartupInfo,
                &mut info,
            )
        })?;
        // SAFETY: successful CreateProcessW returned owned non-null handles.
        let process = unsafe { OwnedHandle::from_raw_handle(info.hProcess) };
        let _initial_thread = unsafe { OwnedHandle::from_raw_handle(info.hThread) };
        drop(attributes);
        // Closing our write handles allows readers to see EOF after tree exit.
        drop(stdout_write);
        drop(stderr_write);
        drop(stdin_read);
        Ok(Self {
            job,
            process,
            stdin: stdin_write.map(|file| tokio::fs::File::from_std(std::fs::File::from(file))),
            stdout: Some(tokio::fs::File::from_std(std::fs::File::from(stdout_read))),
            stderr: Some(tokio::fs::File::from_std(std::fs::File::from(stderr_read))),
        })
    }

    pub(super) fn take_pipes(&mut self) -> (tokio::fs::File, tokio::fs::File) {
        (
            self.stdout.take().expect("piped stdout"),
            self.stderr.take().expect("piped stderr"),
        )
    }

    pub(super) fn take_stdin(&mut self) -> tokio::fs::File {
        self.stdin.take().expect("piped stdin")
    }

    fn try_wait(&self) -> io::Result<Option<ExitStatus>> {
        // SAFETY: process handle remains owned and valid for both calls.
        match unsafe { WaitForSingleObject(self.process.as_raw_handle(), 0) } {
            WAIT_TIMEOUT => Ok(None),
            WAIT_OBJECT_0 => {
                let mut code = 0;
                check(unsafe { GetExitCodeProcess(self.process.as_raw_handle(), &mut code) })?;
                Ok(Some(ExitStatus::from_raw(code)))
            }
            _ => Err(io::Error::last_os_error()),
        }
    }

    fn terminate_tree(&self) -> io::Result<()> {
        // This also covers grandchildren which have closed their output pipes.
        check(unsafe { TerminateJobObject(self.job.as_raw_handle(), 1) })
    }

    async fn wait_empty(&self) -> io::Result<()> {
        let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
        loop {
            let mut info: JOBOBJECT_BASIC_ACCOUNTING_INFORMATION = unsafe { zeroed() };
            check(unsafe {
                QueryInformationJobObject(
                    self.job.as_raw_handle(),
                    JobObjectBasicAccountingInformation,
                    (&mut info as *mut JOBOBJECT_BASIC_ACCOUNTING_INFORMATION).cast(),
                    size_of::<JOBOBJECT_BASIC_ACCOUNTING_INFORMATION>() as u32,
                    null_mut(),
                )
            })?;
            if info.ActiveProcesses == 0 {
                return Ok(());
            }
            if tokio::time::Instant::now() >= deadline {
                return Err(io::Error::new(
                    io::ErrorKind::TimedOut,
                    "The terminated process tree did not exit promptly.",
                ));
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    }

    pub(super) async fn wait_and_terminate_descendants(&mut self) -> io::Result<ExitStatus> {
        loop {
            if let Some(status) = self.try_wait()? {
                self.terminate_tree()?;
                self.wait_empty().await?;
                return Ok(status);
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    }

    pub(super) async fn terminate_and_wait(&mut self) -> io::Result<()> {
        self.terminate_tree()?;
        self.wait_empty().await
    }
}

impl Drop for OwnedChild {
    fn drop(&mut self) {
        let _ = self.terminate_tree();
    }
}

fn check(success: i32) -> io::Result<()> {
    if success == 0 {
        Err(io::Error::last_os_error())
    } else {
        Ok(())
    }
}

fn owned(handle: HANDLE) -> io::Result<OwnedHandle> {
    if handle.is_null() {
        return Err(io::Error::last_os_error());
    }
    // SAFETY: callers pass a successfully created, uniquely owned kernel handle.
    Ok(unsafe { OwnedHandle::from_raw_handle(handle) })
}

fn pipe() -> io::Result<(OwnedHandle, OwnedHandle)> {
    let security = SECURITY_ATTRIBUTES {
        nLength: size_of::<SECURITY_ATTRIBUTES>() as u32,
        lpSecurityDescriptor: null_mut(),
        bInheritHandle: 1,
    };
    let (mut read, mut write) = (null_mut(), null_mut());
    check(unsafe { CreatePipe(&mut read, &mut write, &security, 0) })?;
    // SAFETY: successful CreatePipe returned two uniquely owned valid handles.
    let read = unsafe { OwnedHandle::from_raw_handle(read) };
    let write = unsafe { OwnedHandle::from_raw_handle(write) };
    check(unsafe { SetHandleInformation(read.as_raw_handle(), HANDLE_FLAG_INHERIT, 0) })?;
    Ok((read, write))
}

fn wide(value: &OsStr) -> io::Result<Vec<u16>> {
    let mut value: Vec<u16> = value.encode_wide().collect();
    if value.contains(&0) {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "An executable argument contains a NUL character.",
        ));
    }
    value.push(0);
    Ok(value)
}

/// Microsoft C runtime argument quoting; shell syntax is never evaluated.
fn quote(value: &OsStr, output: &mut Vec<u16>) -> io::Result<()> {
    let value = wide(value)?;
    output.push(b'"' as u16);
    let mut slashes = 0;
    for &unit in &value[..value.len() - 1] {
        if unit == b'\\' as u16 {
            slashes += 1;
            continue;
        }
        output.extend(std::iter::repeat_n(
            b'\\' as u16,
            if unit == b'"' as u16 {
                slashes * 2 + 1
            } else {
                slashes
            },
        ));
        output.push(unit);
        slashes = 0;
    }
    output.extend(std::iter::repeat_n(b'\\' as u16, slashes * 2));
    output.push(b'"' as u16);
    Ok(())
}

struct Attributes {
    // usize storage supplies pointer alignment; allocation remains stable.
    storage: Vec<usize>,
}

impl Attributes {
    fn new(count: u32) -> io::Result<Self> {
        let mut bytes = 0;
        unsafe {
            InitializeProcThreadAttributeList(null_mut(), count, 0, &mut bytes);
        }
        if bytes == 0 {
            return Err(io::Error::last_os_error());
        }
        let mut storage = vec![0_usize; bytes.div_ceil(size_of::<usize>())];
        check(unsafe {
            InitializeProcThreadAttributeList(storage.as_mut_ptr().cast(), count, 0, &mut bytes)
        })?;
        Ok(Self { storage })
    }
    fn pointer(&mut self) -> LPPROC_THREAD_ATTRIBUTE_LIST {
        self.storage.as_mut_ptr().cast()
    }
}

impl Drop for Attributes {
    fn drop(&mut self) {
        unsafe {
            DeleteProcThreadAttributeList(self.pointer());
        }
    }
}
