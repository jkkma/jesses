<script lang="ts">
  import type { MediaFile } from '$lib/ipc/generated';
  import type { FileListMode } from '$lib/source-workspace';
  import { formatBytes } from '$lib/components/shared/format';
  import SourceThumbnail from './SourceThumbnail.svelte';
  let {
    files,
    selectedIds,
    primary,
    mode,
    batchNameTemplate,
    batchExample,
    statuses,
    recentPaths,
    importing,
    desktop,
    onselect,
    onmode,
    onname,
    onadd,
    onfolder,
    onrecent,
    onload,
    onremove,
    onmove,
    onsort,
  }: {
    files: MediaFile[];
    selectedIds: string[];
    primary?: MediaFile;
    mode: FileListMode;
    batchNameTemplate: string;
    batchExample: string;
    statuses: Record<string, { glyph: string; text: string; note?: string }>;
    recentPaths: string[];
    importing: boolean;
    desktop: boolean;
    onselect: (ids: string[]) => void;
    onmode: (mode: FileListMode) => void;
    onname: (value: string) => void;
    onadd: () => void;
    onfolder: () => void;
    onrecent: (path: string) => void;
    onload: () => void;
    onremove: () => void;
    onmove: (offset: number) => void;
    onsort: (sort: string) => void;
  } = $props();
  function select(event: MouseEvent, file: MediaFile) {
    if (event.ctrlKey || event.metaKey)
      onselect(
        selectedIds.includes(file.id)
          ? selectedIds.filter((id) => id !== file.id)
          : [...selectedIds, file.id],
      );
    else if (event.shiftKey && selectedIds.length) {
      const start = files.findIndex((entry) => entry.id === selectedIds[0]);
      const end = files.findIndex((entry) => entry.id === file.id);
      onselect(
        files.slice(Math.min(start, end), Math.max(start, end) + 1).map((entry) => entry.id),
      );
    } else onselect([file.id]);
  }
</script>

