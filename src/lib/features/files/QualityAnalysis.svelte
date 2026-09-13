<script lang="ts">
  import { analyzeQuality, chooseMediaFiles, isDesktop, probeMedia } from '$lib/ipc/client';
  import type { MediaFile, QualityMetric, QualityResult, QualityRequest } from '$lib/ipc/generated';
  import AnalysisExport from '$lib/components/shared/AnalysisExport.svelte';
  import { errorMessage } from '$lib/components/shared/format';
  let { file, sample = false }: { file: MediaFile; sample?: boolean } = $props();
  let expanded = $state(false);
  let candidate = $state<MediaFile | null>(null);
  let referenceIndex = $state<number | undefined>();
  let candidateIndex = $state<number | undefined>();
  let referenceStart = $state<number | undefined>(0);
  let candidateStart = $state<number | undefined>(0);
  let count = $state<number | undefined>(240);
  let metric = $state<QualityMetric>('ssim');
  let result = $state<QualityResult | null>(null);
  let completedRequest = $state<QualityRequest | null>(null);
  let error = $state<string | null>(null);
  let pending = $state(false);
  let selecting = $state(false);
  let point = $state(0);
  let generation = 0;
  let pickerGeneration = 0;
  let controller: AbortController | undefined;
  const valid = $derived(
    candidate &&
      typeof referenceIndex === 'number' &&
      typeof candidateIndex === 'number' &&
      [referenceStart, candidateStart].every(
        (v) => typeof v === 'number' && Number.isInteger(v) && v >= 0 && v <= 999999,
      ) &&
      typeof count === 'number' &&
      Number.isInteger(count) &&
      count >= 1 &&
      count <= 60000,
  );
  const finite = $derived(result?.points.flatMap((p) => (p.score === null ? [] : [p.score])) ?? []);
  const low = $derived(finite.length ? Math.min(...finite) : 0);
  const high = $derived(finite.length ? Math.max(...finite) : 1);
  const chart = $derived(
    result?.points
      .filter((_, i) => i % Math.max(1, Math.ceil(result!.points.length / 500)) === 0)
      .map(
        (p) =>
          `${8 + (p.frame / Math.max(1, result!.frameCount - 1)) * 264},${100 - (((p.score ?? high) - low) / Math.max(0.00001, high - low)) * 90}`,
      )
      .join(' ') ?? '',
  );
  function cancel() {
    generation++;
    controller?.abort();
    controller = undefined;
    pending = false;
  }
  $effect(() => {
    void file.path;
    referenceIndex = file.streams.find((s) => s.kind === 'video')?.index;
    candidate = null;
    candidateIndex = undefined;
    pickerGeneration++;
    selecting = false;
    return () => {
      pickerGeneration++;
      cancel();
    };
  });
  $effect(() => {
    void referenceIndex;
    void candidate?.path;
    void candidateIndex;
    void referenceStart;
    void candidateStart;
    void count;
    void metric;
    result = null;
    completedRequest = null;
    error = null;
    point = 0;
    return cancel;
  });
  async function choose() {
    const selection = ++pickerGeneration;
    selecting = true;
    error = null;
    try {
      const paths = await chooseMediaFiles();
      if (selection !== pickerGeneration || !paths.length) return;
      if (paths.length !== 1) throw new Error('Choose one candidate video.');
      const value = await probeMedia(paths[0]);
      if (selection !== pickerGeneration) return;
      const video = value.streams.find((s) => s.kind === 'video');
      if (!video) throw new Error('The candidate has no video stream.');
      candidate = value;
      candidateIndex = video.index;
    } catch (cause) {
      if (selection === pickerGeneration) error = errorMessage(cause);
    } finally {
      if (selection === pickerGeneration) selecting = false;
    }
  }
  async function analyze() {
    if (!valid || !candidate) return;
    cancel();
    const run = ++generation;
    const active = new AbortController();
    controller = active;
    pending = true;
    error = null;
    result = null;
    try {
      const request: QualityRequest = {
        referencePath: file.path,
        referenceStreamIndex: referenceIndex!,
        referenceStartFrame: referenceStart!,
        candidatePath: candidate.path,
        candidateStreamIndex: candidateIndex!,
        candidateStartFrame: candidateStart!,
        frameCount: count!,
        metric,
      };
      const value = await analyzeQuality(request, active.signal);
      if (run === generation) {
        result = value;
        completedRequest = request;
      }
    } catch (cause) {
      if (run === generation && !active.signal.aborted) error = errorMessage(cause);
    } finally {
      if (run === generation) {
        pending = false;
        controller = undefined;
      }
    }
  }
  const score = (value: number | null) =>
    value === null ? '∞ (identical pixels)' : value.toFixed(metric === 'ssim' ? 6 : 3);
</script>

