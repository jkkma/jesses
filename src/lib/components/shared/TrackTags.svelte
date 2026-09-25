<script lang="ts">
  import type { MediaStream } from '$lib/ipc/generated';
  import type { TrackTags } from './track-layout';

  let {
    stream,
    value,
    disabled,
    onchange,
  }: {
    stream: MediaStream;
    value: TrackTags;
    disabled: boolean;
    onchange: (value: TrackTags) => void;
  } = $props();

  function setText(field: 'title' | 'language', text: string | undefined) {
    const next = { ...value };
    if (text === undefined) delete next[field];
    else next[field] = text;
    onchange(next);
  }

  function setFlag(field: 'default' | 'forced', selected: string) {
    const next = { ...value };
    if (selected === '') delete next[field];
    else next[field] = selected === 'yes';
    onchange(next);
  }
</script>

<div class="tags" role="group" aria-label={`Tags and flags for stream #${stream.index}`}>
  <div class="text-field">
    <label class="override">
      <input
        type="checkbox"
        checked={value.title !== undefined}
        {disabled}
        onchange={(event) =>
          setText('title', event.currentTarget.checked ? (stream.title ?? '') : undefined)}
      /> Change title
    </label>
    {#if value.title !== undefined}
      <input
        aria-label="Track title"
        value={value.title}
        {disabled}
        maxlength="4096"
        oninput={(event) => setText('title', event.currentTarget.value)}
      />
      <small>Leave blank to clear the title.</small>
    {/if}
  </div>
  <div class="text-field">
    <label class="override">
      <input
        type="checkbox"
        checked={value.language !== undefined}
        {disabled}
        onchange={(event) =>
          setText('language', event.currentTarget.checked ? (stream.language ?? '') : undefined)}
      /> Change language
    </label>
    {#if value.language !== undefined}
      <input
        aria-label="Track language"
        value={value.language}
        {disabled}
        maxlength="64"
        oninput={(event) => setText('language', event.currentTarget.value)}
      />
      <small>Leave blank to clear the language tag.</small>
    {/if}
  </div>
  <label
    >Default flag<select
      aria-label="Track default flag"
      value={value.default === undefined ? '' : value.default ? 'yes' : 'no'}
      {disabled}
      onchange={(event) => setFlag('default', event.currentTarget.value)}
    >
      <option value="">Keep source</option>
      <option value="yes">Yes</option>
      <option value="no">No</option>
    </select></label
  >
  <label
    >Forced flag<select
      aria-label="Track forced flag"
      value={value.forced === undefined ? '' : value.forced ? 'yes' : 'no'}
      {disabled}
      onchange={(event) => setFlag('forced', event.currentTarget.value)}
    >
      <option value="">Keep source</option>
      <option value="yes">Yes</option>
      <option value="no">No</option>
    </select></label
  >
</div>

<style>
  .tags {
    display: grid;
    grid-template-columns: repeat(auto-fit, minmax(min(100%, 12rem), 1fr));
    gap: 8px 12px;
    padding: 8px 0;
  }
  label,
  .text-field {
    display: grid;
    gap: 4px;
    font-size: 11px;
  }
  .override {
    display: flex;
    align-items: center;
    gap: 5px;
  }
  select,
  input:not([type='checkbox']) {
    width: 100%;
    min-width: 0;
    padding: 6px 8px;
    border: 1px solid var(--border);
    border-radius: 4px;
    color: var(--foreground);
    background: var(--background);
    font: inherit;
  }
  small {
    color: var(--muted-foreground);
    font-size: 10px;
  }
</style>
