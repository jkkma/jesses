<script lang="ts">
  import type { ComponentProps } from 'svelte';
  import SingleEncode from './SingleEncode.svelte';
  import type { MediaFile } from '$lib/ipc/generated';

  let {
    files,
    file,
    ...props
  }: ComponentProps<typeof SingleEncode> & {
    files: MediaFile[];
  } = $props();
  let sourceId = $state('');
  const id = $props.id();
  const source = $derived(sourceId ? files.find((entry) => entry.id === sourceId) : file);
  const missing = $derived(!!sourceId && !source);
</script>

{#snippet sourcePicker()}
  <section
    class="source-mapping"
    aria-label={`${props.backend === 'av1an' ? 'AV1AN' : 'Quick Convert'} source mapping`}
  >
    <div class="source-field">
      <label for={id}>Source for this encode</label>
      <select {id} bind:value={sourceId}>
        <option value="">Use the current media selection</option>
        {#each files as entry (entry.id)}
          <option value={entry.id}>{entry.name}</option>
        {/each}
        {#if missing}<option value={sourceId}>Removed source — choose a file</option>{/if}
      </select>
    </div>
    <p>
      {#if missing}
        The chosen source was removed. Choose an encoding source to continue.
      {:else if sourceId}
        Video, geometry and color use {source?.name}. Tracks, container metadata and chapters can be
        chosen below.
      {:else}
        Pin a source here to keep encoding it while you inspect other files.
      {/if}
    </p>
  </section>
{/snippet}
<SingleEncode {...props} {files} file={source} {sourcePicker} />

<style>
  .source-mapping {
    display: grid;
    gap: 4px;
    flex: 0 1 390px;
    min-width: 240px;
  }
  .source-field {
    display: flex;
    align-items: center;
    flex-wrap: wrap;
    gap: 6px;
    font-size: 12px;
    font-weight: 600;
    max-width: 100%;
  }
  select {
    width: 260px;
    max-width: 100%;
    min-height: 34px;
    padding: 0 8px;
    border: 1px solid var(--border);
    background: var(--background);
    font-size: 11px;
  }
  p {
    margin: 0;
    font-size: 11px;
    color: var(--muted-foreground);
    overflow-wrap: anywhere;
  }
</style>
