<script lang="ts">
  import type { EncodeBackend, MediaStream } from '$lib/ipc/generated';
  import { temporalError, type TemporalDraft } from './temporal-options';
  let {
    value,
    video,
    backend,
    disabled = false,
    onchange,
  }: {
    value: TemporalDraft;
    video?: MediaStream;
    backend: EncodeBackend;
    disabled?: boolean;
    onchange: (value: TemporalDraft) => void;
  } = $props();
  const issue = $derived(temporalError(value, backend));
</script>

<fieldset class="temporal-options" {disabled}>
  <legend>Frame processing</legend>
  <p class="small-muted">
    Source field order: {video?.fieldOrder ?? 'not reported'}. Source frame rate: {video?.frameRate ??
      'not reported'}.
  </p>
  <label
    >Deinterlacing
    <select
      aria-label="Deinterlacing"
      value={value.deinterlace}
      disabled={disabled || backend === 'av1an'}
      onchange={(event) =>
        onchange({
          ...value,
          deinterlace: event.currentTarget.value as TemporalDraft['deinterlace'],
        })}
    >
      <option value="off">Off — progressive source</option><option value="frame"
        >BWDIF — keep source rate</option
      ><option value="bob">BWDIF bob — double source rate</option>
    </select>
  </label>
  {#if value.deinterlace !== 'off'}
    <label
      >Source field order
      <select
        aria-label="Source field order"
        value={value.fieldOrder}
        onchange={(event) =>
          onchange({
            ...value,
            fieldOrder: event.currentTarget.value as TemporalDraft['fieldOrder'],
          })}
      >
        <option value="topFirst">Top field first (TFF)</option><option value="bottomFirst"
          >Bottom field first (BFF)</option
        >
      </select>
    </label>
    <p class="small-muted">
      All source frames must be interlaced with the selected field order. Mixed/progressive segments
      are rejected. BWDIF runs after source-frame trimming, before tone mapping and graphics. QTGMC
      and cadence repair are separate workflows.
    </p>
  {/if}
  <label
    ><input
      type="checkbox"
      checked={value.changeRate}
      disabled={disabled || backend === 'av1an'}
      onchange={(event) => onchange({ ...value, changeRate: event.currentTarget.checked })}
    />Set output frame rate</label
  >
  {#if value.changeRate}
    <div class="rate-fields">
      <label
        >FPS numerator<input
          aria-label="FPS numerator"
          type="number"
          min="1"
          max="12000000"
          step="1"
          value={value.numerator}
          oninput={(event) =>
            onchange({
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
          max="100000"
          step="1"
          value={value.denominator}
          oninput={(event) =>
            onchange({
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
        onchange({
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
  {#if issue}<p role="alert" class="small-muted">{issue}</p>{/if}
</fieldset>

<style>
  .temporal-options {
    display: grid;
    gap: 10px;
    min-width: 0;
    border: 1px solid var(--border);
    padding: 12px;
    border-radius: 8px;
  }
  legend {
    font-weight: 600;
    padding: 0 4px;
  }
  label {
    display: grid;
    gap: 6px;
    font-size: 12px;
  }
  label:has(input[type='checkbox']) {
    display: flex;
    align-items: center;
  }
  .rate-fields {
    display: grid;
    grid-template-columns: 1fr 1fr;
    gap: 8px;
  }
  input,
  select {
    min-width: 0;
  }
</style>
