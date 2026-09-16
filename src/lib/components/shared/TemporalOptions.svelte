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
    >Source reconstruction
    <select
      aria-label="Source reconstruction"
      value={value.deinterlace}
      {disabled}
      onchange={(event) =>
        onchange({
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
    {#if value.deinterlace.startsWith('qtgmc')}
      <label
        >QTGMC preset
        <select
          aria-label="QTGMC preset"
          value={value.qtgmcPreset}
          onchange={(event) =>
            onchange({
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
          onchange={(event) => onchange({ ...value, combedFallback: event.currentTarget.checked })}
        />Deinterlace frames still combed after field matching</label
      >
    {/if}
    <p class="small-muted">
      Processing runs after trimming and before tone mapping and graphics. BWDIF and QTGMC require a
      uniformly interlaced source. Inverse telecine reconstructs film frames and removes one
      duplicate from each five-frame 3:2 cycle.
    </p>
  {:else if value.deinterlace === 'exactDuplicates'}
    <p class="small-muted">
      A decoded-frame SHA-256 scan reports exact repeated-run lengths and their transitions. Repair
      is accepted only when the unique-frame count exactly preserves duration at the selected output
      rate. Lossy near-duplicates are not removed; genuine repeated static pictures can be
      indistinguishable from padding, so use this only for a known padded capture.
    </p>
  {/if}
  <label
    ><input
      type="checkbox"
      checked={value.changeRate}
      {disabled}
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
  <label
    >Output aspect ratio
    <select
      aria-label="Output aspect ratio"
      value={value.aspect}
      onchange={(event) =>
        onchange({ ...value, aspect: event.currentTarget.value as TemporalDraft['aspect'] })}
    >
      <option value="off">Square pixels (default)</option><option value="sample"
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
            onchange({
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
            onchange({
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
