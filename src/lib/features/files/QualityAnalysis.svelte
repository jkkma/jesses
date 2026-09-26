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
  let runSsim = $state(true);
  let runPsnr = $state(false);
  let runVmaf = $state(false);
  let inspectedMetric = $state<QualityMetric>('ssim');
  let alignment = $state<'none' | 'cropReference' | 'resizeReference' | 'cropAndResizeReference'>(
    'none',
  );
  let vmafModel = $state<'standard' | 'negative' | 'fourK'>('standard');
  let subsample = $state<number | undefined>(1);
  let fixFrameRate = $state(false);
  let reports = $state<Array<{ request: QualityRequest; result: QualityResult }>>([]);
  const displayed = $derived(
    reports.find((entry) => entry.request.metric === inspectedMetric) ?? reports[0],
  );
  const result = $derived(displayed?.result ?? null);
  const completedRequest = $derived(displayed?.request ?? null);
  let error = $state<string | null>(null);
  let pending = $state(false);
  let compared = $state(0);
  let selecting = $state(false);
  let point = $state(0);
  let generation = 0;
  let pickerGeneration = 0;
  let controller: AbortController | undefined;
  const valid = $derived(
    candidate &&
      (runSsim || runPsnr || runVmaf) &&
      typeof referenceIndex === 'number' &&
      typeof candidateIndex === 'number' &&
      [referenceStart, candidateStart].every(
        (v) => typeof v === 'number' && Number.isInteger(v) && v >= 0 && v <= 999999,
      ) &&
      typeof count === 'number' &&
      Number.isInteger(count) &&
      count >= 1 &&
      count <= 60000 &&
      referenceStart! + count <= 1_000_000 &&
      candidateStart! + count <= 1_000_000 &&
      typeof subsample === 'number' &&
      Number.isInteger(subsample) &&
      subsample >= 1 &&
      subsample <= 1000,
  );
  const finite = $derived(result?.points.flatMap((p) => (p.score === null ? [] : [p.score])) ?? []);
  const low = $derived(finite.length ? Math.min(...finite) : 0);
  const high = $derived(finite.length ? Math.max(...finite) : 1);
  const chart = $derived(
    result?.points
      .map((p, index) => ({ p, index }))
      .filter(({ index }) => index % Math.max(1, Math.ceil(result!.points.length / 500)) === 0)
      .map(
        ({ p, index }) =>
          `${8 + (index / Math.max(1, result!.frameCount - 1)) * 264},${100 - (((p.score ?? high) - low) / Math.max(0.00001, high - low)) * 90}`,
      )
      .join(' ') ?? '',
  );
  function cancel() {
    generation++;
    controller?.abort();
    controller = undefined;
    pending = false;
  }
  function stop() {
    const finished = reports.length;
    cancel();
    error = finished
      ? `Comparison canceled. ${finished} completed score${finished === 1 ? ' remains' : 's remain'} available.`
      : 'Comparison canceled.';
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
    void runSsim;
    void runPsnr;
    void runVmaf;
    void alignment;
    void vmafModel;
    void subsample;
    void fixFrameRate;
    reports = [];
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
    compared = 0;
    error = null;
    reports = [];
    try {
      const selected: QualityMetric[] = [
        ...(runSsim ? (['ssim'] as const) : []),
        ...(runPsnr ? (['psnr'] as const) : []),
        ...(runVmaf ? (['vmaf'] as const) : []),
      ];
      const frozen = {
        referencePath: file.path,
        referenceStreamIndex: referenceIndex!,
        referenceStartFrame: referenceStart!,
        candidatePath: candidate.path,
        candidateStreamIndex: candidateIndex!,
        candidateStartFrame: candidateStart!,
        frameCount: count!,
        options: { alignment, vmafModel, subsample: subsample!, fixFrameRate },
      };
      for (const metric of selected) {
        const request: QualityRequest = { ...frozen, metric };
        const value = await analyzeQuality(request, active.signal);
        if (run !== generation || active.signal.aborted) return;
        reports = [...reports, { request, result: value }];
        compared++;
        if (reports.length === 1) inspectedMetric = metric;
      }
    } catch (cause) {
      if (run === generation && !active.signal.aborted)
        error = `${errorMessage(cause)}${reports.length ? ` ${reports.length} completed score${reports.length === 1 ? ' remains' : 's remain'} available.` : ''}`;
    } finally {
      if (run === generation) {
        pending = false;
        controller = undefined;
      }
    }
  }
  const score = (value: number | null, metric: QualityMetric) =>
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
        reports = [];
        error = null;
        pickerGeneration++;
        selecting = false;
      }
    }}>Compare video quality {expanded ? '−' : '+'}</button
  >
  {#if expanded}
    <p>
      The selected file is the reference. Choose a candidate and corresponding frames. Pixel format
      and color must match. Alignment can crop or resize the reference; HDR tone mapping is not
      applied.
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
    </div>
    <fieldset disabled={pending}>
      <legend>Metrics to calculate</legend>
      <label class="check"><input type="checkbox" bind:checked={runSsim} />SSIM</label>
      <label class="check"><input type="checkbox" bind:checked={runPsnr} />PSNR</label>
      <label class="check"><input type="checkbox" bind:checked={runVmaf} />VMAF</label>
    </fieldset>
    <details>
      <summary>Alignment and sampling</summary>
      <div class="fields">
        <label
          >Reference alignment<select
            aria-label="Reference alignment"
            bind:value={alignment}
            disabled={pending}
            ><option value="none">Keep original frames</option>
            <option value="cropReference">Auto crop reference</option>
            <option value="resizeReference">Resize reference to candidate</option>
            <option value="cropAndResizeReference">Auto crop, then resize reference</option></select
          ></label
        >
        {#if runVmaf}<label
            >VMAF model<select aria-label="VMAF model" bind:value={vmafModel} disabled={pending}
              ><option value="standard">v0.6.1</option><option value="negative">v0.6.1 NEG</option
              ><option value="fourK">4K v0.6.1</option></select
            ></label
          >{/if}
        <label
          >Score every Nth frame<input
            aria-label="Score every Nth frame"
            type="number"
            min="1"
            max="1000"
            step="1"
            bind:value={subsample}
            disabled={pending}
          /></label
        >
        <label class="check"
          ><input type="checkbox" bind:checked={fixFrameRate} disabled={pending} />Pair by frame
          number when timestamps differ</label
        >
      </div>
      <p>
        Auto crop samples the selected reference interval. Check that the detected edges do not cut
        content. Anamorphic inputs are compared at their displayed shape when their storage or pixel
        aspect differs.
      </p>
    </details>
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
      >{#if pending}<button type="button" onclick={stop}>Cancel comparison</button>{/if}
    </div>
    {#if pending}<p role="status">
        Checking decoded frames, then measuring the selected interval. {compared} of {(runSsim
          ? 1
          : 0) +
          (runPsnr ? 1 : 0) +
          (runVmaf ? 1 : 0)} metrics complete. Long sources can take several minutes.
      </p>{/if}
    {#if error}<p role="alert">{error}</p>{/if}
    {#if result}
      {#if reports.length > 1}<div class="actions" aria-label="Completed metric scores">
          {#each reports as entry}<button
              type="button"
              aria-pressed={displayed?.request.metric === entry.request.metric}
              onclick={() => {
                inspectedMetric = entry.request.metric;
                point = 0;
              }}
              >{entry.request.metric.toUpperCase()}: {score(
                entry.result.score,
                entry.request.metric,
              )}</button
            >{/each}
        </div>{/if}
      <p class="result">
        {result.metric.toUpperCase()}: {score(result.score, result.metric)} · {result.frameCount.toLocaleString()}
        scored frames
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
      <p>
        Frame {result.points[point]?.frame ?? point}: {score(
          result.points[point]?.score ?? null,
          result.metric,
        )}
      </p>
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
  details {
    margin: 8px 0;
    font-size: 11px;
  }
  summary {
    cursor: pointer;
  }
  .check {
    display: flex;
    align-items: center;
    gap: 8px;
  }
  .check input {
    width: auto;
  }
  fieldset {
    display: flex;
    flex-wrap: wrap;
    gap: 4px 14px;
    border: 1px solid var(--border);
    border-radius: 4px;
    margin: 8px 0;
    padding: 2px 9px;
  }
  legend {
    font-size: 11px;
    padding: 0 4px;
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
