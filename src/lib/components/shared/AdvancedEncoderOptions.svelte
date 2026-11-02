<script lang="ts">
  import { getEncoderParameters } from '$lib/ipc/client';
  import { contentPresets, applicableContentPreset } from './content-presets';
  import sourceHelp from '$lib/encoder-source-help.json';
  import SourceDialog from './SourceDialog.svelte';
  import type {
    EncodeBackend,
    EncoderParameter,
    EncoderParameterCatalog,
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
  let catalog = $state<EncoderParameterCatalog | null>(null),
    category = $state(''),
    error = $state(''),
    help = $state<string | null>(null),
    pendingPreset = $state<string | null>(null);
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
  function covers(preset: (typeof presets)[number], filled: EncoderParameter[]) {
    return (
      filled.length > 0 &&
      filled.every((item) =>
        preset.parameters.some(
          (p) =>
            p.name === item.name &&
            p.value.replaceAll(',', ':') === item.value.replaceAll(',', ':'),
        ),
      )
    );
  }
  function applyPreset(name: string, confirmed = false) {
    if (!catalog) return;
    const preset = presets.find((p) => p.name === name),
      filled = currentFilled();
    if (!preset && !filled.length) return;
    if (preset && filled.length === preset.parameters.length && covers(preset, filled)) return;
    if (!confirmed && filled.length && !presets.some((p) => covers(p, filled))) {
      pendingPreset = name;
      return;
    }
    onraw?.({});
    if (!preset) {
      onchange([]);
      error = '';
      return;
    }
    const result = applicableContentPreset(preset.parameters, catalog, encoder);
    onchange(result.accepted);
    error = result.skipped.length ? `Unavailable arguments: ${result.skipped.join(', ')}.` : '';
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
    {pendingPreset
      ? `Applying '${pendingPreset}' replaces them all. Continue?`
      : 'Clearing empties them all. Continue?'}
  </p>
  <div class="help-actions">
    <button onclick={() => (pendingPreset = null)}>No</button><button
      onclick={() => {
        const choice = pendingPreset ?? '';
        pendingPreset = null;
        applyPreset(choice, true);
      }}>Yes</button
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
