# Lean Encode Verification Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Default encodes skip every verification-only pass and finish with one frame-and-stream-count check; the existing "full output verification" switch, relabeled "Thorough verification", restores today's checks.

**Architecture:** One helper, `thorough_verification(settings)`, gates each verification-only pass in the shared encode flow (`crates/media-runtime/src/jobs/encode.rs`, `encode_video_mode`). A new module `jobs/output_counts.rs` performs the default run's only output check. Thorough mode is exactly today's behaviour with the switch on. The Settings checkbox is relabeled; the stored field keeps its name.

**Tech Stack:** Rust 2024 (pinned toolchain 1.98.1, MSVC), Tokio, FFmpeg/FFprobe CLI, Svelte 5 + Playwright.

**Spec:** `docs/superpowers/specs/2026-11-08-lean-encode-verification-design.md`

Line numbers below are as of commit `9677b12`; they drift as tasks land. Locate code by the quoted text.

## Global Constraints

- Stored field stays `full_output_verification` (Rust) / `fullOutputVerification` (TS), default `false`. Plans saved without it run the default way.
- Settings label, exact: `Thorough verification (slower)`. Tooltip, exact: `Off: the finished file is checked by its frame and stream counts only, and an interrupted job is rerun from the start. On: the source and tools are fingerprinted so an interrupted job can resume, every frame is decoded, and every track of the output is re-checked.`
- Frame or stream count mismatch: error code `OUTPUT_VALIDATION_FAILED`; the temporary is removed and nothing is published.
- Thorough mode keeps today's switch-on behaviour (resume workspace, full output decode, every track check).
- Passes that build the output stay in both modes: packet-timestamp frame plan and first-frame read, blocking full scan when packets are unsuitable, audio trim scan (`audio::scan_until`), the full-track `audio::scan` for external converted tracks and for AAC into MP4/MOV, tone-map GPU probe and peak measurement, trim/subtitle/chapter preparation, AV1AN preprocess, AV1AN `decoded_identity`, `verify_source_join`, and the producer observation inside `validate_video_checkpoint` when source filters are on.
- Cheap gates stay in both modes: tool version and capability checks, audio delay/conversion fixture encodes, CPU tone-map `check_tools`, `av1an::validate_input`.
- Untouched: `Source::verify`, `original_guard.verify()`, `publish_verified` (`jobs.rs` ~2042), the edited-command paths' `command_edit::verify_actual_output` (encode.rs ~1629, ~1806; no plan exists there to count against), alpha and lossless checks (only run in those modes).
- Shell is PowerShell. Refresh PATH before cargo: `$env:Path = [Environment]::GetEnvironmentVariable('Path','Machine') + ';' + [Environment]::GetEnvironmentVariable('Path','User')`.
- Never run Playwright and `cargo test --workspace` at the same time.
- Commits use the repo-local identity (`jkkma <16640398+jkkma@users.noreply.github.com>`, already configured) and end with `Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>`.
- Real-tool test environment (PowerShell, same session as the cargo command):

```powershell
$r = "$env:USERPROFILE\scoop\apps\jesses\current\resources"; $t = "$r\tools"
$env:JESSES_TEST_TOOL_RESOURCES = $r
$env:JESSES_FFMPEG = "$t\ffmpeg\ffmpeg.exe"; $env:JESSES_FFPROBE = "$t\ffmpeg\ffprobe.exe"
$env:JESSES_SVT_AV1 = "$t\svt-av1-hdr\SvtAv1EncApp.exe"; $env:JESSES_SVT_AV1_HDR = "$t\svt-av1-hdr\SvtAv1EncApp.exe"
$env:JESSES_X264 = "$t\x264\x264.exe"; $env:JESSES_X265 = "$t\x265\x265.exe"
$env:Path = "$t\ffmpeg;$t\mkvmerge;$t\x265;$t\vpxenc;$t\svt-av1-hdr;" + $env:Path
```

## Review Focus

