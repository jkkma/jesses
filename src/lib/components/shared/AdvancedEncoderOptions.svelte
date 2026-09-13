<script lang="ts">
  import { untrack } from 'svelte';
  import type {
    EncodeBackend,
    EncoderParameter,
    EncoderParameterCatalog,
    EncoderParameterPreset,
    VideoEncoder,
  } from '$lib/ipc/generated';
  import {
    getEncoderParameters,
    getParameterPresets,
    saveParameterPreset,
    removeParameterPreset,
  } from '$lib/ipc/client';
  import { parameterError } from './encoder-parameters';
  let {
    encoder,
    backend,
    value,
    disabled = false,
    onchange,
  }: {
    encoder: VideoEncoder;
    backend: EncodeBackend;
    value: EncoderParameter[];
    disabled?: boolean;
    onchange: (value: EncoderParameter[]) => void;
  } = $props();
  let expanded = $state(false);
  let catalog = $state<EncoderParameterCatalog | null>(null);
  let busy = $state(false);
  let error = $state<string | null>(null);
  let name = $state('');
  let selectedPreset = $state('');
  let presets = $state<EncoderParameterPreset[]>([]);
  let presetsReady = $state(false);
  let presetBusy = $state(false);
  let controller: AbortController | undefined;
  let generation = 0;
  const issue = $derived(parameterError(value, catalog));
  const availablePresets = $derived(
    presets.filter((preset) => preset.encoder === encoder && preset.backend === backend),
  );
  function message(cause: unknown): string {
    return cause instanceof Error
      ? cause.message
      : String((cause as { message?: string })?.message ?? cause);
  }
  async function load() {
    controller?.abort();
    const current = ++generation;
    const signal = (controller = new AbortController()).signal;
    busy = true;
    error = null;
    catalog = null;
    presetsReady = false;
    presets = [];
    try {
      const [result, stored] = await Promise.allSettled([
        getEncoderParameters(encoder, backend, signal),
        getParameterPresets(),
      ]);
      if (!signal.aborted && current === generation) {
        if (result.status === 'fulfilled') catalog = result.value;
        else error = message(result.reason);
        if (stored.status === 'fulfilled') {
          presets = stored.value;
          presetsReady = true;
        } else error = message(stored.reason);
      }
    } catch (cause) {
      if (!signal.aborted && current === generation)
        error =
          cause instanceof Error
            ? cause.message
            : String((cause as { message?: string })?.message ?? cause);
    } finally {
      if (current === generation) busy = false;
    }
  }
  $effect(() => {
    encoder;
    backend;
    const open = expanded;
    untrack(() => {
      controller?.abort();
      generation++;
      busy = false;
      catalog = null;
      error = null;
      selectedPreset = '';
      if (open) void load();
    });
    return () => {
      controller?.abort();
      generation++;
    };
  });
  function update(parameter: string, text: string | undefined) {
    onchange([
      ...value.filter((value) => value.name !== parameter),
      ...(text === undefined ? [] : [{ name: parameter, value: text }]),
    ]);
  }
  async function save() {
    if (!presetsReady || presetBusy) return;
    const current = generation;
    const chosenName = name.trim();
    presetBusy = true;
    try {
      if (issue) throw new Error(issue);
      const result = await saveParameterPreset({
        name: chosenName,
        encoder,
        backend,
        parameters: value.map((item) => ({ name: item.name, value: item.value })),
      });
      if (current === generation) {
        presets = result;
        selectedPreset = chosenName;
        error = null;
      }
    } catch (cause) {
      if (current === generation) error = message(cause);
    } finally {
      presetBusy = false;
    }
  }
  function applyPreset() {
    const preset = availablePresets.find((preset) => preset.name === selectedPreset);
    if (!preset) return;
    const issue = parameterError(preset.parameters, catalog);
    if (issue) {
      error = issue;
      return;
    }
    onchange(preset.parameters.map((value) => ({ name: value.name, value: value.value })));
    error = null;
  }
  async function remove() {
    const preset = availablePresets.find((preset) => preset.name === selectedPreset);
    if (!preset || !presetsReady || presetBusy) return;
    const current = generation;
    presetBusy = true;
    try {
      const result = await removeParameterPreset({
        name: preset.name,
        encoder: preset.encoder,
        backend: preset.backend,
      });
      if (current === generation) {
        presets = result;
        selectedPreset = '';
        error = null;
      }
    } catch (cause) {
      if (current === generation) error = message(cause);
    } finally {
      presetBusy = false;
    }
  }
</script>

