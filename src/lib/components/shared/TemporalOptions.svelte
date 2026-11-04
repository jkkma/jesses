<script lang="ts">
  import { onDestroy, untrack } from 'svelte';
  import { detectFields, isDesktop } from '$lib/ipc/client';
  import { errorMessage } from './format';
  import type { EncodeBackend, MediaStream } from '$lib/ipc/generated';
  import {
    selectedTemporal,
    FPS_MAX_NUMERATOR,
    FPS_MAX_DENOMINATOR,
    temporalError,
    temporalSummary,
    type TemporalDraft,
  } from './temporal-options';
  let {
    value,
    video,
    backend,
    sourcePath,
    disabled = false,
    autoApply = true,
    onchange,
  }: {
    value: TemporalDraft;
    video?: MediaStream;
    backend: EncodeBackend;
    sourcePath?: string;
    disabled?: boolean;
    autoApply?: boolean;
    onchange: (value: TemporalDraft) => void;
  } = $props();
  let fieldDetection = $state<Awaited<ReturnType<typeof detectFields>> | null>(null);
  let detecting = $state(false);
  let detectionError = $state('');
  let detectionController: AbortController | undefined;
  let previousSource = '';
  let editRevision = 0;
  function update(next: TemporalDraft) {
    editRevision++;
    onchange({ ...next, fieldDetectionManual: true });
  }
  $effect(() => {
    const source = `${sourcePath ?? ''}\0${video?.index ?? ''}`;
    if (source !== previousSource) {
      detectionController?.abort();
      previousSource = source;
      fieldDetection = null;
      detectionError = '';
      detecting = false;
      editRevision = 0;
      if (sourcePath && video && isDesktop() && !disabled) untrack(() => void detect(true));
    }
  });
  onDestroy(() => detectionController?.abort());
  async function detect(automatic = false) {
    if (!sourcePath || !video || disabled) return;
    detectionController?.abort();
    const controller = new AbortController();
    const revision = editRevision;
    const eligibleForAutomaticApplication =
      value.deinterlace === 'off' && !value.fieldDetectionManual;
    detectionController = controller;
    detecting = true;
    fieldDetection = null;
    detectionError = '';
    try {
      const result = await detectFields(
        { inputPath: sourcePath, videoStreamIndex: video.index },
        controller.signal,
      );
      if (!controller.signal.aborted) {
        fieldDetection = result;
        if (
          automatic &&
          autoApply &&
          eligibleForAutomaticApplication &&
          revision === editRevision &&
          result.order !== 'undetermined'
        )
          applyResult(result);
      }
    } catch (cause) {
      if (!controller.signal.aborted && !automatic) detectionError = errorMessage(cause);
    } finally {
      if (detectionController === controller) detecting = false;
    }
  }
  function applyDetection() {
    if (!fieldDetection || fieldDetection.order === 'undetermined') return;
    editRevision++;
    applyResult(fieldDetection);
  }
  function applyResult(result: NonNullable<typeof fieldDetection>) {
    const deinterlace = result.telecined
      ? 'inverseTelecine'
      : result.order === 'progressive'
        ? 'off'
        : 'frame';
    const fieldOrder =
      result.order === 'progressive'
        ? value.fieldOrder
        : result.order === 'bottomFirst'
          ? 'bottomFirst'
          : 'topFirst';
    if (value.deinterlace !== deinterlace || value.fieldOrder !== fieldOrder)
      onchange({ ...value, deinterlace, fieldOrder });
  }
  const issue = $derived(temporalError(value, backend));
  const configuredSummary = $derived(
    temporalSummary(selectedTemporal(value)).replace(/^ · /, '') ||
      'Source timing and aspect ratio',
  );
  const summary = $derived(issue ? `Needs attention · ${issue}` : configuredSummary);
</script>

