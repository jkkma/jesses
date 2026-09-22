<script lang="ts">
  import type { Av1anGrainSettings } from '$lib/ipc/generated';
  import {
    chooseUtilityFile,
    readAv1anGrainTable,
    makeAv1anGrainPreset,
    inspectUtilityCapabilities,
    isDesktop,
  } from '$lib/ipc/client';
  import { errorMessage } from './format';
  let {
    value,
    disabled = false,
    onchange,
  }: {
    value: Av1anGrainSettings | undefined;
    disabled?: boolean;
    onchange: (value: Av1anGrainSettings | undefined) => void;
  } = $props();
  let mode = $state<'encoder' | 'table' | 'preset'>('encoder');
  let presets = $state<string[]>([]);
  let preset = $state('');
  let pending = $state(false);
  let error = $state('');
  let abort = $state<AbortController | undefined>();
  let disposed = false;
  const table = $derived(value?.table ?? null);
  const id = $props.id();
  $effect(() => () => {
    disposed = true;
    abort?.abort();
  });
  $effect(() => {
    if (value?.table && mode === 'encoder') mode = 'table';
  });
  function patch(next: Partial<Av1anGrainSettings>) {
    onchange({
      table,
      denoise: value?.denoise ?? false,
      denoiseStrength: value?.denoiseStrength ?? 4,
      ...next,
    });
  }
  async function chooseTable() {
    error = '';
    try {
      const path = await chooseUtilityFile('Choose AV1 grain table', ['tbl', 'txt']);
      if (!path || disposed) return;
      const text = await readAv1anGrainTable(path);
      if (disposed) return;
      mode = 'table';
      patch({ table: text });
    } catch (e) {
      if (!disposed) error = errorMessage(e);
    }
  }
  async function loadPresets() {
    error = '';
    pending = true;
    try {
      const result = await inspectUtilityCapabilities();
      if (disposed) return;
      presets = result.grainPresets;
      preset ||= presets[0] ?? '';
    } catch (e) {
      if (!disposed) error = errorMessage(e);
    } finally {
      if (!disposed) pending = false;
    }
  }
  async function generate() {
    error = '';
    pending = true;
    const controller = new AbortController();
    abort = controller;
    try {
      const text = await makeAv1anGrainPreset(preset, controller.signal);
      if (disposed || controller.signal.aborted) return;
      mode = 'preset';
      patch({ table: text });
    } catch (e) {
      if (!disposed && !controller.signal.aborted) error = errorMessage(e);
    } finally {
      if (!disposed) pending = false;
      if (abort === controller) abort = undefined;
    }
  }
</script>

<details class="grain">
  <summary
    >SVT grain analysis and tables <small
      >{table
        ? 'Grain table selected'
        : value?.denoise
          ? 'Encoder denoising on'
          : 'Encoder denoising off'}</small
    ></summary
  >
  <fieldset disabled={disabled || pending}>
    <label for={`${id}-mode`}>Grain delivery</label>
    <select
      id={`${id}-mode`}
      value={mode}
      onchange={(e) => {
        mode = e.currentTarget.value as typeof mode;
        if (mode === 'encoder') onchange(undefined);
      }}
    >
      <option value="encoder">Encoder analysis · use film grain strength above</option>
      <option value="table">Grain table file</option>
      <option value="preset">Film-stock preset</option>
    </select>
    {#if mode === 'table'}<button type="button" disabled={!isDesktop()} onclick={chooseTable}
        >Choose grain table</button
      >{/if}
    {#if mode === 'preset'}
      <button type="button" disabled={!isDesktop()} onclick={loadPresets}
        >Load film-stock presets</button
      >
      {#if presets.length}<label
          >Film-stock preset<select bind:value={preset}
            >{#each presets as name}<option value={name}>{name}</option>{/each}</select
          ></label
        >
        <button type="button" onclick={generate}>Prepare grain preset</button>{/if}
    {/if}
    <label class="check"
      ><input
        type="checkbox"
        checked={value?.denoise ?? false}
        onchange={(e) => patch({ denoise: e.currentTarget.checked })}
      />{table ? 'Denoise picture before applying table' : 'Use encoder denoised picture'}</label
    >
    {#if table && value?.denoise}<label
        >Denoise strength<input
          type="number"
          min="1"
          max="16"
          step="1"
          value={value.denoiseStrength}
          oninput={(e) => patch({ denoiseStrength: e.currentTarget.valueAsNumber })}
        /></label
      >{/if}
    {#if table}<p>
        {table.split('\n').filter((line) => line.startsWith('E ')).length} table segments stored with
        this job. The original file is read only; recovery uses these saved table bytes.
      </p>
      <button
        type="button"
        onclick={() => {
          mode = 'encoder';
          onchange(undefined);
        }}>Clear grain table</button
      >{/if}
    <p>
      Encoder analysis uses the film grain strength above. Tables replace that strength. SVT builds
      may use only the first table segment; grain synthesis approximates texture and does not
      restore the original grain exactly.
    </p>
  </fieldset>
  {#if pending}<p>
      {abort ? 'Preparing grain settings…' : 'Loading film-stock presets…'}
      {#if abort}<button type="button" onclick={() => abort?.abort()}>Cancel</button>{/if}
    </p>{/if}
  {#if error}<p role="alert">{error}</p>{/if}
</details>

<style>
  .grain {
    grid-column: 1 / -1;
    border: 1px solid var(--border);
    border-radius: 0.5rem;
    min-width: 0;
  }
  summary {
    padding: 0.75rem;
    cursor: pointer;
    font-size: 0.82rem;
    font-weight: 600;
  }
  small {
    color: var(--muted-foreground);
    font-weight: 400;
    margin-left: 0.6rem;
  }
  fieldset {
    margin: 0;
    padding: 0 0.85rem 0.85rem;
    border: 0;
    display: grid;
    gap: 0.65rem;
  }
  label {
    display: grid;
    gap: 0.35rem;
    font-size: 0.82rem;
  }
  .check {
    display: flex;
    align-items: center;
  }
  select {
    max-width: 100%;
    width: fit-content;
  }
  input[type='number'] {
    width: 8rem;
  }
  button {
    justify-self: start;
  }
  button,
  select,
  input[type='number'] {
    border: 1px solid var(--border);
    border-radius: 0.3rem;
    padding: 0.45rem;
  }
  p {
    font-size: 0.77rem;
    margin: 0.4rem 0;
    color: var(--muted-foreground);
  }
  [role='alert'] {
    color: var(--destructive);
    padding: 0.5rem;
  }
</style>
