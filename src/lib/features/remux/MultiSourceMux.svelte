<script lang="ts">
  import ContainerOptions from '$lib/components/shared/ContainerOptions.svelte';
  import {
    destinationContainer,
    containerDestination,
  } from '$lib/components/shared/container-options';
  import { untrack } from 'svelte';
  import { ArrowDown, ArrowUp, Play } from '@lucide/svelte';
  import { Button } from '$lib/components/ui/button';
  import { chooseRemuxDestination } from '$lib/ipc/client';
  import { errorMessage } from '$lib/components/shared/format';
  import type { MediaFile, MediaStream, MuxRequest } from '$lib/ipc/generated';

  let {
    files,
    primaryId,
    disabled,
    onstart,
  }: {
    files: MediaFile[];
    primaryId: string | undefined;
    disabled: boolean;
    onstart: (request: MuxRequest) => Promise<void>;
  } = $props();

  type TrackDraft = {
    key: string;
    sourceId: string;
    stream: MediaStream;
    included: boolean;
    title: string;
    language: string;
    defaultFlag: string;
    forcedFlag: string;
  };
  let sourceIds = $state<string[]>([]);
  let tracks = $state<TrackDraft[]>([]);
  let metadataId = $state('');
  let chaptersId = $state('');
  let destination = $state('');
  let initialized = false;
  let submitting = $state(false);
  let error = $state<string | null>(null);
  const available = $derived(files.filter((file) => !file.id.startsWith('jesses-synthetic')));
  const sources = $derived(sourceIds.flatMap((id) => available.filter((file) => file.id === id)));
  const included = $derived(tracks.filter((track) => track.included));
  const locked = $derived(disabled || submitting);
  const validation = $derived(
    sources.length === 0
      ? 'Choose a source file.'
      : sources.length > 32
        ? 'Choose at most 32 source files.'
        : included.length > 256
          ? 'Choose at most 256 tracks.'
          : !included.some((track) => ['video', 'audio'].includes(track.stream.kind))
            ? 'Select at least one video or audio track.'
            : included.some(
                  (track) =>
                    !['video', 'audio', 'subtitle', 'attachment'].includes(track.stream.kind),
                )
              ? 'Deselect unsupported stream types.'
              : included.some(
                    (track) =>
                      track.language !== (track.stream.language ?? '') &&
                      !/^(?:[a-zA-Z]{3})?$/.test(track.language),
                  )
                ? 'Use a three-letter language code, or clear the language.'
                : !destination.trim()
                  ? 'Choose a new output destination.'
                  : null,
  );

  function reconcile(next: string[]) {
    sourceIds = next;
    const previous = new Map(tracks.map((track) => [track.key, track]));
    const candidates = next.flatMap((id) => {
      const file = available.find((source) => source.id === id);
      return (
        file?.streams.map((stream): TrackDraft => {
          const key = JSON.stringify([id, stream.index]);
          return (
            previous.get(key) ?? {
              key,
              sourceId: id,
              stream,
              included: stream.kind !== 'data',
              title: stream.title ?? '',
              language: stream.language ?? '',
              defaultFlag: '',
              forcedFlag: '',
            }
          );
        }) ?? []
      );
    });
    const keys = new Set(candidates.map((track) => track.key));
    const kept = tracks.filter((track) => keys.has(track.key));
    const additions = candidates.filter((track) => !previous.has(track.key));
    const combined = [...kept, ...additions];
    tracks = [
      ...combined.filter((track) => track.stream.kind !== 'attachment'),
      ...combined.filter((track) => track.stream.kind === 'attachment'),
    ];
    if (!next.includes(metadataId)) metadataId = next[0] ?? '';
    if (chaptersId && !next.includes(chaptersId)) chaptersId = next[0] ?? '';
    error = null;
  }

  $effect(() => {
    const current = available;
    const primary = primaryId;
    untrack(() => {
      if (!initialized && current.length) {
        const first = current.find((file) => file.id === primary) ?? current[0];
        initialized = true;
        chaptersId = first.id;
        destination = first.path.replace(/\.[^./\\]+$/, '') + '_mux.mkv';
        reconcile([first.id]);
      } else {
        reconcile(sourceIds.filter((id) => current.some((file) => file.id === id)));
      }
    });
  });

  function toggleSource(id: string) {
    reconcile(
      sourceIds.includes(id) ? sourceIds.filter((source) => source !== id) : [...sourceIds, id],
    );
  }
  function canMove(position: number, offset: number) {
    const next = tracks[position + offset];
    return (
      !!next &&
      (next.stream.kind === 'attachment') === (tracks[position].stream.kind === 'attachment')
    );
  }
  function move(position: number, offset: number) {
    if (!canMove(position, offset)) return;
    const next = [...tracks];
    [next[position], next[position + offset]] = [next[position + offset], next[position]];
    tracks = next;
  }
  function sourceName(id: string) {
    return sources.find((file) => file.id === id)?.name ?? id;
  }
  async function chooseOutput() {
    try {
      const selected = await chooseRemuxDestination(destination);
      if (selected) destination = selected;
    } catch (cause) {
      error = errorMessage(cause);
    }
  }
  async function start() {
    if (locked || validation) return;
    submitting = true;
    error = null;
    const request: MuxRequest = {
      sources: sources.map((source) => ({ id: source.id, inputPath: source.path })),
      tracks: included.map((track) => ({
        sourceId: track.sourceId,
        streamIndex: track.stream.index,
        title: track.title === (track.stream.title ?? '') ? null : track.title,
        language:
          track.language === (track.stream.language ?? '') ? null : track.language.toLowerCase(),
        default: track.defaultFlag === '' ? null : track.defaultFlag === 'yes',
        forced: track.forcedFlag === '' ? null : track.forcedFlag === 'yes',
      })),
      metadataSourceId: metadataId,
      chaptersSourceId: chaptersId || null,
      outputPath: destination.trim(),
    };
    try {
      await onstart(request);
    } catch (cause) {
      error = errorMessage(cause);
    } finally {
      submitting = false;
    }
  }
