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
    <div class="trim-fields">
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
    </div>
    <p>
      Frame 0 is the first frame. Audio must use a conversion. Text subtitle overlaps and chapters
      are clipped to this interval; overlaps with timed ASS effects or WebVTT inline timestamps are
      rejected. The end must fit the source.
    </p>
    {#if error}<p role="alert">{error}</p>{/if}
  {/if}
</div>

<style>
  .trim-options {
    margin-top: 14px;
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
    grid-template-columns: repeat(2, minmax(0, 1fr));
    gap: 12px;
    margin-top: 12px;
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
