<script lang="ts">
  import {
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
  /** A preset waiting for confirmation; null parameters clear the grid. */
  type PendingPreset = { name: string; parameters: EncoderParameter[] | null };
  let catalog = $state<EncoderParameterCatalog | null>(null),
    category = $state(''),
    error = $state(''),
    help = $state<string | null>(null),
    pendingPreset = $state<PendingPreset | null>(null);
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
  async function deletePreset() {
    const preset = chosenSaved;
    if (!preset) return;
    try {
      saved = (await removeParameterPreset({ name: preset.name, encoder, backend })) ?? [];
      savedChoice = '';
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
        disabled={!catalog || disabled}
        title={preset.description}
        onclick={() => applyPreset(preset.name)}>{preset.name}</button
      >{/each}<button {disabled} onclick={() => applyPreset('')}>Clear</button>
  </div>{/if}
{#if isDesktop()}<div class="preset-row" role="group" aria-label="Saved presets">
    <span>My presets</span><select
      aria-label="Saved preset"
      bind:value={savedChoice}
      disabled={disabled || !savedHere.length}
      ><option value="">{savedHere.length ? 'Choose a saved preset' : 'None saved yet'}</option
      >{#each savedHere as preset}<option value={preset.name}>{preset.name}</option>{/each}</select
    ><button
      disabled={!catalog || disabled || !chosenSaved}
      onclick={() => chosenSaved && applyParameters(chosenSaved.name, chosenSaved.parameters)}
      >Apply</button
    ><button
      disabled={disabled || !value.length}
      title="Save the arguments set in this grid under a name"
      onclick={() => {
        saveName = chosenSaved?.name ?? '';
        saveError = '';
        saveOpen = true;
      }}>Save as…</button
    ><button disabled={disabled || !chosenSaved} onclick={() => void deletePreset()}>Delete</button>
  </div>{/if}
<div role="tablist" aria-label="Encoder argument categories" class="categories">
  {#each groups as group}<button
      role="tab"
      aria-selected={category === group}
      class:active={category === group}
      onclick={() => (category = group)}>{group}</button
    >{/each}
</div>
<div class="grid">
  <table>
    <thead><tr><th>Argument</th><th>Value</th><th>Description, Possible Values</th></tr></thead
    ><tbody>
      {#each (catalog?.parameters ?? []).filter((p) => (p.group || 'Arguments') === category && !virtuals.some((managed) => managed.name === p.name)) as spec}<tr
          oncontextmenu={(event) => {
            event.preventDefault();
            help = spec.name;
          }}
          ><td>{spec.argument}</td><td
            ><input
              aria-label={`${spec.label} value`}
              {disabled}
              value={value.find((item) => item.name === spec.name)?.value ?? ''}
              oninput={(event) => update(spec.name, event.currentTarget.value)}
            /></td
          ><td>{sourceRows[spec.name]?.summary ?? spec.description.split('\n\n')[0]}</td></tr
        >{/each}
      {#each virtuals.filter((p) => p.group === category) as spec}<tr
          oncontextmenu={(event) => {
            event.preventDefault();
            help = spec.name;
          }}
          ><td>{spec.name}</td><td
            ><input
              aria-label={`${spec.name} value`}
              {disabled}
              value={raw[spec.name] ?? ''}
              oninput={(event) => onraw?.({ ...raw, [spec.name]: event.currentTarget.value })}
              onblur={(event) => {
                if (spec.name === 'fgs-table') onresource?.(spec.name, event.currentTarget.value);
              }}
            /></td
          ><td>{sourceRows[spec.name]?.summary ?? ''}</td></tr
        >{/each}
    </tbody>
  </table>
</div>
{#if help}{@const spec = catalog?.parameters.find((p) => p.name === help)}<SourceDialog
    title="Argument Details"
    open={true}
    onclose={() => (help = null)}
    width={720}
    ><div class="help-content">
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
      <div class="help-actions"><button onclick={() => (help = null)}>Close</button></div>
    </div></SourceDialog
  >{/if}
<SourceDialog
  title="Replace the arguments already set?"
  open={pendingPreset !== null}
  onclose={() => (pendingPreset = null)}
  ><p>
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
    <button onclick={() => (pendingPreset = null)}>No</button><button
      onclick={() => {
        const choice = pendingPreset;
        pendingPreset = null;
        if (choice) applyParameters(choice.name, choice.parameters, true);
      }}>Yes</button
    >
  </div></SourceDialog
>
<SourceDialog title="Save preset" open={saveOpen} onclose={() => (saveOpen = false)} width={440}
  ><label class="save-name"
    >Preset name<input
      data-dialog-initial-focus
      maxlength="64"
      bind:value={saveName}
      onkeydown={(event) => {
        if (event.key === 'Enter') void savePreset();
      }}
    /></label
  >
  <p>
    Saves the {value.length} argument{value.length === 1 ? '' : 's'} set in this grid for this encoder
    and route. HDR and grain file fields are not saved.
  </p>
  {#if savedHere.some((preset) => preset.name === saveName.trim())}<p>
      This replaces the saved preset with the same name.
    </p>{/if}
  {#if saveError}<p role="alert">{saveError}</p>{/if}
  <div class="help-actions">
    <button onclick={() => (saveOpen = false)}>Cancel</button><button
      disabled={!saveName.trim() || savingPreset}
      onclick={() => void savePreset()}>Save</button
    >
  </div></SourceDialog
>

<style>
  .preset-row {
    display: flex;
    gap: 6px;
    align-items: center;
    flex-wrap: wrap;
    margin: 6px 0;
    font-size: 12px;
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
  .categories {
    display: flex;
    gap: 2px;
    flex-wrap: wrap;
    border-bottom: 1px solid var(--border);
  }
  button {
    color: inherit;
    background: var(--background);
    border: 1px solid var(--border);
    padding: 5px 10px;
    cursor: pointer;
  }
  .categories button {
    border-bottom: 0;
  }
  .active {
    font-weight: bold;
  }
  .grid {
    overflow: auto;
    max-height: 50vh;
  }
  table {
    width: 100%;
    border-collapse: collapse;
    font-size: 12px;
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
  input {
    width: 100%;
    padding: 4px;
    color: inherit;
    background: var(--background);
    border: 1px solid var(--border);
  }
  .help-content {
    max-height: 70vh;
    overflow: auto;
  }
  .help-content strong {
    font-size: 18px;
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
</style>
