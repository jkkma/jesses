//! Relay the encoder's actual Y4M input and attest its aspect ratio to the
//! completed chunk. AV1/VP9 IVF cannot preserve this metadata itself.
use media_core::VideoEncoder;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{
    env,
    ffi::OsString,
    fs::{self, File, OpenOptions},
    io::{self, BufRead, BufReader, Read, Seek, SeekFrom, Write},
    path::{Path, PathBuf},
    process::{Command, ExitStatus, Stdio},
};

pub(super) const CONFIG_ENV: &str = "JESSES_AV1AN_ENCODER_CONFIG";
pub(super) const RECEIPTS_NAME: &str = "aspect-receipts.jsonl";
pub(super) const MAX_RECEIPT_BYTES: usize = 512;
const MAX_HEADER_BYTES: usize = 8192;

#[derive(Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(super) struct Config {
    pub(super) encoder: PathBuf,
    pub(super) chunk_directory: PathBuf,
    pub(super) receipts: PathBuf,
    pub(super) family: VideoEncoder,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(super) struct Observation {
    pub(super) name: String,
    pub(super) width: u32,
    pub(super) height: u32,
    pub(super) sar: (u32, u32),
    pub(super) bytes: u64,
    pub(super) sha256: String,
}

/// Only a staged encoder alias and this invocation's explicit child
/// environment can activate the helper.
pub(crate) fn run_from_environment() -> Option<i32> {
    let executable = env::current_exe().ok()?;
    let family = [
        VideoEncoder::SvtAv1,
        VideoEncoder::AomAv1,
        VideoEncoder::VpxStandalone,
    ]
    .into_iter()
    .find(|&family| is_encoder_alias(&executable, family))?;
    let encoded = env::var_os(CONFIG_ENV)?;
    let result = encoded
        .to_str()
        .ok_or_else(|| invalid("Encoder configuration is not UTF-8."))
        .and_then(|value| serde_json::from_str::<Config>(value).map_err(io::Error::other))
        .and_then(|config| {
            if (family == VideoEncoder::SvtAv1 && !config.family.is_svt())
                || (family != VideoEncoder::SvtAv1 && family != config.family)
            {
                return Err(invalid("Encoder configuration does not match the alias."));
            }
            run(
                &config,
                &executable,
                &env::args_os().skip(1).collect::<Vec<_>>(),
            )
        });
    Some(match result {
        Ok(status) => status.code().unwrap_or(1),
        Err(error) => {
            let _ = writeln!(io::stderr(), "Jesses AV1AN encoder helper: {error}");
            1
        }
    })
}

fn invalid(message: &str) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, message)
}

fn app_error(error: media_core::AppError) -> io::Error {
    io::Error::other(error.message)
}

fn is_encoder_alias(path: &Path, family: VideoEncoder) -> bool {
    let binary = super::encoder::binary(family);
    #[cfg(windows)]
    {
        path.file_name()
            .is_some_and(|name| name.eq_ignore_ascii_case(format!("{binary}.exe")))
    }
    #[cfg(not(windows))]
    {
        path.file_name().is_some_and(|name| name == binary)
    }
}

/// Match the final argv that the pinned Av1an appends. It also calls these
/// binaries to check capabilities, for target-quality probes, and for first
/// passes. Those commands retain their original inherited stdin.
fn chunk_argument(family: VideoEncoder, args: &[OsString]) -> Option<&Path> {
    let output = if family.is_svt() {
        let [input, stdin, .., flag, output] = args else {
            return None;
        };
        if input != "-i" || stdin != "stdin" || flag != "-b" {
            return None;
        }
        output
    } else {
        let [passes, .., flag, output, stdin] = args else {
            return None;
        };
        if (passes != "--passes=1" && passes != "--passes=2") || flag != "-o" || stdin != "-" {
            return None;
        }
        output
    };
    let output = Path::new(output);
    (output
        .extension()
        .is_some_and(|extension| extension == "ivf")
        && output
            .file_stem()
            .and_then(|stem| stem.to_str())
            .is_some_and(|name| name.len() == 5 && name.bytes().all(|byte| byte.is_ascii_digit())))
    .then_some(output)
}

