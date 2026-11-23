<script lang="ts">
  import { tick } from 'svelte';
  import type { MediaFile } from '$lib/ipc/generated';
  import type { FileListMode } from '$lib/source-workspace';
  import { displayPath, fileName, formatBytes } from '$lib/components/shared/format';
  import { placePopover } from '$lib/components/shared/popover-position';
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

  const uid = $props.id();
  const helpId = `${uid}-batch-help`;
  const recentId = `${uid}-recent-menu`;
  const sortId = `${uid}-sort-menu`;

  let rowsEl = $state<HTMLDivElement>();
  let helpTrigger = $state<HTMLButtonElement>();
  let recentTrigger = $state<HTMLButtonElement>();
  let sortTrigger = $state<HTMLButtonElement>();
  let recentPopover = $state<HTMLDivElement>();
  let sortPopover = $state<HTMLDivElement>();

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

  function handleListKeys(event: KeyboardEvent) {
    if (event.key === 'Delete' && selectedIds.length && !importing) {
      event.preventDefault();
      onremove();
      return;
    }
    if ((event.ctrlKey || event.metaKey) && event.key.toLowerCase() === 'a') {
      event.preventDefault();
      onselect(files.map((file) => file.id));
      return;
    }
    if (!files.length) return;
    if (!['ArrowDown', 'ArrowUp', 'Home', 'End'].includes(event.key)) return;
    event.preventDefault();
    const activeId = selectedIds.at(-1);
    const current = activeId ? files.findIndex((file) => file.id === activeId) : -1;
    const next =
      event.key === 'Home'
        ? 0
        : event.key === 'End'
          ? files.length - 1
          : event.key === 'ArrowDown'
            ? Math.min(files.length - 1, current + 1)
            : Math.max(0, current <= 0 ? 0 : current - 1);
    if (event.shiftKey && selectedIds.length) {
      const anchor = files.findIndex((file) => file.id === selectedIds[0]);
      const start = anchor < 0 ? next : anchor;
      onselect(
        files.slice(Math.min(start, next), Math.max(start, next) + 1).map((entry) => entry.id),
      );
    } else {
      onselect([files[next].id]);
    }
    void tick().then(() => {
      rowsEl?.querySelectorAll<HTMLButtonElement>('[role="option"]')[next]?.focus();
    });
  }
</script>