1. **MP4/MOV output with selected subtitles in a default run:** an empty subtitle track must still be rejected before encoding, so cue counts are fetched for that case. Test: Task 2 `cue_counts_are_needed_only_for_subtitles_outside_matroska`.
2. **Two-pass (bitrate) encodes in a default run:** there is no resume workspace, so the pass-one checkpoint must not panic. Test: Task 4 `default_two_pass_encode_needs_no_resume_workspace`.
3. **Custom source filters in a default run:** the producer clock must still set the plan's size and frame count, and the count check must use that count. Test: Task 5 source-filter case in `default_encode_checks_only_frame_and_stream_counts`.
4. **Plans saved before the switch existed (no `fullOutputVerification`):** they run the default way. Test: Task 2 `thorough_verification_follows_the_saved_switch`.
5. **Source replaced or modified during a default run:** still refused with `SOURCE_CHANGED`. Covered by keeping `Source::verify` and `publish_verified` outside every gate; the Task 5 reviewer checks the diff leaves `original_guard.verify()` and the `finalize_verified` call outside the thorough-only branch.

---

### Task 1: Commit the tested speedups

**Files:** the six modified files already in the working tree.

- [ ] **Step 1: Confirm they pass**

Run: `cargo test -p media-runtime --lib` then `cargo clippy -p media-runtime --all-targets -- -D warnings`
Expected: `test result: ok. 454 passed; 0 failed; 41 ignored`; clippy exits 0.

- [ ] **Step 2: Commit the log thinning**

```powershell
git add crates/media-runtime/src/supervisor.rs crates/media-runtime/src/supervisor/tests.rs
git commit -m "Keep one progress redraw per interval in job logs" -m "Encoders redraw their progress line with a bare carriage return several times a second. Every redraw still reaches the job's events, but the job log keeps one per 10 seconds plus the last before other output, so a long encode no longer rotates its command line and warnings away. A 5-minute 4K encode's log shrank from 746 KB to 10 KB." -m "Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

- [ ] **Step 3: Commit the fingerprint changes**

`git add crates/media-runtime/src/jobs/standalone_recovery.rs crates/media-runtime/src/jobs/encode.rs`, subject `Fingerprint the source beside the encoder and once for its donors`; body: fresh jobs hash the source in the background and settle it at the first checkpoint; a donor that is the source file reuses its digest; the recorded manifest is unchanged.

- [ ] **Step 4: Commit the subtitle check batching**

`git add crates/media-runtime/src/jobs/trim_source.rs crates/media-runtime/src/jobs/trim_packets.rs`, subject `Check converted subtitle tracks four at a time from one probe`.

- [ ] **Step 5: Verify**

Run: `git log -3 --format="%h %an <%ae> | %cn <%ce> | %s"; git status --short`
Expected: three commits by `jkkma` as author and committer; only `.claude/` untracked.

---

### Task 2: One switch and a header-only source probe

**Files:**

- Modify: `crates/media-runtime/src/jobs/encode.rs` (~3441 helper, ~2131 and ~3451 callers, ~506 source probe)
- Modify: `crates/media-runtime/src/jobs.rs` (~2307-2399 `probe`)
- Modify: `crates/media-runtime/src/jobs/container.rs` (new fn near `preflight`, ~271)
- Test: test modules in the same three files

**Interfaces:**

- Produces: `fn thorough_verification(settings: &media_core::EncodeSettings) -> bool` in encode.rs (rename of `full_output_verification`; same body).
- Produces: `async fn probe_headers(executable: &Path, path: &Path, cancel: &watch::Receiver<bool>, selected: Option<&[u32]>) -> Result<Document, AppError>` in jobs.rs.
- Produces: `fn probe_args(count_packets: bool) -> Vec<OsString>` (private, jobs.rs), used by `probe` and `probe_headers`.
- Produces: `pub(super) fn needs_cue_counts(output: &Path, document: &Document, selected: &[u32]) -> bool` in container.rs.

- [ ] **Step 1: Write the failing tests**

```rust
// jobs.rs tests
#[test]
fn probe_args_count_packets_only_when_asked() {
    let counted = probe_args(true);
    let headers = probe_args(false);
    for args in [&counted, &headers] {
        for flag in ["-show_streams", "-show_format", "-show_chapters", "-show_data_hash"] {
            assert!(args.iter().any(|arg| arg == flag), "{flag}");
        }
    }
    assert!(counted.iter().any(|arg| arg == "-count_packets"));
    assert!(!headers.iter().any(|arg| arg == "-count_packets"));
}

