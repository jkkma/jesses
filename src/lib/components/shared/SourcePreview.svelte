<script lang="ts">
  import { onDestroy, untrack } from 'svelte';
  import Button from '$lib/components/ui/button/button.svelte';
  import { detectCrop, previewFrame } from '$lib/ipc/client';
  import type {
    AutoCropResult,
    CropSettings,
    FramePreviewResult,
    MediaFile,
  } from '$lib/ipc/generated';
  import { errorMessage } from './format';

  let {
    file,
    videoStreamIndex,
    crop,
    onapply,
    disabled = false,
  }: {
    file: MediaFile;
    videoStreamIndex: number;
    crop: CropSettings;
    onapply: (crop: CropSettings) => void;
    disabled?: boolean;
  } = $props();

  const id = $props.id();
  const stream = $derived(file.streams.find((item) => item.index === videoStreamIndex));
  const frameDuration = $derived.by(() => {
    const [numerator, denominator] = (stream?.frameRate ?? '').split('/').map(Number);
    return numerator > 0 && denominator > 0 ? denominator / numerator : 0.05;
  });
  const maximum = $derived(
    file.durationSeconds === null
      ? 86400
      : Math.max(0, Math.min(86400, file.durationSeconds - frameDuration)),
  );
  let expanded = $state(false);
  let position = $state(0);
  let preview = $state<FramePreviewResult | null>(null);
  let proposal = $state<AutoCropResult | null>(null);
  let loading = $state(false);
  let detecting = $state(false);
  let previewError = $state('');
  let cropError = $state('');
  let applied = $state(false);
  let previewAbort: AbortController | null = null;
  let cropAbort: AbortController | null = null;
  let previewGeneration = 0;
  let cropGeneration = 0;
  let activeSource = '';
  let timer: ReturnType<typeof setTimeout> | undefined;

  const shownCrop = $derived(proposal?.crop ?? crop);
  const cropFits = $derived(
    !!preview &&
      Object.values(shownCrop).every((edge) => Number.isFinite(edge) && edge >= 0) &&
      shownCrop.left + shownCrop.right < preview.sourceWidth &&
      shownCrop.top + shownCrop.bottom < preview.sourceHeight,
  );
  const identityMatches = $derived(
    !!preview && !!proposal && preview.sourceFingerprint === proposal.sourceFingerprint,
  );

  function cancel() {
    clearTimeout(timer);
    ++previewGeneration;
    ++cropGeneration;
    previewAbort?.abort();
    cropAbort?.abort();
    previewAbort = null;
    cropAbort = null;
    loading = false;
    detecting = false;
  }

  $effect(() => {
    const source = `${file.path}\0${videoStreamIndex}`;
    untrack(() => {
      if (activeSource === source) return;
      activeSource = source;
      cancel();
      position = 0;
      preview = null;
      proposal = null;
      previewError = '';
      cropError = '';
      applied = false;
      if (expanded && !disabled) void loadPreview();
    });
  });

  $effect(() => {
    if (disabled) untrack(cancel);
  });
  onDestroy(cancel);

  async function loadPreview() {
    clearTimeout(timer);
    if (disabled || !expanded) return;
    if (!Number.isFinite(position) || position < 0 || position > maximum) {
      previewError = 'Choose a position within this source.';
      return;
    }
    previewAbort?.abort();
    const controller = new AbortController();
    previewAbort = controller;
    const generation = ++previewGeneration;
    const source = activeSource;
    const request = { inputPath: file.path, videoStreamIndex, positionSeconds: position };
    loading = true;
    previewError = '';
    try {
      const result = await previewFrame(request, controller.signal);
      if (controller.signal.aborted || source !== activeSource || generation !== previewGeneration)
        return;
      preview = result;
      if (proposal && proposal.sourceFingerprint !== result.sourceFingerprint) {
        proposal = null;
        cropError = 'The source changed since detection. Detect the crop again for this image.';
      }
    } catch (cause) {
      if (!controller.signal.aborted && source === activeSource && generation === previewGeneration)
        previewError = errorMessage(cause);
    } finally {
      if (generation === previewGeneration) loading = false;
    }
  }

  function seek(input: HTMLInputElement) {
    position = input.valueAsNumber;
    clearTimeout(timer);
    // Cancel immediately; waiting for the debounce must not publish an older
    // seek result under the newly selected position.
    previewAbort?.abort();
    ++previewGeneration;
    loading = false;
    timer = setTimeout(() => void loadPreview(), 180);
  }

  async function detect() {
    if (disabled || !expanded) return;
    cropAbort?.abort();
    const controller = new AbortController();
    cropAbort = controller;
    const generation = ++cropGeneration;
    const source = activeSource;
    const request = { inputPath: file.path, videoStreamIndex };
    detecting = true;
    cropError = '';
    proposal = null;
    applied = false;
    try {
      const result = await detectCrop(request, controller.signal);
      if (controller.signal.aborted || source !== activeSource || generation !== cropGeneration)
        return;
      if (preview && preview.sourceFingerprint !== result.sourceFingerprint) {
        cropError = 'The source changed since the preview. Refresh the image, then detect again.';
        return;
      }
      proposal = result;
    } catch (cause) {
      if (!controller.signal.aborted && source === activeSource && generation === cropGeneration)
        cropError = errorMessage(cause);
    } finally {
      if (generation === cropGeneration) detecting = false;
    }
  }

  function apply() {
    if (disabled || !proposal?.crop || !identityMatches) return;
    onapply({ ...proposal.crop });
    proposal = null;
    applied = true;
  }