<details class="advanced-parameters" bind:open={expanded}>
  <summary>Advanced encoder parameters{value.length ? ` (${value.length} overrides)` : ''}</summary>
  {#if expanded}
    <p class="small-muted">
      The speed preset applies first; these explicit overrides apply afterward. Saved parameter
      presets replace the override list and leave quality, speed, paths and source processing
      controls as configured. Presets are saved in this application instance’s preferences,
      including its separate portable configuration.
    </p>
    {#if busy}<p role="status">Checking the installed encoder catalog…</p>{:else if catalog}
      <p class="small-muted">{catalog.route}: {catalog.toolVersion}</p>
      <fieldset {disabled}>
        <legend>Validated scalar overrides</legend>
        {#each catalog.parameters as spec (spec.name)}
          {@const current = value.find((value) => value.name === spec.name)}
          <div class="parameter-row">
            <label
              ><input
                type="checkbox"
                aria-label={`Override ${spec.label}`}
                checked={!!current}
                onchange={(event) =>
                  update(spec.name, event.currentTarget.checked ? String(spec.minimum) : undefined)}
              />{spec.label}</label
            >
            {#if current}<input
                type="number"
                aria-label={`${spec.label} value`}
                value={current.value}
                min={spec.minimum}
                max={spec.maximum}
                step="1"
                oninput={(event) => update(spec.name, event.currentTarget.value)}
              />{/if}
            <small>{spec.argument} · {spec.minimum}–{spec.maximum}</small>
          </div>
        {/each}
        {#if !catalog.parameters.length}<p>
            This build advertises none of the qualified overrides.
          </p>{/if}
        <button type="button" onclick={() => onchange([])} disabled={disabled || !value.length}
          >Clear overrides</button
        >
      </fieldset>
      {#each catalog.notes as note}<p class="small-muted">{note}</p>{/each}
      <div class="preset-controls">
        <label
          >Parameter preset name<input
            aria-label="Parameter preset name"
            bind:value={name}
            maxlength="64"
            {disabled}
          /></label
        >
        <button
          type="button"
          onclick={save}
          disabled={disabled || !presetsReady || presetBusy || !!issue || !name.trim()}
          >Save parameter preset</button
        >
        <p class="small-muted">
          Saving an existing name replaces that preset for this encoder and workflow.
        </p>
        <label
          >Saved parameter preset<select
            aria-label="Saved parameter preset"
            bind:value={selectedPreset}
            {disabled}
            ><option value="">Choose a preset</option>{#each availablePresets as preset}<option
                value={preset.name}>{preset.name}</option
              >{/each}</select
          ></label
        >
        <button
          type="button"
          onclick={applyPreset}
          disabled={disabled || !presetsReady || presetBusy || !selectedPreset}
          >Apply parameter preset</button
        >
        <button
          type="button"
          onclick={remove}
          disabled={disabled || !presetsReady || presetBusy || !selectedPreset}
          >Remove saved parameter preset</button
        >
      </div>
    {:else}<button type="button" onclick={load} {disabled}>Check installed parameters</button>{/if}
    {#if issue}<p role="alert">{issue}</p>{/if}
    {#if error}<p role="alert">{error}</p>{/if}
  {/if}
</details>

<style>
  .advanced-parameters {
    border: 1px solid var(--border);
    border-radius: 8px;
    padding: 12px;
    min-width: 0;
  }
  summary {
    cursor: pointer;
    font-size: 13px;
    font-weight: 600;
  }
  fieldset,
  .preset-controls {
    display: grid;
    gap: 10px;
    min-width: 0;
    margin-top: 12px;
  }
  fieldset {
    border: 1px solid var(--border);
    padding: 10px;
  }
  .parameter-row {
    display: grid;
    grid-template-columns: minmax(0, 1fr) 80px;
    gap: 8px;
    align-items: center;
  }
  .parameter-row label {
    display: flex;
    gap: 8px;
    align-items: center;
    font-size: 12px;
  }
  .parameter-row small {
    grid-column: 1/-1;
    color: var(--muted-foreground);
  }
  .preset-controls label {
    display: grid;
    gap: 6px;
    font-size: 12px;
  }
  input,
  select {
    min-width: 0;
    max-width: 100%;
  }
  button {
    border: 1px solid var(--border);
    border-radius: 6px;
    padding: 8px;
    background: var(--background);
    color: var(--foreground);
    font-size: 12px;
    cursor: pointer;
  }
  button:disabled {
    opacity: 0.5;
    cursor: default;
  }
</style>
