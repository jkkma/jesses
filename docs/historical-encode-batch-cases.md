# Historical encode and batch case inventory

**Current-scope correction (2026-11-02):** This document preserves the wording
and dispositions of the earlier inventory. References below to NVENC as an
available or deferred encoder are superseded by the explicit CPU-only scope;
hardware video encoders are excluded. The previously pending copied-audio with
encoded-video trim case now has active browser coverage in `encode.spec.ts` and
Linux native evidence in [source copied trim](source-copied-trim.md). Other
historical statements are not evidence of the current state by themselves; see
[the current UI audit](source-ui-current-audit.md) and phase receipts.

The 201 historical Playwright cases below used the retired Jesses inspector, explicit
pre-run batch review and/or queue/history screens. This is an individual-case
working inventory, not a claim that those cases pass in the source UI. A
replacement must preserve each applicable source behavior; explicit preview
invalidation and user-managed queue/history actions are obsolete because the
fork has no such screens. Native execution proof is separate from mocked UI.

**Covered** means the relevant source-flow assertion is exercised at the
referenced location. **Retire** records an absent old control or an assertion
that contradicts the fork; **native gap** and **open source gap** name the
remaining limitations explicitly. Both old suites are preserved in the
[batch archive](historical-tests/batch.spec.ts.txt) and
[encode archive](historical-tests/encode.spec.ts.txt); their active replacements
exercise source Run. Line numbers refer to historical cases, including expanded
parameter variants.

## batch.spec.ts

