//! Inspect the native backend independently of the desktop UI:
//! `cargo run -p media-runtime --example inspect -- "path/to/media.mkv"`
//! With no argument, print local tool capabilities instead.

use std::{env, io::Write, process::ExitCode};

use media_runtime::{AppError, get_capabilities, probe_media};
use serde::Serialize;

fn print_json(value: &impl Serialize) -> Result<(), AppError> {
    let mut stdout = std::io::stdout().lock();
    serde_json::to_writer_pretty(&mut stdout, value)
        .map_err(|error| AppError::new("OUTPUT_FAILED", error.to_string(), None))?;
    writeln!(stdout).map_err(|error| AppError::new("OUTPUT_FAILED", error.to_string(), None))
}

async fn run() -> Result<(), AppError> {
    let mut arguments = env::args_os().skip(1);
    let path = arguments.next();
    if arguments.next().is_some() {
        return Err(AppError::new(
            "INVALID_ARGUMENTS",
            "Provide one local media path, or no argument to list tool capabilities.",
            None,
        ));
    }
    match path {
        Some(path) => {
            let path = path.into_string().map_err(|_| {
                AppError::new(
                    "INVALID_INPUT",
                    "The file path cannot be represented as Unicode.",
                    None,
                )
            })?;
            print_json(&probe_media(path).await?)
        }
        None => print_json(&get_capabilities().await),
    }
}

#[tokio::main]
async fn main() -> ExitCode {
    match run().await {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            let mut stderr = std::io::stderr().lock();
            let _ = serde_json::to_writer_pretty(&mut stderr, &error);
            let _ = writeln!(stderr);
            ExitCode::FAILURE
        }
    }
}