fn same_path(left: &Path, right: &Path) -> bool {
    crate::batch::path_key(left) == crate::batch::path_key(right)
}

fn final_output(config: &Config, args: &[OsString]) -> io::Result<Option<PathBuf>> {
    let Some(path) = chunk_argument(config.family, args) else {
        return Ok(None);
    };
    let output = if path.is_absolute() {
        path.to_owned()
    } else {
        env::current_dir()?.join(path)
    };
    let parent = output
        .parent()
        .ok_or_else(|| invalid("No encoder output directory."))?;
    let parent = match parent.canonicalize() {
        Ok(path) => path,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(error),
    };
    let expected = config.chunk_directory.canonicalize()?;
    if !same_path(&parent, &expected) {
        return Ok(None);
    }
    Ok(Some(expected.join(
        output.file_name().expect("validated chunk filename"),
    )))
}

fn run(config: &Config, executable: &Path, args: &[OsString]) -> io::Result<ExitStatus> {
    let executable = executable.canonicalize()?;
    if !config.encoder.is_absolute()
        || !config.chunk_directory.is_absolute()
        || !config.receipts.is_absolute()
        || !is_encoder_alias(&config.encoder, config.family)
        || config
            .receipts
            .file_name()
            .is_none_or(|name| name != RECEIPTS_NAME)
        || !same_path(
            &config
                .receipts
                .parent()
                .expect("absolute receipt parent")
                .canonicalize()?,
            executable.parent().expect("absolute executable parent"),
        )
    {
        return Err(invalid("Invalid encoder helper paths."));
    }
    let encoder = config.encoder.canonicalize()?;
    if same_path(&encoder, &executable) {
        return Err(invalid("The configured encoder resolves to the helper."));
    }
    let mut command = Command::new(encoder);
    command
        .args(args)
        .env_remove(CONFIG_ENV)
        .stdin(Stdio::inherit())
        .stdout(Stdio::inherit())
        .stderr(Stdio::inherit());
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        use windows_sys::Win32::System::Threading::CREATE_NO_WINDOW;
        command.creation_flags(CREATE_NO_WINDOW);
    }
    let Some(output) = final_output(config, args)? else {
        return command.status();
    };
    let directory =
        super::recovery::launch_directory_guard(&config.chunk_directory).map_err(app_error)?;
    match fs::symlink_metadata(&output) {
        Ok(_) => {
            super::recovery::verify_single_link_file(&output).map_err(app_error)?;
        }
        Err(error) if error.kind() == io::ErrorKind::NotFound => {}
        Err(error) => return Err(error),
    }

    // A large buffer keeps the extra relay inexpensive for high-bit-depth
    // content. Forward the original bytes including all Y4M extensions.
    let mut source = BufReader::with_capacity(128 * 1024, io::stdin().lock());
    let (header, width, height, sar) = read_header(&mut source)?;
    let mut encoder = command.stdin(Stdio::piped()).spawn()?;
    let mut input = encoder.stdin.take().expect("piped encoder stdin");
    let relay = input
        .write_all(&header)
        .and_then(|()| io::copy(&mut source, &mut input));
    drop(input);
    if let Err(error) = &relay
        && error.kind() != io::ErrorKind::BrokenPipe
    {
        let _ = encoder.kill();
    }
    let status = encoder.wait()?;
    if !status.success() {
        return Ok(status);
    }
    relay?;
    super::recovery::verify_launch_directory(&config.chunk_directory, &directory)
        .map_err(app_error)?;

    let output_guard = super::recovery::open_launch_file(&output).map_err(app_error)?;
    let mut hasher = Sha256::new();
    let bytes = io::copy(&mut output_guard.file(), &mut hasher)?;
    super::recovery::verify_launch_file(&output_guard).map_err(app_error)?;
    if bytes == 0 || output_guard.file().metadata()?.len() != bytes {
        return Err(invalid(
            "The encoder did not produce a complete chunk file.",
        ));
    }
    append_receipt(
        &config.receipts,
        &Observation {
            name: output
                .file_stem()
                .and_then(|name| name.to_str())
                .expect("validated chunk name")
                .into(),
            width,
            height,
            sar,
            bytes,
            sha256: format!("{:x}", hasher.finalize()),
        },
    )?;
    Ok(status)
}

