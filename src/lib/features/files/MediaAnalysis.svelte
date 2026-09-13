<script lang="ts">
  import { analyzeBitrate, isDesktop } from '$lib/ipc/client';
  import type { BitrateResult, MediaFile } from '$lib/ipc/generated';
  import { errorMessage, formatBytes } from '$lib/components/shared/format';
  import AnalysisExport from '$lib/components/shared/AnalysisExport.svelte';

  let { file, sample = false }: { file: MediaFile; sample?: boolean } = $props();
  let expanded = $state(false);
  let streamIndex = $state<number | undefined>();
  let windowSeconds = $state(1);
  let result = $state<BitrateResult | null>(null);
  let selectedPoint = $state(0);
  let pending = $state(false);
  let error = $state<string | null>(null);
  let controller: AbortController | undefined;
  let generation = 0;
  const streams = $derived(
    file.streams.filter((stream) => ['video', 'audio', 'subtitle'].includes(stream.kind)),
  );
  const point = $derived(result?.points[selectedPoint]);
  const plot = $derived(
    result?.points
      .map(
        (point, index, points) =>
          `${8 + (index / Math.max(1, points.length - 1)) * 264},${100 - (point.megabitsPerSecond / Math.max(0.000001, result!.peakWindowMegabitsPerSecond)) * 90}`,
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
    const source = file.path;
    void source;
    streamIndex = streams[0]?.index;
    result = null;
    error = null;
    return cancel;
  });

  $effect(() => {
    void streamIndex;
    void windowSeconds;
    result = null;
    error = null;
    selectedPoint = 0;
    return cancel;
  });

  async function analyze() {
    if (typeof streamIndex !== 'number') return;
    cancel();
    const run = ++generation;
    const active = new AbortController();
    controller = active;
    result = null;
    error = null;
    pending = true;
    try {
      const value = await analyzeBitrate(
        { inputPath: file.path, streamIndex, windowSeconds },
        active.signal,
      );
      if (run !== generation) return;
      result = value;
      selectedPoint = 0;
    } catch (cause) {
      if (run === generation && !active.signal.aborted) error = errorMessage(cause);
    } finally {
      if (run === generation) {
        pending = false;
        controller = undefined;
      }
    }
  }
</script>