<section class="file-list" aria-label="File List">
  <div class="mode-line">
    <label
      >Mode <select
        value={mode}
        onchange={(event) => onmode(event.currentTarget.value as FileListMode)}
        ><option value="mux">Muxing Mode - Merge all loaded files into one output</option><option
          value="batch">Batch Processing Mode - Run the task on each file separately</option
        ></select
      ></label
    >
    {#if mode === 'batch'}<label
        >Output Names <input
          value={batchNameTemplate}
          placeholder={'{name}'}
          oninput={(event) => onname(event.currentTarget.value)}
        /></label
      >
      <details class="help">
        <summary aria-label="Batch naming help">?</summary>
        <p>
          Use {`{name}`}, {`{ext}`}, {`{index}`}, {`{codec}`}, {`{crf}`}, {`{quality}`}, {`{preset}`},
          {`{width}`}, {`{height}`} and {`{date}`} in the output name.
        </p>
      </details>
      <span class="batch-example">{batchExample}</span>{/if}
  </div>
  <div class="files-body">
    <div class="rows" role="listbox" aria-label="Files" aria-multiselectable="true">
      {#each files as file (file.id)}<button
          type="button"
          role="option"
          class:selected={selectedIds.includes(file.id)}
          aria-selected={selectedIds.includes(file.id)}
          title={statuses[file.id]?.note ||
            (file.source?.kind === 'imageSequence' ? file.source.directoryPath : file.path)}
          onclick={(event) => select(event, file)}
          ondblclick={() => {
            onselect([file.id]);
            onload();
          }}
          >{#if statuses[file.id]}<span aria-hidden="true">{statuses[file.id].glyph}</span
            >{/if}<span>{file.name} ({formatBytes(file.sizeBytes)})</span
          >{#if statuses[file.id]}<span class="batch-status">{statuses[file.id].text}</span
            >{/if}</button
        >
      {:else}<div class="empty">
          Drag &amp; drop media files anywhere in this window to load them.
        </div>{/each}
    </div>
    <SourceThumbnail file={primary} />
  </div>
  <div class="file-actions">
    <span class="file-count"
      >{files.length}
      {files.length === 1 ? 'file' : 'files'} loaded.{#if files.length > 1 && mode === 'mux'}
        Double click any of them or use the Load Tracks button to load their tracks.{/if}</span
    >
    <button type="button" onclick={onadd} disabled={!desktop || importing}>Add Files…</button
    ><button type="button" onclick={onfolder} disabled={!desktop || importing}>Add Folder…</button>
    <details>
      <summary class:disabled={!recentPaths.length}>Recent…</summary>
      <div class="menu">
        {#each recentPaths as path}<button
            type="button"
            onclick={(event) => {
              onrecent(path);
              (event.currentTarget.closest('details') as HTMLDetailsElement).open = false;
            }}>{path}</button
          >{/each}
      </div>
    </details>
    <button
      type="button"
      onclick={onload}
      disabled={!selectedIds.length || importing || mode === 'batch'}>Load Tracks</button
    >
    <details>
      <summary>Sort…</summary>
      <div class="menu">
        {#each [['name:asc', 'Name (A-Z)'], ['name:desc', 'Name (Z-A)'], ['size:desc', 'Size (Largest First)'], ['size:asc', 'Size (Smallest First)'], ['mtime:desc', 'Most Recent First'], ['mtime:asc', 'Oldest First']] as [value, label]}<button
            type="button"
            onclick={(event) => {
              onsort(value);
              (event.currentTarget.closest('details') as HTMLDetailsElement).open = false;
            }}>{label}</button
          >{/each}
      </div>
    </details>
    <button
      type="button"
      aria-label="Move file up"
      onclick={() => onmove(-1)}
      disabled={selectedIds.length !== 1}>▲</button
    ><button
      type="button"
      aria-label="Move file down"
      onclick={() => onmove(1)}
      disabled={selectedIds.length !== 1}>▼</button
    ><button type="button" onclick={onremove} disabled={!selectedIds.length || importing}
      >Remove</button
    >
  </div>
</section>

<style>
  .batch-example {
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
    color: var(--muted-foreground);
    font-size: 11px;
  }
  .batch-status {
    margin-left: auto;
    color: var(--muted-foreground);
    font-size: 12px;
  }
  .file-list {
    height: 100%;
    display: grid;
    grid-template-rows: auto minmax(300px, 1fr) auto;
    gap: 0.6rem;
  }
  .mode-line,
  label {
    display: flex;
    align-items: center;
    gap: 0.6rem;
  }
  .mode-line {
    flex-wrap: wrap;
  }
  select,
  input {
    background: var(--background);
    color: var(--foreground);
    border: 1px solid var(--border);
    border-radius: 4px;
    padding: 0.45rem;
  }
  select {
    min-width: 420px;
    max-width: 100%;
  }
  .files-body {
    min-height: 300px;
    display: grid;
    grid-template-columns: minmax(0, 1fr) minmax(200px, 384px);
    gap: 10px;
  }
  .rows {
    position: relative;
    overflow: auto;
    border-radius: 5px;
    background: var(--background);
  }
  .rows > button {
    display: block;
    text-align: left;
    width: 100%;
    border: 0;
    border-left: 4px solid var(--primary);
    border-radius: 0;
    padding: 0.5rem 0.7rem;
    color: var(--foreground);
    background: transparent;
  }
  .rows > button.selected {
    background: var(--accent);
    color: var(--foreground);
  }
  .empty {
    display: grid;
    place-items: center;
    height: 100%;
    color: var(--muted-foreground);
  }
  .file-actions {
    display: flex;
    flex-wrap: wrap;
    align-items: center;
    justify-content: flex-end;
    gap: 0.4rem;
  }
  .file-count {
    flex: 1;
    min-width: 180px;
    color: var(--muted-foreground);
    font-size: 0.85rem;
  }
  button,
  summary {
    padding: 0.4rem 0.7rem;
    border: 0;
    border-radius: 4px;
    background: var(--secondary);
    color: var(--secondary-foreground);
    cursor: pointer;
  }
  button:disabled,
  .disabled {
    opacity: 0.5;
    cursor: default;
  }
  details {
    position: relative;
  }
  summary {
    list-style: none;
  }
  .menu,
  .help p {
    position: absolute;
    bottom: 100%;
    right: 0;
    z-index: 4;
    width: 250px;
    display: grid;
    padding: 0.3rem;
    background: var(--background);
    border: 1px solid var(--border);
  }
  .menu button {
    overflow: hidden;
    text-overflow: ellipsis;
    text-align: left;
    white-space: nowrap;
    background: none;
  }
  .menu button:hover {
    background: var(--accent);
  }
  .help p {
    bottom: auto;
    top: 100%;
    left: 0;
    right: auto;
    line-height: 1.5;
  }
  .rows,
  select,
  input,
  button,
  summary {
    border-radius: 0;
  }
  .rows {
    border: 1px solid var(--border);
  }
  .rows > button {
    border-left: 4px solid transparent;
    border-bottom: 1px solid var(--rule);
  }
  .rows > button.selected {
    border-left-color: var(--primary);
    background: var(--accent);
  }
  .rows > button:hover {
    background: var(--secondary);
  }
  .menu {
    background: var(--popover);
    box-shadow: 3px 3px 0 #454b3c38;
  }
</style>
