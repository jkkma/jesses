//! Preserve CMD argument bytes across the extra parsing performed by pipes.
//! Original command text lives in stage files, not on a pipe-side /C string.
use super::*;

pub(super) struct Plan {
    pub scripts: Vec<(String, String)>,
    pub literals: Vec<(String, Option<OsString>)>,
    pub stages: Vec<usize>,
}

fn invalid(message: &str) -> AppError {
    AppError::new("COMMAND_EDIT_INVALID", message, None)
}
fn prefix() -> &'static str {
    "@echo off\r\nsetlocal EnableExtensions DisableDelayedExpansion\r\n"
}
fn invoke(name: &str) -> String {
    format!("cmd.exe /D /V:ON /S /C \"\"!JESSES_COMMAND_ROOT!\\{name}\"\"")
}
fn escaped_end(text: &str, at: usize) -> usize {
    if text.as_bytes().get(at + 1..at + 3) == Some(b"\r\n") {
        at + 3
    } else {
        at + 1 + text[at + 1..].chars().next().map_or(0, char::len_utf8)
    }
}

fn boundaries(text: &str) -> Result<Vec<(usize, usize, bool)>, AppError> {
    let bytes = text.as_bytes();
    let mut at = 0;
    let mut quote = false;
    let mut depth = 0_u32;
    let mut result = Vec::new();
    while at < bytes.len() {
        match bytes[at] {
            b'^' if !quote => {
                at = escaped_end(text, at);
                continue;
            }
            b'"' => quote = !quote,
            b'(' if !quote => depth += 1,
            b')' if !quote => {
                depth = depth.checked_sub(1).ok_or_else(|| {
                    invalid("The command contains an unmatched closing parenthesis.")
                })?
            }
            b'|' if !quote && depth == 0 => {
                let double = bytes.get(at + 1) == Some(&b'|');
                result.push((at, if double { 2 } else { 1 }, !double));
                if double {
                    at += 1;
                }
            }
            b'&' if !quote
                && depth == 0
                && at
                    .checked_sub(1)
                    .is_none_or(|previous| bytes[previous] != b'>') =>
            {
                let double = bytes.get(at + 1) == Some(&b'&');
                result.push((at, if double { 2 } else { 1 }, false));
                if double {
                    at += 1;
                }
            }
            b'\n' if !quote && depth == 0 => result.push((at, 1, false)),
            _ => {}
        }
        at += 1;
    }
    if quote || depth != 0 {
        return Err(invalid(
            "Close the quoted argument or parenthesized group before running.",
        ));
    }
    Ok(result)
}

fn normalize_lines(text: &str) -> String {
    let bytes = text.as_bytes();
    let mut result = String::new();
    let mut at = 0;
    let mut quote = false;
    while at < bytes.len() {
        if bytes[at] == b'^' && !quote {
            let end = escaped_end(text, at);
            result.push_str(&text[at..end]);
            at = end;
            continue;
        }
        if bytes[at] == b'"' {
            quote = !quote;
        }
        if !quote && (bytes[at] == b'|' || (bytes[at] == b'&' && bytes.get(at + 1) == Some(&b'&')))
        {
            let length = if bytes.get(at + 1) == Some(&bytes[at]) {
                2
            } else {
                1
            };
            result.push_str(&text[at..at + length]);
            at += length;
            let start = at;
            while at < bytes.len() && bytes[at].is_ascii_whitespace() {
                at += 1;
            }
            if text[start..at].contains('\n') {
                result.push(' ');
            } else {
                result.push_str(&text[start..at]);
            }
            continue;
        }
        let character = text[at..].chars().next().unwrap();
        result.push(character);
        at += character.len_utf8();
    }
    result
}

