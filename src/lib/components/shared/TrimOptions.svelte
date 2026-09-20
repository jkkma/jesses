<script lang="ts">
  import type { TrimDraft } from './trim-options';
  let {
    idPrefix,
    draft,
    disabled = false,
    error = null,
    onchange,
  }: {
    idPrefix: string;
    draft: TrimDraft;
    disabled?: boolean;
    error?: string | null;
    onchange: (value: TrimDraft) => void;
  } = $props();
  function number(event: Event): number | undefined {
    const value = (event.currentTarget as HTMLInputElement).value;
    return value === '' ? undefined : Number(value);
  }
  function milliseconds(event: Event): number | undefined {
    const value = (event.currentTarget as HTMLInputElement).value;
    if (value === '') return undefined;
    const seconds = Number(value);
    return Number.isFinite(seconds) ? Math.round(seconds * 1_000) : undefined;
  }
</script>

<div class="trim-options">
  <label class="choice" for={`${idPrefix}-trim`}
    ><input
      id={`${idPrefix}-trim`}
      type="checkbox"
      checked={draft.enabled}
      {disabled}
      onchange={(event) => onchange({ ...draft, enabled: event.currentTarget.checked })}
    />Trim video interval</label
  >
  {#if draft.enabled}
    <fieldset class="trim-mode" {disabled}>
      <legend>Boundary unit</legend>
      <label
        ><input
          type="radio"
          name={`${idPrefix}-trim-mode`}
          value="frames"
          checked={draft.mode === 'frames'}
          onchange={() => onchange({ ...draft, mode: 'frames' })}
        />Frames</label
      >
      <label
        ><input
          type="radio"
          name={`${idPrefix}-trim-mode`}
          value="time"
          checked={draft.mode === 'time'}
          onchange={() => onchange({ ...draft, mode: 'time' })}
        />Time</label
      >
    </fieldset>
    <div class="trim-fields">
      {#if draft.mode === 'frames'}
        <div class="field">
          <label for={`${idPrefix}-trim-start`}>Start frame</label><input
            id={`${idPrefix}-trim-start`}
            type="number"
            min="0"
            max="4294967295"
            step="1"
            value={draft.startFrame ?? ''}
            {disabled}
            oninput={(event) => onchange({ ...draft, startFrame: number(event) })}
          />
        </div>
        <div class="field">
          <label for={`${idPrefix}-trim-end`}>End frame (excluded)</label><input
            id={`${idPrefix}-trim-end`}
            type="number"
            min="1"
            max="4294967295"
            step="1"
            value={draft.endFrameExclusive ?? ''}
            {disabled}
            oninput={(event) => onchange({ ...draft, endFrameExclusive: number(event) })}
          />
        </div>
      {:else}
        <div class="field">
          <label for={`${idPrefix}-trim-start-time`}>Start time (seconds)</label><input
            id={`${idPrefix}-trim-start-time`}
            type="number"
            min="0"
            max="4294967.295"
            step="0.001"
            value={draft.startMilliseconds === undefined ? '' : draft.startMilliseconds / 1_000}
            {disabled}
            oninput={(event) => onchange({ ...draft, startMilliseconds: milliseconds(event) })}
          />
        </div>
        <div class="field">
          <label for={`${idPrefix}-trim-end-time`}>End time (excluded)</label><input
            id={`${idPrefix}-trim-end-time`}
            type="number"
            min="0.001"
            max="4294967.295"
            step="0.001"
            value={draft.endMilliseconds === undefined ? '' : draft.endMilliseconds / 1_000}
            {disabled}
            oninput={(event) => onchange({ ...draft, endMilliseconds: milliseconds(event) })}
          />
        </div>
      {/if}
    </div>
    <p>
      {draft.mode === 'frames'
        ? 'Frame 0 is the first frame.'
        : 'Time boundaries use source timestamps to millisecond precision; only frames whose timestamps are inside the interval are selected.'}
      Audio must use a conversion. Text subtitle overlaps and chapters are clipped to this interval; overlaps
      with timed ASS effects or WebVTT inline timestamps are rejected. The end must fit the source.
    </p>
    {#if error}<p role="alert">{error}</p>{/if}
  {/if}
</div>

<style>
  .trim-options {
    margin-top: 10px;
  }
  .choice {
    display: flex;
    align-items: center;
    gap: 9px;
  }
  .choice input {
    width: 16px;
    height: 16px;
    accent-color: #ad5326;
  }
  .trim-fields {
    display: grid;
    grid-template-columns: repeat(auto-fit, minmax(min(100%, 9rem), 1fr));
    gap: 12px;
    margin-top: 10px;
  }
  .trim-fields input {
    width: min(100%, 9rem);
  }
  .trim-mode {
    display: flex;
    gap: 16px;
    margin: 10px 0 0;
    padding: 0;
    border: 0;
  }
  .trim-mode legend {
    margin-bottom: 6px;
    color: var(--text-muted);
    font-size: 12px;
  }
  .trim-mode label {
    display: flex;
    align-items: center;
    gap: 6px;
  }
  p {
    margin-top: 8px;
    color: var(--text-muted);
    font-size: 12px;
    line-height: 1.5;
  }
  [role='alert'] {
    color: #a3362f;
  }
</style>