// encode.rs tests
#[test]
fn thorough_verification_follows_the_saved_switch() {
    let with = |full_output_verification| EncodeSettings {
        encoding_preferences: Some(media_core::EncodingPreferences {
            keyframe_interval_seconds: 10, mp4_faststart: true, full_output_verification,
        }),
        ..EncodeSettings::default()
    };
    assert!(!thorough_verification(&EncodeSettings::default()));
    assert!(!thorough_verification(&with(false)));
    assert!(thorough_verification(&with(true)));
    let saved: media_core::EncodingPreferences =
        serde_json::from_value(serde_json::json!({"keyframeIntervalSeconds": 10, "mp4Faststart": true})).unwrap();
    assert!(!saved.full_output_verification);
}

// container.rs tests (Document built with serde_json::from_value, as subtitles.rs tests do)
#[test]
fn cue_counts_are_needed_only_for_subtitles_outside_matroska() {
    // streams: {index 0, codec_type "video"}, {index 1, codec_type "subtitle"}
    assert!(!needs_cue_counts(Path::new("/out/a.mkv"), &document, &[0, 1]));
    assert!(needs_cue_counts(Path::new("/out/a.mp4"), &document, &[0, 1]));
    assert!(needs_cue_counts(Path::new("/out/a.mov"), &document, &[0, 1]));
    assert!(!needs_cue_counts(Path::new("/out/a.mp4"), &document, &[0]));
}
```

- [ ] **Step 2: Run them to see them fail**

Run: `cargo test -p media-runtime --lib -- probe_args_count thorough_verification_follows cue_counts_are_needed`
Expected: compile errors for the three missing functions.

- [ ] **Step 3: Implement**

- Rename the helper and its two callers.
- Build `probe`'s args with `probe_args(true)`. `probe_headers` uses `probe_args(false)` and the same post-processing, with one difference. A video stream without `start_time` gets `packet_start_time` from `first_packet_start` whatever its count, because headers carry no count. Subtitle streams get none, as today when a count is zero. Share the body through a private `probe_with(.., count_packets: bool)`.
- `needs_cue_counts` decides the container the same way `container::preflight` does, and returns true when that container is not Matroska and a selected stream is a subtitle.
- At encode.rs ~506, call `probe_headers` first. Replace the result with `probe(...)` when `thorough_verification(settings) || container::needs_cue_counts(&output, &document, &request.stream_indices)`.

- [ ] **Step 4: Run the tests and the library suite**

Run: `cargo test -p media-runtime --lib`
Expected: all pass (457 passed).

- [ ] **Step 5: Commit** — `Probe source headers only unless a count is needed`

---

### Task 3: Skip the pre-encode verification passes

**Files:**

- Modify: `crates/media-runtime/src/jobs/encode.rs`: `source_frames` (~2961-3048) and its caller (~772-783); the audio timeline loop (~728-759); the "Will encode"/"Validated" log (~1168); AV1AN `prepared.validate` (~1280-1282); the real-tool test at ~5897; `decoded_defects_after_the_packet_plan_fail_without_publication` (~5030)

**Interfaces:**

- Consumes: `thorough_verification` (Task 2).
- Produces: `source_frames(&self, id, ffprobe, input, plan, stream, duration, trim, preview, thorough: bool, cancel)`.
- Produces: the test `default_encode_checks_only_frame_and_stream_counts`, extended by Tasks 4 and 5.

- [ ] **Step 1: Turn the fast-verification test into the default-run test**

Rename `fast_verification_checks_every_frame_clock_without_decoding_the_output` to `default_encode_checks_only_frame_and_stream_counts`. Keep its three cases and `encode_with_audio(.., false)`. Replace its log assertions with:

```rust
assert_eq!(job.state, JobState::Succeeded, "{job:#?}");
assert!(!job.logs.iter().any(|line| line.contains("verified alongside the encoder")), "{:#?}", job.logs);
assert!(job.logs.iter().any(|line| line.starts_with("Planned ")
    && line.ends_with("constant-rate frames from packet timestamps.")), "{:#?}", job.logs);
