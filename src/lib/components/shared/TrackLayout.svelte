<script lang="ts">
  import type { EncodeTrackOverride, EncodeTrackRef, MediaFile } from '$lib/ipc/generated';
  import type { ExternalTrackChoice } from './external-tracks';
  import TrackTags from './TrackTags.svelte';
  import { isMovTimecode, movTimecodeChoice, type MovTimecodeChoice } from './mov-timecode';
  import {
    effectiveTrackOrder,
    sourceDonor,
    type SourceDonor,
    type TrackTags as TagValues,
  } from './track-layout';

  let {
    files,
    primary,
    defaults,
    order,
    overrides,
    external,
    metadataDonor,
    chaptersDonor,
    movTimecode,
    outputIsMov,
    issue,
    disabled,
    onorder,
    onprimarytags,
    onexternaltags,
    onmetadata,
    onchapters,
    ontimecode,
  }: {
    files: MediaFile[];
    primary: MediaFile | undefined;
    defaults: { media: EncodeTrackRef[]; attachments: EncodeTrackRef[] };
    order: EncodeTrackRef[];
    overrides: EncodeTrackOverride[];
    external: ExternalTrackChoice[];
    metadataDonor: SourceDonor | undefined;
    chaptersDonor: SourceDonor | undefined;
    movTimecode: MovTimecodeChoice | undefined;
    outputIsMov: boolean;
    issue: string | null;
    disabled: boolean;
    onorder: (value: EncodeTrackRef[]) => void;
    onprimarytags: (streamIndex: number, value: TagValues) => void;
    onexternaltags: (inputPath: string, streamIndex: number, value: TagValues) => void;
    onmetadata: (value: SourceDonor | undefined) => void;
    onchapters: (value: SourceDonor | undefined) => void;
    ontimecode: (value: MovTimecodeChoice | undefined) => void;
  } = $props();
  const arranged = $derived(effectiveTrackOrder(order, defaults));
  const mediaCount = $derived(defaults.media.length);
  const realFiles = $derived(files.filter((file) => !file.id.startsWith('jesses-synthetic')));
  const timecodes = $derived(
    realFiles.flatMap((file) =>
      file.streams.filter(isMovTimecode).map((stream) => ({ file, stream })),
    ),
  );

  function donor(value: string): SourceDonor | undefined {
    const file = realFiles.find((entry) => entry.id === value);
    return file && file.path !== primary?.path ? sourceDonor(file) : undefined;
  }

  function move(index: number, direction: -1 | 1) {
    const next = arranged.map((ref) => ({ ...ref }));
    const other = index + direction;
    [next[index], next[other]] = [next[other], next[index]];
    onorder(next);
  }

  function chooseTimecode(value: string) {
    if (!value) {
      ontimecode(undefined);
      return;
    }
    const option = timecodes.find(
      ({ file, stream }) => JSON.stringify([file.id, stream.index]) === value,
    );
    if (option) ontimecode(movTimecodeChoice(option.file, option.stream));
  }
</script>

