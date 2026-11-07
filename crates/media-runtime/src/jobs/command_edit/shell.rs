//! The owner outlives the supervised process and every command in its program.
use super::*;
#[cfg(any(windows, test))]
mod windows;

pub(crate) struct PreparedShell {
    pub command: CommandSpec,
    pub environment: supervisor::ChildEnvironment,
    #[cfg(windows)]
    _directory: super::super::rate_control::Stats,
    #[cfg(windows)]
    stages: Vec<usize>,
}

impl PreparedShell {
    pub fn new(
        program: &str,
        cwd: Option<PathBuf>,
        log: &Path,
        environment: Option<&supervisor::ChildEnvironment>,
    ) -> Result<Self, AppError> {
        #[cfg(unix)]
        {
            let _ = log;
            Ok(Self {
                command: shell_command(program, cwd),
                environment: environment.cloned().unwrap_or_default(),
            })
        }
        #[cfg(windows)]
        {
            use std::io::Write;
            let id = format!(
                "command-{}-{}",
                std::process::id(),
                NEXT_ID.fetch_add(1, Ordering::Relaxed)
            );
            let directory = super::super::rate_control::Stats::create(log, &id)?;
            let original = cwd
                .map(Ok)
                .unwrap_or_else(std::env::current_dir)
                .map_err(|cause| files::error("COMMAND_EDIT_INVALID", cause.to_string(), log))?;
            let plan = windows::Plan::new(program, &original)?;
            for (name, body) in plan.scripts {
                let path = directory.path.join(name);
                let mut file = std::fs::OpenOptions::new()
                    .write(true)
                    .create_new(true)
                    .open(&path)
                    .map_err(|cause| {
                        files::error("COMMAND_EDIT_INVALID", cause.to_string(), &path)
                    })?;
                file.write_all(body.as_bytes())
                    .and_then(|()| file.sync_all())
                    .map_err(|cause| {
                        files::error("COMMAND_EDIT_INVALID", cause.to_string(), &path)
                    })?;
            }
            let mut environment = environment.cloned().unwrap_or_default();
            environment.variables.push((
                "JESSES_COMMAND_ROOT",
                Some(directory.path.as_os_str().to_owned()),
            ));
            environment.dynamic_variables.extend(plan.literals);
            let command = CommandSpec {
                executable: std::env::var_os("COMSPEC")
                    .map(PathBuf::from)
                    .unwrap_or_else(|| PathBuf::from(r"C:\Windows\System32\cmd.exe")),
                // An explicit relative path: hardened systems set
                // NoDefaultCurrentDirectoryInExePath, which stops CMD from
                // searching the current directory for a bare script name.
                args: vec![
                    "/D".into(),
                    "/V:OFF".into(),
                    "/C".into(),
                    r".\jesses.stats-main.cmd".into(),
                ],
                cwd: Some(directory.path.clone()),
            };
            Ok(Self {
                command,
                environment,
                _directory: directory,
                stages: plan.stages,
            })
        }
    }
    pub fn verify(&self) -> Result<(), AppError> {
        #[cfg(windows)]
        {
            self._directory.check()?;
            for stage in &self.stages {
                let started = self
                    ._directory
                    .path
                    .join(format!("jesses.stats-stage-{stage}.started"));
                if std::fs::symlink_metadata(&started)
                    .is_err_and(|cause| cause.kind() == std::io::ErrorKind::NotFound)
                {
                    continue;
                }
                let path = self
                    ._directory
                    .path
                    .join(format!("jesses.stats-stage-{stage}.status"));
                let valid = std::fs::symlink_metadata(&path)
                    .is_ok_and(|metadata| metadata.is_file() && metadata.len() <= 32);
                if !valid
                    || std::fs::read_to_string(&path)
                        .ok()
                        .and_then(|value| value.trim().parse::<i32>().ok())
                        .is_none()
                {
                    return Err(files::error(
                        "EDITED_COMMAND_FAILED",
                        "A pipeline stage did not finish with a valid execution receipt.",
                        &path,
                    ));
                }
            }
        }
        Ok(())
    }
}

/// Batch-file quoting follows Microsoft's CRT quoted-pair/backslash rules.
/// Percent signs are doubled for a batch file; delayed expansion is disabled.
#[cfg(any(windows, test))]
pub(super) fn quote_windows(value: &str) -> String {
    let mut result = String::from("\"");
    let mut slashes = 0;
    for character in value.chars() {
        if character == '\\' {
            slashes += 1;
            continue;
        }
        if character == '"' {
            result.push_str(&"\\".repeat(slashes * 2));
            result.push_str("\"\"");
        } else {
            result.push_str(&"\\".repeat(slashes));
            if character == '%' {
                result.push_str("%%");
            } else {
                result.push(character);
            }
        }
        slashes = 0;
    }
    result.push_str(&"\\".repeat(slashes * 2));
    result.push('"');
    result
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn windows_batch_arguments_keep_percent_quotes_and_trailing_backslashes() {
        assert_eq!(
            quote_windows("a & (b) %PATH% ^ !"),
            "\"a & (b) %%PATH%% ^ !\""
        );
        assert_eq!(quote_windows("title=\"A&B\""), "\"title=\"\"A&B\"\"\"");
        assert_eq!(quote_windows("C:\\folder\\"), "\"C:\\folder\\\\\"");
    }
}