if extension != "mp4" {
    assert!(!job.logs.iter().any(|line| line.contains("to verify its complete sample timeline")), "{:#?}", job.logs);
}
assert_encoded(&output, codec, frames).await;
```

- [ ] **Step 2: Run it to see it fail**

Run (real-tool environment): `cargo test -p media-runtime --lib -- --ignored default_encode_checks_only_frame_and_stream_counts`
Expected: FAIL on the "verified alongside the encoder" assertion.

- [ ] **Step 3: Implement**

- Bind `let thorough = thorough_verification(settings);` once near the top of `encode_video_mode`; Tasks 3-5 use it.
- In the audio timeline loop (~728-759), skip `audio::scan` (the untrimmed, full-track arm) unless `thorough`, the track is external (`external.audio_source(track.stream_index).is_some()`), or it is AAC going into MP4/MOV (`container::format(&output)?` is `Mp4` or `Mov`, as the priming gate at ~860 checks). A skipped track pushes no timeline and logs no "Decoding selected audio…" phase. Trimmed jobs keep `audio::scan_until` for every track.
- `source_frames` takes `thorough` and spawns the scan only when `concurrent && !preview && thorough`. Without the scan, it logs exactly `Planned {count} constant-rate frames from packet timestamps.` and still returns the packet plan. The blocking fallback scan (packets unsuitable) is unchanged.
- The caller passes `thorough_verification(settings)`.
- At ~1168, write "Will encode" when `source_timing::pending() || !thorough_verification(settings)`.
- At ~1280, call AV1AN `prepared.validate(..)` only when `thorough_verification(settings)`. Keep `decoded_identity`.
- In `decoded_defects_after_the_packet_plan_fail_without_publication` (~5030), set `encoding_preferences: Some(EncodingPreferences { keyframe_interval_seconds: 10, mp4_faststart: true, full_output_verification: true })`. It tests the scan, which now runs only in thorough mode.

- [ ] **Step 4: Run both real-tool tests and the library suite**

Run: `cargo test -p media-runtime --lib -- --ignored default_encode_checks_only_frame_and_stream_counts decoded_defects_after_the_packet_plan` then `cargo test -p media-runtime --lib`
Expected: both ignored tests pass; library suite passes.

- [ ] **Step 5: Commit** — `Verify decoded source frames only in thorough runs`

---

### Task 4: Keep a resume workspace only in thorough runs

**Files:**

- Modify: `crates/media-runtime/src/jobs/encode.rs`: standalone branch (~1357-1463), QTGMC identity (~1681-1691), pass-one checkpoint (~2083-2093), checkpoint block (~2135-2162)
- Test: `default_encode_checks_only_frame_and_stream_counts`; new `default_two_pass_encode_needs_no_resume_workspace` (encode.rs tests)

**Interfaces:**

- Consumes: `thorough_verification` (Task 2).
- Produces: `standalone_recovery: Option<Recovery>` is `None` for every default run.

- [ ] **Step 1: Write the failing assertions and test**

Add to each case of `default_encode_checks_only_frame_and_stream_counts`:

```rust
assert!(job.standalone_recovery.is_none(), "{job:#?}");
assert!(!job.logs.iter().any(|line| line.starts_with("Fingerprinting")), "{:#?}", job.logs);
```

New `#[ignore = "requires real FFmpeg, FFprobe and x264"]` test `default_two_pass_encode_needs_no_resume_workspace`. It encodes like `encode_with_audio(VideoEncoder::X264, 5, "mkv", "aac", false)` with `rate_control` set to a two-pass bitrate mode, as the two-pass case in `crates/media-runtime/tests/rate_control_jobs.rs` (~240) does. Add a `rate_control: Option<VideoRateControl>` parameter to `encode_with_audio` and pass `None` from the existing callers. Assertions: `Succeeded` and `job.standalone_recovery.is_none()`.

- [ ] **Step 2: Run them to see them fail**