</script>

<section class="mux-workspace" aria-label="Combine source tracks">
  <div class="panel mux-panel">
    <h2>Source files</h2>
    <p class="small-muted">
      Choose imported files, then arrange the tracks below. Sources stay read-only.
    </p>
    <div class="sources">
      {#each available as file (file.id)}
        <label
          ><input
            type="checkbox"
            aria-label={`Use source ${file.name}`}
            checked={sourceIds.includes(file.id)}
            disabled={locked}
            onchange={() => toggleSource(file.id)}
          /><span>{file.name}<small>{file.path}</small></span></label
        >
      {/each}
      {#if !available.length}<p class="small-muted">Add local media from the Files tab.</p>{/if}
    </div>
    <div class="owners">
      <div class="field">
        <label for="mux-metadata-source">Container metadata from</label><select
          id="mux-metadata-source"
          bind:value={metadataId}
          disabled={locked || !sources.length}
          >{#each sources as source (source.id)}<option value={source.id}>{source.name}</option
            >{/each}</select
        >
      </div>
      <div class="field">
        <label for="mux-chapter-source">Chapters from</label><select
          id="mux-chapter-source"
          bind:value={chaptersId}
          disabled={locked || !sources.length}
          ><option value="">No chapters</option>{#each sources as source (source.id)}<option
              value={source.id}>{source.name}</option
            >{/each}</select
        >
      </div>
    </div>
  </div>

  <div class="panel mux-panel">
    <h2>Output track order</h2>
    <p class="small-muted">
      Unchanged fields preserve source metadata. Clear a title or language to remove it. Attachments
      follow media tracks.
    </p>
    {#each tracks as track, position (track.key)}
      {@const label = `${sourceName(track.sourceId)} #${track.stream.index}`}
      <div class="track" aria-label={`Track ${label}`}>
        <div class="track-heading">
          <label
            ><input
              type="checkbox"
              aria-label={`Include ${label}`}
              bind:checked={track.included}
              disabled={locked}
            /><span
              ><strong>{sourceName(track.sourceId)} · #{track.stream.index}</strong><small
                >{track.stream.kind} · {track.stream.codec ?? 'Unknown codec'}</small
              ></span
            ></label
          >
          <div class="track-order">
            <button
              class="icon-button"
              aria-label={`Move ${label} up`}
              disabled={locked || !canMove(position, -1)}
              onclick={() => move(position, -1)}><ArrowUp size={14} /></button
            ><button
              class="icon-button"
              aria-label={`Move ${label} down`}
              disabled={locked || !canMove(position, 1)}
              onclick={() => move(position, 1)}><ArrowDown size={14} /></button
            >
          </div>
        </div>
        <div class="track-fields">
          <div class="field">
            <label for={`mux-title-${position}`}>Title</label><input
              id={`mux-title-${position}`}
              aria-label={`Title for ${label}`}
              bind:value={track.title}
              disabled={locked || !track.included}
            />
          </div>
          {#if track.stream.kind !== 'attachment'}
            <div class="field">
              <label for={`mux-language-${position}`}>Language</label><input
                id={`mux-language-${position}`}
                aria-label={`Language for ${label}`}
                bind:value={track.language}
                disabled={locked || !track.included}
                placeholder="eng"
              />
            </div>
            <div class="field">
              <label for={`mux-default-${position}`}>Default</label><select
                id={`mux-default-${position}`}
                aria-label={`Default for ${label}`}
                bind:value={track.defaultFlag}
                disabled={locked || !track.included}
                ><option value="">Preserve</option><option value="yes">Yes</option><option
                  value="no">No</option
                ></select
              >
            </div>
            <div class="field">
              <label for={`mux-forced-${position}`}>Forced</label><select
                id={`mux-forced-${position}`}
                aria-label={`Forced for ${label}`}
                bind:value={track.forcedFlag}
                disabled={locked || !track.included}
                ><option value="">Preserve</option><option value="yes">Yes</option><option
                  value="no">No</option
                ></select
              >
            </div>
          {/if}
        </div>
      </div>
    {/each}
  </div>

  <div class="panel mux-panel">
    <div class="field">
      <label for="mux-destination">Combined destination</label><input
        id="mux-destination"
        bind:value={destination}
        disabled={locked}
        placeholder="Choose a new media file"
      />
      <ContainerOptions
        value={destinationContainer(destination)}
        onchange={(value) => (destination = containerDestination(destination, value))}
        disabled={locked}
      />
    </div>
    <p class="small-muted">
      Existing files are never replaced. Packet payloads, timing, track metadata, and chapters are
      checked before the output is published.
    </p>
    <div class="actions">
      <Button variant="outline" onclick={chooseOutput} disabled={locked}
        >Choose combined destination</Button
      ><Button onclick={start} disabled={locked || !!validation}
        ><Play size={14} />{submitting ? 'Starting…' : 'Start combined remux'}</Button
      >
    </div>
    {#if validation}<p class="small-muted">{validation}</p>{/if}
    {#if error}<p role="alert" class="job-error">{error}</p>{/if}
  </div>
</section>

<style>
  .mux-workspace {
    display: grid;
    gap: 20px;
    min-width: 0;
  }
  .mux-panel {
    padding: 20px;
    min-width: 0;
    display: grid;
    gap: 14px;
  }
  h2 {
    font-size: 15px;
    font-weight: 600;
  }
  .sources {
    display: grid;
    gap: 12px;
  }
  .sources label,
  .track-heading label {
    display: flex;
    align-items: center;
    gap: 10px;
    min-width: 0;
    font-size: 13px;
  }
  input[type='checkbox'] {
    width: 16px;
    height: 16px;
    flex: 0 0 auto;
    accent-color: #ad5326;
  }
  small {
    display: block;
    margin-top: 4px;
    font-size: 11px;
    color: var(--muted-foreground);
    overflow-wrap: anywhere;
  }
  .sources span,
  .track-heading span {
    min-width: 0;
    overflow-wrap: anywhere;
  }
  .owners {
    display: grid;
    grid-template-columns: repeat(2, minmax(0, 1fr));
    gap: 14px;
  }
  .track {
    border-top: 1px solid var(--border);
    padding-top: 14px;
    min-width: 0;
  }
  .track-heading {
    display: flex;
    justify-content: space-between;
    gap: 10px;
    align-items: center;
    margin-bottom: 12px;
  }
  .track-order,
  .actions {
    display: flex;
    gap: 8px;
    flex-wrap: wrap;
  }
  .track-order {
    flex: 0 0 auto;
  }
  .track-fields {
    display: grid;
    grid-template-columns: minmax(0, 2fr) repeat(3, minmax(0, 1fr));
    gap: 12px;
  }
  .field {
    min-width: 0;
  }
  .field input,
  .field select {
    width: 100%;
    min-width: 0;
  }
  @media (max-width: 850px) {
    .owners,
    .track-fields {
      grid-template-columns: repeat(2, minmax(0, 1fr));
    }
  }
</style>