</script>

<div class="source-preview">
  <button
    type="button"
    class="preview-toggle"
    aria-expanded={expanded}
    onclick={() => {
      expanded = !expanded;
      if (expanded && !preview) void loadPreview();
      if (!expanded) cancel();
    }}
    ><span class="disclosure-arrow" class:expanded aria-hidden="true"></span>Source preview &
    automatic crop</button
  >
  {#if expanded}
    <div class="preview-content">
      <p class="small-muted">
        Inspect the source before resize and borders. The outline shows
        {proposal?.crop ? 'the proposed crop' : 'your current crop'}.
      </p>
      {#if preview}
        <figure>
          <div class="preview-image" style={`aspect-ratio: ${preview.width} / ${preview.height}`}>
            <img
              src={preview.imageDataUrl}
              alt={`Source video preview of ${file.name}`}
              width={preview.width}
              height={preview.height}
            />
            {#if cropFits}
              <div
                class="crop-outline"
                class:proposed={!!proposal?.crop}
                style={`left:${(shownCrop.left / preview.sourceWidth) * 100}%;top:${(shownCrop.top / preview.sourceHeight) * 100}%;right:${(shownCrop.right / preview.sourceWidth) * 100}%;bottom:${(shownCrop.bottom / preview.sourceHeight) * 100}%`}
                aria-hidden="true"
              ></div>
            {/if}
          </div>
          <figcaption class="small-muted">
            Source {preview.sourceWidth} × {preview.sourceHeight}; requested position
            {preview.positionSeconds.toFixed(2)} s.
            {#if preview.toneMapped}HDR is shown as SDR for this preview only.{/if}
          </figcaption>
        </figure>
      {:else}
        <p class="preview-placeholder small-muted">
          {loading ? 'Reading a source frame…' : 'Choose a position and load a source frame.'}
        </p>
      {/if}
      <div class="preview-controls">
        <div class="position-field">
          <label for={`${id}-position`}>Position (seconds)</label>
          <input
            id={`${id}-position`}
            type="number"
            min="0"
            max={maximum}
            step="0.01"
            value={position}
            {disabled}
            onchange={(event) => seek(event.currentTarget)}
          />
        </div>
        <Button variant="outline" onclick={loadPreview} {disabled}>
          {loading ? 'Updating preview…' : 'Refresh preview'}
        </Button>
      </div>
      {#if file.durationSeconds !== null && maximum > 0}
        <input
          class="seek-slider"
          type="range"
          min="0"
          max={maximum}
          step="0.01"
          value={position}
          aria-label="Source preview position"
          {disabled}
          oninput={(event) => seek(event.currentTarget)}
        />
      {/if}
      {#if previewError}<p role="alert" class="analysis-error">{previewError}</p>{/if}
      <div class="analysis-actions">
        <Button variant="outline" onclick={detect} disabled={disabled || detecting}>
          {detecting ? 'Detecting crop…' : 'Detect black borders'}
        </Button>
        {#if loading || detecting}<Button variant="ghost" onclick={cancel}>Cancel analysis</Button
          >{/if}
      </div>
      {#if proposal}
        <div class="crop-proposal" aria-live="polite">
          <p>{proposal.message}</p>
          {#if proposal.crop}
            <p class="crop-values">
              Proposed crop: top {proposal.crop.top}, right {proposal.crop.right}, bottom
              {proposal.crop.bottom}, left {proposal.crop.left} pixels.
            </p>
            <p class="small-muted">
              {proposal.agreementPercent}% of usable frame detections agree across
              {proposal.sampleCount} sampled positions.
            </p>
            <div class="analysis-actions">
              <Button onclick={apply} disabled={disabled || !identityMatches}
                >Apply detected crop</Button
              >
              <Button variant="ghost" onclick={() => (proposal = null)}>Dismiss proposal</Button>
            </div>
            {#if !preview}<p class="small-muted">
                Load a source preview before applying this crop.
              </p>{/if}
          {/if}
        </div>
      {/if}
      {#if applied}<p class="small-muted" role="status">Detected crop applied to this file.</p>{/if}
      {#if cropError}<p role="alert" class="analysis-error">{cropError}</p>{/if}
    </div>
  {/if}
</div>

<style>
  .source-preview {
    margin-top: 16px;
    border: 1px solid var(--border);
    border-radius: 8px;
    min-width: 0;
  }
  .preview-toggle {
    display: flex;
    align-items: center;
    gap: 8px;
    width: 100%;
    padding: 12px;
    border: 0;
    border-radius: 8px;
    color: inherit;
    background: transparent;
    text-align: left;
    cursor: pointer;
    font-size: 12px;
    font-weight: 600;
  }
  .preview-toggle:focus-visible {
    outline: 2px solid var(--ring);
    outline-offset: 2px;
  }
  .disclosure-arrow {
    width: 6px;
    height: 6px;
    border-right: 1.5px solid currentColor;
    border-top: 1.5px solid currentColor;
    transform: rotate(45deg);
  }
  .disclosure-arrow.expanded {
    transform: rotate(135deg);
  }
  .preview-content {
    display: grid;
    gap: 12px;
    padding: 0 12px 12px;
  }
  figure {
    margin: 0;
    min-width: 0;
  }
  .preview-image {
    position: relative;
    overflow: hidden;
    background: #111;
  }
  img {
    display: block;
    width: 100%;
    height: 100%;
    object-fit: contain;
  }
  .crop-outline {
    position: absolute;
    border: 2px solid #ffb26b;
    box-shadow: 0 0 0 9999px #0008;
    pointer-events: none;
  }
  .crop-outline.proposed {
    border-style: dashed;
  }
  figcaption {
    margin-top: 8px;
  }
  .preview-controls,
  .analysis-actions {
    display: flex;
    align-items: end;
    flex-wrap: wrap;
    gap: 8px;
  }
  .position-field {
    display: grid;
    gap: 6px;
    font-size: 12px;
  }
  .position-field input {
    width: 140px;
  }
  .seek-slider {
    width: 100%;
    min-width: 0;
    accent-color: #ad5326;
  }
  .preview-placeholder,
  .crop-proposal {
    border-radius: 6px;
    background: var(--muted);
    padding: 12px;
  }
  .crop-proposal {
    display: grid;
    gap: 10px;
    font-size: 12px;
  }
  p {
    margin: 0;
    overflow-wrap: anywhere;
  }
  .analysis-error {
    color: var(--destructive);
    font-size: 12px;
  }
</style>