Run: `cargo test -p media-runtime --lib -- --ignored default_encode_checks_only default_two_pass_encode`
Expected: FAIL. The first fails on the `Fingerprinting` or `standalone_recovery` assertion. The two-pass test fails the same way: today's pass-one checkpoint always runs and records a recovery summary.

- [ ] **Step 3: Implement**

- In the standalone branch, run the "Fingerprinting…" phase message and `Recovery::prepare` only when `thorough_verification(settings)`. Otherwise the branch yields `None` and leaves `standalone_recovery` and the `standalone_*` checkpoints at `None`.
- Replace each `.expect("standalone recovery")` with `if let Some(recovery) = standalone_recovery.as_mut()`. At ~1681, only compute the QTGMC `decoded_identity` when a recovery exists, since it only feeds `remember_source_qtgmc`.
- Optional checkpoints (~2309-2319, ~2645-2655) and cleanup already handle `None`.

- [ ] **Step 4: Run the real-tool tests and the library suite**

Run: same as Step 2, then `cargo test -p media-runtime --lib`
Expected: pass.

- [ ] **Step 5: Commit** — `Keep a resume workspace only in thorough runs`

---

### Task 5: Check only frame and stream counts after the mux

**Files:**

- Create: `crates/media-runtime/src/jobs/output_counts.rs`; register `mod output_counts;` after `mod output_metadata;` in `crates/media-runtime/src/jobs.rs` (~96)
- Modify: `crates/media-runtime/src/jobs/encode.rs`: `validate_video_checkpoint` (~3519-3650) and its call (~2106-2134); post-mux block (~2396-2847); `finalize_verified` call (~2854-2887); tests at ~5772 and ~5760

**Interfaces:**

- Consumes: `thorough_verification`, `probe_headers` (Task 2).
- Produces: `pub(super) fn check(document: &metadata::Document, kinds: &[&str], video_position: usize, frames: usize) -> Result<(), AppError>`.
- Produces: `pub(super) async fn verify(ffprobe: &Path, output: &Path, kinds: &[&str], video_position: usize, frames: usize, cancel: &watch::Receiver<bool>) -> Result<(), AppError>`.
- Produces: `validate_video_checkpoint(.., decode: bool, count: bool, cancel)`. With `count == false` it probes headers only, applies the producer observation, and returns before `validate_elementary_checkpoint` and the count.

- [ ] **Step 1: Write the failing unit tests** (in `output_counts.rs`; documents from `serde_json::from_value` with `index`, `codec_type`, `nb_read_packets`)

```rust
#[test] fn matching_counts_and_kinds_pass()               // [video "48", audio "70"], kinds ["video","audio"], pos 0, 48 -> Ok
#[test] fn a_missing_or_extra_frame_fails()               // frames 47 and 49 -> Err code "OUTPUT_VALIDATION_FAILED"
#[test] fn a_missing_extra_or_reordered_stream_fails()    // kinds ["video"], ["video","audio","subtitle"], ["audio","video"] -> Err
#[test] fn an_unreadable_video_count_fails()              // nb_read_packets absent, and "N/A" -> Err
```

Error messages: `The finished file has {actual} video frames; {frames} were planned.` and `The finished file's streams are {actual:?}; {kinds:?} were planned.`

- [ ] **Step 2: Extend the default-run test and run everything to see it fail**

Add to each case of `default_encode_checks_only_frame_and_stream_counts`:

```rust
assert!(job.logs.iter().any(|line| line.starts_with("Output check:")), "{:#?}", job.logs);
assert!(!job.logs.iter().any(|line| line.contains(FAST_VERIFIED) || line.starts_with("Verified all")));
```

Add a fourth case with source filters on. Copy the filter settings from the first test in `crates/media-runtime/tests/source_filter_trim_jobs.rs`, with a filter that changes picture size. Assert `Succeeded`, the `Output check:` line, and the filtered size in the output probe.

Run: `cargo test -p media-runtime --lib -- output_counts` and the ignored default-run test.
Expected: unit tests fail to compile; the real-tool test fails on `Output check:`.

- [ ] **Step 3: Implement `output_counts`**

