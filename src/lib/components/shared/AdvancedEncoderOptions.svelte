<script lang="ts">
  import { tick } from 'svelte';
  import {
    chooseUtilityFile,
    getEncoderParameters,
    getParameterPresets,
    isDesktop,
    removeParameterPreset,
    saveParameterPreset,
  } from '$lib/ipc/client';
  import { contentPresets, applicableContentPreset } from './content-presets';
  import { errorMessage } from './format';
  import sourceHelp from '$lib/encoder-source-help.json';
  import SourceDialog from './SourceDialog.svelte';
  import type {
    EncodeBackend,
    EncoderParameter,
    EncoderParameterCatalog,
    EncoderParameterPreset,
    VideoEncoder,
  } from '$lib/ipc/generated';
  let {
    encoder,
    backend,
    value,
    disabled = false,
    catalogRevision = 0,
    catalogUnavailable = false,
    onchange,
    oncatalog,
    raw = {},
    onraw,
    onresource,
  }: {
    encoder: VideoEncoder;
    backend: EncodeBackend;
    value: EncoderParameter[];
    disabled?: boolean;
    catalogRevision?: number;
    catalogUnavailable?: boolean;
    onchange: (value: EncoderParameter[]) => void;
    oncatalog?: (catalog: EncoderParameterCatalog, revision: number) => void;
    raw?: Record<string, string>;
    onraw?: (value: Record<string, string>) => void;
    onresource?: (name: string, path: string) => void;
  } = $props();
  const uid = $props.id();
  /** A preset waiting for confirmation; null parameters clear the grid. */
  type PendingPreset = { name: string; parameters: EncoderParameter[] | null };
  let catalog = $state<EncoderParameterCatalog | null>(null),
    category = $state(''),
    argFilter = $state(''),
    error = $state(''),
    help = $state<string | null>(null),
    pendingPreset = $state<PendingPreset | null>(null),
    confirmDeletePreset = $state<EncoderParameterPreset | null>(null),
    categoryListEl = $state<HTMLDivElement>();
  // Named presets the user saved, kept in preferences per encoder and route.
  let saved = $state<EncoderParameterPreset[]>([]),
    savedChoice = $state(''),
    saveOpen = $state(false),
    saveName = $state(''),
    saveError = $state(''),
    savingPreset = $state(false);
  const savedHere = $derived(
    saved.filter((preset) => preset.encoder === encoder && preset.backend === backend),
  );
  const chosenSaved = $derived(savedHere.find((preset) => preset.name === savedChoice));
  $effect(() => {
    if (!isDesktop()) return;
    let alive = true;
    void getParameterPresets()
      .then((list) => {
        if (alive) saved = list ?? [];
      })
      .catch((cause) => {
        if (alive) error = `Saved presets are unavailable: ${errorMessage(cause)}`;
      });
    return () => {
      alive = false;
    };
  });
  async function savePreset() {
    const name = saveName.trim();
    if (!name || savingPreset) return;
    savingPreset = true;
    saveError = '';
    try {
      saved =
        (await saveParameterPreset({
          name,
          encoder,
          backend,
          parameters: value.map((item) => ({ name: item.name, value: item.value })),
        })) ?? [];
      savedChoice = name;
      saveOpen = false;
    } catch (cause) {
      saveError = errorMessage(cause);
    } finally {
      savingPreset = false;
    }
  }
  function requestDeletePreset() {
    const preset = chosenSaved;
    if (!preset) return;
    confirmDeletePreset = preset;
  }
  async function deletePreset() {
    const preset = confirmDeletePreset;
    confirmDeletePreset = null;
    if (!preset) return;
    try {
      saved = (await removeParameterPreset({ name: preset.name, encoder, backend })) ?? [];
      if (savedChoice === preset.name) savedChoice = '';
    } catch (cause) {
      error = errorMessage(cause);
    }
  }
  type HelpRow = { summary: string; details: string; examples: string };
  const sourceRows = $derived(
    (sourceHelp as Record<string, Record<string, HelpRow>>)[encoder] ?? {},
  );
  const virtuals = $derived(
    encoder === 'x265Standalone' || encoder === 'x265'
      ? [
          { name: 'master-display', group: 'HDR Metadata' },
          { name: 'max-cll', group: 'HDR Metadata' },
        ]
      : encoder.startsWith('svtAv1')
        ? [
            { name: 'fgs-table', group: 'Film Grain & Noise' },
            { name: 'dolby-vision-rpu', group: 'HDR Metadata' },
            { name: 'hdr10plus-json', group: 'HDR Metadata' },
          ]
        : [],
  );
  const groups = $derived([
    ...new Set([
      ...(catalog?.parameters ?? []).map((p) => p.group || 'Arguments'),
      ...virtuals.map((p) => p.group),
    ]),
  ]);
  const activeCategory = $derived(groups.includes(category) ? category : (groups[0] ?? ''));
  const activeCategoryIndex = $derived(Math.max(0, groups.indexOf(activeCategory)));
  const filterQuery = $derived(argFilter.trim().toLowerCase());
  const visibleCatalogParams = $derived(
    (catalog?.parameters ?? []).filter((p) => {
      if (virtuals.some((managed) => managed.name === p.name)) return false;
      if (filterQuery) {
        const summary = (sourceRows[p.name]?.summary ?? p.description).toLowerCase();
        return (
          p.name.toLowerCase().includes(filterQuery) ||
          p.argument.toLowerCase().includes(filterQuery) ||
          summary.includes(filterQuery)
        );
      }
      return (p.group || 'Arguments') === activeCategory;
    }),
  );
  const visibleVirtuals = $derived(
    virtuals.filter((p) => {
      if (filterQuery) {
        const summary = (sourceRows[p.name]?.summary ?? '').toLowerCase();
        return p.name.toLowerCase().includes(filterQuery) || summary.includes(filterQuery);
      }
      return p.group === activeCategory;
    }),
  );
  function groupModifiedCount(group: string) {
    const catCount = (catalog?.parameters ?? []).filter(
      (p) =>
        (p.group || 'Arguments') === group &&
        !virtuals.some((m) => m.name === p.name) &&
        value.some((item) => item.name === p.name && item.value.trim() !== ''),
    ).length;
    const virtCount = virtuals.filter(
      (p) => p.group === group && (raw[p.name] ?? '').trim() !== '',
    ).length;
    return catCount + virtCount;
  }
  const browsableVirtuals = new Set(['fgs-table', 'dolby-vision-rpu', 'hdr10plus-json']);
  async function browseVirtualResource(name: string) {
    if (!browsableVirtuals.has(name)) return;
    const spec =
      name === 'fgs-table'
        ? { title: 'Pick a film grain table', extensions: ['tbl'] }
        : name === 'dolby-vision-rpu'
          ? { title: 'Pick a Dolby Vision RPU file', extensions: ['bin', 'rpu'] }
          : { title: 'Pick an HDR10+ JSON file', extensions: ['json'] };
    try {
      const path = await chooseUtilityFile(spec.title, spec.extensions);
      if (path) {
        onraw?.({ ...raw, [name]: path });
        if (name === 'fgs-table') onresource?.(name, path);
      }
    } catch (cause) {
      error = errorMessage(cause);
    }
  }
  const presets = $derived(contentPresets(encoder));
  const helpRow = $derived(help ? sourceRows[help] : undefined);
  const examples = $derived(
    (helpRow?.examples ?? '')
      .replaceAll('\\n', '\n')
      .split('\n')
      .filter((line) => line.trim())
      .map((line) => {
        const [value, ...text] = line.split('|');
        return { value: value.trim(), text: text.join('|').trim() };
      }),
  );
  function currentFilled() {
    return [
      ...value,
      ...Object.entries(raw)
        .filter(([, v]) => v.trim())
        .map(([name, value]) => ({ name, value })),
    ];
  }
  function covers(parameters: EncoderParameter[], filled: EncoderParameter[]) {
    return (
      filled.length > 0 &&
      filled.every((item) =>
        parameters.some(
          (p) =>
            p.name === item.name &&
            p.value.replaceAll(',', ':') === item.value.replaceAll(',', ':'),
        ),
      )
    );
  }
  /** Applies a built-in or saved preset, or clears the grid when `parameters` is null. */
  function applyParameters(name: string, parameters: EncoderParameter[] | null, confirmed = false) {
    if (!catalog) return;
    const filled = currentFilled();
    if (!parameters && !filled.length) return;
    if (parameters && filled.length === parameters.length && covers(parameters, filled)) return;
    const known = [...presets, ...savedHere].map((preset) => preset.parameters);
    if (!confirmed && filled.length && !known.some((list) => covers(list, filled))) {
      pendingPreset = { name, parameters };
      return;
    }
    onraw?.({});
    if (!parameters) {
      onchange([]);
      error = '';
      return;
    }
    const result = applicableContentPreset(parameters, catalog, encoder);
    onchange(result.accepted);
    error = result.skipped.length ? `Unavailable arguments: ${result.skipped.join(', ')}.` : '';
  }
  function applyPreset(name: string) {
    applyParameters(name, presets.find((p) => p.name === name)?.parameters ?? null);
  }
  function handleCategoryKeys(event: KeyboardEvent, index: number) {
    if (!groups.length) return;
    const next =
      event.key === 'Home'
        ? 0
        : event.key === 'End'
          ? groups.length - 1
          : event.key === 'ArrowRight'
            ? (index + 1) % groups.length
            : event.key === 'ArrowLeft'
              ? (index + groups.length - 1) % groups.length
              : -1;
    if (next < 0) return;
    event.preventDefault();
    argFilter = '';
    category = groups[next];
    void tick().then(() => {
      const target = categoryListEl?.querySelectorAll<HTMLButtonElement>('[role="tab"]')[next];
      target?.scrollIntoView({
        block: 'nearest',
        inline: 'nearest',
        container: 'nearest',
      } as ScrollIntoViewOptions);
      target?.focus();
    });
  }
  function handleRowContextMenu(event: MouseEvent, name: string) {
    if ((event.target as HTMLElement | null)?.closest('input, textarea')) return;
    event.preventDefault();
    help = name;
  }
  $effect(() => {
    const currentEncoder = encoder,
      currentBackend = backend,
      revision = catalogRevision,
      unavailable = catalogUnavailable;
    const controller = new AbortController();
    catalog = null;
    error = '';
    if (!unavailable)
      void getEncoderParameters(currentEncoder, currentBackend, controller.signal)
        .then((result) => {
          if (!controller.signal.aborted) {
            catalog = result;
            category = result.parameters[0]?.group || 'Arguments';
            oncatalog?.(result, revision);
          }
        })
        .catch((cause) => {
          if (!controller.signal.aborted)
            error = cause instanceof Error ? cause.message : String(cause);
        });
    return () => controller.abort();
  });
  function update(name: string, text: string) {
    onchange([
      ...value.filter((item) => item.name !== name),
      ...(text.trim() ? [{ name, value: text }] : []),
    ]);
  }
