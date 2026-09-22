<script lang="ts">
  import { estimateAv1anResources } from '$lib/ipc/client';
  import type { Av1anResourceEstimate, VideoEncoder } from '$lib/ipc/generated';
  let {
    encoder,
    workers,
    sourceWidth,
    sourceHeight,
    outputWidth,
    outputHeight,
    filtered = false,
    floatFilter = false,
    disabled = false,
    onapply,
  }: {
    encoder: VideoEncoder;
    workers: number | undefined;
    sourceWidth?: number | null;
    sourceHeight?: number | null;
    outputWidth?: number;
    outputHeight?: number;
    filtered?: boolean;
    floatFilter?: boolean;
    disabled?: boolean;
    onapply: (workers: number, threads: number, slices: number) => void;
  } = $props();
  let estimate = $state<Av1anResourceEstimate | null>(null);
  $effect(() => {
    const request = {
      encoder,
      workers: workers ?? 1,
      sourceWidth: sourceWidth ?? 0,
      sourceHeight: sourceHeight ?? 0,
      outputWidth: outputWidth ?? sourceWidth ?? 0,
      outputHeight: outputHeight ?? sourceHeight ?? 0,
      filtered,
      floatFilter,
    };
    let current = true;
    estimate = null;
    if (
      request.sourceWidth &&
      request.sourceHeight &&
      request.workers >= 1 &&
      request.workers <= 64
    )
      estimateAv1anResources(request)
        .then((value) => {
          if (current) estimate = value;
        })
        .catch(() => {});
    return () => {
      current = false;
    };
  });
</script>

{#if estimate}
  <div class="resources full-width" aria-label="AV1AN memory guidance">
    <p>
      Estimated memory: {(estimate.estimatedMemoryMib / 1024).toFixed(1)} GiB total · {(
        estimate.perWorkerMib / 1024
      ).toFixed(1)} GiB per worker.
      {#if estimate.availableMemoryMib !== null}{(estimate.availableMemoryMib / 1024).toFixed(1)} GiB
        currently available.{/if}
    </p>
    {#if estimate.warning}<p class="warning">{estimate.warning}</p>{/if}
    <button
      type="button"
      {disabled}
      onclick={() =>
        estimate &&
        onapply(
          estimate.suggestedWorkers,
          estimate.suggestedThreads,
          estimate.suggestedSceneSlices,
        )}>Use suggested parallelism</button
    >
    <p>
      {estimate.logicalProcessors} logical processors · Suggested {estimate.suggestedWorkers} workers
      × {estimate.suggestedThreads} encoder threads. Estimates vary by encoder preset and decoder caches;
      monitor actual memory use.
    </p>
  </div>
{/if}

<style>
  .resources {
    display: grid;
    gap: 0.5rem;
    padding: 0.7rem;
    border: 1px solid var(--border);
    border-radius: 0.4rem;
  }
  p {
    margin: 0;
    font-size: 0.77rem;
    color: var(--muted-foreground);
  }
  .warning {
    color: var(--foreground);
  }
  button {
    justify-self: start;
    padding: 0.4rem 0.65rem;
    border: 1px solid var(--border);
    border-radius: 0.3rem;
  }
</style>
