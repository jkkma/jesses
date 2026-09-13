# Live av1an pause validation

Local Windows x64 qualification on 2026-09-13. Live pause retains the running
process tree, memory and open files. Continue resumes that same tree. Stop and
keep progress remains the separate durable-recovery action.

- Two owned-process gates pass: three generations stop producing heartbeat data,
  duplicate Pause/Continue requests are idempotent, Continue restarts every
  worker, and Cancel while frozen terminates all descendants. A second gate
  pauses beyond the encode timeout and verifies that suspended time is excluded;
  the running-time timeout still terminates the tree after Continue.
- Actual av1an gate `live_pause_continues_exact_output_and_cancels_while_frozen`
  passes with 5fish: pause/continue produces the expected 720 decoded frames;
  a second frozen job cancels without publishing output or retaining source locks.
  Source hashes remain unchanged.
- Browser recovery coverage passes the Pause/Continue transitions, explicit
  command payloads, Stop availability while paused, and Cancel while paused.

Pause is available only while the main av1an process is attached. Preparation
and final muxing can still be canceled. Windows freezes the owned Job Object;
an unsupported operating-system operation fails explicitly. Linux uses the owned
process group, verifies stopped descendants and closes the group reference
before reaping its leader. Linux execution and the final packaged desktop UI
remain separate qualification gates. Restarted paused jobs become interrupted;
they never resume automatically.

Native Windows verification used frozen executable SHA-256
`49c84b2425f90efa7842acc5d297fd2dd4c23458550d575a5843e1e744fb6777`.
The complete original 720p episode ran with SVT-AV1-HDR CRF30/preset2 and two
av1an workers. Pause retained the same av1an, VSPipe, FFmpeg, SVT, FFprobe and
console-helper process identities; all six consumed exactly zero CPU over
76.6947 seconds. Continue returned the job to Running, and it succeeded.

Independent verification found 34,047 frames with exactly matching video
timestamps, 27 streams, 61,159 copied AAC packets, 353 copied subtitle packets,
and 24 identical font attachments. Copied packet payloads, PTS and DTS match
exactly. AAC packet duration tags differ by at most one Matroska millisecond;
all 501,014,528 decoded PCM bytes are identical. Original source SHA-256, size
and modification time remain unchanged. The 212,540,214-byte output has SHA-256
`e2d0807f1ddbb31178e994914e5af5b20a367ebb4aed9a3702c168d2d1af655b`.
Evidence is under `Videos/Jesses-complete-native-20260913`, including
`native-pause-output-independent.json`, both CPU samples and native screenshots.
The earlier 20-second trial finished before Pause was clicked and is not pause
evidence. These results do not qualify a subsequently rebuilt package or Linux.
