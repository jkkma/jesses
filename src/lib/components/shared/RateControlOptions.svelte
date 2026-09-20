<script lang="ts">
  import { validRate, type RateDraft } from './rate-control-options';
  let {
    idPrefix,
    draft,
    disabled,
    onchange,
  }: {
    idPrefix: string;
    draft: RateDraft;
    disabled: boolean;
    onchange: (value: RateDraft) => void;
  } = $props();
</script>

<div class="rate-control" class:expanded={draft.mode !== 'quality'}>
  <div class="rate-fields">
    <div class="control">
      <label for={`${idPrefix}-rate-mode`}>Rate control</label>
      <select
        id={`${idPrefix}-rate-mode`}
        value={draft.mode}
        {disabled}
        onchange={(event) =>
          onchange({ ...draft, mode: event.currentTarget.value as RateDraft['mode'] })}
      >
        <option value="quality">Constant quality (CRF)</option>
        <option value="lossless">Lossless</option>
        <option value="bitrate">Video bitrate</option>
        <option value="targetSize">Target file size</option>
      </select>
    </div>
    {#if draft.mode === 'bitrate'}
      <div class="control compact-control">
        <label for={`${idPrefix}-video-bitrate`}>Video bitrate (kb/s)</label>
        <input
          id={`${idPrefix}-video-bitrate`}
          type="number"
          min="1"
          max="100000"
          step="1"
          value={draft.bitrate ?? ''}
          {disabled}
          oninput={(event) =>
            onchange({
              ...draft,
              bitrate:
                event.currentTarget.value === '' ? undefined : event.currentTarget.valueAsNumber,
            })}
        />
      </div>
      <label class="check"
        ><input
          type="checkbox"
          checked={draft.twoPass}
          {disabled}
          onchange={(event) => onchange({ ...draft, twoPass: event.currentTarget.checked })}
        />Two passes</label
      >
    {:else if draft.mode === 'targetSize'}
      <div class="control compact-control">
        <label for={`${idPrefix}-target-size`}>Target file size (MiB)</label>
        <input
          id={`${idPrefix}-target-size`}
          type="number"
          min="1"
          max="1048576"
          step="1"
          value={draft.targetSize ?? ''}
          {disabled}
          oninput={(event) =>
            onchange({
              ...draft,
              targetSize:
                event.currentTarget.value === '' ? undefined : event.currentTarget.valueAsNumber,
            })}
        />
      </div>
    {/if}
  </div>
  {#if draft.mode === 'lossless'}
    <p>
      Preserves the frames after your selected filters. Output is usually much larger. The installed
      encoder must support lossless mode, and every decoded pixel is verified before saving.
    </p>
  {:else if draft.mode === 'bitrate'}
    <p>
      Decimal kb/s for video. Audio and container bytes are additional. Two passes improve bitrate
      allocation.
    </p>
  {:else if draft.mode === 'targetSize'}
    <p>
      Approximate final size per file; 1 MiB = 1,048,576 bytes. Selected audio, subtitles, and
      attachments are measured before two video passes. Content and container overhead affect the
      result.
    </p>
  {/if}
  {#if !validRate(draft)}<p role="alert">
      Enter a whole number from 1 to {draft.mode === 'bitrate' ? '100000 kb/s' : '1048576 MiB'}.
    </p>{/if}
</div>

<style>
  .rate-control {
    display: grid;
    gap: 0.45rem;
    padding: 0.85rem;
    border: 1px solid var(--border);
    border-radius: 0.5rem;
  }
  .rate-control.expanded {
    grid-column: 1 / -1;
  }
  .rate-fields {
    display: grid;
    grid-template-columns: repeat(auto-fit, minmax(min(100%, 12rem), 1fr));
    gap: 0.7rem 1rem;
    align-items: end;
  }
  .control {
    display: grid;
    gap: 0.35rem;
  }
  label {
    font-size: 0.82rem;
    font-weight: 600;
  }
  select,
  input[type='number'] {
    min-height: 2.35rem;
    padding: 0.45rem 0.6rem;
    color: inherit;
    background: var(--background);
    border: 1px solid var(--border);
    border-radius: 0.35rem;
  }
  select {
    width: min(100%, 19rem);
  }
  .compact-control input {
    width: min(100%, 9rem);
  }
  .check {
    display: flex;
    align-items: center;
    gap: 0.5rem;
    min-height: 2.35rem;
  }
  p {
    margin: 0;
    color: var(--muted-foreground);
    font-size: 0.77rem;
    line-height: 1.5;
  }
  [role='alert'] {
    color: var(--destructive);
  }
</style>