</script>

{#if error}<p role="alert">{error}</p>{/if}
{#if presets.length}<div class="preset-row">
    <span>Presets</span>{#each presets as preset}<button
        type="button"
        disabled={!catalog || disabled}
        title={preset.description}
        onclick={() => applyPreset(preset.name)}
      >
        {preset.name}
      </button>{/each}<button
      type="button"
      disabled={disabled || !currentFilled().length}
      onclick={() => applyPreset('')}
    >
      Clear
    </button>
  </div>{/if}
{#if isDesktop()}<div class="preset-row" role="group" aria-label="Saved presets">
    <span>My presets</span><select
      aria-label="Saved preset"
      bind:value={savedChoice}
      disabled={disabled || !savedHere.length}
    >
      <option value="">
        {savedHere.length ? 'Choose a saved preset' : 'None saved yet'}
      </option>{#each savedHere as preset}<option value={preset.name}>{preset.name}</option>{/each}
    </select><button
      type="button"
      disabled={!catalog || disabled || !chosenSaved}
      onclick={() => chosenSaved && applyParameters(chosenSaved.name, chosenSaved.parameters)}
    >
      Apply
    </button><button
      type="button"
      disabled={disabled || !value.length}
      title="Save the arguments set in this grid under a name"
      onclick={() => {
        saveName = chosenSaved?.name ?? '';
        saveError = '';
        saveOpen = true;
      }}
    >
      Save as…
    </button><button
      type="button"
      disabled={disabled || !chosenSaved}
      onclick={requestDeletePreset}
    >
      Delete
    </button>{#if !presets.length && currentFilled().length}<button
        type="button"
        {disabled}
        onclick={() => applyParameters('', null)}
      >
        Clear
      </button>{/if}
  </div>{:else if !presets.length && currentFilled().length}<div class="preset-row">
    <button type="button" {disabled} onclick={() => applyParameters('', null)}>Clear</button>
  </div>{/if}
<search class="filter-bar">
  <input
    type="search"
    aria-label="Filter arguments"
    placeholder="Filter arguments…"
    bind:value={argFilter}
  />
  {#if filterQuery}<button type="button" onclick={() => (argFilter = '')}>Show all</button>{/if}
</search>
<div
  role="tablist"
  aria-label="Encoder argument categories"
  class="categories"
  bind:this={categoryListEl}
>
  {#each groups as group, index}{@const count = groupModifiedCount(group)}<button
      type="button"
      id={`${uid}-cat-${index}`}
      role="tab"
      aria-selected={!filterQuery && activeCategory === group}
      tabindex={activeCategory === group ? 0 : -1}
      class:active={!filterQuery && activeCategory === group}
      onkeydown={(event) => handleCategoryKeys(event, index)}
      onclick={() => {
        argFilter = '';
        category = group;
      }}
    >
      {group}{count ? ` (${count})` : ''}
    </button>{/each}
</div>
<!-- svelte-ignore a11y_no_noninteractive_tabindex (The scrollable arguments table needs keyboard access.) -->
<div
  class="grid"
  role={filterQuery ? 'region' : 'tabpanel'}
  tabindex="0"
  aria-label={filterQuery ? `Filtered arguments matching ${argFilter.trim()}` : undefined}
  aria-labelledby={!filterQuery && groups.length ? `${uid}-cat-${activeCategoryIndex}` : undefined}
>
  <table>
    <caption class="visually-hidden">Encoder arguments</caption>
    <thead>
      <tr>
        <th scope="col">Argument</th>
        <th scope="col">Value</th>
        <th scope="col">Description, Possible Values</th>
      </tr>
    </thead>
    <tbody>
      {#each visibleCatalogParams as spec}<tr
          oncontextmenu={(event) => handleRowContextMenu(event, spec.name)}
        >
          <td>
            <div class="arg-cell">
              <span class="mono">{spec.argument}</span><button
                type="button"
                class="arg-help"
                aria-label={`Details for ${spec.argument}`}
                title="Show argument details"
                onclick={() => (help = spec.name)}
              >
                ?
              </button>
            </div>
          </td><td>
            <input
              aria-label={`${spec.label} value`}
              {disabled}
              value={value.find((item) => item.name === spec.name)?.value ?? ''}
              oninput={(event) => update(spec.name, event.currentTarget.value)}
            />
          </td><td>{sourceRows[spec.name]?.summary ?? spec.description.split('\n\n')[0]}</td>
        </tr>{/each}
      {#each visibleVirtuals as spec}<tr
          oncontextmenu={(event) => handleRowContextMenu(event, spec.name)}
        >
          <td>
            <div class="arg-cell">
              <span class="mono">{spec.name}</span><button
                type="button"
                class="arg-help"
                aria-label={`Details for ${spec.name}`}
                title="Show argument details"
                onclick={() => (help = spec.name)}
              >
                ?
              </button>
            </div>
          </td><td>
            <div class="virtual-value">
              <input
                aria-label={`${spec.name} value`}
                {disabled}
                value={raw[spec.name] ?? ''}
                oninput={(event) => onraw?.({ ...raw, [spec.name]: event.currentTarget.value })}
                onblur={(event) => {
                  if (spec.name === 'fgs-table') onresource?.(spec.name, event.currentTarget.value);
                }}
              />{#if isDesktop() && browsableVirtuals.has(spec.name)}<button
                  type="button"
                  class="arg-browse"
                  aria-label={`Browse ${spec.name}`}
                  title={`Browse ${spec.name}`}
                  {disabled}
                  onclick={() => void browseVirtualResource(spec.name)}
                >
                  …
                </button>{/if}
            </div>
          </td><td>
            {sourceRows[spec.name]?.summary ??
              (spec.name === 'fgs-table' ? 'Path to an AV1 film grain table (.tbl) file.' : '')}
          </td>
        </tr>{/each}
      {#if filterQuery && !visibleCatalogParams.length && !visibleVirtuals.length}
        <tr><td colspan="3" class="small-muted">No arguments match "{argFilter.trim()}".</td></tr>
      {/if}
    </tbody>
  </table>
</div>
{#if help}{@const spec = catalog?.parameters.find((p) => p.name === help)}<SourceDialog
    title="Argument Details"
    open={true}
    closedby="any"
    onclose={() => (help = null)}
    width={720}
  >
    <div class="help-content">
      <strong>{spec?.argument ?? `--${help}`}</strong>
      <p>{helpRow?.summary ?? spec?.description.split('\n\n')[0] ?? ''}</p>
      <p class="details">
        {helpRow?.details ||
          "No extended description has been written for this argument yet. The line above is what the encoder's own documentation says about it."}
      </p>
      {#if examples.length}<h3>EXAMPLES</h3>
        {#each examples as example}<div class="example">
            <code>{example.value}</code><span>{example.text}</span>
          </div>{/each}{/if}
      <div class="help-actions">
        <button type="button" onclick={() => (help = null)}>Close</button>
      </div>
    </div>
  </SourceDialog>{/if}
<SourceDialog
  title="Replace the arguments already set?"
  open={pendingPreset !== null}
  onclose={() => (pendingPreset = null)}
>
  <p>
    {currentFilled().length} argument{currentFilled().length === 1 ? ' is' : 's are'} set, and {currentFilled()
      .length === 1
      ? 'it does'
      : 'they do'} not match a preset:
  </p>
  <p>
    {currentFilled()
      .map((p) => `${p.name} ${p.value}`)
      .join(', ')}
  </p>
  <p>
    {pendingPreset?.parameters
      ? `Applying '${pendingPreset.name}' replaces them all. Continue?`
      : 'Clearing empties them all. Continue?'}
  </p>
  <div class="help-actions">
    <!-- svelte-ignore a11y_autofocus -->
    <button type="button" autofocus onclick={() => (pendingPreset = null)}> No </button><button
      type="button"
      onclick={() => {
        const choice = pendingPreset;
        pendingPreset = null;
        if (choice) applyParameters(choice.name, choice.parameters, true);
      }}
    >
      Yes
    </button>
  </div>
</SourceDialog>
<SourceDialog title="Save preset" open={saveOpen} onclose={() => (saveOpen = false)} width={440}>
  <form
    onsubmit={(event) => {
      event.preventDefault();
      if (saveName.trim() && !savingPreset) void savePreset();
    }}
  >
    <label class="save-name">
      <!-- svelte-ignore a11y_autofocus -->
      Preset name<input autofocus maxlength="64" bind:value={saveName} />
    </label>
    <p>
      Saves the {value.length} argument{value.length === 1 ? '' : 's'} set in this grid for this encoder
      and route. HDR and grain file fields are not saved.
    </p>
    {#if savedHere.some((preset) => preset.name === saveName.trim())}<p>
        This replaces the saved preset with the same name.
      </p>{/if}
    {#if saveError}<p role="alert">{saveError}</p>{/if}
    <div class="help-actions">
      <button type="button" onclick={() => (saveOpen = false)}>Cancel</button><button
        type="submit"
        disabled={!saveName.trim() || savingPreset}
      >
        Save
      </button>
    </div>
  </form>
</SourceDialog>
<SourceDialog
  title="Delete saved preset?"
  open={confirmDeletePreset !== null}
  onclose={() => (confirmDeletePreset = null)}
  width={440}
>
  {#if confirmDeletePreset}
    <p>Delete the saved preset "{confirmDeletePreset.name}"? This cannot be undone.</p>
    <div class="help-actions">
      <!-- svelte-ignore a11y_autofocus -->
      <button type="button" autofocus onclick={() => (confirmDeletePreset = null)}>
        Keep preset
      </button>
      <button type="button" class="danger" onclick={() => void deletePreset()}>
        Delete preset
      </button>
    </div>
  {/if}
</SourceDialog>

<style>
  .preset-row {
    display: flex;
    gap: 6px;
    align-items: center;
    flex-wrap: wrap;
    margin: 6px 0;
    font-size: 0.7475rem;
  }
  .preset-row select {
    color: inherit;
    background: var(--background);
    border: 1px solid var(--border);
    padding: 4px;
    min-width: 180px;
  }
  .save-name {
    display: grid;
    gap: 4px;
  }
  .filter-bar {
    display: flex;
    gap: 6px;
    align-items: center;
    margin: 6px 0;
  }
  .filter-bar input {
    width: min(260px, 100%);
  }
  .virtual-value {
    display: flex;
    gap: 4px;
    align-items: center;
  }
  .virtual-value input {
    flex: 1;
    min-width: 0;
  }
  .arg-browse {
    flex: 0 0 auto;
    padding: 4px 7px;
  }
  button.danger {
    border-color: var(--destructive);
    color: var(--destructive);
  }
  button.danger:hover:not(:disabled) {
    background: var(--destructive);
    color: var(--primary-foreground);
  }
  .categories {
    display: flex;
    gap: 2px;
    flex-wrap: nowrap;
    overflow-x: auto;
    border-bottom: 1px solid var(--border);
  }
  button {
    color: inherit;
    background: var(--background);
    border: 1px solid var(--border);
    padding: 5px 10px;
    cursor: pointer;
  }
  button:hover:not(:disabled) {
    background: var(--secondary);
  }
  .categories button {
    position: relative;
    flex: 0 0 auto;
    border-color: transparent;
    border-bottom: 0;
    background: transparent;
    color: var(--muted-foreground);
    white-space: nowrap;
  }
  .categories button:hover:not([aria-selected='true']) {
    background: var(--secondary);
    color: var(--foreground);
  }
  .categories button[aria-selected='true'] {
    background: var(--primary);
    color: var(--primary-foreground);
    border-color: var(--primary);
  }
  .categories button[aria-selected='true']::after {
    content: '';
    position: absolute;
    inset: auto 0 0;
    border-bottom: 2px solid var(--primary-foreground);
  }
  .grid {
    overflow: auto;
    scrollbar-gutter: stable;
    max-height: 50dvh;
  }
  table {
    width: 100%;
    border-collapse: collapse;
    font-size: 0.7475rem;
  }
  tbody tr {
    content-visibility: auto;
    contain-intrinsic-block-size: auto 1.75rem;
  }
  th,
  td {
    text-align: left;
    padding: 4px 6px;
    border-bottom: 1px solid var(--border);
  }
  th:first-child {
    width: 250px;
  }
  th:nth-child(2) {
    width: 110px;
  }
  .arg-cell {
    display: flex;
    align-items: center;
    justify-content: space-between;
    gap: 6px;
  }
  .arg-help {
    flex: 0 0 auto;
    min-inline-size: 24px;
    min-block-size: 24px;
    padding: 0 5px;
    font-size: 0.6906rem;
    line-height: 1;
    color: var(--muted-foreground);
    background: var(--secondary);
    border: 1px solid var(--border);
  }
  .arg-help:hover:not(:disabled) {
    background: var(--accent);
    color: var(--accent-foreground);
  }
  input {
    width: 100%;
    padding: 4px;
    color: inherit;
    background: var(--background);
    border: 1px solid var(--border);
  }
  .help-content {
    max-height: 70dvh;
    overflow: auto;
  }
  .help-content strong {
    font-size: 1.1213rem;
  }
  .details {
    white-space: pre-wrap;
  }
  .example {
    display: grid;
    grid-template-columns: minmax(80px, auto) 1fr;
    gap: 8px;
    margin: 6px 0;
  }
  .example code {
    max-width: 340px;
    overflow-wrap: anywhere;
    background: var(--muted);
    padding: 4px;
  }
  .help-actions {
    display: flex;
    justify-content: flex-end;
    gap: 8px;
    margin-top: 12px;
  }
  p[role='alert'] {
    color: var(--destructive);
    font-weight: 600;
  }
</style>
