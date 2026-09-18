<script lang="ts">
  import { onDestroy } from 'svelte';
  import {
    isDesktop,
    chooseUtilityDestination,
    chooseUtilityFile,
    inspectUtilityCapabilities,
    runUtility,
  } from '$lib/ipc/client';
  import type {
    MediaFile,
    UtilityRequest,
    UtilityResult,
    UtilityCapabilities,
    GrainRequest,
    GrainSource,
    LadderEncoder,
    LadderMetric,
  } from '$lib/ipc/generated';
  import { errorMessage, formatBytes, fileName } from '$lib/components/shared/format';
  import ImageWorkflows from './ImageWorkflows.svelte';
  let { files, onimport }: { files: MediaFile[]; onimport: (paths: string[]) => void } = $props();
  let kind = $state<
    'keyframeCut' | 'concat' | 'colorMetadataTransfer' | 'subtitleOcr' | 'grain' | 'crfLadder'
  >('keyframeCut');
  let source = $state('');
  let stream = $state(0);
  let reference = $state('');
  let referenceStream = $state(0);
  let start = $state(0);
  let end = $state(10);
  let order = $state<string[]>([]);
  let language = $state('eng');
  let grainOperation = $state<'measure' | 'extract' | 'apply' | 'rewriteHeaders' | 'remove'>(
    'extract',
  );
  let grainSource = $state<'table' | 'preset' | 'photonNoise'>('photonNoise');
  let tablePath = $state('');
  let grainPreset = $state('');
  let iso = $state(400);
  let chroma = $state(false);
  let encoder = $state<LadderEncoder>('h264');
  let preset = $state('medium');
  let pixel = $state('yuv420p');
  let crfs = $state('18, 23, 28');
  let samples = $state(3);
  let seconds = $state(2);
  let metric = $state<LadderMetric>('ssim');
  let threshold = $state('');
  let pending = $state(false);
  let checking = $state(false);
  let error = $state<string | null>(null);
  let capabilities = $state<UtilityCapabilities | null>(null);
  let result = $state<UtilityResult | null>(null);
  let controller: AbortController | undefined;
  let capabilityController: AbortController | undefined;
  const selected = $derived(files.find((f) => f.path === source));
  const referenceFile = $derived(files.find((f) => f.path === reference));
  $effect(() => {
    if (!source && files.length) source = files[0].path;
  });
  $effect(() => {
    stream =
      selected?.streams.find((s) => s.kind === (kind === 'subtitleOcr' ? 'subtitle' : 'video'))
        ?.index ?? 0;
  });
  $effect(() => {
    referenceStream = referenceFile?.streams.find((s) => s.kind === 'video')?.index ?? 0;
  });
  onDestroy(() => {
    controller?.abort();
    capabilityController?.abort();
  });
  async function inspect() {
    checking = true;
    error = null;
    const active = new AbortController();
    capabilityController = active;
    try {
      capabilities = await inspectUtilityCapabilities(active.signal);
      if (!grainPreset) grainPreset = capabilities.grainPresets[0] ?? '';
    } catch (e) {
      error = errorMessage(e);
    } finally {
      checking = false;
      capabilityController = undefined;
    }
  }
  function reorder(i: number, delta: number) {
    const j = i + delta;
    if (j < 0 || j >= order.length) return;
    const changed = [...order];
    [changed[i], changed[j]] = [changed[j], changed[i]];
    order = changed;
  }
  function add(path: string) {
    if (path && !order.includes(path)) order = [...order, path];
  }
  async function chooseTable() {
    try {
      const path = await chooseUtilityFile('Choose AV1 grain table', ['tbl', 'txt']);
      if (path) tablePath = path;
    } catch (e) {
      error = errorMessage(e);
    }
  }
  async function run() {
    pending = true;
    error = null;
    result = null;
    const active = new AbortController();
    controller = active;
    try {
      if (kind !== 'concat' && !selected) throw new Error('Import and choose a source first.');
      let request: UtilityRequest;
      if (kind === 'crfLadder') {
        const parts = crfs.split(',').map((v) => v.trim());
        const values = parts.map(Number);
        if (parts.some((v) => !v) || values.some((v) => !Number.isInteger(v) || v < 0 || v > 63))
          throw new Error('Enter comma-separated whole CRF values.');
        if (threshold.trim() && !Number.isFinite(Number(threshold)))
          throw new Error('Enter a numeric recommendation threshold or leave it empty.');
        request = {
          kind,
          request: {
            inputPath: source,
            videoStreamIndex: stream,
            encoder,
            preset,
            pixelFormat: pixel,
            crfs: values,
            sampleCount: samples,
            sampleSeconds: seconds,
            metric,
            recommendationThreshold: threshold.trim() ? Number(threshold) : null,
          },
        };
      } else {
        const ext =
          kind === 'subtitleOcr'
            ? 'srt'
            : kind === 'grain' && ['measure', 'extract'].includes(grainOperation)
              ? 'tbl'
              : 'mkv';
        const output = await chooseUtilityDestination(kind + '-output.' + ext, [ext]);
        if (!output) return;
        if (kind === 'keyframeCut')
          request = {
            kind,
            request: {
              inputPath: source,
              outputPath: output,
              startSeconds: start,
              endSeconds: end,
            },
          };
        else if (kind === 'concat')
          request = { kind, request: { inputPaths: [...order], outputPath: output } };
        else if (kind === 'colorMetadataTransfer')
          request = {
            kind,
            request: {
              inputPath: source,
              inputVideoStreamIndex: stream,
              metadataSourcePath: reference,
              metadataSourceVideoStreamIndex: referenceStream,
              outputPath: output,
            },
          };
        else if (kind === 'subtitleOcr')
          request = {
            kind,
            request: {
              inputPath: source,
              subtitleStreamIndex: stream,
              language,
              outputPath: output,
            },
          };
        else {
          let grain: GrainRequest;
          if (grainOperation === 'measure')
            grain = {
              operation: 'measure',
              sourcePath: source,
              denoisedPath: reference,
              outputTablePath: output,
            };
          else if (grainOperation === 'extract')
            grain = { operation: 'extract', inputPath: source, outputTablePath: output };
          else if (grainOperation === 'remove')
            grain = { operation: 'remove', inputPath: source, outputPath: output };
          else {
            const settings: GrainSource =
              grainSource === 'table'
                ? { kind: 'table', tablePath }
                : grainSource === 'preset'
                  ? { kind: 'preset', preset: grainPreset }
                  : { kind: 'photonNoise', iso, chroma };
            grain = {
              operation: grainOperation,
              inputPath: source,
              outputPath: output,
              source: settings,
            };
          }
          request = { kind: 'grain', request: grain };
        }
      }
      result = await runUtility(request, active.signal);
    } catch (e) {
      error = active.signal.aborted ? 'Utility canceled.' : errorMessage(e);
    } finally {
      pending = false;
      controller = undefined;
    }
  }