impl Plan {
    pub fn new(program: &str, cwd: &Path) -> Result<Self, AppError> {
        let mut plan = Self {
            scripts: Vec::new(),
            literals: Vec::new(),
            stages: Vec::new(),
        };
        let rewritten = plan.rewrite(&normalize_lines(program), 0)?;
        plan.scripts.push((
            "jesses.stats-main.cmd".into(),
            format!(
                "{}cd /d {}\r\n{rewritten}\r\n",
                prefix(),
                quote_windows(&cwd.to_string_lossy())
            ),
        ));
        let mut values = std::collections::HashMap::new();
        for (_, script) in &mut plan.scripts {
            let mut ascii = String::new();
            let mut run = String::new();
            let flush = |run: &mut String,
                         ascii: &mut String,
                         values: &mut std::collections::HashMap<String, String>,
                         literals: &mut Vec<(String, Option<OsString>)>| {
                if run.is_empty() {
                    return;
                }
                let next = values.len();
                let name = values
                    .entry(run.clone())
                    .or_insert_with(|| {
                        let name = format!("JESSES_COMMAND_LITERAL_{next}");
                        literals.push((name.clone(), Some(OsString::from(run.as_str()))));
                        name
                    })
                    .clone();
                ascii.push('%');
                ascii.push_str(&name);
                ascii.push('%');
                run.clear();
            };
            for character in script.chars() {
                if character.is_ascii() {
                    flush(&mut run, &mut ascii, &mut values, &mut plan.literals);
                    ascii.push(character);
                } else {
                    run.push(character);
                }
            }
            flush(&mut run, &mut ascii, &mut values, &mut plan.literals);
            *script = ascii;
        }
        Ok(plan)
    }

    fn rewrite(&mut self, text: &str, nesting: u8) -> Result<String, AppError> {
        if nesting >= 64 {
            return Err(invalid("The command contains more than 64 nested groups."));
        }
        let bytes = text.as_bytes();
        let mut flat = String::new();
        let mut at = 0;
        let mut quote = false;
        while at < bytes.len() {
            if bytes[at] == b'^' && !quote {
                let end = escaped_end(text, at);
                flat.push_str(&text[at..end]);
                at = end;
                continue;
            }
            if bytes[at] == b'"' {
                quote = !quote;
            }
            if bytes[at] == b'(' && !quote {
                let start = at + 1;
                let mut end = start;
                let mut depth = 1;
                let mut nested_quote = false;
                while end < bytes.len() && depth != 0 {
                    if bytes[end] == b'^' && !nested_quote {
                        end = escaped_end(text, end);
                        continue;
                    }
                    if bytes[end] == b'"' {
                        nested_quote = !nested_quote;
                    }
                    if !nested_quote {
                        if bytes[end] == b'(' {
                            depth += 1;
                        } else if bytes[end] == b')' {
                            depth -= 1;
                        }
                    }
                    if depth != 0 {
                        end += 1;
                    }
                }
                if depth != 0 {
                    return Err(invalid(
                        "Close the parenthesized command group before running.",
                    ));
                }
                flat.push('(');
                flat.push_str(&self.rewrite(&text[start..end], nesting + 1)?);
                flat.push(')');
                at = end + 1;
                continue;
            }
            let character = text[at..].chars().next().unwrap();
            flat.push(character);
            at += character.len_utf8();
        }
        let separators = boundaries(&flat)?;
        let mut result = String::new();
        let mut start = 0;
        for (at, length, pipeline) in separators
            .iter()
            .copied()
            .filter(|(_, _, pipeline)| !pipeline)
        {
            let _ = pipeline;
            result.push_str(&self.pipeline(&flat[start..at])?);
            result.push_str(&flat[at..at + length]);
            start = at + length;
        }
        result.push_str(&self.pipeline(&flat[start..])?);
        Ok(result)
    }