- `379` batch review carries the NVENC constant QP setting — Excluded — hardware video encoders are outside the current CPU-only scope.
- `394` batch review carries HDR metadata overrides and invalidates when edited — Covered — batch.spec.ts verifies frozen Direct x265 HDR metadata; source has no separate review screen.
- `422` FFmpeg x265 batch accepts tagged HDR10 and carries an authored mastering override — Retire — Quick exposes Direct x265, not the old FFmpeg x265 inspector route; native legacy route tests remain separate.
- `462` FFmpeg x265 batch flags HLG and incomplete HDR tags for tone mapping — Retire — Quick exposes Direct x265, not the old FFmpeg x265 inspector route; native legacy route tests remain separate.
- `489` batch explains other operations without running them or expanding a reviewed selection — Covered — source-shell unsupported-batch dialogs.
- `534` batch audio-only conversion previews each selected audio track without video — Covered — batch.spec.ts mixed and all-nonvideo inputs.
- `558` crop resize and borders batch preview retains each source framing and immutable queue history — Covered — batch.spec.ts verifies per-input crop, resize/border intent and frozen requests; the old queue/history panel is absent.
- `622` crop resize and borders batch crop edit invalidates an outstanding preview — Retire — explicit preview invalidation has no fork control; Run snapshot covered by batch.spec.ts.
- `622` crop resize and borders batch width edit invalidates an outstanding preview — Retire — explicit preview invalidation has no fork control; Run snapshot covered by batch.spec.ts.
- `622` crop resize and borders batch resize edit invalidates an outstanding preview — Retire — explicit preview invalidation has no fork control; Run snapshot covered by batch.spec.ts.
- `622` crop resize and borders batch border edit invalidates an outstanding preview — Retire — explicit preview invalidation has no fork control; Run snapshot covered by batch.spec.ts.
- `622` crop resize and borders batch bordersEnabled edit invalidates an outstanding preview — Retire — explicit preview invalidation has no fork control; Run snapshot covered by batch.spec.ts.
- `660` batch preview remains keyboard accessible after editing source fields — Retire — no explicit batch preview screen; source Run keyboard and dialog tests remain.
- `677` crop and resize default framing permits mixed source compatibility results in batch preview — Covered — batch.spec.ts verifies distinct source dimensions and a crop failure isolated to one input.
- `699` crop resize and borders batch rejects invalid per-file values before requesting preview — Covered — batch.spec.ts verifies an invalid crop fails its own input while the other succeeds.
- `742` crop resize and border batch drafts restore per encoder and workflow with independent av1an framing — Retire — the old per-encoder framing inspector/draft model is absent; source crop/resize dialogs and Quick/AV1AN settings are covered separately.
- `791` SVT fork svtAv1FiveFish batch snapshots retain reviewed controls and output names — Retire — the fork exposes one SVT encoder, not the old separate 5fish/HDR choices; source SVT selection and literal-zero request have replacement coverage.
- `858` SVT fork svtAv1FiveFish batch requires its exact tool — Retire — the fork exposes one SVT encoder, not the old separate 5fish/HDR choices; source SVT selection and literal-zero request have replacement coverage.
- `791` SVT fork svtAv1Hdr batch snapshots retain reviewed controls and output names — Retire — the fork exposes one SVT encoder, not the old separate 5fish/HDR choices; source SVT selection and literal-zero request have replacement coverage.
- `858` SVT fork svtAv1Hdr batch requires its exact tool — Retire — the fork exposes one SVT encoder, not the old separate 5fish/HDR choices; source SVT selection and literal-zero request have replacement coverage.
- `881` SVT fork batch invalidates a deferred preview after editing Lineart psy bias — Retire — the fork exposes one SVT encoder, not the old separate 5fish/HDR choices; source SVT selection and literal-zero request have replacement coverage.
- `881` SVT fork batch invalidates a deferred preview after editing Texture psy bias — Retire — the fork exposes one SVT encoder, not the old separate 5fish/HDR choices; source SVT selection and literal-zero request have replacement coverage.
- `881` SVT fork batch invalidates a deferred preview after editing HDR tune — Retire — the fork exposes one SVT encoder, not the old separate 5fish/HDR choices; source SVT selection and literal-zero request have replacement coverage.
- `916` SVT fork batch preserves independent standalone and av1an build choices and per-build source drafts — Retire — the fork exposes one SVT encoder, not the old separate 5fish/HDR choices; source SVT selection and literal-zero request have replacement coverage.
- `948` x264 batch defaults and copied tracks become immutable reviewed H.264 queue requests — Covered — batch.spec.ts exercises x264 and source selection across both files; no separate review queue exists.
- `1086` filename templates preview reviewed tokens and edits invalidate the snapshot — Covered — source-encode-parity and source-shell verify source naming templates, reservations and frozen Run settings; no editable preview screen exists.
- `1124` x264 batch encoder switches restore source choices and common settings while av1an stays SVT — Retire — source has a shared codec selector rather than the old per-encoder inspector drafts; batch.spec.ts exercises x264 and AV1AN encoder selections.
- `1193` x264 batch keeps HDR failures visible while queueing only reviewed SDR requests — Native gap — source per-file failure isolation and HDR tone choices are covered in batch.spec.ts; x264 mixed-HDR execution remains separately unqualified.
- `1230` x264 switching away and back invalidates an outstanding batch preview — Retire — the fork has no explicit pre-run review to invalidate; batch.spec.ts verifies immutable settings after Run.
- `1261` x264 batch uses independent discovery gates when x264 is missing — Native gap — source batch no-video tool gating is covered; exact missing-x264 per-file execution is not independently qualified, and the fork has no separate HDR fork selector.
- `1261` x264 batch uses independent discovery gates when svt-av1-hdr is missing — Native gap — source batch no-video tool gating is covered; exact missing-x264 per-file execution is not independently qualified, and the fork has no separate HDR fork selector.
- `1274` folder import passes recursion and reports scan limits, skipped entries, and individual probe failures — Retire — fork Add Folder selects image sequences, not the former media-scan inspector; source-shell covers the folder/sequence flow and files.spec.ts covers failed probes.
- `1309` stopping a scan discards its late result — Retire — the source UI has no Stop import/scan control; source-shell covers the actual image-sequence picker.
- `1321` an old stopped probe cannot add files or finish a newer import — Retire — the source UI has no stopped-probe/import control; batch.spec.ts covers stale results for the source Run flow.
- `1342` batch defaults preserve original per-file stream indices and keep attachments last — Covered — batch.spec.ts checks per-file indices and container filtering; source order, rather than the old attachments-last rule, is authoritative.
- `1422` invalid preview rows remain visible and only ready requests enter one batch command — Covered — batch.spec.ts verifies a failed input is reported and the later input runs; the source has no review-row screen.
- `1442` draft edits discard late preview replies and require review again — Retire — source Run freezes submitted settings instead of discarding a review; batch.spec.ts tests that snapshot.
- `1463` removing a source invalidates a pending preview while common settings survive tab navigation — Covered — batch.spec.ts verifies a reimported file cannot inherit late encode completion; source has no review screen.
- `1480` duplicate clicks submit once and late queued replies do not regress terminal snapshots — Covered — batch.spec.ts verifies Run is disabled while preparing; source-encode-parity tests terminal snapshot and stop handling.
- `1500` a destination admission failure preserves selections and requires a fresh preview — Covered — batch.spec.ts verifies per-input preparation failure and later success; remux.spec.ts covers failure and retry without the old review screen.
- `1512` missing tools and invalid settings block batch preparation; defaults can be restored — Native gap — batch.spec.ts verifies nonvideo files run without video tools; exact missing-video-encoder batch error and reset coverage remains separately unqualified.
- `1535` a batch selects at most 100 imported episodes and the minimum-width layout stays within the window — Covered — source has no 100-file cap; batch.spec.ts and layout suites.
- `1575` browser sample remains available for inspection but cannot become a batch encode — Retire — the source browser preview begins without a sample file or desktop import; workspace.spec.ts covers that state.
- `1585` canceling a native folder picker restores import controls without scanning — Covered — batch.spec.ts verifies canceling the source folder picker leaves import usable without a scan.
- `1596` invalid CRF, empty destination, and empty selection block otherwise available preview — Retire — the old explicit preview, batch selection list and output picker are absent; source quality validity and source File List are tested elsewhere.
- `1617` a late batch reply clears submitted rows while preserving a newly imported selection — Covered — batch.spec.ts verifies an old completion cannot mark a removed and reimported file done.
- `1641` changing grain invalidates a late batch preview and preserves the submitted settings — Retire — explicit preview invalidation absent; source Run freezes settings in batch.spec.ts.
- `1641` changing fallback invalidates a late batch preview and preserves the submitted settings — Retire — explicit preview invalidation absent; source Run freezes settings in batch.spec.ts.
- `1641` changing backend invalidates a late batch preview and preserves the submitted settings — Retire — explicit preview invalidation absent; source Run freezes settings in batch.spec.ts.
- `1641` changing workers invalidates a late batch preview and preserves the submitted settings — Retire — explicit preview invalidation absent; source Run freezes settings in batch.spec.ts.
- `1694` batch validates grain and requires av1an only when selected — Covered — batch.spec.ts verifies source grain requests and encoder applicability; missing-tool behavior remains native qualification.
- `1711` batch rejects invalid parallel chunk counts before native preview — Retire — the fork has Workers, not the old parallel-chunk inspector; batch.spec.ts verifies the source worker setting in per-file requests.
- `1726` batch audio converts independently per source and queues reviewed immutable settings — Covered — batch.spec.ts frozen AAC settings and per-source channel scaling.
- `1763` batch audio codec changes discard an outstanding preview — Retire — explicit preview invalidation absent; source Run freezes audio in batch.spec.ts.
- `1763` batch audio bitrate changes discard an outstanding preview — Retire — explicit preview invalidation absent; source Run freezes audio in batch.spec.ts.
- `1763` batch audio channels changes discard an outstanding preview — Retire — explicit preview invalidation absent; source Run freezes audio in batch.spec.ts.
- `1763` batch audio selection changes discard an outstanding preview — Retire — explicit preview invalidation absent; source Run freezes audio in batch.spec.ts.
- `1795` batch audio drafts are isolated by encoder and workflow, with av1an conversion — Retire — independent old per-encoder inspector drafts are not source controls; source reset and separate Quick/AV1AN coverage remains.
- `1823` AV1AN batch applies saved audio conversion only to new source drafts — Covered — source-encode-parity verifies AV1AN audio preferences across reload; batch.spec.ts verifies audio per source.
- `1860` batch invalid audio blocks preview only for selected source tracks — Retire — source batch refreshes checked tracks and applies common audio settings, rather than the old per-file inspector selections; batch.spec.ts tests this behavior.
- `1884` batch queues reviewed FLAC and MP3 settings independently per file — Covered — batch.spec.ts verifies per-file FLAC, MP3 and Opus in both source workflows; separate review is absent.
- `1909` x265 batch uses encoder quality limits and snapshots reviewed codec requests — Covered — batch.spec.ts verifies Direct/AV1AN x265 and VP9 encoder and quality requests for each source.
- `1909` vp9 batch uses encoder quality limits and snapshots reviewed codec requests — Covered — batch.spec.ts verifies Direct/AV1AN x265 and VP9 encoder and quality requests for each source.
- `1945` batch exposes direct encoders and blocks unsupported NVENC two-pass and target-size requests — Retire — batch tests cover the retained CPU encoder choices; hardware video encoding is excluded by current scope.
- `1983` frame trim batch keeps per-source intervals and invalidates reviewed output after edits — Covered — batch.spec.ts verifies the source frame trim on each file; the separate review screen is absent.
- `2020` rate control batch propagates file-size targets and invalidates the reviewed request — Covered — batch.spec.ts checks immutable decimal target-size requests for each file; the source has no separate review screen.
- `2048` tone mapping batch keeps per-file HDR choices and invalidates its reviewed request — Covered — batch.spec.ts verifies the selected HDR tone curve for every input; the old per-file review screen is absent.
- `2113` av1an scene batch settings invalidate review and preserve queued target parameters — Covered — batch.spec.ts verifies frozen AV1AN source split, worker and target metric settings for each input.
- `2149` av1an x264 batch preserves explicit options and invalidates review when changed — Covered — parameters.spec.ts verifies AV1AN x264 Advanced grid values per batch input; no review screen exists.
- `2211` SVT av1an grain changes invalidate batch review and stay out of x264 requests — Covered — batch.spec.ts verifies AV1AN grain strength for SVT and its omission for x264.
- `2244` AV1AN custom filters invalidate batch preview and remain per encoder — Covered — batch.spec.ts verifies AV1AN custom filter submission and source-encode-parity verifies its editable grid.
- `2281` AV1AN batch restores infrastructure preferences and reset clears them — Covered — source-encode-parity verifies AV1AN audio/worker preference reload; source-shell tests the source Settings reset controls.

