//! Runnable Windows console consumer qualification. No mock process handles.
use std::{
    io::Write,
    path::PathBuf,
    time::{Duration, Instant},
};
const TEXT: &str = "日本語 · café · literal % ! ^ & | ( ) \"quoted\"";
fn child(args: &[std::ffi::OsString]) -> Option<i32> {
    if args.first().and_then(|arg| arg.to_str()) == Some("--fixture-producer") {
        let bytes = (0..=255).cycle().take(2_560_000).collect::<Vec<u8>>();
        std::io::stdout().write_all(&bytes).unwrap();
        eprintln!("binary producer diagnostic {TEXT}");
        return Some(0);
    }
    if args.first().and_then(|arg| arg.to_str()) == Some("--fixture-consumer") {
        let mut output = std::fs::File::create(&args[1]).unwrap();
        std::io::copy(&mut std::io::stdin(), &mut output).unwrap();
        eprintln!("binary consumer diagnostic {TEXT}");
        return Some(0);
    }
    if args.first().and_then(|arg| arg.to_str()) != Some("--fixture-media") {
        return None;
    }
    assert_eq!(args[4].to_str(), Some(TEXT), "literal native argv changed");
    let milliseconds = args[1].to_string_lossy().parse::<u64>().unwrap();
    let code = args[2].to_string_lossy().parse::<i32>().unwrap();
    let marker = PathBuf::from(&args[3]);
    #[cfg(windows)]
    {
        use windows_sys::Win32::System::Console::{GetConsoleMode, GetStdHandle, STD_ERROR_HANDLE};
        let mut mode = 0;
        let terminal = unsafe { GetConsoleMode(GetStdHandle(STD_ERROR_HANDLE), &mut mode) } != 0;
        std::fs::write(
            marker.with_extension("terminal"),
            if terminal { b"yes".as_slice() } else { b"no" },
        )
        .unwrap();
    }
    let started = Instant::now();
    let mut ticks = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&marker)
        .unwrap();
    while started.elapsed() < Duration::from_millis(milliseconds) {
        writeln!(ticks, "tick").unwrap();
        ticks.flush().unwrap();
        println!("{TEXT}");
        eprintln!("diagnostic {TEXT}");
        std::thread::sleep(Duration::from_millis(25));
    }
    std::fs::write(marker.with_extension("finished"), b"tool finished").unwrap();
    Some(code)
}
fn main() {
    if let Some(code) = media_runtime::supervisor::console_helper_main() {
        std::process::exit(code);
    }
    let args = std::env::args_os().skip(1).collect::<Vec<_>>();
    if let Some(code) = child(&args) {
        std::process::exit(code);
    }
    #[cfg(windows)]
    {
        media_runtime::supervisor::initialize_console_host().unwrap();
        let root = args
            .first()
            .map(PathBuf::from)
            .unwrap_or_else(|| PathBuf::from("target/console-qualification"));
        let runtime = tokio::runtime::Builder::new_multi_thread()
            .enable_all()
            .build()
            .unwrap();
        if args.first().is_some_and(|arg| arg == "--fixture-gui-host") {
            runtime.block_on(windows::qualify_gui_host(
                PathBuf::from(&args[1]),
                PathBuf::from(&args[2]),
            ));
        } else {
            runtime.block_on(windows::qualify(root));
        }
    }
    #[cfg(not(windows))]
    {
        let _ = args;
        eprintln!(
            "This fixture must execute natively on Windows; cross-compilation is not a pass."
        );
        std::process::exit(2);
    }
}
#[cfg(windows)]
mod windows {
    use super::*;
    use base64::{Engine, engine::general_purpose::STANDARD};
    use media_core::{ConsoleWindowMode, GeneralPreferences};
    use media_runtime::supervisor::{self, CommandSpec, ConsoleProfile, SupervisorError};
    use std::path::Path;
    use tokio::sync::watch;
    use windows_sys::Win32::UI::WindowsAndMessaging::{FindWindowW, PostMessageW, WM_CLOSE};
    fn settings(mode: ConsoleWindowMode, av1an: bool) {
        supervisor::configure_console_preferences(&GeneralPreferences {
            console_window: mode,
            av1an_console_visible: av1an,
            ..Default::default()
        });
    }
    fn snapshots() -> Vec<(String, PathBuf)> {
        supervisor::console_fixture_presentations()
    }
    async fn wait_for(mut condition: impl FnMut() -> bool) {
        tokio::time::timeout(Duration::from_secs(25), async {
            while !condition() {
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .expect("native console condition");
    }
    fn close(title: &str) {
        let title = title.encode_utf16().chain([0]).collect::<Vec<_>>();
        let window = unsafe { FindWindowW(std::ptr::null(), title.as_ptr()) };
        assert!(!window.is_null(), "native console window absent");
        assert_ne!(unsafe { PostMessageW(window, WM_CLOSE, 0, 0) }, 0);
    }
    async fn visible() -> (String, PathBuf) {
        wait_for(|| !snapshots().is_empty()).await;
        let snapshot = snapshots().into_iter().next().unwrap();
        let title = snapshot.0.encode_utf16().chain([0]).collect::<Vec<_>>();
        wait_for(|| !unsafe { FindWindowW(std::ptr::null(), title.as_ptr()) }.is_null()).await;
        snapshot
    }
    fn diagnostics(directory: &Path) -> Vec<u8> {
        let mut result = Vec::new();
        for line in std::fs::read_to_string(directory.join("diagnostics.jsonl"))
            .unwrap()
            .lines()
        {
            if let Ok((_, data)) = serde_json::from_str::<(u8, String)>(line) {
                result.extend(STANDARD.decode(data).unwrap());
            }
        }
        result
    }
    fn spec(tool: &Path, marker: &Path, milliseconds: u64, code: i32) -> CommandSpec {
        CommandSpec {
            executable: tool.to_owned(),
            args: vec![
                "--fixture-media".into(),
                milliseconds.to_string().into(),
                code.to_string().into(),
                marker.as_os_str().to_owned(),
                TEXT.into(),
            ],
            cwd: None,
        }
    }
    async fn capture(
        spec: CommandSpec,
        cancel: watch::Receiver<bool>,
        profile: ConsoleProfile,
    ) -> Result<supervisor::CapturedOutput, SupervisorError> {
        let result = supervisor::with_console_profile(profile, || {
            supervisor::run_capture(&spec, cancel, 1024 * 1024, Duration::from_secs(30))
        })
        .await;
        if let Err(error) = &result {
            eprintln!("Native fixture {profile:?} capture failed: {error:?}");
        }
        result
    }
    async fn no_orphans() {
        wait_for(|| snapshots().is_empty()).await;
    }
    async fn run_av1an(
        spec: CommandSpec,
        cancel: watch::Receiver<bool>,
        log: PathBuf,
    ) -> Result<supervisor::ProcessResult, SupervisorError> {
        let (events, _receiver) = tokio::sync::mpsc::channel(16);
        supervisor::with_console_profile(ConsoleProfile::Av1an, || {
            supervisor::run(&spec, cancel, events, &log, Duration::from_secs(30))
        })
        .await
    }
    pub async fn qualify_gui_host(root: PathBuf, tool: PathBuf) {
        settings(ConsoleWindowMode::Hidden, true);
        let (_owner, cancel) = watch::channel(false);
        let marker = root.join("gui-terminal.ticks");
        let log = root.join("gui-terminal.log");
        let output = run_av1an(spec(&tool, &marker, 100, 0), cancel, log.clone())
            .await
            .unwrap();
        assert!(output.status.success());
        assert_eq!(
            std::fs::read(marker.with_extension("terminal")).unwrap(),
            b"yes"
        );
        let log = std::fs::read_to_string(log).unwrap();
        assert!(log.contains("PowerShell 7.") && log.contains(TEXT));
        no_orphans().await;
    }

    fn create_gui_host(host: &Path, destination: &Path) {
        let mut pe = std::fs::read(host).unwrap();
        let header = u32::from_le_bytes(pe[0x3c..0x40].try_into().unwrap()) as usize;
        assert_eq!(&pe[header..header + 4], b"PE\0\0");
        // PE optional header Subsystem sits at offset 68 for PE32 and PE32+.
        // This test copy exercises the same detached loader mode as release.
        let subsystem = header + 4 + 20 + 68;
        assert_eq!(pe[subsystem..subsystem + 2], 3u16.to_le_bytes());
        pe[subsystem..subsystem + 2].copy_from_slice(&2u16.to_le_bytes());
        std::fs::write(destination, pe).unwrap();
    }

    pub async fn qualify(root: PathBuf) {
        std::fs::create_dir_all(&root).unwrap();
        let root = root.canonicalize().unwrap();
        let host = std::env::current_exe().unwrap();
        let mut folder = root.join("日本語 long path");
        for _ in 0..7 {
            folder = folder.join("long component with spaces 0123456789");
        }
        std::fs::create_dir_all(&folder).unwrap();
        let ffmpeg = folder.join("ffmpeg.exe");
        let av1an = folder.join("av1an.exe");
        std::fs::copy(&host, &ffmpeg).unwrap();
        std::fs::copy(&host, &av1an).unwrap();
        assert!(ffmpeg.as_os_str().len() > 260);
        let mut proof = Vec::<serde_json::Value>::new();
        settings(ConsoleWindowMode::Hidden, false);
        let (_owner, cancel) = watch::channel(false);
        let output = capture(
            spec(&ffmpeg, &root.join("hidden.ticks"), 100, 0),
            cancel,
            ConsoleProfile::General,
        )
        .await
        .unwrap();
        assert!(output.status.success());
        assert!(snapshots().is_empty());
        assert!(String::from_utf8(output.stdout).unwrap().contains(TEXT));
        proof.push(serde_json::json!({"case":"hidden","passed":true}));
        settings(ConsoleWindowMode::Visible, false);
        for index in 0..3 {
            let (_owner, cancel) = watch::channel(false);
            let task = tokio::spawn(capture(
                spec(
                    &ffmpeg,
                    &root.join(format!("visible-{index}.ticks")),
                    300,
                    0,
                ),
                cancel,
                ConsoleProfile::General,
            ));
            let (_, directory) = visible().await;
            wait_for(|| {
                String::from_utf8_lossy(&diagnostics(&directory))
                    .contains(&format!("diagnostic {TEXT}"))
            })
            .await;
            let output = task.await.unwrap().unwrap();
            assert!(output.status.success());
            assert!(String::from_utf8(output.stderr).unwrap().contains(TEXT));
            no_orphans().await;
            assert!(!directory.exists());
        }
        proof.push(serde_json::json!({"case":"visible Unicode long path literal argv and three sequential jobs","passed":true}));
        settings(ConsoleWindowMode::VisibleStayOpen, false);
        let (_owner, cancel) = watch::channel(false);
        let task = tokio::spawn(capture(
            spec(&ffmpeg, &root.join("stay.ticks"), 100, 7),
            cancel,
            ConsoleProfile::General,
        ));
        let (title, directory) = visible().await;
        wait_for(|| std::fs::metadata(directory.join("finished.json")).is_ok_and(|m| m.len() > 0))
            .await;
        assert!(!task.is_finished(), "/K must keep job/queue waiting");
        let text = diagnostics(&directory);
        assert!(String::from_utf8_lossy(&text).contains("Exit status:"));
        std::fs::write(root.join("failed-console-diagnostics.txt"), text).unwrap();
        close(&title);
        assert_eq!(task.await.unwrap().unwrap().status.code(), Some(7));
        no_orphans().await;
        proof.push(serde_json::json!({"case":"stay-open waits until close and preserves real error7","passed":true}));
        let (owner, cancel) = watch::channel(false);
        let task = tokio::spawn(capture(
            spec(&ffmpeg, &root.join("stop-stay.ticks"), 100, 0),
            cancel,
            ConsoleProfile::General,
        ));
        let (_, directory) = visible().await;
        wait_for(|| std::fs::metadata(directory.join("finished.json")).is_ok_and(|m| m.len() > 0))
            .await;
        owner.send_replace(true);
        assert!(matches!(
            task.await.unwrap(),
            Err(SupervisorError::Cancelled)
        ));
        no_orphans().await;
        proof.push(serde_json::json!({"case":"Stop unblocks /K wait","passed":true}));
        settings(ConsoleWindowMode::Visible, false);
        let (_owner, cancel) = watch::channel(false);
        let marker = root.join("close-active.ticks");
        let task = tokio::spawn(capture(
            spec(&ffmpeg, &marker, 10000, 0),
            cancel,
            ConsoleProfile::General,
        ));
        let (title, _) = visible().await;
        wait_for(|| marker.exists()).await;
        close(&title);
        assert!(matches!(
            task.await.unwrap(),
            Err(SupervisorError::Cancelled)
        ));
        let bytes = std::fs::metadata(&marker).unwrap().len();
        tokio::time::sleep(Duration::from_millis(150)).await;
        assert_eq!(std::fs::metadata(&marker).unwrap().len(), bytes);
        no_orphans().await;
        proof.push(
            serde_json::json!({"case":"closing active console cancels actual tool","passed":true}),
        );
        let (owner, cancel) = watch::channel(false);
        let control = supervisor::PauseControl::default();
        let marker = root.join("pause.ticks");
        let command = spec(&ffmpeg, &marker, 10000, 0);
        let paused = control.clone();
        let task = tokio::spawn(async move {
            supervisor::with_pause(paused, capture(command, cancel, ConsoleProfile::General)).await
        });
        visible().await;
        wait_for(|| std::fs::metadata(&marker).is_ok_and(|m| m.len() > 8)).await;
        control.set_paused(true).unwrap();
        tokio::time::sleep(Duration::from_millis(100)).await;
        let bytes = std::fs::metadata(&marker).unwrap().len();
        tokio::time::sleep(Duration::from_millis(200)).await;
        assert_eq!(std::fs::metadata(&marker).unwrap().len(), bytes);
        control.set_paused(false).unwrap();
        wait_for(|| std::fs::metadata(&marker).unwrap().len() > bytes).await;
        owner.send_replace(true);
        assert!(matches!(
            task.await.unwrap(),
            Err(SupervisorError::Cancelled)
        ));
        no_orphans().await;
        proof.push(serde_json::json!({"case":"actual process pause resume cancel","passed":true}));
        settings(ConsoleWindowMode::Hidden, true);
        let (_owner, cancel) = watch::channel(false);
        let marker = root.join("av1an.ticks");
        let log = root.join("av1an.log");
        let task = tokio::spawn(run_av1an(
            spec(&av1an, &marker, 100, 0),
            cancel,
            log.clone(),
        ));
        let (title, _) = visible().await;
        assert!(title.contains("av1an · PowerShell 7"));
        wait_for(|| marker.with_extension("finished").exists()).await;
        assert_eq!(
            std::fs::read(marker.with_extension("terminal")).unwrap(),
            b"yes",
            "AV1AN needs terminal-backed stderr for individual chunk bars"
        );
        let start = Instant::now();
        let output = task.await.unwrap().unwrap();
        assert!(output.status.success());
        assert!(start.elapsed() >= Duration::from_millis(4900));
        assert!(start.elapsed() < Duration::from_secs(9));
        let log = std::fs::read_to_string(log).unwrap();
        assert!(log.contains("PowerShell 7.") && log.contains(TEXT));
        no_orphans().await;
        proof.push(serde_json::json!({"case":"separate AV1AN visible fixed5s tail","passed":true}));
        settings(ConsoleWindowMode::VisibleStayOpen, false);
        let (_owner, cancel) = watch::channel(false);
        let task = tokio::spawn(run_av1an(
            spec(&av1an, &root.join("av1an-hidden.ticks"), 100, 0),
            cancel,
            root.join("av1an-hidden.log"),
        ));
        assert!(task.await.unwrap().unwrap().status.success());
        assert!(snapshots().is_empty());
        proof.push(
            serde_json::json!({"case":"AV1AN hidden independent of general /K","passed":true}),
        );
        settings(ConsoleWindowMode::Visible, false);
        let (_owner, cancel) = watch::channel(false);
        let task = tokio::spawn(capture(
            spec(&ffmpeg, &root.join("shutdown.ticks"), 10000, 0),
            cancel,
            ConsoleProfile::General,
        ));
        visible().await;
        supervisor::shutdown_console_presentations().await;
        assert!(matches!(
            task.await.unwrap(),
            Err(SupervisorError::Cancelled)
        ));
        assert!(snapshots().is_empty());
        proof.push(serde_json::json!({"case":"shutdown joins helpers and actual tool scope","passed":true}));
        settings(ConsoleWindowMode::VisibleStayOpen, false);
        let transferred = root.join("binary-pipeline.bin");
        let producer = CommandSpec {
            executable: ffmpeg.clone(),
            args: vec!["--fixture-producer".into()],
            cwd: None,
        };
        let consumer = CommandSpec {
            executable: ffmpeg.clone(),
            args: vec![
                "--fixture-consumer".into(),
                transferred.as_os_str().to_owned(),
            ],
            cwd: None,
        };
        let (_owner, cancel) = watch::channel(false);
        let (sender, _receiver) = tokio::sync::mpsc::channel(8);
        let log = root.join("binary-pipeline.log");
        let task = tokio::spawn(async move {
            supervisor::run_pipeline(
                &producer,
                &consumer,
                cancel,
                sender,
                &log,
                Duration::from_secs(15),
            )
            .await
        });
        let (title, directory) = visible().await;
        wait_for(|| std::fs::metadata(directory.join("finished.json")).is_ok_and(|m| m.len() > 0))
            .await;
        let text = diagnostics(&directory);
        assert!(String::from_utf8_lossy(&text).contains("binary producer diagnostic"));
        assert!(!text.windows(4).any(|bytes| bytes == [0, 1, 2, 3]));
        close(&title);
        assert_eq!(task.await.unwrap().unwrap().bytes_transferred, 2_560_000);
        assert_eq!(
            std::fs::read(transferred).unwrap(),
            (0..=255).cycle().take(2_560_000).collect::<Vec<u8>>()
        );
        no_orphans().await;
        proof.push(serde_json::json!({"case":"pipeline keeps all binary bytes out of console text","passed":true}));
        settings(ConsoleWindowMode::Hidden, true);
        let (owner, cancel) = watch::channel(false);
        let marker = root.join("av1an-tail-stop.ticks");
        let task = tokio::spawn(run_av1an(
            spec(&av1an, &marker, 100, 0),
            cancel,
            root.join("av1an-tail-stop.log"),
        ));
        visible().await;
        wait_for(|| marker.with_extension("finished").is_file()).await;
        assert!(!task.is_finished());
        owner.send_replace(true);
        assert!(matches!(
            task.await.unwrap(),
            Err(SupervisorError::Cancelled)
        ));
        no_orphans().await;
        proof.push(
            serde_json::json!({"case":"Stop during AV1AN fixed tail closes helper","passed":true}),
        );
        settings(ConsoleWindowMode::Visible, false);
        let (_owner_a, cancel_a) = watch::channel(false);
        let (_owner_b, cancel_b) = watch::channel(false);
        let a = tokio::spawn(capture(
            spec(&ffmpeg, &root.join("concurrent-a.ticks"), 10000, 0),
            cancel_a,
            ConsoleProfile::General,
        ));
        let b = tokio::spawn(capture(
            spec(&ffmpeg, &root.join("concurrent-b.ticks"), 10000, 0),
            cancel_b,
            ConsoleProfile::General,
        ));
        wait_for(|| snapshots().len() == 2).await;
        supervisor::shutdown_console_presentations().await;
        assert!(matches!(a.await.unwrap(), Err(SupervisorError::Cancelled)));
        assert!(matches!(b.await.unwrap(), Err(SupervisorError::Cancelled)));
        assert!(snapshots().is_empty());
        proof.push(serde_json::json!({"case":"concurrent helpers are all canceled and joined at shutdown","passed":true}));
        let gui_host = root.join("gui-console-fixture.exe");
        create_gui_host(&host, &gui_host);
        let (_owner, cancel) = watch::channel(false);
        let result = capture(
            CommandSpec {
                executable: gui_host,
                args: vec![
                    "--fixture-gui-host".into(),
                    root.as_os_str().to_owned(),
                    av1an.as_os_str().to_owned(),
                ],
                cwd: None,
            },
            cancel,
            ConsoleProfile::Hidden,
        )
        .await
        .unwrap();
        assert!(
            result.status.success(),
            "GUI relay host: {}",
            String::from_utf8_lossy(&result.stderr)
        );
        proof.push(serde_json::json!({"case":"GUI-subsystem host attaches to the PowerShell 7 console and preserves relay pipes","passed":true}));
        std::fs::write(
            root.join("console-qualification.json"),
            serde_json::to_vec_pretty(&proof).unwrap(),
        )
        .unwrap();
        println!(
            "{} native Windows console cases passed; {}",
            proof.len(),
            root.display()
        );
    }
}