<section class="file-list" aria-label="File List">
  <div class="mode-line">
    <label>
      Mode <select
        value={mode}
        onchange={(event) => onmode(event.currentTarget.value as FileListMode)}
      >
        <option value="mux">Muxing Mode - Merge all loaded files into one output</option><option
          value="batch"
        >
          Batch Processing Mode - Run the task on each file separately
        </option>
      </select>
    </label>
    {#if mode === 'batch'}<label>
        Output Names <input
          value={batchNameTemplate}
          placeholder={'{name}'}
          oninput={(event) => onname(event.currentTarget.value)}
        />
      </label>
      <button
        type="button"
        class="help-trigger"
        aria-label="Batch naming help"
        popovertarget={helpId}
        bind:this={helpTrigger}
      >
        ?
      </button>
      <div
        id={helpId}
        popover="auto"
        class="popover-surface help-popover"
        use:placePopover={{ trigger: () => helpTrigger, placement: 'below', align: 'start' }}
      >
        <p>
          Use {`{name}`}, {`{ext}`}, {`{index}`}, {`{codec}`}, {`{crf}`}, {`{quality}`}, {`{preset}`},
          {`{width}`}, {`{height}`} and {`{date}`} in the output name.
        </p>
      </div>
      <span class="batch-example">{batchExample}</span>{/if}
  </div>
  <div class="files-body">
    <div
      class="rows"
      role="listbox"
      aria-label="Files"
      aria-multiselectable="true"
      tabindex="-1"
      bind:this={rowsEl}
      onkeydown={handleListKeys}
    >
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
        >
          {#if statuses[file.id]}<span class="batch-glyph" aria-hidden="true">
              {statuses[file.id].glyph}
            </span>{/if}<span class="file-name">
            {file.name} ({formatBytes(file.sizeBytes)})
          </span>{#if statuses[file.id]}<span class="batch-status">
              {statuses[file.id].text}{#if statuses[file.id].note}<span class="batch-note">
                  : {statuses[file.id].note}
                </span>{/if}
            </span>{/if}
        </button>
      {:else}<div class="empty">
          Drag &amp; drop media files anywhere in this window to load them.
        </div>{/each}
    </div>
    <SourceThumbnail file={primary} />
  </div>
  <div class="file-actions">
    <span class="file-count">
      {`${files.length} ${files.length === 1 ? 'file' : 'files'} loaded.${
        files.length > 1 && mode === 'mux'
          ? ' Double click any of them or use the Load Tracks button to load their tracks.'
          : ''
      }`}
    </span>
    <button type="button" onclick={onadd} disabled={!desktop || importing}>
      Add Files…
    </button><button type="button" onclick={onfolder} disabled={!desktop || importing}>
      Add Folder…
    </button><button
      type="button"
      popovertarget={recentId}
      disabled={!recentPaths.length}
      bind:this={recentTrigger}
    >
      Recent…
    </button>
    <div
      id={recentId}
      popover="auto"
      class="popover-surface menu recent"
      bind:this={recentPopover}
      use:placePopover={{
        trigger: () => recentTrigger,
        placement: 'above',
        align: 'end',
        constrainToMain: true,
      }}
    >
      {#each recentPaths as path}{@const shown = displayPath(path)}<button
          type="button"
          title={shown}
          aria-label={shown}
          onclick={() => {
            onrecent(path);
            recentPopover?.hidePopover();
          }}
        >
          <span class="recent-name">{fileName(shown)}</span><span class="recent-folder">
            {shown
              .slice(0, Math.max(0, shown.length - fileName(shown).length))
              .replace(/[\\/]$/, '')}
          </span>
        </button>{:else}<span class="recent-empty">No recent files yet.</span>{/each}
    </div>
    <button
      type="button"
      onclick={onload}
      disabled={!selectedIds.length || importing || mode === 'batch'}
    >
      Load Tracks
    </button><button type="button" popovertarget={sortId} bind:this={sortTrigger}>Sort…</button>
    <div
      id={sortId}
      popover="auto"
      class="popover-surface menu"
      bind:this={sortPopover}
      use:placePopover={{
        trigger: () => sortTrigger,
        placement: 'above',
        align: 'end',
        constrainToMain: true,
      }}
    >
      {#each [['name:asc', 'Name (A-Z)'], ['name:desc', 'Name (Z-A)'], ['size:desc', 'Size (Largest First)'], ['size:asc', 'Size (Smallest First)'], ['mtime:desc', 'Most Recent First'], ['mtime:asc', 'Oldest First']] as [value, label]}<button
          type="button"
          onclick={() => {
            onsort(value);
            sortPopover?.hidePopover();
          }}
        >
          {label}
        </button>{/each}
    </div>
    <button
      type="button"
      aria-label="Move file up"
      onclick={() => onmove(-1)}
      disabled={selectedIds.length !== 1}
    >
      ▲
    </button><button
      type="button"
      aria-label="Move file down"
      onclick={() => onmove(1)}
      disabled={selectedIds.length !== 1}
    >
      ▼
    </button><button type="button" onclick={onremove} disabled={!selectedIds.length || importing}>
      Remove
    </button>
  </div>
</section>

<style>
  .batch-example {
    flex: 1 1 0;
    min-width: 160px;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
    color: var(--muted-foreground);
    font-size: 0.85rem;
  }
  .batch-status {
    margin-left: auto;
    max-width: 55%;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
    color: var(--muted-foreground);
    font-size: 0.92rem;
  }
  .file-name {
    min-width: 0;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  /* The list shrinks before the buttons below it leave the panel on short
     windows (Batch mode adds a naming row above it). */
  .file-list {
    height: 100%;
    display: grid;
    grid-template-rows: auto minmax(100px, 1fr) auto;
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
    padding: 0.45rem;
  }
  select {
    min-width: min(420px, 100%);
    max-width: 100%;
  }
  .files-body {
    min-height: 100px;
    display: grid;
    grid-template-columns: minmax(0, 1fr) minmax(200px, 384px);
    grid-template-rows: minmax(0, 1fr);
    gap: 10px;
  }
  .rows {
    position: relative;
    overflow: auto;
    border: 1px solid var(--border);
    background: var(--background);
  }
  .rows > button {
    display: flex;
    align-items: baseline;
    gap: 0.5rem;
    text-align: left;
    width: 100%;
    border: 0;
    border-left: 4px solid transparent;
    border-bottom: 1px solid var(--rule);
    padding: 0.5rem 0.7rem;
    color: var(--foreground);
    background: transparent;
  }
  .rows > button:hover {
    background: var(--secondary);
  }
  .rows > button.selected {
    border-left-color: var(--primary);
    background: var(--accent);
    color: var(--foreground);
  }
  .empty {
    display: grid;
    place-items: center;
    height: 100%;
    padding: 12px;
    text-align: center;
    color: var(--muted-foreground);
  }
  .file-actions {
    display: flex;
    flex-wrap: wrap;
    align-items: center;
    justify-content: flex-end;
    gap: 0.35rem;
  }
  .file-count {
    flex: 1 1 100px;
    min-width: 100px;
    color: var(--muted-foreground);
    font-size: 0.85rem;
  }
  button {
    padding: 0.4rem 0.68rem;
    border: 0;
    background: var(--secondary);
    color: var(--secondary-foreground);
    cursor: pointer;
  }
  button:hover:not(:disabled) {
    background: var(--accent);
    color: var(--accent-foreground);
  }
  button:disabled {
    opacity: 0.5;
    cursor: not-allowed;
  }
  .help-trigger {
    min-width: 28px;
    padding: 0.4rem 0.55rem;
    font-weight: 600;
  }
  .popover-surface {
    position: fixed;
    margin: 0;
    padding: 0.3rem;
    overflow-y: auto;
    border: 1px solid var(--border);
    background: var(--popover);
    color: var(--popover-foreground);
    box-shadow: 3px 3px 0 #454b3c38;
  }
  .popover-surface:not(:popover-open) {
    display: none;
  }
  .help-popover {
    width: min(280px, calc(100vw - 24px));
    padding: 0.5rem 0.7rem;
    line-height: 1.5;
  }
  .menu {
    width: 250px;
  }
  .menu:popover-open {
    display: grid;
  }
  .menu button {
    overflow: hidden;
    text-overflow: ellipsis;
    text-align: left;
    white-space: nowrap;
    background: transparent;
    color: var(--popover-foreground);
  }
  .menu button:hover:not(:disabled) {
    background: var(--accent);
    color: var(--accent-foreground);
  }
  .menu.recent {
    width: min(420px, 70vw);
  }
  .menu.recent button {
    display: grid;
    gap: 1px;
  }
  .recent-name,
  .recent-folder {
    overflow: hidden;
    text-overflow: ellipsis;
  }
  .recent-folder {
    color: var(--muted-foreground);
    font-size: 0.85rem;
  }
  .recent-empty {
    padding: 0.4rem 0.7rem;
    color: var(--muted-foreground);
  }
  @media (max-width: 820px) {
    .file-actions {
      gap: 0.25rem;
    }
    .file-actions > button {
      padding: 0.36rem 0.5rem;
      font-size: 0.92rem;
    }
  }
</style>
