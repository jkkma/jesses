<script lang="ts">
  import { onDestroy, untrack } from 'svelte';
  import { previewFrame, isDesktop } from '$lib/ipc/client';
  import type { FramePreviewResult, MediaFile } from '$lib/ipc/generated';
  import { errorMessage } from '$lib/components/shared/format';
  import { previewMaximum } from '$lib/components/shared/preview-position';
  let { file, sample = false }: { file: MediaFile; sample?: boolean } = $props();
  const id = $props.id();
  const videos = $derived(file.streams.filter((s) => s.kind === 'video'));
  let expanded = $state(false);
  let streamIndex = $state(0);
  let position = $state(0);
  let result = $state<FramePreviewResult | null>(null);
  let busy = $state(false);
  let error = $state('');
  let activeFile: MediaFile | undefined;
  let generation = 0;
  let controller: AbortController | undefined;
  let timer: ReturnType<typeof setTimeout> | undefined;
  const max = $derived(
    previewMaximum(
      file.durationSeconds,
      videos.find((video) => video.index === streamIndex),
    ),
  );
  const disabled = $derived(sample || !isDesktop());

  function cancel() {
    clearTimeout(timer);
    ++generation;
    controller?.abort();
    controller = undefined;
    busy = false;
  }
  $effect(() => {
    const currentFile = file;
    const first = videos[0]?.index ?? 0;
    untrack(() => {
      if (activeFile === currentFile) return;
      activeFile = currentFile;
      cancel();
      streamIndex = first;
      position = 0;
      result = null;
      error = '';
      if (expanded) void load();
    });
  });
  onDestroy(cancel);
  async function load() {
    cancel();
    if (disabled || !expanded) return;
    result = null;
    if (!Number.isFinite(position) || position < 0 || position > max) {
      error = 'Choose a position within this video stream.';
      return;
    }
    const current = ++generation;
    const pending = new AbortController();
    controller = pending;
    const request = {
      inputPath: file.path,
      videoStreamIndex: streamIndex,
      positionSeconds: position,
      displayOrientation: true,
    };
    busy = true;
    error = '';
    try {
      const image = await previewFrame(request, pending.signal);
      if (current === generation && !pending.signal.aborted) result = image;
    } catch (cause) {
      if (current === generation && !pending.signal.aborted) error = errorMessage(cause);
    } finally {
      if (current === generation) busy = false;
    }
  }
  function seek(value: number) {
    cancel();
    position = value;
    result = null;
    error = '';
    if (!Number.isFinite(value) || value < 0 || value > max) {
      error = 'Choose a position within this video stream.';
      return;
    }
    timer = setTimeout(() => void load(), 180);
  }
</script>

{#if videos.length}
  <section class="thumbnail" aria-label="Video thumbnail">
    <button
      type="button"
      class="thumbnail-toggle"
      aria-expanded={expanded}
      onclick={() => {
        expanded = !expanded;
        if (expanded) void load();
        else cancel();
      }}>Video thumbnail & scrubbing</button
    >
    {#if expanded}
      <div class="thumbnail-body">
        {#if videos.length > 1}
          <label for={`${id}-stream`}>Thumbnail video stream</label>
          <select
            id={`${id}-stream`}
            bind:value={streamIndex}
            onchange={() => {
              result = null;
              void load();
            }}
          >
            {#each videos as video}<option value={video.index}
                >#{video.index} · {video.title ?? video.codec ?? 'Video'}</option
              >{/each}
          </select>
        {/if}
        {#if result}
          <img
            src={result.imageDataUrl}
            alt={`Video thumbnail of ${file.name}`}
            width={result.width}
            height={result.height}
          />
          <p>
            {result.positionSeconds.toFixed(2)} s · Display orientation and pixel aspect ratio applied.{result.toneMapped
              ? ' HDR shown as SDR.'
              : ''}
          </p>
        {/if}
        <label for={`${id}-position`}>Thumbnail position (seconds)</label>
        <input
          id={`${id}-position`}
          type="number"
          min="0"
          {max}
          step="0.05"
          value={position}
          {disabled}
          oninput={(e) => seek(e.currentTarget.valueAsNumber)}
        />
        {#if max > 0}<input
            aria-label="Scrub thumbnail"
            type="range"
            min="0"
            {max}
            step="0.05"
            value={position}
            {disabled}
            oninput={(e) => seek(e.currentTarget.valueAsNumber)}
          />{/if}
        <button type="button" onclick={load} {disabled}
          >{busy ? 'Updating thumbnail…' : 'Refresh thumbnail'}</button
        >
        {#if busy}<button type="button" onclick={cancel}>Cancel thumbnail</button>{/if}
        {#if disabled}<p>Thumbnails require a real source in the desktop app.</p>{/if}
        {#if error}<p role="alert">{error}</p>{/if}
      </div>
    {/if}
  </section>
{/if}

<style>
  .thumbnail {
    border-top: 1px solid var(--rule);
    padding-inline: 17px;
  }
  .thumbnail-toggle {
    width: 100%;
    padding: 12px 0;
    text-align: left;
    font-size: 12px;
    font-weight: 600;
    color: var(--foreground);
    background: transparent;
  }
  .thumbnail-body {
    display: grid;
    gap: 8px;
    padding-bottom: 12px;
    font-size: 11px;
  }
  img {
    width: 100%;
    height: auto;
    max-height: 400px;
    object-fit: contain;
    background: #111;
  }
  p {
    margin: 0;
    color: var(--muted-foreground);
    overflow-wrap: anywhere;
  }
  input,
  select {
    min-width: 0;
    width: 100%;
  }
  button:focus-visible {
    outline: 2px solid var(--ring);
  }
</style>