    fn pipeline(&mut self, text: &str) -> Result<String, AppError> {
        let pipes: Vec<_> = boundaries(text)?
            .into_iter()
            .filter(|(_, _, pipeline)| *pipeline)
            .collect();
        if pipes.is_empty() {
            return Ok(text.to_owned());
        }
        let mut stages = Vec::new();
        let mut stage_ids = Vec::new();
        let mut start = 0;
        for (at, _, _) in pipes
            .iter()
            .copied()
            .chain(std::iter::once((text.len(), 0, true)))
        {
            let source = text[start..at].trim();
            if source.is_empty() {
                return Err(invalid("Each pipeline stage must contain a command."));
            }
            let id = self.stages.len();
            self.stages.push(id);
            stage_ids.push(id);
            let inner = format!("jesses.stats-source-{id}.cmd");
            let wrapper = format!("jesses.stats-stage-{id}.cmd");
            self.scripts
                .push((inner.clone(), format!("{}{source}\r\n", prefix())));
            self.scripts.push((wrapper.clone(), format!("{}>\"%JESSES_COMMAND_ROOT%\\jesses.stats-stage-{id}.started\" echo started\r\n{}\r\nset \"JESSES_COMMAND_STATUS=%errorlevel%\"\r\n>\"%JESSES_COMMAND_ROOT%\\jesses.stats-stage-{id}.status\" echo %JESSES_COMMAND_STATUS%\r\nexit /b %JESSES_COMMAND_STATUS%\r\n", prefix(), invoke(&inner))));
            stages.push(invoke(&wrapper));
            start = at + 1;
        }
        // Put aggregate status into the expression itself so && and || see
        // upstream failures immediately, including explicitly handled errors.
        let checker = format!("jesses.stats-check-{}.cmd", stage_ids[0]);
        let mut checks = format!("{}set \"JESSES_PIPE_FAILURE=0\"\r\n", prefix());
        for stage in stage_ids {
            checks.push_str(&format!("if not exist \"%JESSES_COMMAND_ROOT%\\jesses.stats-stage-{stage}.status\" exit /b 1\r\nset \"JESSES_PIPE_STATUS=\"\r\nset /p JESSES_PIPE_STATUS=<\"%JESSES_COMMAND_ROOT%\\jesses.stats-stage-{stage}.status\"\r\nif not \"%JESSES_PIPE_STATUS%\"==\"0\" set \"JESSES_PIPE_FAILURE=%JESSES_PIPE_STATUS%\"\r\n"));
        }
        checks.push_str("exit /b %JESSES_PIPE_FAILURE%\r\n");
        self.scripts.push((checker.clone(), checks));
        Ok(format!("({}\r\n{})", stages.join(" | "), invoke(&checker)))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn quoting_grouping_redirects_and_multiline_keep_literal_operators() {
        let plan = Plan::new("(cd \"C:\\literal && (folder)\" && producer.exe \"a|b\" \"x&y\") 2>&1 | consumer.exe > \"out & file\" &&\n echo \"&|\"", Path::new("C:\\日本語")).unwrap();
        assert_eq!(plan.stages.len(), 2);
        assert!(
            plan.scripts
                .iter()
                .any(|(_, body)| body.contains("\"a|b\" \"x&y\""))
        );
        assert!(plan.scripts.iter().any(|(_, body)| body.contains("2>&1")));
        assert!(
            plan.scripts
                .iter()
                .any(|(_, body)| body.contains("consumer.exe > \"out & file\""))
        );
        assert!(plan.scripts.iter().all(|(_, body)| body.is_ascii()));
        assert_eq!(plan.literals.len(), 1);
        let nested = Plan::new(
            "((a.exe ^| literal | b.exe) && c.exe) | d.exe && echo done",
            Path::new("C:\\x"),
        )
        .unwrap();
        assert_eq!(nested.stages.len(), 4);
        assert!(
            nested
                .scripts
                .iter()
                .any(|(_, body)| body.contains("a.exe ^| literal"))
        );
        let handled = Plan::new(
            "(producer.exe | consumer.exe) || echo recovered",
            Path::new("C:\\x"),
        )
        .unwrap();
        assert!(
            handled
                .scripts
                .iter()
                .any(|(name, body)| name.starts_with("jesses.stats-check-")
                    && body.contains("exit /b %JESSES_PIPE_FAILURE%"))
        );
        assert!(
            handled
                .scripts
                .iter()
                .any(|(name, body)| name == "jesses.stats-main.cmd"
                    && body.contains("|| echo recovered"))
        );
        assert!(Plan::new("producer.exe |", Path::new("C:\\x")).is_err());
    }

    #[cfg(windows)]
    #[tokio::test]
    async fn native_cmd_preserves_argv_binary_pipes_and_checks_every_pipeline_stage() {
        let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../../target/test-artifacts/source-command-edit")
            .join(format!(
                "windows-{}-{}-日本語-%%PATH%%-!",
                std::process::id(),
                NEXT_ID.fetch_add(1, Ordering::Relaxed)
            ));
        std::fs::create_dir_all(&root).unwrap();
        let source = root.join("argv.rs");
        let executable = root.join("argv helper.exe");
        std::fs::write(&source, r#"use std::io::{Read,Write};fn main(){let mut args=std::env::args().skip(1);let mode=args.next().unwrap();let path=std::path::PathBuf::from(args.next().unwrap());let mut text=String::new();for arg in args{for unit in arg.encode_utf16(){use std::fmt::Write;write!(&mut text,"{unit:04x}").unwrap();}text.push('\n');}std::fs::write(&path,text).unwrap();if mode=="producer"||mode=="fail"{std::io::stdout().write_all(b"raw\0binary\xff\n").unwrap();if mode=="fail"{std::process::exit(7);}}else{let mut data=Vec::new();std::io::stdin().read_to_end(&mut data).unwrap();std::fs::write(path.with_extension("pipe"),data).unwrap();}}"#).unwrap();
        let built = std::process::Command::new("rustc")
            .args(["--edition=2024", "--crate-name", "jesses_command_argv"])
            .arg(&source)
            .arg("-o")
            .arg(&executable)
            .output()
            .unwrap();
        assert!(
            built.status.success(),
            "{}",
            String::from_utf8_lossy(&built.stderr)
        );
        let expected = [
            "",
            "space path",
            "A&B",
            "(parentheses)",
            "100%",
            "%PATH%",
            "caret^",
            "bang!",
            "title=\"quoted & | value\"",
            "C:\\folder\\",
            "日本語",
            "back\\\"quote&!%",
        ];
        let encoded: String = expected
            .iter()
            .map(|value| {
                let mut text = value
                    .encode_utf16()
                    .map(|unit| format!("{unit:04x}"))
                    .collect::<String>();
                text.push('\n');
                text
            })
            .collect();
        let cwd = root.join("working & (%PATH%) ^! 日本語");
        std::fs::create_dir(&cwd).unwrap();
        for (fail, handled) in [(false, false), (true, false), (true, true)] {
            let left = root.join(format!("left-{fail}-{handled}.args"));
            let right = root.join(format!("right-{fail}-{handled}.args"));
            let command = |mode: &str, output: &Path| CommandSpec {
                executable: executable.clone(),
                args: std::iter::once(OsString::from(mode))
                    .chain(std::iter::once(output.as_os_str().to_owned()))
                    .chain(expected.iter().map(OsString::from))
                    .collect(),
                cwd: Some(cwd.clone()),
            };
            let producer = format_command(&command(if fail { "fail" } else { "producer" }, &left));
            let consumer = format_command(&command("consumer", &right));
            let mut text = format!(
                "({producer} | {consumer}) &&\r\n echo \"quoted & | literals\" > {}",
                quote_windows(
                    &root
                        .join(format!("redirect-{fail}-{handled}.txt"))
                        .to_string_lossy()
                )
            );
            let recovered = root.join("recovered.txt");
            if handled {
                text.push_str(&format!(
                    " || echo recovered > {}",
                    quote_windows(&recovered.to_string_lossy())
                ));
            }
            let prepared =
                PreparedShell::new(&text, Some(root.clone()), &root.join("test.log"), None)
                    .unwrap();
            // Hardened systems stop CMD from running bare names out of the
            // current directory; every script must still be found.
            let mut environment = prepared.environment.clone();
            environment
                .variables
                .push(("NoDefaultCurrentDirectoryInExePath", Some("1".into())));
            let (_owner, cancel) = watch::channel(false);
            let result = supervisor::run_capture_with_environment(
                &prepared.command,
                cancel,
                1024 * 1024,
                Duration::from_secs(30),
                Some(&environment),
            )
            .await
            .unwrap();
            assert_eq!(
                result.status.success(),
                !fail || handled,
                "{}",
                String::from_utf8_lossy(&result.stderr)
            );
            assert_eq!(std::fs::read_to_string(&left).unwrap(), encoded);
            assert_eq!(std::fs::read_to_string(&right).unwrap(), encoded);
            assert_eq!(
                std::fs::read(right.with_extension("pipe")).unwrap(),
                b"raw\0binary\xff\n"
            );
            prepared.verify().unwrap();
            assert_eq!(recovered.exists(), handled);
        }
        // Leave failures intact for a native runner's collected artifact log.
        std::fs::remove_dir_all(&root).unwrap();
    }
}
