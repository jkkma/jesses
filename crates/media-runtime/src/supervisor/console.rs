//! Source console policy. Presentation never owns or interprets media commands.
use super::*;
use media_core::{ConsoleWindowMode, GeneralPreferences};
use std::sync::{Arc, atomic::AtomicU8};
#[cfg(windows)]
mod windows;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ConsoleProfile {
    Hidden,
    General,
    Av1an,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Mode {
    Hidden,
    Visible,
    StayOpen,
}
static POLICY: AtomicU8 = AtomicU8::new(4);
tokio::task_local! {static PROFILE:ConsoleProfile;static OUTPUT:Output;}
type Output = Arc<dyn Fn(u8, &[u8]) + Send + Sync>;

pub fn configure_console_preferences(preferences: &GeneralPreferences) {
    let general = match preferences.console_window {
        ConsoleWindowMode::Hidden => 0,
        ConsoleWindowMode::Visible => 1,
        ConsoleWindowMode::VisibleStayOpen => 2,
    };
    POLICY.store(
        general
            | if preferences.av1an_console_visible {
                4
            } else {
                0
            },
        Ordering::Release,
    );
}
#[inline(never)]
pub fn with_console_profile<F, Fut>(
    profile: ConsoleProfile,
    make: F,
) -> impl std::future::Future<Output = Fut::Output>
where
    F: FnOnce() -> Fut,
    Fut: std::future::Future,
{
    PROFILE.scope(profile, Box::pin(make()))
}
fn mode(policy: u8, profile: ConsoleProfile, windows: bool) -> Mode {
    if !windows {
        return Mode::Hidden;
    }
    match profile {
        ConsoleProfile::Hidden => Mode::Hidden,
        ConsoleProfile::Av1an => Mode::Hidden,
        ConsoleProfile::General => match policy & 3 {
            1 => Mode::Visible,
            2 => Mode::StayOpen,
            _ => Mode::Hidden,
        },
    }
}

#[cfg(windows)]
pub(super) use windows::run_av1an_terminal;

#[cfg(windows)]
pub(super) fn av1an_terminal_enabled(profile: ConsoleProfile) -> bool {
    profile == ConsoleProfile::Av1an && POLICY.load(Ordering::Acquire) & 4 != 0
}
pub(super) fn logged_profile(specs: &[&CommandSpec]) -> ConsoleProfile {
    if let Ok(profile) = PROFILE.try_with(|value| *value) {
        return profile;
    }
    let stems = specs
        .iter()
        .filter_map(|spec| spec.executable.file_stem().and_then(|value| value.to_str()))
        .map(str::to_ascii_lowercase)
        .collect::<Vec<_>>();
    if stems.iter().any(|name| name == "av1an") {
        ConsoleProfile::Av1an
    } else if stems.iter().any(|name| {
        matches!(
            name.as_str(),
            "ffmpeg" | "ffprobe" | "mkvmerge" | "mkvextract" | "mkvinfo" | "grav1synth"
        )
    }) {
        ConsoleProfile::General
    } else {
        ConsoleProfile::Hidden
    }
}
pub(super) fn explicit_profile() -> ConsoleProfile {
    PROFILE
        .try_with(|value| *value)
        .unwrap_or(ConsoleProfile::Hidden)
}
pub(crate) fn source_tool_console_profile(executable: &Path) -> ConsoleProfile {
    match executable
        .file_stem()
        .and_then(|value| value.to_str())
        .map(str::to_ascii_lowercase)
        .as_deref()
    {
        Some("ffmpeg" | "ffprobe" | "mkvmerge" | "mkvextract" | "mkvinfo" | "grav1synth") => {
            ConsoleProfile::General
        }
        _ => ConsoleProfile::Hidden,
    }
}
pub(super) fn emit(stream: u8, bytes: &[u8]) {
    let _ = OUTPUT.try_with(|output| output(stream, bytes));
}

#[cfg_attr(not(windows), allow(dead_code))]
pub(super) trait Outcome {
    fn console_exit(&self) -> (i32, String);
}
#[inline(never)]
fn start<F, Fut>(make: F, cancel: watch::Receiver<bool>) -> std::pin::Pin<Box<Fut>>
where
    F: FnOnce(watch::Receiver<bool>) -> Fut,
    Fut: std::future::Future,
{
    Box::pin(make(cancel))
}
impl Outcome for ProcessResult {
    fn console_exit(&self) -> (i32, String) {
        (
            self.status.code().unwrap_or(1),
            format!("Exit status: {}", self.status),
        )
    }
}
impl Outcome for CapturedOutput {
    fn console_exit(&self) -> (i32, String) {
        (
            self.status.code().unwrap_or(1),
            format!("Exit status: {}", self.status),
        )
    }
}
impl<T> Outcome for StreamedOutput<T> {
    fn console_exit(&self) -> (i32, String) {
        (
            self.status.code().unwrap_or(1),
            format!("Exit status: {}", self.status),
        )
    }
}
impl Outcome for PipelineResult {
    fn console_exit(&self) -> (i32, String) {
        (
            if self.producer_status.success() && self.consumer_status.success() {
                0
            } else {
                1
            },
            format!(
                "Producer exit: {}; consumer exit: {}",
                self.producer_status, self.consumer_status
            ),
        )
    }
}

pub(super) async fn scope<T, F, Fut>(
    profile: ConsoleProfile,
    specs: &[&CommandSpec],
    cancel: watch::Receiver<bool>,
    make: F,
) -> Result<T, SupervisorError>
where
    T: Outcome,
    F: FnOnce(watch::Receiver<bool>) -> Fut,
    Fut: std::future::Future<Output = Result<T, SupervisorError>>,
{
    #[cfg(windows)]
    {
        windows::scope(
            mode(POLICY.load(Ordering::Acquire), profile, true),
            specs,
            cancel,
            make,
        )
        .await
    }
    #[cfg(not(windows))]
    {
        let _ = (mode(POLICY.load(Ordering::Acquire), profile, false), specs);
        start(make, cancel).await
    }
}
/// Called by the desktop/qualification host before starting its UI.
pub fn initialize_console_host() -> io::Result<()> {
    #[cfg(windows)]
    {
        windows::initialize_host()?;
    }
    Ok(())
}
/// A Windows helper invocation exits before Tauri or preference initialization.
pub fn console_helper_main() -> Option<i32> {
    #[cfg(windows)]
    {
        windows::helper_main()
    }
    #[cfg(not(windows))]
    {
        None
    }
}
pub async fn shutdown_console_presentations() {
    #[cfg(windows)]
    {
        windows::shutdown().await;
    }
}
#[cfg(feature = "console-fixture")]
pub fn console_fixture_presentations() -> Vec<(String, PathBuf)> {
    #[cfg(windows)]
    {
        windows::snapshots()
    }
    #[cfg(not(windows))]
    {
        Vec::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn source_policy_keeps_av1an_separate_and_unix_without_terminals() {
        for policy in 0..8 {
            assert_eq!(mode(policy, ConsoleProfile::General, false), Mode::Hidden);
            assert_eq!(mode(policy, ConsoleProfile::Av1an, false), Mode::Hidden);
            assert_eq!(mode(policy, ConsoleProfile::Hidden, true), Mode::Hidden);
        }
        assert_eq!(mode(4, ConsoleProfile::General, true), Mode::Hidden);
        assert_eq!(mode(4, ConsoleProfile::Av1an, true), Mode::Hidden);
        assert_eq!(mode(1, ConsoleProfile::General, true), Mode::Visible);
        assert_eq!(mode(2, ConsoleProfile::General, true), Mode::StayOpen);
        assert_eq!(mode(2, ConsoleProfile::Av1an, true), Mode::Hidden);
    }
    #[test]
    fn source_logged_tool_selection_excludes_ocr_python_and_capability_captures() {
        let spec = |name: &str| CommandSpec {
            executable: PathBuf::from(name),
            args: vec![],
            cwd: None,
        };
        for name in [
            "ffmpeg.exe",
            "ffprobe.exe",
            "mkvmerge.exe",
            "mkvextract.exe",
            "mkvinfo.exe",
            "grav1synth.exe",
        ] {
            assert_eq!(logged_profile(&[&spec(name)]), ConsoleProfile::General);
        }
        assert_eq!(logged_profile(&[&spec("av1an.exe")]), ConsoleProfile::Av1an);
        for name in [
            "python.exe",
            "tesseract.exe",
            "seconv.exe",
            "vspipe.exe",
            "x264.exe",
        ] {
            assert_eq!(logged_profile(&[&spec(name)]), ConsoleProfile::Hidden);
        }
        assert_eq!(
            logged_profile(&[&spec("ffmpeg.exe"), &spec("x264.exe")]),
            ConsoleProfile::General
        );
        assert_eq!(explicit_profile(), ConsoleProfile::Hidden);
    }
}
