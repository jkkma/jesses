<script lang="ts">
  import { onDestroy, untrack } from 'svelte';
  import { isDesktop, previewFrame } from '$lib/ipc/client';
  import type { MediaFile, FramePreviewResult } from '$lib/ipc/generated';
  let { file }: { file: MediaFile | undefined } = $props();
  let result = $state<FramePreviewResult | null>(null);
  let busy = $state(false);
  let index = $state(0);
  let controller: AbortController | undefined;
  let generation = 0;
  let previous = '';
  const video = $derived(file?.streams.find((stream) => stream.kind === 'video'));
  async function load() {
    controller?.abort();
    const ticket = ++generation;
    if (!file || !video || !isDesktop()) {
      result = null;
      return;
    }
    const current = new AbortController();
    controller = current;
    const duration = video.durationSeconds ?? file.durationSeconds ?? 0;
    const positionSeconds = Math.max(0, (duration * (index + 1)) / 5);
    busy = true;
    try {
      const next = await previewFrame(
        {
          inputPath: file.path,
          videoStreamIndex: video.index,
          positionSeconds,
          displayOrientation: true,
        },
        current.signal,
      );
      if (generation === ticket && !current.signal.aborted) result = next;
    } catch {
      if (generation === ticket) result = null;
    } finally {
      if (generation === ticket) busy = false;
    }
  }
  $effect(() => {
    const identity = `${file?.path ?? ''}\0${video?.index ?? ''}`;
    if (identity !== previous) {
      previous = identity;
      untrack(() => {
        index = 0;
        result = null;
        void load();
      });
    }
  });
  onDestroy(() => {
    generation++;
    controller?.abort();
  });
</script>

<div class="source-thumbnail">
  <button
    type="button"
    class="image"
    aria-label="Show next thumbnail"
    disabled={!video || busy}
    onclick={() => {
      index = (index + 1) % 4;
      void load();
    }}
  >
    {#if result}<img src={result.imageDataUrl} alt={`Thumbnail of ${file?.name}`} />{:else}<span
        >{busy ? 'Loading thumbnail…' : 'No thumbnail'}</span
      >{/if}
  </button>
  {#if result}<small
      >Showing thumbnail {index + 1}/4 ({result.positionSeconds.toFixed(1)}s). Click for next
      thumbnail.</small
    >{/if}
</div>

<style>
  .source-thumbnail {
    width: min(100%, 384px);
    display: grid;
    gap: 0.3rem;
    align-self: start;
    align-content: start;
  }
  .image {
    width: 100%;
    aspect-ratio: 16 / 9;
    padding: 0;
    overflow: hidden;
    border: 1px solid var(--rule);
    border-radius: var(--radius);
    background: var(--background);
    color: var(--muted-foreground);
  }
  img {
    width: 100%;
    height: 100%;
    object-fit: contain;
  }
  small {
    color: var(--muted-foreground);
  }
</style>