</script>

<section class="utilities" aria-label="Media utilities">
  <header>
    <div>
      <span class="eyebrow">Focused media tasks</span>
      <h1>Media utilities</h1>
      <p>
        Cut, join, inspect, or compare local media without opening a full encode workflow. Outputs
        are checked before saving to a new destination.
      </p>
    </div>
    <button type="button" onclick={inspect} disabled={checking || pending || !isDesktop()}
      >{checking ? 'Checking tools…' : 'Check utility tools'}</button
    >
  </header>
  {#if capabilities}<details>
      <summary>Available utility tools and models</summary>
      <ul>
        {#each capabilities.dependencies as dependency}<li>
            <strong>{dependency.id}: {dependency.available ? 'Available' : 'Unavailable'}</strong> — {dependency.detail}
          </li>{/each}
      </ul>
      <p>OCR languages: {capabilities.ocrLanguages.join(', ') || 'No models found'}</p>
      {#each capabilities.notes as note}<p>{note}</p>{/each}
    </details>{/if}
  <div class="utility-card">
    <div class="fields">
      <label
        >Utility<select bind:value={kind} disabled={pending}
          ><option value="keyframeCut">Lossless keyframe cut</option><option value="concat"
            >Lossless concatenate</option
          ><option value="colorMetadataTransfer">Transfer color metadata</option><option
            value="subtitleOcr">Subtitle OCR</option
          ><option value="grain">AV1 film grain</option><option value="crfLadder">CRF ladder</option
          ></select
        ></label
      >
      {#if kind !== 'concat'}<label
          >Source<select bind:value={source} disabled={pending}
            ><option value="">Choose a source</option>{#each files as file}<option value={file.path}
                >{file.name}</option
              >{/each}</select
          ></label
        >{/if}
      {#if ['colorMetadataTransfer', 'subtitleOcr', 'crfLadder'].includes(kind)}<label
          >{kind === 'subtitleOcr' ? 'Subtitle' : 'Video'} stream<select
            bind:value={stream}
            disabled={pending}
            >{#each selected?.streams.filter((s) => s.kind === (kind === 'subtitleOcr' ? 'subtitle' : 'video')) ?? [] as s}<option
                value={s.index}>Stream {s.index} · {s.codec ?? 'unknown'} {s.language ?? ''}</option
              >{/each}</select
          ></label
        >{/if}
    </div>
    {#if kind === 'keyframeCut'}
      <p>
        Stream-copy cuts start at a usable keyframe at or before the requested position. The result
        reports the actual interval. Use Quick Convert trim for frame-exact cuts.
      </p>
      <div class="fields">
        <label
          >Start (seconds)<input
            type="number"
            min="0"
            step="0.001"
            bind:value={start}
            disabled={pending}
          /></label
        ><label
          >End (seconds)<input
            type="number"
            min="0.001"
            step="0.001"
            bind:value={end}
            disabled={pending}
          /></label
        >
      </div>
    {:else if kind === 'concat'}
      <p>
        Join files in the displayed order without re-encoding. Video/audio formats and stream
        layouts must match.
      </p>
      <label
        >Add an imported file<select
          value=""
          onchange={(e) => add(e.currentTarget.value)}
          disabled={pending}
          ><option value="">Choose a file</option>{#each files as file}<option
              value={file.path}
              disabled={order.includes(file.path)}>{file.name}</option
            >{/each}</select
        ></label
      >
      <ol>
        {#each order as path, i}<li>
            {fileName(path)}
            <button
              type="button"
              aria-label={'Move concat source ' + (i + 1) + ' up'}
              onclick={() => reorder(i, -1)}
              disabled={pending || i === 0}>↑</button
            ><button
              type="button"
              aria-label={'Move concat source ' + (i + 1) + ' down'}
              onclick={() => reorder(i, 1)}
              disabled={pending || i === order.length - 1}>↓</button
            ><button
              type="button"
              onclick={() => (order = order.filter((_, j) => j !== i))}
              disabled={pending}>Remove</button
            >
          </li>{/each}
      </ol>
    {:else if kind === 'subtitleOcr'}
      <label
        >OCR languages<input
          bind:value={language}
          placeholder="eng or eng+spa"
          disabled={pending}
        /></label
      >
      <p>
        Recognize bitmap subtitle text with installed Tesseract language models. Review the saved
        SRT for recognition errors.
      </p>
    {/if}
    {#if kind === 'colorMetadataTransfer' || (kind === 'grain' && grainOperation === 'measure')}
      <div class="fields">
        <label
          >{kind === 'grain' ? 'Denoised reference' : 'Metadata reference'}<select
            bind:value={reference}
            disabled={pending}
            ><option value="">Choose reference</option>{#each files as file}<option
                value={file.path}>{file.name}</option
              >{/each}</select
          ></label
        >{#if kind === 'colorMetadataTransfer'}<label
            >Reference video stream<select bind:value={referenceStream} disabled={pending}
              >{#each referenceFile?.streams.filter((s) => s.kind === 'video') ?? [] as s}<option
                  value={s.index}>Stream {s.index} · {s.codec}</option
                >{/each}</select
            ></label
          >{/if}
      </div>
      <p>
        {kind === 'grain'
          ? 'Original and denoised videos must have matching geometry, timing and frame count.'
          : 'Copy supported color and HDR declarations while preserving encoded picture data. This changes tags; it does not convert the image colors.'}
      </p>
    {/if}
    {#if kind === 'grain'}
      <div class="fields">
        <label
          >Grain operation<select bind:value={grainOperation} disabled={pending}
            ><option value="extract">Extract grain table</option><option value="measure"
              >Measure against denoised video</option
            ><option value="apply">Apply grain</option><option value="rewriteHeaders"
              >Replace grain headers</option
            ><option value="remove">Remove grain</option></select
          ></label
        >
        {#if grainOperation === 'apply' || grainOperation === 'rewriteHeaders'}<label
            >Grain source<select bind:value={grainSource} disabled={pending}
              ><option value="photonNoise">Photon noise strength</option><option value="table"
                >Grain table</option
              ><option value="preset">Tool preset</option></select
            ></label
          >
          {#if grainSource === 'table'}<label
              >Table<input bind:value={tablePath} readonly /><button
                type="button"
                onclick={chooseTable}
                disabled={pending}>Choose grain table</button
              ></label
            >{:else if grainSource === 'preset'}<label
              >Preset<select bind:value={grainPreset} disabled={pending}
                ><option value="">Check utility tools first</option
                >{#each capabilities?.grainPresets ?? [] as value}<option {value}>{value}</option
                  >{/each}</select
              ></label
            >{:else}<label
              >ISO strength<input
                type="number"
                min="1"
                step="1"
                bind:value={iso}
                disabled={pending}
              /></label
            ><label
              ><input type="checkbox" bind:checked={chroma} disabled={pending} />Include chroma
              grain</label
            >{/if}
        {/if}
      </div>
    {/if}
    {#if kind === 'crfLadder'}
      <p>
        Compare aligned samples using the selected FFmpeg encoder. Size estimates describe video
        only. CRF and preset recommendations may not transfer to another encoder build or workflow.
      </p>
      <div class="fields">
        <label
          >Encoder<select
            bind:value={encoder}
            disabled={pending}
            onchange={() => (preset = encoder === 'av1' ? '8' : encoder === 'vp9' ? '2' : 'medium')}
            ><option value="h264">H.264 · libx264</option><option value="hevc"
              >HEVC · libx265</option
            ><option value="av1">AV1 · libsvtav1</option><option value="vp9"
              >VP9 · libvpx-vp9</option
            ></select
          ></label
        ><label>Preset / speed<input bind:value={preset} disabled={pending} /></label><label
          >Pixel format<select bind:value={pixel} disabled={pending}
            ><option value="yuv420p">8-bit 4:2:0</option><option value="yuv420p10le"
              >10-bit 4:2:0</option
            ></select
          ></label
        ><label>CRF values<input bind:value={crfs} disabled={pending} /></label><label
          >Sample count<input
            type="number"
            min="1"
            max="10"
            bind:value={samples}
            disabled={pending}
          /></label
        ><label
          >Seconds per sample<input
            type="number"
            min="0.1"
            max="30"
            step="0.1"
            bind:value={seconds}
            disabled={pending}
          /></label
        ><label
          >Quality metric<select bind:value={metric} disabled={pending}
            ><option value="none">Size and speed only</option><option value="ssim">SSIM</option
            ><option value="psnr">PSNR</option><option value="vmaf">VMAF</option></select
          ></label
        >{#if metric !== 'none'}<label
            >Recommendation threshold<input
              bind:value={threshold}
              placeholder="Use metric default"
              disabled={pending}
            /></label
          >{/if}
      </div>
    {/if}
    <button
      class="primary-action"
      type="button"
      onclick={run}
      disabled={pending || !isDesktop() || (kind === 'concat' ? order.length < 2 : !selected)}
      >{pending ? 'Running…' : kind === 'crfLadder' ? 'Analyze ladder' : 'Run utility'}</button
    >{#if pending}<button type="button" onclick={() => controller?.abort()}>Cancel utility</button
      >{/if}
    {#if error}<p role="alert">{error}</p>{/if}
    {#if result}
      <div class="result" role="status">
        <h2>Completed</h2>
        <p>{result.result.message}</p>
        {#if result.kind === 'crfLadder'}
          <p>Recommended CRF: {result.result.recommendedCrf ?? 'No rung meets the threshold'}</p>
          <div class="table-scroll">
            <table>
              <thead
                ><tr
                  ><th>CRF</th><th>Quality</th><th>Video kb/s</th><th>Estimated video size</th><th
                    >Encode seconds</th
                  ></tr
                ></thead
              ><tbody
                >{#each result.result.rungs as rung}<tr
                    ><td>{rung.crf}</td><td>{rung.score?.toFixed(4) ?? '—'}</td><td
                      >{rung.bitrateKbps.toFixed(1)}</td
                    ><td>{formatBytes(rung.projectedSizeBytes)}</td><td
                      >{rung.encodeSeconds.toFixed(2)}</td
                    ></tr
                  >{/each}</tbody
              >
            </table>
          </div>
        {:else}<p class="path">{result.result.outputPath}</p>
          {#if result.kind === 'artifact'}{@const outputPath = result.result.outputPath}<button
              type="button"
              onclick={() => onimport([outputPath])}>Import output</button
            >{/if}{/if}
        <details>
          <summary>Validation details</summary>{#each result.result.diagnostics as line}<p>
              {line}
            </p>{/each}
        </details>
      </div>
    {/if}
  </div>
  <ImageWorkflows {files} {onimport} />
</section>

<style>
  .utilities {
    padding: 1rem 0;
  }
  .utility-card {
    padding: 1.25rem;
    background: #e3dacc;
    border: 1px solid #c0b7aa;
    margin: 1rem 0;
  }
  header {
    display: flex;
    align-items: start;
    justify-content: space-between;
    gap: 1rem;
  }
  .fields {
    display: flex;
    flex-wrap: wrap;
    gap: 1rem;
    margin: 1rem 0;
  }
  label {
    display: flex;
    flex-direction: column;
    gap: 0.35rem;
  }
  input,
  select,
  button {
    font: inherit;
    padding: 0.5rem;
    border: 1px solid #9b8c7a;
    background: #f0eee6;
  }
  .primary-action {
    background: var(--primary);
    border-color: var(--primary);
    color: var(--primary-foreground);
  }
  button:disabled {
    cursor: not-allowed;
    opacity: 0.55;
  }
  button {
    cursor: pointer;
    margin: 0.25rem 0.4rem 0.25rem 0;
  }
  input[type='checkbox'] {
    width: 1rem;
    height: 1rem;
  }
  p {
    max-width: 95ch;
  }
  .path {
    overflow-wrap: anywhere;
  }
  .result {
    border-top: 1px solid #b0a392;
    margin-top: 1rem;
    padding-top: 1rem;
  }
  .table-scroll {
    overflow: auto;
  }
  table {
    border-collapse: collapse;
  }
  th,
  td {
    text-align: left;
    padding: 0.6rem;
    border-bottom: 1px solid #b0a392;
  }
  li {
    margin: 0.5rem 0;
  }
</style>