`verify` runs `ffprobe -v error -count_packets -show_entries stream=index,codec_type,nb_read_packets -of json -i <output>` through `supervisor::run_capture`. It parses the result as `metadata::Document` and calls `check`. Errors carry the output path.

- [ ] **Step 4: Gate the encode flow**

- **Checkpoint check:** call `validate_video_checkpoint` when `thorough || settings.source_filters`, with `decode = thorough` and `count = thorough`.
- **Phase message:** after the mux, in default runs use `Checking the finished file's frame and stream counts.`
- **Default branch after the mux:**
  - Call `output_counts::verify(&ffprobe, &temp.path, &kinds, video_position, frame_count, cancel)`.
  - `kinds` is `output_selected`'s codec types in order, plus `"attachment"` when `settings_attachment` is `Some`.
  - Then log exactly `Output check: {frame_count} video frames and {kinds.len()} streams, as planned. Thorough verification is off; turn it on in Settings to also re-check every frame and track.`
- **Thorough branch:** today's checks from `probe(&ffprobe, &temp.path, ..)` (~2400) through the data-stream comparisons (~2847). The `!decodes_output` fast path is unreachable now, because thorough implies `decodes_output`; remove it.
- **Kept in both branches:** `seal_source_iso` (~2397), the target-size log, the finalizing checkpoint (a no-op without recovery), alpha/lossless, `original_guard.verify()`, `standalone_hdr.finish()`.
- **If alpha/lossless need `artifact`:** in default runs, probe it with `probe(..)` only when those modes are on.
- **Finalize:** pass `dynamic_hdr: None` to `finalize_verified` unless thorough.
- **Dead code:** delete helpers the compiler then reports unused (for example `frame_sample`), with their unit tests.
- **Rename** `encodes_external_sample_with_fast_and_full_output_verification` to `encodes_external_sample_in_default_and_thorough_modes`. Its mode labels become `default`/`thorough`. Assert an `Output check:` line iff default, and `FAST_VERIFIED` never.
- **`svt_av1_mp4_reads_the_copied_clock_from_packets`** (thorough): add `assert!(!job.logs.iter().any(|line| line.starts_with("Output check:")))`.

- [ ] **Step 5: Run unit, clippy and real-tool tests**

Run: `cargo test -p media-runtime --lib`; `cargo clippy -p media-runtime --all-targets -- -D warnings`; `cargo test -p media-runtime --lib -- --ignored default_encode_checks_only svt_av1_mp4_reads_the_copied_clock x265_mp4_proves_each_copy vpxenc_webm_proves_each_copy`
Expected: all pass; clippy exits 0.

- [ ] **Step 6: Commit** — `Check only frame and stream counts after the mux by default`

---

### Task 6: Relabel the switch

**Files:**

- Modify: `src/lib/features/source/SourceSettings.svelte` (~169-178)
- Modify: `crates/media-core/src/preferences.rs` (~58-61, ~91-94 doc comments)
- Regenerate: `src/lib/ipc/generated.ts` via `pnpm contracts`
- Test: `tests/frontend/source-encode-parity.spec.ts`

**Interfaces:** none new; same field and IPC shape.

- [ ] **Step 1: Write the failing Playwright test**

```ts
test('thorough verification is off by default and travels with the encode request', async ({
  page,
}) => {
  await page.getByRole('button', { name: 'Settings', exact: true }).click();
  const settings = page.getByRole('region', { name: 'Settings' });
  const thorough = settings.getByLabel('Thorough verification (slower)');
  await expect(thorough).not.toBeChecked();
  await thorough.check();
  await expect
    .poll(() =>
      page.evaluate(() => (globalThis as any).__preferences.general.fullOutputVerification),
    )
    .toBe(true);
  await page.getByRole('button', { name: 'File List', exact: true }).click();
  await page.getByRole('button', { name: 'Add Files…' }).click();
  await page.getByRole('button', { name: 'Load Tracks' }).click();
  await page.getByRole('button', { name: 'Quick Convert', exact: true }).click();
  const quick = page.getByRole('region', { name: 'Quick Convert', exact: true });
  await quick.getByLabel('Output Path', { exact: true }).fill('/media/output');
  await page.getByRole('button', { name: 'Run: Encode', exact: true }).click();
  await expect
    .poll(() =>
      page.evaluate(() =>
        (globalThis as any).__sourceCalls.some((c: any) => c.command === 'mock_start_encode'),
      ),
    )
    .toBe(true);
  const calls = await page.evaluate(() => (globalThis as any).__sourceCalls);
  const request = calls.find((c: any) => c.command === 'mock_start_encode').payload.request;
  expect(request.settings.encodingPreferences.fullOutputVerification).toBe(true);
});
```