<details class="track-layout">
  <summary>Output track order, labels & source data</summary>
  <div class="layout-content">
    <p>
      Choose where container metadata and chapters come from. The video always uses the encoding
      source.
    </p>
    <div class="donors">
      <label
        >Container metadata source<select
          value={metadataDonor?.invalidated ? '__stale__' : (metadataDonor?.sourceId ?? '')}
          {disabled}
          onchange={(event) => onmetadata(donor(event.currentTarget.value))}
        >
          <option value="">Video source (default)</option>
          {#each realFiles as file (file.id)}<option value={file.id}>{file.name}</option>{/each}
          {#if metadataDonor?.invalidated}<option value="__stale__"
              >Removed or changed source</option
            >{/if}
        </select></label
      >
      <label
        >Chapters source<select
          value={chaptersDonor?.invalidated ? '__stale__' : (chaptersDonor?.sourceId ?? '')}
          {disabled}
          onchange={(event) => onchapters(donor(event.currentTarget.value))}
        >
          <option value="">Video source (default)</option>
          {#each realFiles as file (file.id)}<option value={file.id}>{file.name}</option>{/each}
          {#if chaptersDonor?.invalidated}<option value="__stale__"
              >Removed or changed source</option
            >{/if}
        </select></label
      >
    </div>
    {#if (outputIsMov && timecodes.length) || movTimecode}
      <label class="timecode-field"
        >MOV timecode track<select
          value={movTimecode?.invalidated || (movTimecode && !outputIsMov)
            ? '__stale__'
            : movTimecode
              ? JSON.stringify([movTimecode.sourceId, movTimecode.streamIndex])
              : ''}
          {disabled}
          onchange={(event) => chooseTimecode(event.currentTarget.value)}
        >
          <option value="">No copied timecode</option>
          {#if outputIsMov}
            {#each timecodes as { file, stream } (JSON.stringify([file.id, stream.index]))}
              <option value={JSON.stringify([file.id, stream.index])}
                >{file.name} · stream #{stream.index}</option
              >
            {/each}
          {/if}
          {#if movTimecode?.invalidated || !outputIsMov}<option value="__stale__"
              >Selected timecode needs attention</option
            >{/if}
        </select><small
          >Copies a genuine tmcd data track into MOV output. The full video and its frame cadence
          must stay unchanged.</small
        ></label
      >
    {/if}
    <p>
      Move media tracks in output order. Attachments stay after media and can be reordered there.
      Open a track to change its labels or flags.
    </p>
    <div class="track-list">
      {#each arranged as ref, index (JSON.stringify(ref))}
        {@const source = ref.inputPath
          ? files.find((file) => file.path === ref.inputPath)
          : primary}
        {@const stream = source?.streams.find((entry) => entry.index === ref.streamIndex)}
        {@const tags = ref.inputPath
          ? external.find(
              (choice) =>
                choice.inputPath === ref.inputPath && choice.streamIndex === ref.streamIndex,
            )
          : overrides.find((choice) => choice.streamIndex === ref.streamIndex)}
        <div
          class="track-row"
          role="group"
          aria-label={`Output track ${source?.name ?? 'unavailable source'} stream #${ref.streamIndex}`}
        >
          <div class="track-line">
            <span>
              <strong>{source?.name ?? ref.inputPath ?? 'Video source'}</strong> · #{ref.streamIndex}
              ·
              {stream?.kind ?? 'unavailable'} · {stream?.codec ?? 'unknown codec'}
            </span>
            <div class="move-actions">
              <button
                type="button"
                disabled={disabled || index === 0 || index === mediaCount}
                aria-label={`Move ${source?.name ?? 'source'} stream #${ref.streamIndex} up`}
                onclick={() => move(index, -1)}>↑</button
              >
              <button
                type="button"
                disabled={disabled || index === arranged.length - 1 || index === mediaCount - 1}
                aria-label={`Move ${source?.name ?? 'source'} stream #${ref.streamIndex} down`}
                onclick={() => move(index, 1)}>↓</button
              >
            </div>
          </div>
          {#if stream && ['video', 'audio', 'subtitle'].includes(stream.kind)}
            <details class="track-tags">
              <summary>Labels & flags</summary>
              <TrackTags
                {stream}
                value={tags ?? {}}
                {disabled}
                onchange={(next) =>
                  ref.inputPath
                    ? onexternaltags(ref.inputPath, ref.streamIndex, next)
                    : onprimarytags(ref.streamIndex, next)}
              />
            </details>
          {/if}
        </div>
      {:else}
        <p>Choose a video source to arrange output tracks.</p>
      {/each}
    </div>
    {#if issue}<p class="issue" role="alert">{issue}</p>{/if}
  </div>
</details>

<style>
  .track-layout {
    margin: 2px 14px 14px;
    border: 1px solid var(--border);
    border-radius: 8px;
    min-width: 0;
  }
  summary {
    cursor: pointer;
    padding: 10px 12px;
    font-size: 12px;
    font-weight: 700;
  }
  .layout-content {
    display: grid;
    gap: 10px;
    max-height: 360px;
    overflow-y: auto;
    padding: 0 12px 12px;
  }
  p {
    margin: 0;
    color: var(--muted-foreground);
    font-size: 11px;
    line-height: 1.45;
  }
  .donors {
    display: grid;
    grid-template-columns: repeat(auto-fit, minmax(min(100%, 12rem), 1fr));
    gap: 8px;
  }
  .donors label {
    display: grid;
    gap: 4px;
    font-size: 11px;
    font-weight: 600;
  }
  .timecode-field {
    display: grid;
    gap: 4px;
    max-width: 360px;
    font-size: 11px;
    font-weight: 600;
  }
  .timecode-field small {
    color: var(--muted-foreground);
    font-size: 10px;
    font-weight: 400;
  }
  select {
    min-width: 0;
    width: 100%;
    padding: 6px 8px;
    border: 1px solid var(--border);
    border-radius: 4px;
    background: var(--background);
    color: var(--foreground);
    font: inherit;
  }
  .track-list {
    display: grid;
    gap: 5px;
  }
  .track-row {
    border-top: 1px solid var(--border);
    min-width: 0;
  }
  .track-line {
    display: flex;
    align-items: center;
    justify-content: space-between;
    gap: 8px;
    padding-top: 5px;
    font-size: 11px;
    overflow-wrap: anywhere;
  }
  .move-actions {
    display: flex;
    flex: 0 0 auto;
    gap: 4px;
  }
  .move-actions button {
    min-width: 24px;
    border: 1px solid var(--border);
    border-radius: 4px;
    background: var(--background);
    cursor: pointer;
  }
  .track-tags summary {
    padding: 4px 0 6px;
    color: var(--muted-foreground);
    font-size: 10px;
    font-weight: 500;
  }
  .issue {
    color: #a3362f;
  }
</style>