<section class="analysis" aria-label="Bitrate analysis">
  <button
    class="disclosure"
    type="button"
    aria-expanded={expanded}
    onclick={() => {
      expanded = !expanded;
      if (!expanded) cancel();
    }}
  >
    <span>Bitrate analysis</span><span aria-hidden="true">{expanded ? '−' : '+'}</span>
  </button>
  {#if expanded}
    <p>Inspect compressed packet sizes over time. Container overhead is excluded.</p>
    <label
      >Stream
      <select
        aria-label="Bitrate stream"
        bind:value={streamIndex}
        disabled={pending || !streams.length}
      >
        {#each streams as stream (stream.index)}
          <option value={stream.index}
            >#{stream.index} {stream.kind} · {stream.codec ?? 'unknown'}</option
          >
        {/each}
      </select>
    </label>
    <label
      >Window
      <select aria-label="Bitrate window" bind:value={windowSeconds} disabled={pending}>
        {#each [0.1, 1, 5, 10, 30, 60, 300, 3600] as seconds}
          <option value={seconds}>{seconds} {seconds === 1 ? 'second' : 'seconds'}</option>
        {/each}
      </select>
    </label>
    <div class="actions">
      <button
        type="button"
        onclick={analyze}
        disabled={pending || sample || !isDesktop() || typeof streamIndex !== 'number'}
      >
        {pending ? 'Scanning packets…' : 'Analyze bitrate'}
      </button>
      {#if pending}<button type="button" onclick={cancel}>Cancel analysis</button>{/if}
    </div>
    {#if sample || !isDesktop()}<p>Import a local file in the desktop app to analyze it.</p>{/if}
    {#if error}<p class="error" role="alert">{error}</p>{/if}
    {#if pending}<p role="status">Reading packets. Longer files may take a few minutes.</p>{/if}
    {#if result}
      <dl aria-label="Bitrate results">
        <div>
          <dt>Average</dt>
          <dd>
            {result.averageMegabitsPerSecond === null
              ? 'Duration unknown'
              : `${result.averageMegabitsPerSecond.toFixed(3)} Mb/s`}
          </dd>
        </div>
        <div>
          <dt>Peak window</dt>
          <dd>{result.peakWindowMegabitsPerSecond.toFixed(3)} Mb/s</dd>
        </div>
        <div>
          <dt>Packet payload</dt>
          <dd>{formatBytes(result.packetBytes)}</dd>
        </div>
        <div>
          <dt>Packets</dt>
          <dd>{result.packetCount}</dd>
        </div>
      </dl>
      {#if result.points.length}
        <svg
          viewBox="0 0 280 110"
          role="img"
          aria-label={`Packet bitrate over ${result.points.length} windows, peak ${result.peakWindowMegabitsPerSecond.toFixed(3)} megabits per second`}
        >
          <path d="M8 5 V100 H272" class="axis" />
          <polyline points={plot} />
          <line
            x1={8 + (selectedPoint / Math.max(1, result.points.length - 1)) * 264}
            x2={8 + (selectedPoint / Math.max(1, result.points.length - 1)) * 264}
            y1="5"
            y2="100"
            class="cursor"
          />
        </svg>
        <label
          >Inspect window
          <input
            aria-label="Inspect bitrate window"
            type="range"
            min="0"
            max={result.points.length - 1}
            step="1"
            bind:value={selectedPoint}
          />
        </label>
        {#if point}<p class="point" aria-live="polite">
            {point.startSeconds.toFixed(2)}–{(point.startSeconds + result.windowSeconds).toFixed(2)} s
            · {point.megabitsPerSecond.toFixed(3)} Mb/s · {point.packetBytes} bytes
          </p>{/if}
        <p>
          Windows use source presentation timestamps and their full width, including the final
          window.
        </p>
      {/if}
      {#if result.dtsFallbackCount !== '0'}<p>
          {result.dtsFallbackCount} packets use decode timestamps because presentation timestamps are
          missing.
        </p>{/if}
      {#if result.untimedPacketCount !== '0'}<p>
          {result.untimedPacketCount} packets ({formatBytes(result.untimedPacketBytes)}) have no
          timestamp and are excluded from the graph and average.
        </p>{/if}
      <AnalysisExport
        report={{
          kind: 'bitrate',
          request: {
            inputPath: file.path,
            streamIndex: result.streamIndex,
            windowSeconds: result.windowSeconds,
          },
          result,
        }}
      />
    {/if}
  {/if}
</section>

<style>
  .analysis {
    padding: 18px;
    border-bottom: 1px solid var(--border);
  }
  button,
  select {
    font: inherit;
    color: inherit;
    border: 1px solid var(--border);
    border-radius: 5px;
    background: var(--background);
    padding: 6px 8px;
  }
  button {
    cursor: pointer;
  }
  button:disabled {
    opacity: 0.55;
    cursor: default;
  }
  .disclosure {
    display: flex;
    width: 100%;
    align-items: center;
    justify-content: space-between;
    font-weight: 600;
    border: 0;
    padding: 0;
    background: transparent;
  }
  label {
    display: grid;
    gap: 4px;
    margin-top: 10px;
    font-size: 11px;
  }
  select {
    width: 100%;
  }
  p {
    color: var(--muted-foreground);
    font-size: 11px;
    line-height: 1.5;
    margin: 10px 0 0;
  }
  .actions {
    display: flex;
    flex-wrap: wrap;
    gap: 6px;
    margin-top: 12px;
    font-size: 11px;
  }
  dl {
    font-size: 11px;
    margin: 14px 0 6px;
  }
  dl div {
    display: flex;
    justify-content: space-between;
    gap: 12px;
    margin-top: 6px;
  }
  dt {
    color: var(--muted-foreground);
  }
  dd {
    margin: 0;
    text-align: right;
  }
  svg {
    display: block;
    width: 100%;
    margin-top: 10px;
    overflow: visible;
  }
  polyline {
    fill: none;
    stroke: var(--primary);
    stroke-width: 1.5;
  }
  .axis {
    fill: none;
    stroke: var(--border);
  }
  .cursor {
    stroke: currentColor;
    stroke-width: 0.7;
    stroke-dasharray: 2 2;
  }
  input[type='range'] {
    width: 100%;
    accent-color: var(--primary);
  }
  .point {
    font-variant-numeric: tabular-nums;
    color: inherit;
  }
  .error {
    color: var(--destructive);
  }
</style>