- [ ] **Step 2: Run it to see it fail**

Run: `pnpm exec playwright test tests/frontend/source-encode-parity.spec.ts -g "thorough verification" --reporter=line`
Expected: FAIL, label not found.

- [ ] **Step 3: Implement**

- Set the label and tooltip to the exact Global Constraints text.
- Doc comment on `GeneralPreferences::full_output_verification`: `Thorough verification. Off (default), an encode checks only the finished file's frame and stream counts and keeps no resume workspace. On, the source and tools are fingerprinted for resume, every output frame is decoded and every track is re-checked.`
- Doc comment on `EncodingPreferences::full_output_verification`: `Absent in plans saved before this option: those run without thorough verification.`
- Run `pnpm contracts`.

- [ ] **Step 4: Run the checks**

Run: the Step 2 command; then `pnpm run contracts:check`, `pnpm run check`, `pnpm exec prettier --check src/lib/features/source/SourceSettings.svelte tests/frontend/source-encode-parity.spec.ts`
Expected: test passes; contracts in sync; svelte-check 0 errors; prettier clean.

- [ ] **Step 5: Commit** — `Label the verification switch Thorough verification`

---

### Task 7: Opt verification tests into thorough mode and accept on the 4K job

**Files:** integration tests under `crates/media-runtime/tests/` and ignored tests in `crates/media-runtime/src/jobs/*.rs`, as found below.

- [ ] **Step 1: Run the ignored real-tool suites that exercise encode jobs**

Run each, in the real-tool environment:

- `cargo test -p media-runtime --lib -- --ignored jobs::encode`
- `cargo test -p media-runtime --test source_trim_jobs -- --ignored`
- the same for `source_subtitle_trim_jobs`, `source_copy_trim_jobs`, `trim_jobs`, `audio_jobs`, `tone_map_jobs`, `source_ui_controls`, `migration_hdr`.

`source_svt_jobs` fails 12/12 on this machine at baseline (av1an version check); it is not a signal here.

- [ ] **Step 2: Opt in only the tests that test verification**

A test that fails only because an expected verification error or log line no longer appears in a default run tests thorough mode. Give its settings `encoding_preferences: Some(EncodingPreferences { .., full_output_verification: true })`. Any other failure is a regression: stop and fix it in the task that caused it. Re-run until every suite from Step 1 passes or matches its baseline.

- [ ] **Step 3: Full frontend and Rust checks**

Run: `pnpm exec playwright test --reporter=line`; then `cargo test -p media-runtime --lib`; `cargo clippy -p media-runtime --all-targets -- -D warnings`
Expected: Playwright all pass (300+); library suite passes; clippy exits 0.

- [ ] **Step 4: Rebuild and run the 4K acceptance job**

- Rebuild the app (`pnpm exec vite build`, then `pnpm tauri build --no-bundle` with LTO off and 16 codegen units, and the override that blanks `beforeBuildCommand`).
- Re-run the 5-minute job with the switch off: SVT-AV1-HDR, CRF 20, preset 4, Spline (GPU), exact 1920×1080 letterbox, Opus 128, MKV. Run it in the GUI, or headless with `cargo run --release -p media-runtime --example encode_request -- REQUEST.json` using the job's saved `request` and `encodeSettings`.
- Record preparation time (Run to encoder start) and finishing time (encoder exit to success).
- Compare the output's packets with `C:\Users\Deydy\Videos\Resident.Evil.2026.1080p.rerun.mkv`.

Expected: success with an `Output check:` line; identical video, audio and subtitle packets; preparation near 30 s; finishing at the mux time plus a few seconds.

- [ ] **Step 5: Commit** — `Run verification tests in thorough mode`