<details class="temporal-options">
  <summary>
    <span class="summary-content">
      <span>Frame processing</span>
      <small class:summary-error={!!issue}>{summary}</small>
    </span>
  </summary>
  <fieldset {disabled}>
    <p class="small-muted">
      Source field order: {video?.fieldOrder ?? 'not reported'}. Source frame rate: {video?.frameRate ??
        'not reported'}.
    </p>
    {#if sourcePath && video}
      <div class="detection">
        <button type="button" onclick={() => detect()} disabled={disabled || detecting}
          >{detecting ? 'Inspecting fields…' : 'Detect field order'}</button
        >
        {#if detecting}<button type="button" onclick={() => detectionController?.abort()}
            >Cancel detection</button
          >{/if}
        {#if fieldDetection}
          <p role="status">{fieldDetection.evidence}</p>
          {#if fieldDetection.order !== 'undetermined'}<button
              type="button"
              onclick={applyDetection}
              {disabled}>Apply detected processing</button
            >{/if}
        {/if}
        {#if detectionError}<p role="alert">{detectionError}</p>{/if}
      </div>
    {/if}
    <label
      >Source reconstruction
      <select
        aria-label="Source reconstruction"
        value={value.deinterlace}
        {disabled}
        onchange={(event) =>
          update({
            ...value,
            deinterlace: event.currentTarget.value as TemporalDraft['deinterlace'],
          })}
      >
        <option value="off">Off — progressive source</option><option value="frame"
          >BWDIF — keep source rate</option
        ><option value="bob">BWDIF bob — double source rate</option><option value="qtgmcFrame"
          >QTGMC — keep source rate</option
        ><option value="qtgmcBob">QTGMC bob — double source rate</option><option
          value="inverseTelecine">Inverse telecine — repair 3:2 cadence</option
        ><option value="exactDuplicates">Padded capture — remove exact duplicate frames</option>
      </select>
    </label>
    {#if value.deinterlace !== 'off' && value.deinterlace !== 'exactDuplicates'}
      <label
        >Source field order
        <select
          aria-label="Source field order"
          value={value.fieldOrder}
          onchange={(event) =>
            update({
              ...value,
              fieldOrder: event.currentTarget.value as TemporalDraft['fieldOrder'],
            })}
        >
          <option value="topFirst">Top field first (TFF)</option><option value="bottomFirst"
            >Bottom field first (BFF)</option
          >
        </select>
      </label>
      {#if value.deinterlace.startsWith('qtgmc')}
        <label
          >QTGMC preset
          <select
            aria-label="QTGMC preset"
            value={value.qtgmcPreset}
            onchange={(event) =>
              update({
                ...value,
                qtgmcPreset: event.currentTarget.value as TemporalDraft['qtgmcPreset'],
              })}
          >
            <option value="faster">Faster</option><option value="fast">Fast</option><option
              value="medium">Medium</option
            ><option value="slow">Slow</option><option value="slower">Slower</option>
          </select>
        </label>
      {:else if value.deinterlace === 'inverseTelecine'}
        <label
          ><input
            type="checkbox"
            checked={value.combedFallback}
            onchange={(event) => update({ ...value, combedFallback: event.currentTarget.checked })}
          />Deinterlace frames still combed after field matching</label
        >
      {/if}
      <p class="small-muted">
        Processing runs after trimming and before tone mapping and graphics. BWDIF and QTGMC require
        a uniformly interlaced source. Inverse telecine reconstructs film frames and removes one
        duplicate from each five-frame 3:2 cycle.
      </p>
    {:else if value.deinterlace === 'exactDuplicates'}
      <p class="small-muted">
        A decoded-frame SHA-256 scan reports exact repeated-run lengths and their transitions.
        Repair is accepted only when the unique-frame count exactly preserves duration at the
        selected output rate. Lossy near-duplicates are not removed; genuine repeated static
        pictures can be indistinguishable from padding, so use this only for a known padded capture.
      </p>
    {/if}
    <label
      ><input
        type="checkbox"
        checked={value.changeRate}
        {disabled}
        onchange={(event) => update({ ...value, changeRate: event.currentTarget.checked })}
      />Set output frame rate</label
    >
    {#if value.changeRate}
      <div class="rate-fields">
        <label
          >FPS numerator<input
            aria-label="FPS numerator"
            type="number"
            min="1"
            max={FPS_MAX_NUMERATOR}
            step="1"
            value={value.numerator}
            oninput={(event) =>
              update({
                ...value,
                numerator:
                  event.currentTarget.value === '' ? undefined : event.currentTarget.valueAsNumber,
              })}
          /></label
        >
        <label
          >FPS denominator<input
            aria-label="FPS denominator"
            type="number"
            min="1"
            max={FPS_MAX_DENOMINATOR}
            step="1"
            value={value.denominator}
            oninput={(event) =>
              update({
                ...value,
                denominator:
                  event.currentTarget.value === '' ? undefined : event.currentTarget.valueAsNumber,
              })}
          /></label
        >
      </div>
      <p class="small-muted">
        Frames are duplicated or dropped to keep playback speed and audio timing. This does not
        interpolate motion, repair cadence or accept variable-rate/timestamp-gap sources.
      </p>
    {/if}
    <label
      >Resize filter
      <select
        aria-label="Resize filter"
        value={value.resizeFilter}
        onchange={(event) =>
          update({
            ...value,
            resizeFilter: event.currentTarget.value as TemporalDraft['resizeFilter'],
          })}
      >
        <option value="lanczos">Lanczos</option><option value="bicubic">Bicubic</option><option
          value="bilinear">Bilinear</option
        ><option value="nearest">Nearest neighbor</option>
      </select>
    </label>
    <p class="small-muted">The resize filter applies only when the framing width changes.</p>
    <label
      >Output aspect ratio
      <select
        aria-label="Output aspect ratio"
        value={value.aspect}
        onchange={(event) =>
          update({ ...value, aspect: event.currentTarget.value as TemporalDraft['aspect'] })}
      >
        <option value="off">Preserve source aspect ratio</option><option value="sample"
          >Custom sample aspect ratio (SAR)</option
        ><option value="display">Custom display aspect ratio (DAR)</option>
      </select>
    </label>
    {#if value.aspect !== 'off'}
      <div class="rate-fields">
        <label
          >{value.aspect === 'sample' ? 'SAR' : 'DAR'} numerator<input
            aria-label="Aspect ratio numerator"
            type="number"
            min="1"
            max="65535"
            step="1"
            value={value.aspectNumerator}
            oninput={(event) =>
              update({
                ...value,
                aspectNumerator:
                  event.currentTarget.value === '' ? undefined : event.currentTarget.valueAsNumber,
              })}
          /></label
        >
        <label
          >{value.aspect === 'sample' ? 'SAR' : 'DAR'} denominator<input
            aria-label="Aspect ratio denominator"
            type="number"
            min="1"
            max="65535"
            step="1"
            value={value.aspectDenominator}
            oninput={(event) =>
              update({
                ...value,
                aspectDenominator:
                  event.currentTarget.value === '' ? undefined : event.currentTarget.valueAsNumber,
              })}
          /></label
        >
      </div>
      <p class="small-muted">
        This changes display metadata after crop, resize and borders. It does not resample the
        picture.
      </p>
    {/if}
    {#if issue}<p role="alert" class="small-muted">{issue}</p>{/if}
  </fieldset>
</details>

<style>
  .temporal-options {
    min-width: 0;
    border: 1px solid var(--border);
    border-radius: var(--radius);
  }
  summary {
    padding: 11px 12px;
    cursor: pointer;
    font-size: 13px;
    font-weight: 600;
  }
  .summary-content {
    display: inline-flex;
    justify-content: space-between;
    gap: 12px;
    width: calc(100% - 1.4rem);
    vertical-align: middle;
  }
  .summary-content small {
    color: var(--muted-foreground);
    font-size: 11px;
    font-weight: 400;
    text-align: right;
  }
  .summary-error {
    color: var(--destructive);
  }
  fieldset {
    display: grid;
    grid-template-columns: repeat(auto-fit, minmax(min(100%, 14rem), 1fr));
    gap: 10px 12px;
    min-width: 0;
    margin: 0;
    padding: 0 12px 12px;
    border: 0;
  }
  label {
    display: grid;
    gap: 6px;
    font-size: 12px;
  }
  label:has(input[type='checkbox']) {
    display: flex;
    align-items: center;
    grid-column: 1 / -1;
  }
  p {
    grid-column: 1 / -1;
    margin: 0;
  }
  .rate-fields {
    display: grid;
    grid-template-columns: repeat(2, minmax(0, 9rem));
    gap: 8px;
    grid-column: 1 / -1;
  }
  .detection {
    grid-column: 1 / -1;
    display: flex;
    gap: 8px;
    flex-wrap: wrap;
    align-items: center;
  }
  .detection p {
    flex-basis: 100%;
  }
  input,
  select {
    min-width: 0;
  }
  input[type='number'] {
    width: min(100%, 9rem);
  }
  select {
    width: min(100%, 22rem);
  }
  @media (max-width: 560px) {
    .summary-content {
      display: inline-grid;
    }
    .summary-content small {
      text-align: left;
    }
    .rate-fields {
      grid-template-columns: repeat(auto-fit, minmax(min(100%, 8rem), 1fr));
    }
  }
</style>