<section aria-label="Quality comparison" class="quality">
  <button
    class="disclosure"
    type="button"
    aria-expanded={expanded}
    onclick={() => {
      expanded = !expanded;
      if (!expanded) {
        cancel();
        pickerGeneration++;
        selecting = false;
      }
    }}>Compare video quality {expanded ? '−' : '+'}</button
  >
  {#if expanded}
    <p>
      The selected file is the reference. Choose a candidate and corresponding frames. Dimensions,
      pixel format, color and cadence must match. No automatic resize or tone mapping is applied.
    </p>
    <label
      >Reference video<select
        aria-label="Reference video"
        bind:value={referenceIndex}
        disabled={pending}
        >{#each file.streams.filter((s) => s.kind === 'video') as stream}<option
            value={stream.index}>#{stream.index} · {stream.codec ?? 'Video'}</option
          >{/each}</select
      ></label
    >
    <button type="button" onclick={choose} disabled={pending || selecting || sample || !isDesktop()}
      >{selecting ? 'Reading candidate…' : 'Choose candidate video'}</button
    >
    {#if candidate}
      <p class="path" title={candidate.path}>{candidate.name}</p>
      <label
        >Candidate video<select
          aria-label="Candidate video"
          bind:value={candidateIndex}
          disabled={pending}
          >{#each candidate.streams.filter((s) => s.kind === 'video') as stream}<option
              value={stream.index}>#{stream.index} · {stream.codec ?? 'Video'}</option
            >{/each}</select
        ></label
      >
    {/if}
    <div class="fields">
      <label
        >Reference start frame<input
          type="number"
          min="0"
          max="999999"
          step="1"
          bind:value={referenceStart}
          disabled={pending}
        /></label
      >
      <label
        >Candidate start frame<input
          type="number"
          min="0"
          max="999999"
          step="1"
          bind:value={candidateStart}
          disabled={pending}
        /></label
      >
      <label
        >Frames to compare<input
          type="number"
          min="1"
          max="60000"
          step="1"
          bind:value={count}
          disabled={pending}
        /></label
      >
      <label
        >Metric<select aria-label="Metric" bind:value={metric} disabled={pending}
          ><option value="ssim">SSIM</option><option value="psnr">PSNR</option><option value="vmaf"
            >VMAF v0.6.1</option
          ></select
        ></label
      >
    </div>
    <p>
      Frames are numbered from zero. Confirm that both intervals show the same content; matching
      timestamps alone cannot establish this.
    </p>
    <div class="actions">
      <button
        type="button"
        onclick={analyze}
        disabled={!valid || pending || selecting || sample || !isDesktop()}
        >{pending ? 'Comparing…' : 'Compare selected frames'}</button
      >{#if pending}<button type="button" onclick={cancel}>Cancel comparison</button>{/if}
    </div>
    {#if pending}<p role="status">
        Checking decoded frames, then measuring the selected interval. Long sources can take several
        minutes.
      </p>{/if}
    {#if error}<p role="alert">{error}</p>{/if}
    {#if result}
      <p class="result">
        {result.metric.toUpperCase()}: {score(result.score)} · {result.frameCount.toLocaleString()} frames
      </p>
      {#if result.model}<p>Model: {result.model}</p>{/if}
      <svg
        viewBox="0 0 280 112"
        role="img"
        aria-label={`${result.metric.toUpperCase()} score by frame`}
        ><polyline points={chart} fill="none" stroke="currentColor" stroke-width="1.5" /></svg
      >
      <label
        >Inspect comparison frame<input
          type="range"
          min="0"
          max={result.frameCount - 1}
          step="1"
          bind:value={point}
        /></label
      >
      <p>Frame {point}: {score(result.points[point]?.score ?? null)}</p>
      <p>{result.message}</p>
      {#if completedRequest}<AnalysisExport
          report={{ kind: 'quality', request: completedRequest, result }}
        />{/if}
    {/if}
  {/if}
</section>

<style>
  .quality {
    padding: 18px;
    border-top: 1px solid var(--border);
  }
  .disclosure {
    border: 0;
    background: transparent;
    padding-left: 0;
    font-weight: 600;
  }
  button {
    font: inherit;
    font-size: 11px;
    color: var(--foreground);
    background: var(--background);
    border: 1px solid var(--border);
    border-radius: 4px;
    padding: 6px 8px;
    cursor: pointer;
  }
  button:disabled {
    opacity: 0.5;
    cursor: default;
  }
  p {
    font-size: 11px;
    line-height: 1.5;
    color: var(--muted-foreground);
    margin: 9px 0;
  }
  .path {
    overflow-wrap: anywhere;
  }
  .result {
    color: var(--foreground);
    font-weight: 600;
  }
  label {
    display: grid;
    gap: 4px;
    font-size: 11px;
    margin: 8px 0;
  }
  input,
  select {
    width: 100%;
    min-width: 0;
  }
  input[type='number'],
  select {
    font: inherit;
    color: var(--foreground);
    background: var(--background);
    border: 1px solid var(--border);
    border-radius: 4px;
    padding: 7px 9px;
  }
  input:focus-visible,
  select:focus-visible,
  button:focus-visible {
    outline: 2px solid var(--primary);
    outline-offset: 2px;
  }
  input[type='range'] {
    accent-color: var(--primary);
  }
  .fields {
    display: grid;
    grid-template-columns: repeat(2, minmax(0, 1fr));
    gap: 8px;
  }
  .actions {
    display: flex;
    gap: 8px;
    flex-wrap: wrap;
  }
  svg {
    width: 100%;
    color: var(--primary);
  }
</style>