type Y4mHeader = (Vec<u8>, u32, u32, (u32, u32));

fn read_header(input: &mut impl BufRead) -> io::Result<Y4mHeader> {
    let mut bytes = Vec::new();
    input
        .take(MAX_HEADER_BYTES as u64)
        .read_until(b'\n', &mut bytes)?;
    let header = std::str::from_utf8(&bytes)
        .ok()
        .and_then(|text| text.strip_suffix('\n'))
        .and_then(|text| text.strip_prefix("YUV4MPEG2 "))
        .ok_or_else(|| invalid("The encoder input has no complete Y4M header."))?;
    let dimension = |key| {
        header
            .split_ascii_whitespace()
            .find_map(|term| term.strip_prefix(key))
            .and_then(|value| value.parse::<u32>().ok())
            .filter(|&value| value > 0)
            .ok_or_else(|| invalid("The encoder input has invalid Y4M dimensions."))
    };
    let width = dimension('W')?;
    let height = dimension('H')?;
    let sample_aspect =
        super::super::source_filters::y4m_sample_aspect(&bytes).map_err(app_error)?;
    let sar =
        super::super::encode_plan::sample_aspect_ratio(Some(sample_aspect)).map_err(app_error)?;
    Ok((bytes, width, height, sar))
}

fn append_receipt(path: &Path, observation: &Observation) -> io::Result<()> {
    let mut record = serde_json::to_vec(observation).map_err(io::Error::other)?;
    record.push(b'\n');
    if record.len() > MAX_RECEIPT_BYTES {
        return Err(invalid("The encoder aspect receipt is too large."));
    }
    let identity = super::recovery::verify_single_link_file(path).map_err(app_error)?;
    let mut options = OpenOptions::new();
    options.read(true).write(true);
    #[cfg(windows)]
    {
        use std::os::windows::fs::OpenOptionsExt;
        use windows_sys::Win32::Storage::FileSystem::{
            FILE_FLAG_OPEN_REPARSE_POINT, FILE_SHARE_READ, FILE_SHARE_WRITE,
        };
        options
            .share_mode(FILE_SHARE_READ | FILE_SHARE_WRITE)
            .custom_flags(FILE_FLAG_OPEN_REPARSE_POINT);
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.custom_flags(libc::O_NOFOLLOW);
    }
    let mut file = options.open(path)?;
    file.lock()?;
    if !file.metadata()?.is_file()
        || super::recovery::file_identity(&file)? != identity
        || super::recovery::verify_single_link_file(path).map_err(app_error)? != identity
    {
        return Err(invalid("The encoder aspect receipt log changed."));
    }
    // A killed writer can leave only its ending record incomplete. Remove
    // that tail under the same lock before appending the next observation.
    repair_tail(&mut file)?;
    file.write_all(&record)?;
    file.sync_data()
}

