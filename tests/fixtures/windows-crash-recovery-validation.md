# Windows portable crash recovery qualification

Local Windows x64 qualification on 2026-09-20 exercised the portable package
through an isolated Scoop installation on Windows 11 Home, build 26200. This is
bounded crash-recovery and package-isolation evidence, not fresh-image or complete
release qualification. Linux and macOS remain deferred.

## Defect and correction

Force-closing Jesses during an av1an encode previously left the final mux attempt
beside the output. Resume could produce a verified output and remove its recovery
workspace while leaving that earlier, empty attempt behind.

New av1an mux attempts live inside the identity-guarded recovery workspace.
Successful publication first closes and removes the current attempt, then removes
the workspace and crashed attempts. If the current attempt cannot be safely
removed, the workspace and recovery receipt are retained and the successful job
reports the cleanup warning. Old attempts outside the workspace are left alone;
their names do not establish ownership.

## Exact package and native workflow

The rebuilt candidate used these SHA-256 identities:

| Input             | SHA-256                                                            |
| ----------------- | ------------------------------------------------------------------ |
| Portable ZIP      | `73968c047098a0f5ac03530df91ad40f4613f68c38c73367ad4541566f89f663` |
| Jesses executable | `75ad79a9fc95442f5cfa8523625d428073b5143d316a239ce47bce502c34993e` |

The original candidate passed actual Scoop install, update, reset, ordinary
uninstall and reinstall with all persisted bytes unchanged. A subsequent Scoop
update installed the rebuilt candidate without changing the saved data. Launch
used the Scoop shim with external tools removed from `PATH`; normal managed-fork
discovery remained enabled. The selected managed SVT-AV1-HDR binary was identical
to the packaged binary, so this native case does not claim exclusive bundled-tool
discovery.

The native case used an unchanged 1,079-frame 1280 x 720 source, SVT-AV1-HDR,
CRF 30, preset 2, one worker, L-SMASH, standard scene detection, and a maximum
chunk size of 120 frames. Audio, subtitles and 24 attachments were copied.

- Only the Jesses parent was force-terminated after 106 frames were saved. All
  nine observed child processes exited without a separate child-kill operation.
  No final output existed; the recovery workspace and its mux attempt remained.
- Reopening showed **Interrupted** and 106 saved frames without starting work.
  Explicit **Resume** reused the saved chunk with unchanged hash, size and
  modification time while encoding progressed.
- The final AV1 output decoded all 1,079 frames. Independent checks preserved
  video timing, all 27 streams and their types, copied audio/subtitle payloads,
  attachments, and source bytes and modification time. All 1,938 audio and 15
  subtitle packet PTS/DTS values matched the source exactly.
- After success, the recovery receipt, workspace and new attempts were gone.
  No owned encoder processes remained. The original candidate's empty orphan
  was retained separately as defect evidence.

## Automated coverage and isolation

The Windows real-tool tests in `crates/media-runtime/tests/av1an_recovery.rs`
exercise forced parent exit, descendant-exit verification through retained process
handles, idle history reopening, changed-tool rejection, valid saved-chunk reuse,
full output decoding, source preservation and recovery cleanup. A second test
holds a delete-denying handle on the mux attempt and verifies that publication
succeeds while cleanup retains the workspace and reports a warning. Existing
live pause/continue and cancel-while-paused coverage also passed.

The [environment runner](../../docs/windows-clean-qualification.md) passed against
the exact rebuilt ZIP with a cleared child environment, system-only `PATH`, a
fresh profile and read-only program resources. All seven bundled tools resolved
from the extracted package; generated x264 and mainline SVT-AV1 outputs decoded
eight of eight frames. Program-root creation, nested resource creation and an
existing resource write were denied; all five persisted-data locations were
writable. WebView2 153.0.4234.48 was present. The archive was unchanged.

The runner's 12 Windows PowerShell 5.1 safety checks passed, including fast-parent
exit and process-tree cleanup. Targets start only after their gated launcher
belongs to a kill-on-close Windows Job Object. The receipt records
`RestrictedHost` and `qualifiesFreshImage: false`.

Fresh Windows execution, ordinary user PATH registration, broader portable
workflow coverage and the complete output/process-safety matrix remain separate
acceptance work. These checks do not establish arbitrary changed-tool recovery
compatibility or turn older artifacts into qualified release packages.
