# General preferences and recent-media validation

Local Windows x64 qualification on 2026-09-13. General preferences use a bounded,
versioned JSON record in the selected installed or portable configuration folder.
Writes flush a new same-directory file and atomically replace the last record.
Read failures preserve the existing file and block further writes.

Six runtime gates pass: persisted round trip, ordered/deduplicated recent paths,
general edits that retain concurrent recent history, corrupt-file preservation,
failed-write cleanup, bounded input, read-only whitelist import, and concurrent
parameter/general/recent changes with independent configuration roots. A selected
JSON import can supply only `DefaultOutputDir` and the string-encoded `RecentFiles`
array. Unknown keys and invalid platform paths are reported or skipped; encoder
arguments, media drafts, executable paths and automatic resume are excluded.

Four browser gates pass: preferences survive reload, recent media reopens, newly
created encode destinations use the configured directory, existing drafts retain
their destination, and unavailable recent entries stay in history. Import writes
nothing until Apply. Reopening an already loaded entry selects its existing
inspector without another probe or duplicate. Clear recent media removes only
the saved list. Clearing recents and unrelated preference replies preserve
unsaved general edits until Save or explicit import Apply.

Recent lists retain up to 15 entries and are not probed during startup or menu
rendering. The selected entry is inspected only after Open recent. Media imports
remain cancellable and late results do not change the source list. Sources,
existing encode drafts and queued jobs are not modified by a preference import.
A frozen Windows portable desktop build also saved a separate default output
folder, restarted, and used that saved folder for a newly imported source's
encode draft. Successful import populated the portable recent-media record.
The retained configuration has version 1 and revision 2; native screenshots and
the resulting encode receipt are under `Videos/Jesses-complete-native-20260913`.
The same frozen build passed native import Review and Apply with five recent
entries and three unsupported keys skipped. Review left the persisted record
unchanged; Apply updated only the supported general fields. The import JSON's
SHA-256 stayed unchanged. Opening a missing recent file showed an actionable
error and retained the entry. Reopening an already loaded valid file correctly
avoided a duplicate, but kept the previously selected inspector source; the
working source now selects the requested existing entry. Evidence includes `native-preferences-import-review.png`,
`native-recent-missing.png`, and the before/after preference records in the same
Videos evidence directory. Linux and final-artifact qualification remain pending.

The subsequent native portable build rechecked the existing-entry correction:
with two sources already loaded, Open recent switched to the requested existing
inspector and retained exactly two rows. After a normal application close and
restart, the saved recent source reopened; selecting x264 and applying the saved
parameter preset restored `ref=3` and `bframes=4`. This exercises the native
Rust-owned preference record across process restart, separately from transient
media drafts. Both native application processes closed normally. Evidence:
`native-existing-recent-selection.png` and `native-preset-restart-applied.png`
under `Videos/Jesses-final-native-20260913`. These checks supersede the earlier
pending existing-entry recheck; final package/clean-machine and Linux gates
remain open.
