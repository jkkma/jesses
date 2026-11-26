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
    importingName = '',
    importNotice = null,
    importErrors = [],
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
    ondismissImportFeedback,
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
    importingName?: string;
    importNotice?: string | null;
    importErrors?: { name: string; message: string }[];
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
    ondismissImportFeedback?: () => void;
  } = $props();

  const singleSelectedIndex = $derived(
    selectedIds.length === 1 ? files.findIndex((file) => file.id === selectedIds[0]) : -1,
  );

  const uid = $props.id();
  const helpId = `${uid}-batch-help`;
  const recentId = `${uid}-recent-menu`;
  const sortId = `${uid}-sort-menu`;

  let rowsEl = $state<HTMLDivElement>();
  let helpTrigger = $state<HTMLButtonElement>();
  let helpOpen = $state(false);
  let recentTrigger = $state<HTMLButtonElement>();
  let sortTrigger = $state<HTMLButtonElement>();
  let recentPopover = $state<HTMLDivElement>();
  let sortPopover = $state<HTMLDivElement>();
  let focusedId = $state<string>();
  let anchorId: string | undefined;
  const tabStop = $derived(
    files.find((file) => file.id === focusedId) ??
      files.find((file) => selectedIds.includes(file.id)) ??
      files[0],
  );

  function selectRange(target: number, fallback: number) {
    const remembered = files.findIndex((file) => file.id === anchorId);
    const anchor = remembered < 0 ? fallback : remembered;
    anchorId = files[anchor].id;
    onselect(
      files.slice(Math.min(anchor, target), Math.max(anchor, target) + 1).map((file) => file.id),
    );
  }

  function select(event: MouseEvent, file: MediaFile) {
    focusedId = file.id;
    if (event.shiftKey && selectedIds.length) {
      const target = files.findIndex((entry) => entry.id === file.id);
      const fallback = files.findIndex((entry) => selectedIds.includes(entry.id));
      selectRange(target, fallback < 0 ? target : fallback);
      return;
    }
    anchorId = file.id;
    if (event.ctrlKey || event.metaKey) {
      onselect(
        selectedIds.includes(file.id)
          ? selectedIds.filter((id) => id !== file.id)
          : [...selectedIds, file.id],
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
    const current = files.findIndex((file) => file.id === tabStop?.id);
    const next =
      event.key === 'Home'
        ? 0
        : event.key === 'End'
          ? files.length - 1
          : event.key === 'ArrowDown'
            ? Math.min(files.length - 1, current + 1)
            : Math.max(0, current - 1);
    focusedId = files[next].id;
    if (event.shiftKey) selectRange(next, current < 0 ? next : current);
    else if (!event.ctrlKey && !event.metaKey) {
      anchorId = focusedId;
      onselect([focusedId]);
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
        aria-expanded={helpOpen}
        popovertarget={helpId}
        popovertargetaction="show"
        {...{ interestfor: helpId } as Record<string, string>}
        style={`anchor-name: --anchor-${helpId}`}
        bind:this={helpTrigger}
      >
        ?
      </button>
      <div
        id={helpId}
        popover="hint"
        role="tooltip"
        class="popover-surface help-popover"
        style={`position-anchor: --anchor-${helpId}`}
        ontoggle={(event) => (helpOpen = (event as ToggleEvent).newState === 'open')}
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
          tabindex={tabStop?.id === file.id ? 0 : -1}
          class:selected={selectedIds.includes(file.id)}
          aria-selected={selectedIds.includes(file.id)}
          title={statuses[file.id]?.note ||
            (file.source?.kind === 'imageSequence' ? file.source.directoryPath : file.path)}
          onclick={(event) => select(event, file)}
          onfocus={() => (focusedId = file.id)}
          ondblclick={() => {
            if (mode === 'batch' || importing) return;
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
  {#if importing || importNotice || importErrors.length}
    <div class="import-feedback">
      {#if importing}
        <p role="status">{importingName || 'Importing media…'}</p>
      {/if}
      {#if importNotice}
        <p role="status">{importNotice}</p>
      {/if}
      {#if importErrors.length}
        <p role="alert">
          Could not import {importErrors.length === 1
            ? `${importErrors[0].name}: ${importErrors[0].message}`
            : `${importErrors.length} items (first: ${importErrors[0].name}: ${importErrors[0].message}). See log for details.`}
        </p>
      {/if}
      {#if !importing && (importNotice || importErrors.length) && ondismissImportFeedback}
        <button type="button" onclick={ondismissImportFeedback}>Dismiss</button>
      {/if}
    </div>
  {/if}
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
      Add Image Sequence…
    </button><button
      type="button"
      popovertarget={recentId}
      style={`anchor-name: --anchor-${recentId}`}
      disabled={!recentPaths.length || !desktop || importing}
      bind:this={recentTrigger}
    >
      Recent…
    </button>
    <div
      id={recentId}
      popover="auto"
      class="popover-surface menu recent"
      style={`position-anchor: --anchor-${recentId}`}
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
    </button><button
      type="button"
      popovertarget={sortId}
      style={`anchor-name: --anchor-${sortId}`}
      disabled={files.length < 2}
      bind:this={sortTrigger}
    >
      Sort…
    </button>
    <div
      id={sortId}
      popover="auto"
      class="popover-surface menu"
      style={`position-anchor: --anchor-${sortId}`}
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
      disabled={singleSelectedIndex <= 0}
    >
      ▲
    </button><button
      type="button"
      aria-label="Move file down"
      onclick={() => onmove(1)}
      disabled={singleSelectedIndex < 0 || singleSelectedIndex >= files.length - 1}
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
    gap: 0.4875rem;
    container-type: inline-size;
  }
  .mode-line,
  label {
    display: flex;
    align-items: center;
    gap: 0.4875rem;
  }
  .mode-line {
    flex-wrap: wrap;
  }
  .mode-line > label {
    min-width: 0;
    max-width: 100%;
    flex: 0 1 auto;
  }
  select,
  input {
    background: var(--background);
    color: var(--foreground);
    border: 1px solid var(--border);
    padding: 0.3656rem;
  }
  select {
    width: min(420px, 100%);
    min-width: 0;
    max-width: 100%;
  }
  .import-feedback {
    display: flex;
    flex-wrap: wrap;
    align-items: center;
    gap: 6px 10px;
    padding: 5px 8px;
    border: 1px solid var(--border);
    background: var(--background);
    font-size: 0.6906rem;
  }
  .import-feedback p {
    flex: 1 1 200px;
    min-width: 0;
    color: var(--muted-foreground);
  }
  .import-feedback p[role='alert'] {
    color: var(--destructive);
    font-weight: 600;
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
    scrollbar-gutter: stable;
    border: 1px solid var(--border);
    background: var(--background);
  }
  .rows > button {
    display: flex;
    align-items: baseline;
    gap: 0.4063rem;
    text-align: left;
    width: 100%;
    border: 0;
    border-left: 4px solid transparent;
    border-bottom: 1px solid var(--rule);
    padding: 0.4063rem 0.5688rem;
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
    gap: 0.2844rem;
  }
  .file-count {
    flex: 1 1 100px;
    min-width: 100px;
    color: var(--muted-foreground);
    font-size: 0.6906rem;
  }
  button {
    padding: 0.325rem 0.5525rem;
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
    padding: 0.325rem 0.4469rem;
    font-weight: 600;
  }
  .popover-surface {
    margin: 0;
    padding: 0.2438rem;
    overflow-y: auto;
    border: 1px solid var(--border);
    background: var(--popover);
    color: var(--popover-foreground);
    box-shadow: 3px 3px 0 #454b3c38;
  }
  @supports not (position-area: block-end) {
    .popover-surface {
      position: fixed;
    }
  }
  @supports (position-area: block-end) {
    .help-popover {
      inset: auto;
      margin: 4px 0 0 0;
      position-area: block-end span-inline-end;
      position-try-fallbacks:
        flip-block,
        flip-inline,
        flip-block flip-inline;
    }
    .menu {
      inset: auto;
      margin: 0 0 4px 0;
      position-area: block-start span-inline-start;
      position-try-fallbacks:
        flip-block,
        flip-inline,
        flip-block flip-inline;
    }
  }
  .popover-surface:not(:popover-open) {
    display: none;
  }
  .help-popover {
    width: min(280px, calc(100dvw - 24px));
    padding: 0.4063rem 0.5688rem;
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
    width: min(420px, 70dvw);
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
    font-size: 0.6906rem;
  }
  .recent-empty {
    padding: 0.325rem 0.5688rem;
    color: var(--muted-foreground);
  }
  @container (inline-size <= 790px) {
    .file-actions {
      gap: 0.2031rem;
    }
    .file-actions > button {
      padding: 0.2925rem 0.4063rem;
      font-size: 0.7475rem;
    }
  }
</style>