fn repair_tail(file: &mut File) -> io::Result<()> {
    let length = file.metadata()?.len();
    let start = length.saturating_sub(MAX_RECEIPT_BYTES as u64);
    let mut tail = [0; MAX_RECEIPT_BYTES];
    let tail = &mut tail[..(length - start) as usize];
    file.seek(SeekFrom::Start(start))?;
    file.read_exact(tail)?;
    if !tail.is_empty() && !tail.ends_with(b"\n") {
        let end = match tail.iter().rposition(|&byte| byte == b'\n') {
            Some(end) => start + end as u64 + 1,
            None if start == 0 => 0,
            None => {
                return Err(invalid(
                    "The encoder aspect receipt log has an invalid tail.",
                ));
            }
        };
        file.set_len(end)?;
        file.seek(SeekFrom::Start(end))?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(values: &[&str]) -> Vec<OsString> {
        values.iter().map(OsString::from).collect()
    }

    #[test]
    fn only_final_encoder_output_arguments_are_candidates() {
        let svt = VideoEncoder::SvtAv1;
        let aom = VideoEncoder::AomAv1;
        let vpx = VideoEncoder::VpxStandalone;
        let chunk = "chunks/encode/00000.ivf";
        for (family, values) in [
            (svt, args(&["-i", "stdin", "--crf", "30", "-b", chunk])),
            (aom, args(&["--passes=2", "--pass=2", "-o", chunk, "-"])),
            (vpx, args(&["--passes=1", "-o", chunk, "-"])),
        ] {
            assert_eq!(chunk_argument(family, &values), Some(Path::new(chunk)));
        }
        assert_eq!(chunk_argument(svt, &args(&["--version"])), None);
        assert_eq!(
            chunk_argument(svt, &args(&["-i", "stdin", "-b", "NUL"])),
            None
        );
        assert_eq!(
            chunk_argument(
                aom,
                &args(&["--passes=2", "--pass=1", "-o", "/dev/null", "-"])
            ),
            None
        );
        assert_eq!(
            chunk_argument(vpx, &args(&["--passes=1", "-o", "00000-43.ivf", "-"])),
            None
        );
        assert_eq!(
            chunk_argument(svt, &args(&["-i", "file.y4m", "-b", chunk])),
            None
        );
    }

    #[test]
    fn y4m_header_reads_geometry_without_consuming_the_rest_of_the_pipe() {
        let header = b"YUV4MPEG2 W1920 H1080 F24000:1001 Ip A20:10 C420p10 XCOLORRANGE=LIMITED\n";
        let frame = b"FRAME\n\x80\xff\0\x20";
        let payload = [header.as_slice(), frame.as_slice()].concat();
        let mut input = BufReader::with_capacity(11, payload.as_slice());
        assert_eq!(
            read_header(&mut input).unwrap(),
            (header.to_vec(), 1920, 1080, (2, 1))
        );
        let mut remaining = Vec::new();
        input.read_to_end(&mut remaining).unwrap();
        assert_eq!(remaining, frame);

        for value in ["A0:0", "A1:1", "A0:1", ""] {
            let line = format!("YUV4MPEG2 W64 H48 {value}\n");
            assert_eq!(read_header(&mut line.as_bytes()).unwrap().3, (1, 1));
        }
        for line in [
            "YUV4MPEG2 W0 H48 A2:1\n",
            "YUV4MPEG2 W64 H48 A2:0\n",
            "YUV4MPEG2 W64 H48",
        ] {
            assert!(read_header(&mut line.as_bytes()).is_err());
        }
        assert!(read_header(&mut vec![b'x'; MAX_HEADER_BYTES + 1].as_slice()).is_err());
    }

    #[test]
    fn new_receipt_replaces_only_a_torn_tail() {
        let directory = env::temp_dir().join(format!(
            "jesses-av1an-encoder-helper-{}",
            std::process::id()
        ));
        fs::create_dir(&directory).unwrap();
        let path = directory.join(RECEIPTS_NAME);
        let observation = Observation {
            name: "00003".into(),
            width: 64,
            height: 48,
            sar: (2, 1),
            bytes: 10,
            sha256: "0".repeat(64),
        };
        fs::write(&path, b"complete\n{\"name\":").unwrap();
        let result = append_receipt(&path, &observation).and_then(|()| fs::read_to_string(&path));
        fs::remove_file(path).unwrap();
        fs::remove_dir(directory).unwrap();
        let text = result.unwrap();
        let mut lines = text.lines();
        assert_eq!(lines.next(), Some("complete"));
        assert_eq!(
            serde_json::from_str::<Observation>(lines.next().unwrap()).unwrap(),
            observation
        );
        assert_eq!(lines.next(), None);
    }
}
