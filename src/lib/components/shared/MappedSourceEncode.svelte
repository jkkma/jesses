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
      Video, copied tracks, geometry and color settings use {source?.name}. You can inspect other
      files without changing this source.
    {:else}
      Use this when you want to inspect another file without changing the file you will encode.
    {/if}
  </p>
</section>
<SingleEncode {...props} file={source} />

<style>
  .source-mapping {
    display: flex;
    align-items: center;
    gap: 20px;
    flex-wrap: wrap;
    margin-bottom: 20px;
    padding: 16px;
    background: var(--muted);
    border-radius: 8px;
  }
  .source-field {
    display: grid;
    gap: 6px;
    font-size: 12px;
    font-weight: 600;
    max-width: 100%;
  }
  select {
    max-width: min(460px, 100%);
  }
  p {
    margin: 0;
    flex: 1 1 240px;
    font-size: 12px;
    color: var(--muted-foreground);
    overflow-wrap: anywhere;
  }
</style>
