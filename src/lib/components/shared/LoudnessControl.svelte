<script lang="ts">
  import { measureLoudness, isDesktop } from '$lib/ipc/client';
  import type { LoudnessResult } from '$lib/ipc/generated';
  import type { AudioTrackDraft } from './audio-options';
  import { errorMessage } from './format';

  let {
    inputPath,
    settings,
    disabled = false,
    onchange,
  }: {
    inputPath: string;
    settings: AudioTrackDraft;
    disabled?: boolean;
    onchange: (settings: AudioTrackDraft) => void;
  } = $props();
  let expanded = $state(false);
  let target = $state<number | undefined>(-23);
  let peak = $state<number | undefined>(-1);
  let result = $state<LoudnessResult | null>(null);
  let error = $state<string | null>(null);
  let pending = $state(false);
  let controller: AbortController | undefined;
  let generation = 0;
  const valid = $derived(
    typeof target === 'number' &&
      Number.isFinite(target) &&
      target >= -70 &&
      target <= -5 &&
      typeof peak === 'number' &&
      Number.isFinite(peak) &&
      peak >= -9 &&
      peak <= 0,
  );
  const converting = $derived(settings.codec !== 'copy');
  const gain = $derived(settings.gain ? settings.gain.tenthsDb / 10 : 0);

  function cancel() {
    generation++;
    controller?.abort();
    controller = undefined;
    pending = false;
  }
  $effect(() => {
    void inputPath;
    void settings.streamIndex;
    void settings.channels;
    void target;
    void peak;
    result = null;
    error = null;
    return cancel;
  });
  async function measure() {
    if (!valid) return;
    cancel();
    const run = ++generation;
    const active = new AbortController();
    controller = active;
    result = null;
    error = null;
    pending = true;
    try {
      const value = await measureLoudness(
        {
          inputPath,
          streamIndex: settings.streamIndex,
          channels: settings.channels,
          targetLufs: target!,
          peakLimitDbfs: peak!,
        },
        active.signal,
      );
      if (run === generation) result = value;
    } catch (cause) {
      if (run === generation && !active.signal.aborted) error = errorMessage(cause);
    } finally {
      if (run === generation) {
        pending = false;
        controller = undefined;
      }
    }
  }
  function apply() {
    if (!converting || result?.suggestedGainTenthsDb == null || disabled) return;
    onchange({
      ...settings,
      gain: { tenthsDb: result.suggestedGainTenthsDb, sourceFingerprint: result.sourceFingerprint },
    });
  }
</script>

<div
  class="loudness"
  role="group"
  aria-label={`Loudness and gain for stream #${settings.streamIndex}`}
>
  {#if converting}
    <label class="gain"
      >Audio gain (dB)
      <input
        type="number"
        min="-60"
        max="24"
        step="0.1"
        value={Number.isFinite(gain) ? gain : ''}
        {disabled}
        oninput={(event) =>
          onchange({
            ...settings,
            gain: {
              tenthsDb:
                event.currentTarget.value === ''
                  ? Number.NaN
                  : Math.round(event.currentTarget.valueAsNumber * 10),
            },
          })}
      />
    </label>
    <p>Flat gain preserves dynamics. Positive gain can clip; measure first to check headroom.</p>
    {#if settings.gain?.sourceFingerprint}<p>
        Gain comes from this source's loudness measurement. Source changes require a new
        measurement.
      </p>{/if}
  {/if}
  <button
    type="button"
    class="disclosure"
    aria-expanded={expanded}
    onclick={() => {
      expanded = !expanded;
      if (!expanded) cancel();
    }}>Measure loudness {expanded ? '−' : '+'}</button
  >
  {#if expanded}
    <p>
      Measures the complete selected audio track with the chosen channel setting. No audio is
      changed until you apply a gain and encode.
    </p>
    <div class="targets">
      <label
        >Target loudness (LUFS)<input
          type="number"
          min="-70"
          max="-5"
          step="0.1"
          bind:value={target}
          disabled={disabled || pending}
        /></label
      >
      <label
        >Peak limit (dBTP)<input
          type="number"
          min="-9"
          max="0"
          step="0.1"
          bind:value={peak}
          disabled={disabled || pending}
        /></label
      >
    </div>
    <div class="actions">
      <button
        type="button"
        onclick={measure}
        disabled={disabled || pending || !valid || !isDesktop()}
        >{pending ? 'Measuring…' : 'Measure audio track'}</button
      >
      {#if pending}<button type="button" onclick={cancel}>Cancel measurement</button>{/if}
    </div>
    {#if pending}<p role="status">
        Reading the full audio track. Long files can take several minutes.
      </p>{/if}
    {#if error}<p role="alert">{error}</p>{/if}
    {#if result}
      <p class="result">
        Integrated: {result.integratedLufs === null
          ? 'Not measurable'
          : `${result.integratedLufs.toFixed(2)} LUFS`} · True peak: {result.truePeakDbfs === null
          ? 'Not measurable'
          : `${result.truePeakDbfs.toFixed(2)} dBTP`} · Range: {result.loudnessRangeLu === null
          ? 'Unknown'
          : `${result.loudnessRangeLu.toFixed(2)} LU`}
      </p>
      <p>{result.message}</p>
      {#if result.suggestedGainTenthsDb !== null}
        <p>
          Suggested gain: {(result.suggestedGainTenthsDb / 10).toFixed(1)} dB{result.targetLimitedByPeak
            ? ' · Limited by peak headroom'
            : ''}
        </p>
        <button type="button" onclick={apply} disabled={disabled || !converting}
          >Apply measured gain</button
        >
        {#if !converting}<p>Choose an audio conversion codec to apply gain.</p>{/if}
      {/if}
    {/if}
  {/if}
</div>

<style>
  .loudness {
    margin-top: 12px;
    padding-top: 10px;
    border-top: 1px solid var(--border);
  }
  label {
    display: grid;
    gap: 5px;
    font-size: 11px;
  }
  input {
    width: 100%;
    min-width: 0;
    font: inherit;
    color: var(--foreground);
    background: var(--background);
    border: 1px solid var(--border);
    border-radius: 4px;
    padding: 7px 9px;
  }
  input:focus-visible,
  button:focus-visible {
    outline: 2px solid var(--primary);
    outline-offset: 2px;
  }
  .gain {
    max-width: 180px;
  }
  p {
    margin: 8px 0;
    font-size: 11px;
    color: var(--muted-foreground);
    line-height: 1.5;
  }
  .result {
    color: var(--foreground);
  }
  .targets {
    display: grid;
    grid-template-columns: repeat(2, minmax(0, 1fr));
    gap: 10px;
    margin: 10px 0;
  }
  button {
    font: inherit;
    font-size: 11px;
    color: var(--foreground);
    background: var(--background);
    border: 1px solid var(--border);
    border-radius: 4px;
    padding: 6px 9px;
    cursor: pointer;
  }
  button:disabled {
    opacity: 0.5;
    cursor: default;
  }
  .disclosure {
    background: transparent;
    padding-left: 0;
    border: 0;
  }
  .actions {
    display: flex;
    flex-wrap: wrap;
    gap: 8px;
  }
  @media (max-width: 760px) {
    .targets {
      grid-template-columns: 1fr;
    }
  }
</style>
