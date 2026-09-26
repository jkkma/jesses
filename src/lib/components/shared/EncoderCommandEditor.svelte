<script lang="ts">
  import type { EncoderParameter, EncoderParameterCatalog } from '$lib/ipc/generated';
  import { formatEncoderCommand, parseEncoderCommand } from './encoder-command';
  let {
    value,
    catalog,
    disabled = false,
    onchange,
  }: {
    value: EncoderParameter[];
    catalog: EncoderParameterCatalog;
    disabled?: boolean;
    onchange: (value: EncoderParameter[]) => void;
  } = $props();
  let draft = $state('');
  let editing = $state(false);
  let error = $state<string | null>(null);
  let message = $state<string | null>(null);
  let original = $state('');
  const changed = $derived(
    editing &&
      original !== JSON.stringify({ value, encoder: catalog.encoder, backend: catalog.backend }),
  );
  function load() {
    draft = formatEncoderCommand(value, catalog);
    original = JSON.stringify({ value, encoder: catalog.encoder, backend: catalog.backend });
    editing = true;
    error = null;
    message = null;
  }
  function apply() {
    if (disabled || changed) return;
    try {
      const parameters = parseEncoderCommand(draft, catalog);
      onchange(parameters);
      editing = false;
      error = null;
      message = `Applied ${parameters.length} encoder overrides. Preview the command plan to see the resulting commands.`;
    } catch (cause) {
      error = cause instanceof Error ? cause.message : String(cause);
    }
  }
</script>

<details class="argument-editor">
  <summary>Edit encoder arguments as text</summary>
  <p>
    Enter the encoder flags shown in the catalog, using <code>--option value</code> or
    <code>--option=value</code>. Apply replaces the current override list. An empty list clears it.
  </p>
  <button type="button" onclick={load} {disabled}
    >{editing ? 'Reload current arguments' : 'Edit current arguments'}</button
  >
  {#if editing}
    <textarea
      aria-label="Editable encoder arguments"
      bind:value={draft}
      rows="6"
      maxlength="16384"
      {disabled}></textarea>
    {#if changed}<p role="alert">
        The settings changed while this draft was open. Reload current arguments before applying.
      </p>{/if}
    <button type="button" onclick={apply} disabled={disabled || changed}
      >Apply encoder arguments</button
    >
    <button
      type="button"
      onclick={() => {
        editing = false;
        error = null;
      }}
      {disabled}>Discard argument edits</button
    >
  {/if}
  {#if error}<p role="alert">{error}</p>{/if}
  {#if message}<p role="status">{message}</p>{/if}
</details>

<style>
  .argument-editor {
    border: 1px solid var(--border);
    padding: 10px;
    margin: 10px 0;
    min-width: 0;
  }
  summary {
    cursor: pointer;
    font-weight: 600;
  }
  p {
    font-size: 12px;
    overflow-wrap: anywhere;
  }
  textarea {
    display: block;
    width: 100%;
    box-sizing: border-box;
    resize: vertical;
    max-height: 250px;
    font-family: monospace;
    margin: 8px 0;
  }
  button {
    font: inherit;
    padding: 6px 10px;
    margin: 4px 6px 4px 0;
    cursor: pointer;
  }
</style>
