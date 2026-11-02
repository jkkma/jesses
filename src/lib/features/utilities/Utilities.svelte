<script lang="ts">
  import { onDestroy } from 'svelte';
  import {
    isDesktop,
    chooseUtilityDestination,
    chooseUtilityFile,
    chooseOutputFolder,
    inspectUtilityCapabilities,
    runUtility,
    runUtilityBatch,
    setMediaAnalysisPaused,
    detectFields,
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
    DeinterlaceExportMethod,
    DeinterlaceMode,
    FieldOrder,
    QtgmcPreset,
    QualityVmafModel,
    UtilityBatchResult,
    UtilityProgress,
  } from '$lib/ipc/generated';
  import {
    errorMessage,
    formatBytes,
    fileName,
    formatDuration,
  } from '$lib/components/shared/format';
  import ImageWorkflows from './ImageWorkflows.svelte';
  import SourcePreview from '$lib/components/shared/SourcePreview.svelte';
  import contentPresets from '$lib/content-presets.json';
  let { files, onimport }: { files: MediaFile[]; onimport: (paths: string[]) => void } = $props();
  let kind = $state<
    | 'keyframeCut'
    | 'concat'
    | 'colorMetadataTransfer'
    | 'subtitleOcr'
    | 'grain'
    | 'crfLadder'
    | 'deinterlaceExport'
    | 'cadenceRepairExport'
  >('keyframeCut');
  let source = $state('');
  let stream = $state(0);
  let reference = $state('');
  let referenceStream = $state(0);
  let start = $state(0);
  let end = $state(10);
  let order = $state<string[]>([]);
  let language = $state('eng');
  let grainOperation = $state<
    'measureSource' | 'measure' | 'extract' | 'apply' | 'rewriteHeaders' | 'remove'
  >('extract');
  let grainSource = $state<'table' | 'preset' | 'photonNoise'>('photonNoise');
  let tablePath = $state('');
  let grainPreset = $state('');
  let iso = $state(400);
  let chroma = $state(false);
  let denoiseStrength = $state(4);
  let keepDenoised = $state(false);
  let encoder = $state<LadderEncoder>('h264');
  $effect(() => {
    if (['h264Nvenc', 'hevcNvenc'].includes(encoder)) {
      encoder = 'h264';
      preset = 'medium';
      pixel = 'yuv420p';
      crfs = '18, 23, 28';
    }
  });
  let preset = $state('medium');
  let pixel = $state('yuv420p');
  let crfs = $state('18, 23, 28');
  let samples = $state(3);
  let seconds = $state(2);
  let metric = $state<LadderMetric>('vmaf');
  let threshold = $state('');
  let contentPreset = $state('');
  let vmafModel = $state<QualityVmafModel>('standard');
  let samplesDirectory = $state('');
  let deinterlaceMethod = $state<DeinterlaceExportMethod>('qtgmc');
  let deinterlaceMode = $state<DeinterlaceMode>('bob');
  let fieldOrder = $state<FieldOrder>('topFirst');
  let qtgmcPreset = $state<QtgmcPreset>('verySlow');
  let pending = $state(false);
  let batch = $state(false);
  let batchResult = $state<UtilityBatchResult | null>(null);
  let utilityProgress = $state<UtilityProgress | null>(null);
  let rowProgress = $state<Record<number, UtilityProgress>>({});
  let analysisId = $state<string | null>(null);
  let paused = $state(false);
  let pausing = $state(false);
  let checking = $state(false);
  let error = $state<string | null>(null);
  let capabilities = $state<UtilityCapabilities | null>(null);
  let result = $state<UtilityResult | null>(null);
  let controller: AbortController | undefined;
  let capabilityController: AbortController | undefined;
  let fieldController: AbortController | undefined;
  let fieldDetecting = $state(false);
  let fieldResult = $state<Awaited<ReturnType<typeof detectFields>> | null>(null);
  let detectedSource = '';
  const selected = $derived(files.find((f) => f.path === source));
  const grainEstimate = $derived.by(() => {
    const video = selected?.streams.find((item) => item.index === stream);
    const [n, d] = (video?.frameRate ?? '').split('/').map(Number);
    const fps = d ? n / d : n;
    const seconds = video?.durationSeconds ?? selected?.durationSeconds;
    return video?.width && video.height && seconds && fps > 0
      ? (video.width * video.height * seconds * fps) / 7_200_000
      : null;
  });
  function onUtilityProgress(progress: UtilityProgress) {
    utilityProgress = progress;
    if (progress.rowIndex != null) rowProgress = { ...rowProgress, [progress.rowIndex]: progress };
  }
  const referenceFile = $derived(files.find((f) => f.path === reference));
  const ladderPresets = $derived(
    encoder === 'h264'
      ? contentPresets.x264
      : encoder === 'hevc'
        ? contentPresets.x265
        : encoder === 'av1'
          ? contentPresets.svtAv1
          : [],
  );
  $effect(() => {
    if (!source && files.length) source = files[0].path;
  });
  $effect(() => {
    stream =
      selected?.streams.find((s) => s.kind === (kind === 'subtitleOcr' ? 'subtitle' : 'video'))
        ?.index ?? 0;
  });
  $effect(() => {
    const order = selected?.streams.find((s) => s.index === stream)?.fieldOrder;
    if (order && ['bb', 'bt', 'bff'].includes(order)) fieldOrder = 'bottomFirst';
    else if (order && ['tt', 'tb', 'tff'].includes(order)) fieldOrder = 'topFirst';
  });
  $effect(() => {
    referenceStream = referenceFile?.streams.find((s) => s.kind === 'video')?.index ?? 0;
  });
  $effect(() => {
    const identity = `${source}\0${stream}`;
    if (identity !== detectedSource) {
      detectedSource = identity;
      fieldController?.abort();
      fieldResult = null;
      fieldDetecting = false;
    }
  });
  onDestroy(() => {
    controller?.abort();
    capabilityController?.abort();
    fieldController?.abort();
  });
  async function detectSourceFields() {
    fieldController?.abort();
    const active = new AbortController();
    fieldController = active;
    fieldDetecting = true;
    fieldResult = null;
    error = null;
    try {
      const result = await detectFields(
        { inputPath: source, videoStreamIndex: stream },
        active.signal,
      );
      if (!active.signal.aborted) fieldResult = result;
    } catch (e) {
      if (!active.signal.aborted) error = errorMessage(e);
    } finally {
      if (fieldController === active) fieldDetecting = false;
    }
  }
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
  async function chooseSamplesDirectory() {
    try {
      const parent = await chooseOutputFolder();
      if (!parent) return;
      const separator = parent.includes('\\') ? '\\' : '/';
      const name = fileName(source).replace(/\.[^./\\]+$/, '') || 'source';
      samplesDirectory = `${parent.replace(/[\\/]+$/, '')}${separator}${name}-ladder-samples`;
    } catch (e) {
      error = errorMessage(e);
    }
  }
  async function togglePause() {
    if (!analysisId || pausing) return;
    pausing = true;
    try {
      await setMediaAnalysisPaused(analysisId, !paused);
      paused = !paused;
    } catch (e) {
      error = errorMessage(e);
    } finally {
      pausing = false;
    }
  }
  function batchOutput(
    folder: string,
    file: MediaFile,
    index: number,
    ext: string,
    suffix: string = kind,
  ): string {
    const separator = folder.includes('\\') ? '\\' : '/';
    const name = file.name.replace(/\.[^./\\]+$/, '') || 'source';
    return `${folder.replace(/[\\/]+$/, '')}${separator}${name}-${suffix}-${index + 1}.${ext}`;
  }
  function makeBatchRequest(
    request: UtilityRequest,
    file: MediaFile,
    index: number,
    folder: string,
    ext: string,
  ): UtilityRequest {
    const video = file.streams.find((item) => item.kind === 'video')?.index ?? 0;
    const subtitle = file.streams.find((item) => item.kind === 'subtitle')?.index ?? 0;
    const outputPath = batchOutput(folder, file, index, ext);
    switch (request.kind) {
      case 'keyframeCut':
        return {
          kind: request.kind,
          request: { ...request.request, inputPath: file.path, outputPath },
        };
      case 'colorMetadataTransfer':
        return {
          kind: request.kind,
          request: {
            ...request.request,
            inputPath: file.path,
            inputVideoStreamIndex: video,
            outputPath,
          },
        };
      case 'subtitleOcr':
        return {
          kind: request.kind,
          request: {
            ...request.request,
            inputPath: file.path,
            subtitleStreamIndex: subtitle,
            outputPath,
          },
        };
      case 'deinterlaceExport':
        return {
          kind: request.kind,
          request: {
            ...request.request,
            inputPath: file.path,
            videoStreamIndex: video,
            outputPath,
          },
        };
      case 'cadenceRepairExport':
        return {
          kind: request.kind,
          request: {
            ...request.request,
            inputPath: file.path,
            videoStreamIndex: video,
            outputPath,
          },
        };
      case 'crfLadder':
        return {
          kind: request.kind,
          request: {
            ...request.request,
            inputPath: file.path,
            videoStreamIndex: video,
            ...(request.request.keepSamplesDirectory
              ? { keepSamplesDirectory: batchOutput(folder, file, index, 'samples', 'ladder') }
              : {}),
          },
        };
      case 'grain': {
        const grain = request.request;
        if (grain.operation === 'measureSource')
          return {
            kind: 'grain',
            request: {
              ...grain,
              sourcePath: file.path,
              outputTablePath: outputPath,
              ...(keepDenoised
                ? { denoisedOutputPath: batchOutput(folder, file, index, 'mkv', 'denoised') }
                : {}),
            },
          };
        if (grain.operation === 'measure')
          return {
            kind: 'grain',
            request: { ...grain, sourcePath: file.path, outputTablePath: outputPath },
          };
        if (grain.operation === 'extract')
          return {
            kind: 'grain',
            request: { ...grain, inputPath: file.path, outputTablePath: outputPath },
          };
        return { kind: 'grain', request: { ...grain, inputPath: file.path, outputPath } };
      }
      case 'concat':
        return request;
    }
  }
  async function run() {
    pending = true;
    error = null;
    result = null;
    batchResult = null;
    utilityProgress = null;
    rowProgress = {};
    paused = false;
    const active = new AbortController();
    controller = active;
    try {
      if (kind !== 'concat' && !selected) throw new Error('Import and choose a source first.');
      const inBatch = batch && kind !== 'concat';
      const batchFolder = inBatch ? await chooseOutputFolder() : null;
      if (inBatch && !batchFolder) return;
      let request: UtilityRequest;
      let outputExt = 'mkv';
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
            vmafModel,
            ...(contentPreset
              ? {
                  contentPreset,
                  parameters:
                    ladderPresets.find((item) => item.name === contentPreset)?.parameters ?? [],
                }
              : {}),
            ...(samplesDirectory ? { keepSamplesDirectory: samplesDirectory } : {}),
          },
        };
      } else {
        const ext =
          kind === 'subtitleOcr'
            ? 'srt'
            : kind === 'grain' && ['measureSource', 'measure', 'extract'].includes(grainOperation)
              ? 'tbl'
              : 'mkv';
        outputExt = ext;
        const output = inBatch
          ? batchOutput(batchFolder!, selected!, 0, ext)
          : await chooseUtilityDestination(kind + '-output.' + ext, [ext]);
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
        else if (kind === 'deinterlaceExport')
          request = {
            kind,
            request: {
              inputPath: source,
              outputPath: output,
              videoStreamIndex: stream,
              method: deinterlaceMethod,
              mode: deinterlaceMode,
              fieldOrder,
              qtgmcPreset,
            },
          };
        else if (kind === 'cadenceRepairExport')
          request = {
            kind,
            request: { inputPath: source, outputPath: output, videoStreamIndex: stream },
          };
        else {
          let grain: GrainRequest;
          if (grainOperation === 'measureSource') {
            const denoisedOutputPath =
              keepDenoised && !inBatch
                ? await chooseUtilityDestination('denoised-output.mkv', ['mkv'])
                : null;
            if (keepDenoised && !inBatch && !denoisedOutputPath) return;
            grain = {
              operation: 'measureSource',
              sourcePath: source,
              outputTablePath: output,
              denoiseStrength,
              ...(denoisedOutputPath ? { denoisedOutputPath } : {}),
            };
          } else if (grainOperation === 'measure')
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
      if (inBatch) {
        const requests = files.map((file, index) =>
          makeBatchRequest(request, file, index, batchFolder!, outputExt),
        );
        batchResult = await runUtilityBatch(
          { requests },
          active.signal,
          (id) => (analysisId = id),
          onUtilityProgress,
        );
      } else
        result = await runUtility(
          request,
          active.signal,
          (id) => (analysisId = id),
          onUtilityProgress,
        );
    } catch (e) {
      error = active.signal.aborted ? 'Utility canceled.' : errorMessage(e);
    } finally {
      pending = false;
      controller = undefined;
      paused = false;
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
  {#if capabilities}<details class="capabilities">
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
          ><option value="deinterlaceExport">Deinterlace to new file</option><option
            value="cadenceRepairExport">Repair padded cadence</option
          ></select
        ></label
      >
      {#if kind !== 'concat'}<label class="source-field"
          >Source<select bind:value={source} disabled={pending}
            ><option value="">Choose a source</option>{#each files as file}<option value={file.path}
                >{file.name}</option
              >{/each}</select
          ></label
        >{/if}
      {#if ['colorMetadataTransfer', 'subtitleOcr', 'crfLadder', 'deinterlaceExport', 'cadenceRepairExport'].includes(kind)}<label
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
        <label class="compact-field"
          >Start (seconds)<input
            type="number"
            min="0"
            step="0.001"
            bind:value={start}
            disabled={pending}
          /></label
        ><label class="compact-field"
          >End (seconds)<input
            type="number"
            min="0.001"
            step="0.001"
            bind:value={end}
            disabled={pending}
          /></label
        >
      </div>
      {#if selected && selected.streams.some((item) => item.kind === 'video')}
        <SourcePreview
          file={selected}
          videoStreamIndex={selected.streams.find((item) => item.kind === 'video')!.index}
          crop={{ top: 0, right: 0, bottom: 0, left: 0 }}
          onapply={() => {}}
          showCrop={false}
          snapOnStart={true}
          disabled={pending || !isDesktop()}
          onmarkstart={(seconds) => (start = Math.round(seconds * 1_000) / 1_000)}
          onmarkend={(seconds) => (end = Math.round(seconds * 1_000) / 1_000)}
        />
      {/if}
    {:else if kind === 'concat'}
      <p>
        Join files in the displayed order without re-encoding. Video/audio formats and stream
        layouts must match.
      </p>
      <label class="source-field standalone-field"
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
      <label class="compact-field standalone-field"
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
    {:else if kind === 'deinterlaceExport'}
      <p>
        Export a progressive near-lossless H.264 Matroska file. Audio, subtitles, attachments, and
        source files remain separate from this new output.
      </p>
      <div class="fields">
        <label
          >Method<select bind:value={deinterlaceMethod} disabled={pending}
            ><option value="qtgmc">QTGMC · motion compensated</option><option value="bwdif"
              >BWDIF</option
            ><option value="yadif">YADIF</option></select
          ></label
        >
        <label
          >Output rate<select bind:value={deinterlaceMode} disabled={pending}
            ><option value="bob">One frame per field · double rate</option><option value="frame"
              >One frame per source frame</option
            ></select
          ></label
        >
        <label
          >Field order<select bind:value={fieldOrder} disabled={pending}
            ><option value="topFirst">Top field first</option><option value="bottomFirst"
              >Bottom field first</option
            ></select
          ></label
        >
        {#if deinterlaceMethod === 'qtgmc'}<label
            >QTGMC preset<select bind:value={qtgmcPreset} disabled={pending}
              ><option value="faster">Faster</option><option value="fast">Fast</option><option
                value="medium">Medium</option
              ><option value="slow">Slow</option><option value="slower">Slower</option><option
                value="verySlow">Very Slow · default</option
              ></select
            ></label
          >{/if}
      </div>
      <div class="actions">
        <button
          type="button"
          onclick={detectSourceFields}
          disabled={pending || fieldDetecting || !selected || !isDesktop()}
          >{fieldDetecting ? 'Inspecting fields…' : 'Detect field order'}</button
        >{#if fieldDetecting}<button type="button" onclick={() => fieldController?.abort()}
            >Cancel detection</button
          >{/if}{#if fieldResult && ['topFirst', 'bottomFirst'].includes(fieldResult.order)}<button
            type="button"
            onclick={() =>
              (fieldOrder = fieldResult!.order === 'bottomFirst' ? 'bottomFirst' : 'topFirst')}
            >Apply detected field order</button
          >{/if}
      </div>
      {#if fieldResult}<p role="status">{fieldResult.evidence}</p>{/if}
    {:else if kind === 'cadenceRepairExport'}
      <p>
        Repair an interlaced capture with more than 2% repeated-frame padding. The utility compares
        decoded frame content with presentation times, exports at the source's declared rate, and
        leaves fields interlaced for later deinterlacing.
      </p>
    {/if}
    {#if kind === 'colorMetadataTransfer' || (kind === 'grain' && grainOperation === 'measure')}
      <div class="fields">
        <label class="source-field"
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
        <label class="source-field"
          >Grain operation<select bind:value={grainOperation} disabled={pending}
            ><option value="extract">Extract grain table</option><option value="measureSource"
              >Measure grain from source (denoise automatically)</option
            ><option value="measure">Measure against denoised video</option><option value="apply"
              >Apply grain</option
            ><option value="rewriteHeaders">Replace grain headers</option><option value="remove"
              >Remove grain</option
            ></select
          ></label
        >
        {#if grainOperation === 'measureSource'}
          <label class="compact-field"
            >Denoise strength<input
              type="number"
              min="1"
              max="16"
              step="1"
              bind:value={denoiseStrength}
              disabled={pending}
            /></label
          >
          <label class="checkbox-field"
            ><input type="checkbox" bind:checked={keepDenoised} disabled={pending} />Keep denoised
            copy</label
          >
          <p>
            Creates a temporary denoised reference, measures the grain difference, then removes the
            reference unless you keep it. Long sources can take hours.
          </p>
          {#if grainEstimate}<p>
              Approximate source measurement time: {formatDuration(grainEstimate)}. Actual time
              depends on your computer and source.
            </p>{/if}
        {/if}
        {#if grainOperation === 'apply' || grainOperation === 'rewriteHeaders'}<label
            >Grain source<select bind:value={grainSource} disabled={pending}
              ><option value="photonNoise">Photon noise strength</option><option value="table"
                >Grain table</option
              ><option value="preset">Tool preset</option></select
            ></label
          >
          {#if grainSource === 'table'}<label class="source-field"
              >Table<span class="input-action"
                ><input bind:value={tablePath} readonly /><button
                  type="button"
                  onclick={chooseTable}
                  disabled={pending}>Choose grain table</button
                ></span
              ></label
            >{:else if grainSource === 'preset'}<label
              >Preset<select bind:value={grainPreset} disabled={pending}
                ><option value="">Check utility tools first</option
                >{#each capabilities?.grainPresets ?? [] as value}<option {value}>{value}</option
                  >{/each}</select
              ></label
            >{:else}<label class="compact-field"
              >ISO strength<input
                type="number"
                min="1"
                step="1"
                bind:value={iso}
                disabled={pending}
              /></label
            ><label class="checkbox-field"
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
            onchange={() => {
              preset =
                encoder === 'av1'
                  ? '8'
                  : encoder === 'vp9'
                    ? '2'
                    : encoder === 'aomAv1'
                      ? '4'
                      : encoder.endsWith('Nvenc')
                        ? 'p4'
                        : 'medium';
              contentPreset = '';
            }}
            ><option value="h264">H.264 · libx264</option><option value="hevc"
              >HEVC · libx265</option
            ><option value="av1">AV1 · libsvtav1</option><option value="vp9"
              >VP9 · libvpx-vp9</option
            ><option value="aomAv1">AV1 · libaom</option></select
          ></label
        >{#if ladderPresets.length}<label
            >Content preset<select bind:value={contentPreset} disabled={pending}
              ><option value="">None</option>{#each ladderPresets as item}<option value={item.name}
                  >{item.name}</option
                >{/each}</select
            ></label
          >{/if}<label class="compact-field"
          >Preset / speed<input bind:value={preset} disabled={pending} /></label
        ><label
          >Pixel format<select bind:value={pixel} disabled={pending}
            ><option value="yuv420p">8-bit 4:2:0</option><option value="yuv420p10le"
              >10-bit 4:2:0</option
            ></select
          ></label
        ><label>CRF values<input bind:value={crfs} disabled={pending} /></label><label
          class="compact-field"
          >Sample count<input
            type="number"
            min="1"
            max="10"
            bind:value={samples}
            disabled={pending}
          /></label
        ><label class="compact-field"
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
            ><option value="none">Size and speed only</option><option value="vmaf">VMAF</option
            ><option value="ssimulacra2">SSIMULACRA2</option><option value="butteraugliInf"
              >Butteraugli INF · lower is better</option
            ><option value="xpsnrWeighted">Weighted XPSNR</option><option value="ssim">SSIM</option
            ><option value="psnr">PSNR</option></select
          ></label
        >{#if metric !== 'none'}<label class="threshold-field"
            >Recommendation threshold<input
              bind:value={threshold}
              placeholder={metric === 'xpsnrWeighted'
                ? 'No automatic threshold'
                : 'Use metric default'}
              disabled={pending}
            /></label
          >{/if}
        {#if metric === 'vmaf'}<label
            >VMAF model<select bind:value={vmafModel} disabled={pending}
              ><option value="standard">Default</option><option value="negative">NEG</option><option
                value="fourK">4K</option
              ></select
            ></label
          >{/if}
      </div>
      {#if contentPreset}<p>
          {ladderPresets.find((item) => item.name === contentPreset)?.description}
        </p>{/if}
      <div class="actions">
        <button type="button" onclick={chooseSamplesDirectory} disabled={pending}
          >Keep sample encodes…</button
        >{#if samplesDirectory}<button
            type="button"
            onclick={() => (samplesDirectory = '')}
            disabled={pending}>Do not keep samples</button
          >{/if}
      </div>
      {#if samplesDirectory}<label class="source-field standalone-field"
          >New samples directory<input bind:value={samplesDirectory} disabled={pending} /></label
        >
        <p>Choose a new path; an existing directory will not be replaced.</p>{/if}
    {/if}
    {#if kind !== 'concat'}<label class="checkbox-field"
        ><input type="checkbox" bind:checked={batch} disabled={pending} />Run this utility for all {files.length}
        imported {files.length === 1 ? 'file' : 'files'}</label
      >{#if batch}<p>
          Each file uses its first applicable stream and receives a uniquely numbered output in the
          chosen folder. Failures are reported per file; later files still run.
        </p>{/if}{/if}
    <div class="actions">
      <button
        class="primary-action"
        type="button"
        onclick={run}
        disabled={pending || !isDesktop() || (kind === 'concat' ? order.length < 2 : !selected)}
        >{pending ? 'Running…' : kind === 'crfLadder' ? 'Analyze ladder' : 'Run utility'}</button
      >{#if pending}<button type="button" onclick={() => controller?.abort()}>Cancel utility</button
        ><button type="button" onclick={togglePause} disabled={!analysisId || pausing}
          >{pausing ? 'Updating…' : paused ? 'Continue utility' : 'Pause utility'}</button
        >{/if}
    </div>
    {#if error}<p role="alert">{error}</p>{/if}
    {#if pending && utilityProgress}<div class="result" role="status">
        <p>{utilityProgress.phase}: {utilityProgress.message}</p>
        <p>
          Elapsed {formatDuration(
            utilityProgress.elapsedSeconds,
          )}{#if utilityProgress.estimatedRemainingSeconds != null}
            · Approximately {formatDuration(utilityProgress.estimatedRemainingSeconds)} remaining{/if}
        </p>
        {#if utilityProgress.durationSeconds && utilityProgress.progressSeconds != null}<progress
            max={utilityProgress.durationSeconds}
            value={utilityProgress.progressSeconds}
          ></progress>{/if}{#if batch}<ul>
            {#each files as file, index}<li>
                {file.name}: {rowProgress[index]?.status ??
                  'waiting'}{#if rowProgress[index]?.message}
                  — {rowProgress[index].message}{/if}
              </li>{/each}
          </ul>{/if}
      </div>{/if}
    {#if batchResult}<div class="result" role="status">
        <h2>Batch {batchResult.canceled ? 'canceled' : 'finished'}</h2>
        <p>{batchResult.completed} of {files.length} utilities completed.</p>
        <ul>
          {#each batchResult.entries as entry}<li>
              <strong>{files[entry.index]?.name ?? `File ${entry.index + 1}`}</strong>: {entry.error
                ? entry.error.message
                : (entry.result?.result.message ?? 'No result returned.')}
            </li>{/each}
        </ul>
      </div>{/if}
    {#if result}
      <div class="result" role="status">
        <h2>Completed</h2>
        <p>{result.result.message}</p>
        {#if result.kind === 'crfLadder'}
          <p>
            Recommended CRF: {result.result.recommendedCrf ??
              (result.result.recommendationThreshold === null
                ? 'No automatic recommendation'
                : 'No rung meets the threshold')}
          </p>
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
                    ><td>{rung.crf}</td><td
                      >{rung.score?.toFixed(4) ??
                        (result.result.metric === 'xpsnrWeighted' || result.result.metric === 'psnr'
                          ? '∞'
                          : '—')}</td
                    ><td>{rung.bitrateKbps.toFixed(1)}</td><td
                      >{formatBytes(rung.projectedSizeBytes)}</td
                    ><td>{rung.encodeSeconds.toFixed(2)}</td></tr
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
    padding: 0.75rem 0;
  }
  .utility-card {
    padding: 14px;
    background: var(--card);
    border: 1px solid #c0b7aa;
    margin: 12px 0;
  }
  header {
    display: flex;
    align-items: start;
    justify-content: space-between;
    gap: 1rem;
  }
  header p {
    margin-top: 0.4rem;
  }
  .capabilities {
    margin-top: 0.5rem;
    padding: 0.65rem 0.8rem;
    border: 1px solid #c0b7aa;
  }
  .capabilities summary {
    cursor: pointer;
    font-weight: 600;
  }
  .capabilities ul {
    margin: 0.75rem 0;
    padding-left: 1.2rem;
  }
  .fields {
    display: flex;
    flex-wrap: wrap;
    align-items: end;
    gap: 12px;
    margin: 12px 0;
  }
  label {
    display: flex;
    flex-direction: column;
    gap: 0.35rem;
    min-width: 0;
  }
  .fields > label {
    flex: 1 1 13rem;
  }
  .fields > .source-field {
    flex-basis: 20rem;
  }
  .fields > .compact-field {
    flex: 0 1 9rem;
  }
  .fields > .threshold-field {
    flex-basis: 12rem;
  }
  .checkbox-field {
    flex: 0 1 auto;
    flex-direction: row;
    align-items: center;
    padding-block: 0.55rem;
  }
  .standalone-field {
    width: min(100%, 28rem);
    margin-block: 12px;
  }
  .input-action {
    display: flex;
    min-width: 0;
    gap: 8px;
  }
  .input-action input {
    flex: 1 1 12rem;
    min-width: 0;
  }
  .input-action button {
    flex: none;
  }
  input,
  select,
  button {
    font: inherit;
    padding: 0.5rem;
    border: 1px solid #9b8c7a;
    background: var(--background);
    max-width: 100%;
  }
  input[type='number'] {
    width: 9rem;
  }
  .compact-field input {
    width: 9rem;
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
  }
  .actions {
    display: flex;
    flex-wrap: wrap;
    gap: 8px;
    margin-top: 12px;
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
  ol {
    max-height: 16rem;
    overflow: auto;
  }
  @media (max-width: 800px) {
    header {
      align-items: stretch;
      flex-direction: column;
    }
    header > button {
      align-self: flex-start;
    }
    .fields > label {
      flex-basis: 12rem;
    }
    .fields > .compact-field {
      flex-basis: 9rem;
    }
    .fields > .checkbox-field {
      flex-basis: auto;
    }
    .fields > .source-field {
      flex-basis: 100%;
    }
  }
</style>