## encode.spec.ts

- `328` Quick Convert defaults to HDR with anime and mainline SVT as explicit choices — Retire — source exposes a single canonical SVT choice, not the former HDR/5fish selector; source-encode-parity tests its menus and defaults.
- `328` av1an defaults to HDR with anime and mainline SVT as explicit choices — Retire — source exposes a single canonical SVT choice, not the former HDR/5fish selector; source-encode-parity tests its menus and defaults.
- `355` crop resize and borders svtAv1Hdr requests retain immutable framing in job history — Retire — the fork exposes one SVT encoder, not the old separate 5fish/HDR choices; source SVT selection and literal-zero request have replacement coverage.
- `355` crop resize and borders svtAv1FiveFish requests retain immutable framing in job history — Retire — the fork exposes one SVT encoder, not the old separate 5fish/HDR choices; source SVT selection and literal-zero request have replacement coverage.
- `355` crop resize and borders svtAv1 requests retain immutable framing in job history — Covered — source-encode-parity checks transactional source framing, and batch.spec.ts checks immutable per-source geometry; old job-history display is absent.
- `355` crop resize and borders x264 requests retain immutable framing in job history — Covered — source-encode-parity checks transactional source framing, and batch.spec.ts checks immutable per-source geometry; old job-history display is absent.
- `398` exact resize and aspect-ratio borders send per-file intent and show the resulting frame — Covered — source-encode-parity exercises exact resize, SAR and aspect borders; batch.spec.ts checks per-source requests.
- `434` audio-only conversion excludes video and retains selected audio settings — Covered — remux.spec.ts verifies selected audio/subtitle-only requests without inventing video.
- `464` small sources, high frame rates and anamorphic border calculations are accepted — Native gap — source geometry and SAR requests are covered; the exact historical high-frame-rate/small-frame real-output combination has not been requalified.
- `493` fork resolution and ultrawide presets preserve their exact resizing intent — Covered — source-encode-parity exercises source resolution presets and aspect-ratio readouts.
- `520` portrait percentage scaling follows the fork short-axis rounding — Covered — encode.spec.ts checks source short-axis portrait percentage rounding.
- `543` rotated anamorphic framing crops coded pixels then shows rotated dimensions and scaling — Native gap — the fork exposes no rotation setting on VideoStream; analysis.spec.ts covers preview orientation, but the exact rotated crop/resize output has not been independently qualified.
- `571` HDR metadata overrides submit fixed-point values and preserve other defaults — Covered — source-advanced.spec.ts checks fixed-point parsing and batch.spec.ts checks Direct x265 HDR metadata requests.
- `607` FFmpeg x265 accepts HDR10 with unspecified limited range and authored mastering metadata — Retire — Quick exposes Direct x265, not the old FFmpeg x265 inspector route; native legacy route tests remain separate.
- `643` FFmpeg x265 does not imply HLG preservation — Retire — Quick exposes Direct x265, not the old FFmpeg x265 inspector route; native legacy route tests remain separate.
- `669` FFmpeg x265 rejects full-range HDR10 preservation — Retire — Quick exposes Direct x265, not the old FFmpeg x265 inspector route; native legacy route tests remain separate.
- `695` enabling resize on an anamorphic source corrects its display shape by default — Covered — source-encode-parity checks SAR-correct resize, opt-out and resulting request.
- `723` a source containing only audio opens directly in audio-only mode — Retire — the fork retains the selected codec instead of switching to an automatic audio-only UI; remux.spec.ts tests the resulting nonvideo request.
- `747` a source with no audio or video cannot start an audio-only conversion — Covered — remux.spec.ts tests empty track rejection and subtitle-only selection rather than assuming audio is required.
- `759` copy-video mode keeps the selected streams and sends keyframe-aligned trim settings — Covered — encode.spec.ts verifies keyframe-snapped copy-video trim and its retained end.
- `776` black borders toggle retains its draft and omits disabled borders from queued requests — Retire — source uses a border-ratio dropdown, not the old edge toggle; source-encode-parity and batch.spec.ts check actual ratio/border requests.
- `806` black borders reject invalid edges and final dimensions before start or queue — Retire — the old individual border-edge editor is absent; source ratio readouts and native framing validation remain covered separately.
- `843` legacy framing history remains readable when black borders are absent — Retire — source has no old Jesses history screen; runtime compatibility remains separately tested.
- `859` crop and resize invalid edits block both commands and selected video changes recompute dimensions — Covered — source-encode-parity tests crop/resize dialogs and batch.spec.ts isolates a crop that cannot fit a source.
- `907` crop resize and border drafts survive source, encoder and workflow switches with independent av1an framing — Retire — source does not expose the old per-encoder framing inspector; Quick/AV1AN source dialog and settings tests remain.
- `991` tool discovery shows all configured encoder tools as pending before the native reply — Retire — the old encoder tool-discovery inspector is absent; encode.spec.ts tests the source Run gate for unavailable required tools.
- `1019` SVT forks in Quick Convert use explicit defaults and preserve both variants in queue history — Retire — the fork exposes one SVT encoder, not the old separate 5fish/HDR choices; source SVT selection and literal-zero request have replacement coverage.
- `1155` SVT 5fish in Quick Convert validates both biases without applying them to other builds — Retire — the fork exposes one SVT encoder, not the old separate 5fish/HDR choices; source SVT selection and literal-zero request have replacement coverage.
- `1199` SVT fork svtAv1FiveFish in Quick Convert requires its own executable — Retire — the fork exposes one SVT encoder, not the old separate 5fish/HDR choices; source SVT selection and literal-zero request have replacement coverage.
- `1199` SVT fork svtAv1Hdr in Quick Convert requires its own executable — Retire — the fork exposes one SVT encoder, not the old separate 5fish/HDR choices; source SVT selection and literal-zero request have replacement coverage.
- `1019` SVT forks in av1an use explicit defaults and preserve both variants in queue history — Retire — the fork exposes one SVT encoder, not the old separate 5fish/HDR choices; source SVT selection and literal-zero request have replacement coverage.
- `1155` SVT 5fish in av1an validates both biases without applying them to other builds — Retire — the fork exposes one SVT encoder, not the old separate 5fish/HDR choices; source SVT selection and literal-zero request have replacement coverage.
- `1199` SVT fork svtAv1FiveFish in av1an requires its own executable — Retire — the fork exposes one SVT encoder, not the old separate 5fish/HDR choices; source SVT selection and literal-zero request have replacement coverage.
- `1199` SVT fork svtAv1Hdr in av1an requires its own executable — Retire — the fork exposes one SVT encoder, not the old separate 5fish/HDR choices; source SVT selection and literal-zero request have replacement coverage.
- `1226` SVT fork drafts stay separate between sources and workflows without guessing content — Retire — the fork exposes one SVT encoder, not the old separate 5fish/HDR choices; source SVT selection and literal-zero request have replacement coverage.
- `1279` SVT fork switch discards a deferred picker destination even after returning to the same build — Covered — encode.spec.ts checks stale path and error replies after source encoder switches in both source workflows.
- `1279` SVT fork switch discards a deferred picker error even after returning to the same build — Covered — encode.spec.ts checks stale path and error replies after source encoder switches in both source workflows.
- `1304` x264 uses its own defaults, preset names, validation, copied tracks, and immutable job settings — Covered — source-encode-parity checks x264 source defaults and presets; batch.spec.ts checks x264 requests and encode.spec.ts checks its tool gate.
- `1388` x264 and SVT restore independent drafts for each source and reset only the selected encoder — Retire — independent old per-encoder inspector drafts are not source controls; source reset and separate Quick/AV1AN coverage remains.
- `1468` x264 tool discovery is independent when x264 is missing — Covered — encode.spec.ts checks missing x264, and source-encode-parity verifies the single canonical SVT option; the old HDR fork selector is absent.
- `1468` x264 tool discovery is independent when svt-av1-hdr is missing — Covered — encode.spec.ts checks missing x264, and source-encode-parity verifies the single canonical SVT option; the old HDR fork selector is absent.
- `1488` x264 displays 8-bit SDR source depth without offering HDR controls — Retire — source does not expose the old source-depth inspector; source-encode-parity verifies codec color-format choices.
- `1488` x264 displays 10-bit SDR source depth without offering HDR controls — Retire — source does not expose the old source-depth inspector; source-encode-parity verifies codec color-format choices.
- `1505` x264 ignores an enqueue error after source replacement and keeps submission locked until settlement — Covered — encode.spec.ts verifies stale preparation errors after a source replacement and retains the in-flight Run lock.
- `1563` x264 blocks known HDR while SVT remains selectable — Native gap — source offers its codec choices and tone-map controls; exact old x264/HDR rejection semantics are not asserted as source behavior and need real-media qualification.
- `1582` x264 encoder switches and resets invalidate a deferred picker destination — Covered — encode.spec.ts checks stale path and error replies after encoder switches in both source workflows.
- `1582` x264 encoder switches and resets invalidate a deferred picker error — Covered — encode.spec.ts checks stale path and error replies after encoder switches in both source workflows.
- `1629` loudness stays read-only until explicit apply and freezes measured gain in a job — Retire — the fork chooses a normalization target and measures during execution, without the old manual loudness inspector; native normalization tests remain.
- `1665` loudness cancellation discards a late result and channel changes clear measured gain — Retire — the fork chooses a normalization target and measures during execution, without the old manual loudness inspector; native normalization tests remain.
- `1741` encode submits the selected video, quality, preset, copied tracks, and native destination — Covered — source-encode-parity verifies selected video/quality/preset/destination and source-zero-quality checks literal zero.
- `1807` encode draft survives navigation and Reset settings restores defaults — Covered — source-encode-parity and source-shell cover source settings navigation/reset; no old inspector Reset button exists.
- `1837` encoding requires ffmpeg — Covered — encode.spec.ts tests missing ffmpeg, ffprobe and canonical SVT; the old separately named HDR fork is absent.
- `1837` encoding requires ffprobe — Covered — encode.spec.ts tests missing ffmpeg, ffprobe and canonical SVT; the old separately named HDR fork is absent.
- `1837` encoding requires svt-av1-hdr — Covered — encode.spec.ts tests missing ffmpeg, ffprobe and canonical SVT; the old separately named HDR fork is absent.
- `1845` invalid quality and blank output block encoding; audio-only sources use audio mode — Covered — encode.spec.ts tests invalid quality and retry; remux.spec.ts covers source-selected nonvideo and empty tracks.
- `1888` backend compatibility errors remain visible and allow correcting the draft — Covered — remux.spec.ts checks a failed preparation and corrected retry through source Run.
- `1904` encode cancellation and runtime errors follow authoritative job snapshots — Covered — remux.spec.ts verifies source footer Stop and authoritative terminal status.
- `1931` late encode start replies cannot replace a completed channel state — Native gap — the old per-job queue/history surface is absent; source terminal/Stop and shared subscription tests remain, but these exact late reply permutations need independent native proof.
- `1931` late encode cancel replies cannot replace a completed channel state — Native gap — the old per-job queue/history surface is absent; source terminal/Stop and shared subscription tests remain, but these exact late reply permutations need independent native proof.
- `1931` late encode enqueue replies cannot replace a completed channel state — Native gap — the old per-job queue/history surface is absent; source terminal/Stop and shared subscription tests remain, but these exact late reply permutations need independent native proof.
- `1949` reconnected encode jobs distinguish source scans, encoding, and output validation — Retire — the fork has one current-action footer, not the old expanded phase/history inspector; source footer behavior is tested separately.
- `2006` validation stays indeterminate when source duration is unknown and supports cancellation — Retire — the old separate validation-phase/ETA inspector is absent; source progress and cancellation remain covered separately.
- `2032` phase estimates use observed time, age during a stall, and reset before output validation — Retire — the old phase-estimate inspector is absent from the source UI.
- `2082` unknown duration shows estimated speed alone and canceling clears it — Retire — the old per-job speed/ETA inspector is absent from the source UI.
- `2105` encodes can be queued for different sources while another job is running — Retire — fork has no manual queue/history panel; source sequential Run and cancellation tests remain.
- `2215` canceling a queued entry preserves the active job and Stop queue ends all pending work — Retire — fork has no manual queue/history panel; source sequential Run and cancellation tests remain.
- `2238` interrupted history is terminal and never resumes automatically — Retire — fork has no manual queue/history panel; source sequential Run and cancellation tests remain.
- `2250` Quick Convert exposes job storage errors and reconnect preserves the source — Native gap — the source has no old history panel; real job-storage failure/reconnect behavior remains independently unqualified.
- `2250` av1an exposes job storage errors and reconnect preserves the source — Native gap — the source has no old history panel; real job-storage failure/reconnect behavior remains independently unqualified.
- `2265` av1an tab settings are explicit, immutable in queued jobs, and reset safely — Covered — batch.spec.ts freezes source AV1AN worker/split/filter settings, and source-encode-parity tests preference reload.
- `2331` Quick Convert validates grain and stays available without av1an installed — Covered — batch.spec.ts verifies source SVT grain and encoder applicability; encode.spec.ts tests required source tools.
- `2357` av1an accepts only whole worker counts in range — Covered — encode.spec.ts tests source AV1AN worker validation and retry.
- `2370` the dedicated av1an tab requires ffmpeg — Covered — encode.spec.ts tests AV1AN ffmpeg, ffprobe, canonical SVT and AV1AN gating; the old HDR fork selector is absent.
- `2370` the dedicated av1an tab requires ffprobe — Covered — encode.spec.ts tests AV1AN ffmpeg, ffprobe, canonical SVT and AV1AN gating; the old HDR fork selector is absent.
- `2370` the dedicated av1an tab requires svt-av1-hdr — Covered — encode.spec.ts tests AV1AN ffmpeg, ffprobe, canonical SVT and AV1AN gating; the old HDR fork selector is absent.
- `2370` the dedicated av1an tab requires av1an — Covered — encode.spec.ts tests AV1AN ffmpeg, ffprobe, canonical SVT and AV1AN gating; the old HDR fork selector is absent.
- `2386` standalone and av1an drafts stay independent across navigation and reset — Retire — old per-encoder inspector drafts are not source controls; source-encode-parity checks separate source workflows and reset behavior.
- `2449` each encoder restores its own source draft and resets only that source — Retire — independent old per-encoder inspector drafts are not source controls; source reset and separate Quick/AV1AN coverage remains.
- `2523` standalone and av1an submit fixed backends into one shared queue and history — Retire — fork has no manual queue/history panel; source sequential Run and cancellation tests remain.
- `2599` Quick Convert discards a deferred enqueue error after changing source without unlocking duplicate submission — Covered — encode.spec.ts tests stale preparation failure after replacing the active source while Run stays locked.
- `2652` Quick Convert ignores a deferred picker destination after source changes or reset — Covered — encode.spec.ts tests stale picker path/error replies after source encoder changes.
- `2652` Quick Convert ignores a deferred picker error after source changes or reset — Covered — encode.spec.ts tests stale picker path/error replies after source encoder changes.
- `2599` av1an discards a deferred enqueue error after changing source without unlocking duplicate submission — Covered — encode.spec.ts tests stale preparation failure after replacing the active source while Run stays locked.
- `2652` av1an ignores a deferred picker destination after source changes or reset — Covered — encode.spec.ts tests stale picker path/error replies after source encoder changes.
- `2652` av1an ignores a deferred picker error after source changes or reset — Covered — encode.spec.ts tests stale picker path/error replies after source encoder changes.
- `2724` per-track audio settings retain source details and queue independent Opus, AAC, and copied tracks — Covered — external-tracks per-source colliding-index audio settings.
- `2766` audio bitrate validation applies only to included converted tracks and reselect restores the draft — Covered — source-encode-parity tests the actual numeric per-track source audio configuration and remux.spec.ts tests checked track selection.
- `2794` audio drafts are isolated by source and video encoder and workflow with av1an conversion — Retire — the old independent per-encoder audio inspector drafts are absent; source-encode-parity tests source per-track audio and AV1AN preference behavior.
- `2854` AV1AN audio defaults carry safe conversion choices to new sources without gain or stream IDs — Covered — source-encode-parity checks AV1AN audio preference reload, and encode.spec.ts verifies current per-track codec requests.
- `2912` legacy job history remains readable without audio settings — Retire — source has no old Jesses history screen; runtime compatibility remains separately tested.
- `2926` AAC bitrate limits follow 8000 Hz source audio and selected channels — Retire — the fork exposes numeric source bitrate 8–8192 rather than the old sample-rate-dependent UI caps; native codec validation remains separate.
- `2926` AAC bitrate limits follow 44100 Hz source audio and selected channels — Retire — the fork exposes numeric source bitrate 8–8192 rather than the old sample-rate-dependent UI caps; native codec validation remains separate.
- `2958` Opus mono bitrate respects the encoder limit while stereo allows higher rates — Retire — the source numeric bitrate control does not impose the former mono/stereo-specific inspector cap; native codec validation remains separate.
- `2986` Quick Convert queues explicit flac audio settings — Covered — encode.spec.ts submits every source audio codec in Quick and AV1AN, including FLAC, MP3, Vorbis and EAC3.
- `2986` Quick Convert queues explicit mp3 audio settings — Covered — encode.spec.ts submits every source audio codec in Quick and AV1AN, including FLAC, MP3, Vorbis and EAC3.
- `2986` Quick Convert queues explicit vorbis audio settings — Covered — encode.spec.ts submits every source audio codec in Quick and AV1AN, including FLAC, MP3, Vorbis and EAC3.
- `2986` Quick Convert queues explicit eac3 audio settings — Covered — encode.spec.ts submits every source audio codec in Quick and AV1AN, including FLAC, MP3, Vorbis and EAC3.
- `2986` av1an queues explicit flac audio settings — Covered — encode.spec.ts submits every source audio codec in Quick and AV1AN, including FLAC, MP3, Vorbis and EAC3.
- `2986` av1an queues explicit mp3 audio settings — Covered — encode.spec.ts submits every source audio codec in Quick and AV1AN, including FLAC, MP3, Vorbis and EAC3.
- `2986` av1an queues explicit vorbis audio settings — Covered — encode.spec.ts submits every source audio codec in Quick and AV1AN, including FLAC, MP3, Vorbis and EAC3.
- `2986` av1an queues explicit eac3 audio settings — Covered — encode.spec.ts submits every source audio codec in Quick and AV1AN, including FLAC, MP3, Vorbis and EAC3.
- `3015` audio layout restrictions require an explicit downmix without changing the source — Native gap — numeric channels and source layout selection have frontend and audio-runtime coverage; exact uncommon source-layout conversion remains provider-dependent.
- `3043` av1an queues explicit 5.1 and 7.1 audio layouts within codec limits — Covered — encode.spec.ts checks source AV1AN 5.1/7.1 channel settings and scaled bitrate.
- `3072` x265 uses FFmpeg with independent defaults and immutable source-depth queue settings — Retire — Quick exposes Direct x265, not the old FFmpeg x265 inspector route; native legacy route tests remain separate.
- `3118` x265 rejects known HDR without implying a depth conversion — Native gap — source HDR tone curves are tested, but this old encoder-specific HDR rejection assertion is not a source UI contract and real outputs remain separately qualified.
- `3072` vp9 uses FFmpeg with independent defaults and immutable source-depth queue settings — Retire — source Quick/AV1AN use Direct VP9, not the old FFmpeg VP9 inspector; source encoder choices and requests are tested.
- `3118` vp9 rejects known HDR without implying a depth conversion — Native gap — source HDR tone curves are tested, but this old encoder-specific HDR rejection assertion is not a source UI contract and real outputs remain separately qualified.
- `3139` frame trim validates boundaries and copied audio, restores drafts and keeps queued interval immutable — Covered for the source flow — active `encode.spec.ts` submits copied audio with encoded-video frame trim, with Linux real-media evidence in [source copied trim](source-copied-trim.md). The historical pending status is superseded.
- `3179` rate control validates bitrate and target size, restores encoder drafts and preserves queued settings — Covered — source-encode-parity checks decimal target-size and batch.spec.ts tests its immutable per-file requests.
- `3228` tone mapping requires explicit HDR rendering, validates peak and retains queued settings — Covered — external-tracks.spec.ts and batch.spec.ts check source tone curves; the old peak-setting inspector is absent.
- `3290` av1an scene and VMAF controls validate, restore drafts and freeze queued settings — Covered — encode.spec.ts checks workers/segment, batch.spec.ts checks source split/workers/target snapshots, and source-zero-quality tests metric scores.
- `3330` FFmpeg segment reader explains keyframe splitting and reaches the AV1AN request — Covered — encode.spec.ts submits the source AV1AN segment reader; no extra keyframe-help inspector exists.
- `3348` av1an x264 validates independent chunk and probe limits and freezes explicit options — Retire — the old independent chunk/probe controls are absent; source worker/split/Advanced options are exercised by encode.spec.ts and batch.spec.ts.
- `3421` av1an HDR quality targeting becomes available after explicit SDR tone mapping — Covered — encode.spec.ts checks source target gating on HDR and selected SDR tone mapping.
- `3464` av1an memory guidance applies suggested parallelism to the queued request — Retire — the source has no old memory recommendation/apply control.
- `3498` av1an trim is available and requires converted or excluded audio — Open source gap — copied-audio trim is tracked in the contract ledger; AV1AN packet behavior also needs native qualification.
- `3519` SVT av1an grain draft is independent from x264 and saved in queued settings — Covered — batch.spec.ts checks SVT grain and verifies it is omitted for x264.
- `3546` an AV1AN grain table replaces SVT numeric grain strength in the saved request — Covered — source-encode-parity verifies source Advanced grain-table precedence and emitted table contents.
- `3566` a grain table picked for an old SVT draft cannot change another build — Retire — separate SVT fork draft pickers are absent; source picker staleness and grain-table precedence are tested.
- `3587` AV1AN custom filter rows preserve newline editing and freeze in the request — Covered — batch.spec.ts checks frozen source custom filters; source-encode-parity checks the source editable grid.
- `3617` AV1AN restores safe infrastructure preferences across sessions without picture choices — Covered — source-encode-parity tests AV1AN audio/worker settings across reload.
- `3667` AV1AN ignores malformed stored counts and path-bearing filter rows — Native gap — the old filter/path preference editor is absent; malformed persisted source option recovery is not independently requalified.
- `3694` av1an perceptual metric direction and reader dependencies keep immutable queued settings — Covered — source-zero-quality tests all source target metrics/scores; source-advanced tests source-reader requirements and source default mode.
- `3755` weighted XPSNR target enforces its half-point score range — Covered — source-encode-parity verifies the range and step; source-zero-quality verifies the target request.
- `3782` extended SVT quality and dedicated lossless mode freeze their explicit wire settings — Covered — source-zero-quality checks zero separately from explicit native lossless; real SVT provider qualification is tracked separately.
- `3818` standalone binary routes and NVENC expose distinct names, tools, suffixes and rate gates — CPU portion covered — source-encode-parity verifies CPU codec choices/defaults and encode.spec.ts checks source tool gating; NVENC is excluded.
- `3857` NVENC constant QP submits typed quality rather than CRF — Excluded — hardware video encoders are outside the current CPU-only scope.
- `3877` Quick Convert binds video metadata to the mapped source and blocks removed sources — Covered — mux.spec.ts checks source-scoped video/metadata identities and surviving metadata edits; AV1AN external source behavior has separate native tests.
- `3877` av1an binds video metadata to the mapped source and blocks removed sources — Covered — mux.spec.ts checks source-scoped video/metadata identities and surviving metadata edits; AV1AN external source behavior has separate native tests.
