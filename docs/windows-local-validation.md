# Windows local validation handoff

**Current status:** Work resumed on the user's instruction. Start with the
[continuation ledger](windows-continuation-20261102.md) and
[session handoff](windows-session-handoff-20261102.md). The original handoff and
instructions below preserve their historical context; they are not current live
process or qualification claims.

Use the final pushed Jesses revision and record `git rev-parse HEAD` with the
receipts. The native Windows console implementation and runnable fixture are
present. **Native Windows execution has not been performed in the Linux
session.** A successful MSVC target check is compilation evidence only.

The source comparison used the Nmkoder fork revision `a7e9731`. Inspect the
Windows checkout for local changes before updating it, then verify that its
Jesses revision is the final pushed revision reported in the handoff. Retain
logs and receipts under a fresh, versioned directory. Files under `target/` and
the downloaded media are local artifacts; they are not carried by Git.

The product scope is **CPU video encoding only**, including Quick Convert and
AV1AN. Hardware video encoders must not appear as available choices. Vulkan
and libplacebo Spline are required processing capabilities and remain in scope.

## Keep qualification builds within the disk budget

Check the Windows partition's free space before building (`Get-PSDrive -PSProvider
FileSystem`). A full Linux debug-symbol build exhausted this laptop's disk; its
Linux and MSVC debug trees were deliberately cleared after preserving providers,
source caches, media and receipts. They are generated artifacts and are expected
to be absent. The final bounded Linux checks are summarized in
[final Linux validation](final-linux-validation.md); the final UI suite is279/279,
including the exact800×750 conditional form/footer regression. Source AV1AN
New/Saved and typed-FPS behavior is closed locally; Windows still needs its own
native receipts.

For qualification where debug symbols are unnecessary, use a dedicated target
and bounded compilation in PowerShell7:

```powershell
$env:CARGO_TARGET_DIR = Join-Path $PWD 'target\windows-small-qualification'
$env:CARGO_INCREMENTAL = '0'
$env:CARGO_BUILD_JOBS = '2'
$env:CARGO_PROFILE_DEV_DEBUG = '0'
$env:CARGO_PROFILE_TEST_DEBUG = '0'
Remove-Item Env:RUST_MIN_STACK -ErrorAction SilentlyContinue
Get-PSDrive -PSProvider FileSystem
```

Choose and actively monitor a small target-size/free-space budget before each
check; stop its owned process tree rather than fill the partition. The Linux
finishing pass used a2GiB target budget and45GiB free-space floor, peaked at0.782GiB
and removed the target afterward. Preserve receipts outside the build directory,
keep tool/provider/source-cache directories intact, and delete only the exact
generated target once all of its processes are stopped. Do not start another
unrestricted full debug-symbol build by default. `--offline` is suitable only
when the required dependencies are already cached.

## Run the independent console qualification first

Requirements: Windows 10/11, PowerShell 7 (`pwsh`), Git, Rustup, and MSVC C++
Build Tools with a Windows SDK/linker. The repository pins Rust 1.98.1. This
fixture needs no FFmpeg, AV1AN, VapourSynth, media-provider package, Node or Tauri
build: its media-process stand-ins are copies of the fixture executable, and its
console helper is the same native helper mode used by the desktop app.

From the checked-out repository in PowerShell:

```powershell
rustup toolchain install 1.98.1 --profile minimal --component rustfmt --component clippy
rustup target add x86_64-pc-windows-msvc --toolchain 1.98.1
pwsh -NoProfile -File .\scripts\qualify-windows-console.ps1
```

The script creates a fresh directory under
`target\windows-console-qualification\<UTC timestamp>-<unique id>`. Expect
`console-qualification.json` with **12 entries, all `passed: true`**, plus
`native-console-run.log`, `failed-console-diagnostics.txt` and the binary
pipeline log. The JSON is emitted only after every assertion succeeds. A failed
run retains its directory and command log for the Windows Codex session to fix
and rerun; cross-compilation does not produce this success receipt.

The equivalent direct build/run commands are:

```powershell
cargo build -p media-runtime --locked --target x86_64-pc-windows-msvc --features console-fixture --example windows_console_fixture
cargo run -p media-runtime --locked --target x86_64-pc-windows-msvc --features console-fixture --example windows_console_fixture -- target\manual-console-receipt
```

Use a new manual receipt directory. The fixture creates real console windows;
its automated cases close/cancel them. It covers:

1. Hidden mode without helpers, retaining captured tool output.
2. Visible mode, Unicode/literal arguments and executable paths longer than
   260 characters, across three sequential jobs without leftover helpers.
3. Visible/stays-open waiting until the console closes and preserving actual
   tool exit code 7 and its diagnostic output.
4. Stop canceling the finished `/K` wait.
5. Closing an active console canceling its corresponding running tool.
6. Actual process pause/resume/cancel with heartbeat evidence.
7. Separate AV1AN visibility and its automatic five-second tail.
8. AV1AN hidden independently of the general stays-open setting.
9. App-style shutdown joining the helper and tool scope.
10. Exact 2,560,000-byte binary pipeline transfer, absent from console text.
11. Stop during the separate AV1AN tail.
12. Concurrent helpers canceled/joined at shutdown.

## Desktop consumer and source policy

The production desktop entry handles the internal helper mode before starting
Tauri. The existing preferences initialize/update native presentation policy;
there is no additional setting. Actual media tools retain native argv,
JobObjects, captured output, pause/cancel, exit status and output validation.
The owned helper displays diagnostics from a bounded 4 MiB spool. Its readiness
uses an inherited pipe; it does not reopen a writable readiness pathname.
Unicode is decoded incrementally per diagnostic stream and written through
`WriteConsoleW`. Binary producer-to-consumer bytes bypass the text relay.
Helper/process completion is joined and writable spool handles are closed
before owned-directory cleanup.

The general source policy comes from `AvProcess.RunFfmpeg`, `RunFfprobe`, MKV
extract/merge/info and `Grav1synth.Run`. Actual direct-encoder pipelines share a
general presentation. Metadata/validation/help/accounting captures remain hidden
unless they correspond to an explicit source-visible probe. Bitrate Chart sends
an internal chart-purpose marker; Read Bitrates accounting remains hidden.
OCR retains the source's hidden behavior. Edited commands use their original
route's profile without scanning user shell text. AV1AN's actual execution,
including edited programs, uses its separate Windows preference and the
`av1an` title; it does not inherit general `/K`.

General Visible closes after actual tool completion. Visible/stays-open keeps
the current job and queue waiting until that window closes, matching the fork's
`/K` behavior; Stop/shutdown cancel and unblock it. Closing a console while its
tool is active cancels that tool tree. AV1AN has its separate five-second tail.
The real tool's exit status is retained independently of the diagnostic helper.
Unix launches no terminal emulator and adds no tail/stays-open delay.

For a desktop settings check, use the repository's existing Node 24/pnpm
11.19.0/Tauri setup:

```powershell
npm install --global pnpm@11.19.0
pnpm install --frozen-lockfile
pnpm desktop
```

Exercise the existing Settings choices with a short CPU encode/utility and a
separate AV1AN run. Real media work needs the Windows media providers described
in [packaging](packaging.md); they are independent of the console fixture. Do
not substitute a GPU video encoder. Vulkan/libplacebo Spline tone mapping and
GPU processing/metrics remain part of the requested product.

## Evidence and remaining native-only work

Local Linux receipts are under `target/test-artifacts/native-backlog/`:
`console-policy-tests.log`, `console-supervisor-regressions.log`,
`console-linux-check.log`, `console-desktop-check.log`,
`console-windows-check.log`, `console-windows-fixture-check.log` and
`console-svelte-check.log`. Linux supervisor regressions include binary pipes,
large streaming output, process-tree drop/cancel, pause and timeout behavior.
The combined source-image/QTGMC default-stack gate is recorded by its owning
phase, separately from console compilation.

The Windows-local session must run the twelve-case fixture and the actual
Windows desktop/provider workflows. Native window behavior, process freezing,
console close events, long-path execution, five-second timing and Windows
cleanup remain unqualified until those native receipts exist. The source
media/filter/recovery tests remain their own gates; this fixture establishes
console/process presentation, not real encoder or package qualification.

## Continue the migration qualification in Windows Codex

Give the Windows Codex session this repository and this document. The session
should record the pushed SHA, Windows version, tool versions and the output of
each actual check; investigate and fix failures before claiming completion.
Useful baseline commands, after installing the prerequisites above and Node 24,
pnpm 11.19.0 and Python 3.14, are:

```powershell
git rev-parse HEAD
pnpm install --frozen-lockfile
pnpm check
pnpm build
python scripts/test-ipc-registration.py
cargo fmt --all --check
cargo clippy --workspace --all-targets --locked -- -D warnings
cargo test --workspace --locked
pwsh -NoProfile -File .\scripts\qualify-windows-console.ps1
```

The default Rust test run does not execute tests marked ignored for installed
media providers. Inspect each test's requirements and platform restrictions
before enabling it; several retained Linux source-engine probes rely on Linux
wrappers and should not be represented as Windows evidence merely by copying
their logs. The Windows continuation should explicitly exercise and retain
evidence for these native behaviors:

1. Launch the real desktop app, load video/audio/subtitle and image-sequence
   inputs, check picker/track behavior and the source settings at compact and
   wide window sizes. Confirm the encoder menus contain CPU choices only.
2. With the qualified Windows tool deliveries, perform and fully decode short
   CPU Quick Convert and AV1AN encodes. Exercise source trim modes, copied
   audio, subtitle conversion/burn, raw custom filters, image export, held
   command editing, Pause/Resume/Stop, and an interrupted AV1AN job resumed
   with both Saved and New Settings. Check completed chunks and source files
   remain intact, and check output timing/streams with FFprobe.
3. On a physical Vulkan-capable Windows device, exercise libplacebo Spline
   tone mapping on real HDR footage and the compatible Dolby Vision base-layer
   CPU/Auto fallback. Check output color metadata, frame timing, audio and full
   decode. Also check explicit Spline failure when Vulkan is unavailable and
   that it does not silently select a different curve. Do not use a GPU video
   encoder. See [source Dolby combinations](source-dolby-combinations.md) for
   the exact Linux scope and source-clock distinctions.
4. Build/stage and qualify the actual Windows portable package and bundled
   CPU providers using [packaging](packaging.md) and
   [Windows clean-environment qualification](windows-clean-qualification.md).
   A restricted-host receipt is not a clean-image receipt. Exercise the packaged
   desktop app separately: the clean-environment runner does not launch its UI.

The original Dolby GlassBlowing2 P8.1/HDR10-compatible sample was downloaded on
Linux to `~/Downloads/jesses-hdr-samples/` and is not in the Git
repository. Its original filename is
`Dolby-GlassBlowing2-P81-1080p-5994-original-20261101.mp4` and SHA-256 is
`8e86ed6bb71cc71ec0394dad274c335fcfeaaa6801aed3abcea5ee98b6cd1168`.
If it is not accessible from Windows, obtain the same developer sample from
[Dolby's sample page](https://developer.dolby.com/tools-media/sample-media/video-streams/dolby-vision-streams/)
and verify its hash before comparing it with Linux evidence. Preserve the
original; make cuts and output files in a separate test directory.

For a fresh Windows checkout, these PowerShell 7 commands download and verify
that sample into a new local fixture directory, check the selected source-built
FFmpeg pair, generate a short stream-copy clip and probe Vulkan/Spline. Build the
FFmpeg delivery first as described in [packaging](packaging.md), or change the two
executable paths to the actual qualified Windows delivery. A pass of the small
Vulkan probe proves only that this selected provider can run that graph on the
current device; the real HDR app workflow and its output checks still follow.

```powershell
$fixtureDir = "target/windows-hdr-fixture-$([DateTime]::UtcNow.ToString('yyyyMMddTHHmmssZ'))"
New-Item -ItemType Directory -Path $fixtureDir -ErrorAction Stop | Out-Null
$hdr = Join-Path $fixtureDir 'Dolby-GlassBlowing2-original.mp4'
Invoke-WebRequest -Uri 'https://media.developer.dolby.com/DolbyVision_Atmos/mp4/P81_GlassBlowing2_1920x1080%4059.94fps_15200kbps_fmp4.mp4' -OutFile $hdr
$expected = '8e86ed6bb71cc71ec0394dad274c335fcfeaaa6801aed3abcea5ee98b6cd1168'
if ((Get-FileHash -Algorithm SHA256 $hdr).Hash.ToLowerInvariant() -ne $expected) { throw 'Dolby sample hash mismatch' }
$ffmpeg = (Resolve-Path 'target/package-ffmpeg/delivery/ffmpeg.exe').Path
$ffprobe = (Resolve-Path 'target/package-ffmpeg/delivery/ffprobe.exe').Path
& $ffmpeg -hide_banner -version
& $ffmpeg -hide_banner -h filter=libplacebo
& $ffmpeg -hide_banner -loglevel error -init_hw_device vulkan -f lavfi -i 'color=c=black:s=64x64:r=1:d=1' -vf 'libplacebo=tonemapping=spline' -frames:v 1 -f null -
if ($LASTEXITCODE -ne 0) { throw 'Selected FFmpeg Vulkan/Spline probe failed' }
$clip = Join-Path $fixtureDir 'dolby-short.mkv'
& $ffmpeg -hide_banner -n -ss 11 -i $hdr -t 4 -map 0 -c copy $clip
if ($LASTEXITCODE -ne 0) { throw 'HDR fixture stream copy failed' }
& $ffprobe -v error -show_streams -show_format -of json $clip | Set-Content -Encoding utf8 (Join-Path $fixtureDir 'dolby-short-probe.json')
if ($LASTEXITCODE -ne 0) { throw 'HDR fixture probe failed' }
```

The clip is a convenient local fixture, not a claim of identical packet or
clock boundaries to the Linux source-engine derivative. Use the Windows app to
create its own source/outputs and collect Windows-native evidence. In
particular, `scripts/qualify-source-dolby-combinations.py` validates existing
JobManager outputs and request files; it does not generate those outputs for a
fresh Windows checkout, and the Linux receipt paths are not Windows inputs.

`scripts/qualify-windows-console.ps1` is the local entry point. The optional
workflow in `windows-console-qualification.workflow.yml` remains an unactivated
review artifact. Use the pushed revision identified in the final handoff and
record `git rev-parse HEAD` with every Windows receipt.
