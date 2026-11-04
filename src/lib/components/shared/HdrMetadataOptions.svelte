<script lang="ts">
  import { chooseUtilityFile } from '$lib/ipc/client';
  import type { HdrMetadataSettings, HdrMasteringDisplay, VideoEncoder } from '$lib/ipc/generated';
  import { supportsDynamicHdr, validHdrMetadata } from './hdr-metadata-options';
  import { errorMessage } from './format';
  let {
    value,
    encoder,
    disabled,
    scope,
    onchange,
  }: {
    value: HdrMetadataSettings | undefined;
    encoder: VideoEncoder;
    disabled: boolean;
    scope: string;
    onchange: (value: HdrMetadataSettings | undefined) => void;
  } = $props();
  const defaults = (): HdrMasteringDisplay => ({
    red: { x: 35400, y: 14600 },
    green: { x: 8500, y: 39850 },
    blue: { x: 6550, y: 2300 },
    whitePoint: { x: 15635, y: 16450 },
    maxLuminanceTenThousandths: 10000000,
    minLuminanceTenThousandths: 50,
  });
  const keys = ['red', 'green', 'blue', 'whitePoint'] as const;
  let pickerError = $state('');
  let selectionVersion = 0;
  $effect(() => {
    scope;
    selectionVersion++;
    pickerError = '';
  });
  function change(valueNext: HdrMetadataSettings) {
    onchange(Object.keys(valueNext).length ? valueNext : undefined);
  }
  function number(event: Event, scale: number) {
    return Math.round((event.currentTarget as HTMLInputElement).valueAsNumber * scale);
  }
  async function choose(key: 'dolbyVisionRpuPath' | 'hdr10PlusJsonPath') {
    const version = selectionVersion;
    pickerError = '';
    try {
      const path = await chooseUtilityFile(
        key === 'dolbyVisionRpuPath' ? 'Choose Dolby Vision RPU' : 'Choose HDR10+ JSON',
        key === 'dolbyVisionRpuPath' ? ['bin', 'rpu'] : ['json'],
      );
      if (path && version === selectionVersion) change({ ...value, [key]: path });
    } catch (error) {
      if (version === selectionVersion) pickerError = errorMessage(error);
    }
  }
  function clear(key: keyof HdrMetadataSettings) {
    const next = { ...value };
    delete next[key];
    change(next);
  }
</script>

<details class="hdr-options">
  <summary>HDR metadata overrides</summary>
  <fieldset {disabled}>
    <p>
      Optional overrides for HDR output. Leave these off to preserve source metadata. Dynamic
      metadata requires a compatible installed SVT build, matching frame count, and no picture or
      timing transformations.
    </p>
    <label
      ><input
        type="checkbox"
        checked={!!value?.masteringDisplay}
        onchange={(event) =>
          event.currentTarget.checked
            ? change({ ...value, masteringDisplay: defaults() })
            : clear('masteringDisplay')}
      /> Override mastering display</label
    >
    {#if value?.masteringDisplay}
      {@const mastering = value.masteringDisplay}
      <div class="grid">
        {#each keys as key}<label
            >{key === 'whitePoint' ? 'White point' : key} x<input
              type="number"
              min="0"
              max="1"
              step="0.00002"
              value={mastering[key].x / 50000}
              oninput={(event) =>
                change({
                  ...value,
                  masteringDisplay: {
                    ...mastering,
                    [key]: { ...mastering[key], x: number(event, 50000) },
                  },
                })}
            /></label
          ><label
            >{key === 'whitePoint' ? 'White point' : key} y<input
              type="number"
              min="0"
              max="1"
              step="0.00002"
              value={mastering[key].y / 50000}
              oninput={(event) =>
                change({
                  ...value,
                  masteringDisplay: {
                    ...mastering,
                    [key]: { ...mastering[key], y: number(event, 50000) },
                  },
                })}
            /></label
          >{/each}
        <label
          >Maximum luminance (nits)<input
            type="number"
            min="0"
            max="429496.7295"
            step="0.0001"
            value={mastering.maxLuminanceTenThousandths / 10000}
            oninput={(event) =>
              change({
                ...value,
                masteringDisplay: {
                  ...mastering,
                  maxLuminanceTenThousandths: number(event, 10000),
                },
              })}
          /></label
        >
        <label
          >Minimum luminance (nits)<input
            type="number"
            min="0"
            max="429496.7295"
            step="0.0001"
            value={mastering.minLuminanceTenThousandths / 10000}
            oninput={(event) =>
              change({
                ...value,
                masteringDisplay: {
                  ...mastering,
                  minLuminanceTenThousandths: number(event, 10000),
                },
              })}
          /></label
        >
      </div>
    {/if}
    <label
      ><input
        type="checkbox"
        checked={!!value?.contentLight}
        onchange={(event) =>
          event.currentTarget.checked
            ? change({ ...value, contentLight: { maxContent: 1000, maxAverage: 400 } })
            : clear('contentLight')}
      /> Override content light levels</label
    >
    {#if value?.contentLight}<div class="grid">
        <label
          >MaxCLL (nits)<input
            type="number"
            min="0"
            max="65535"
            step="1"
            value={value.contentLight.maxContent}
            oninput={(event) =>
              change({
                ...value,
                contentLight: { ...value!.contentLight!, maxContent: number(event, 1) },
              })}
          /></label
        ><label
          >MaxFALL (nits)<input
            type="number"
            min="0"
            max="65535"
            step="1"
            value={value.contentLight.maxAverage}
            oninput={(event) =>
              change({
                ...value,
                contentLight: { ...value!.contentLight!, maxAverage: number(event, 1) },
              })}
          /></label
        >
      </div>{/if}
    {#if supportsDynamicHdr(encoder) || value?.dolbyVisionRpuPath || value?.hdr10PlusJsonPath}<div
        class="dynamic"
      >
        {#if supportsDynamicHdr(encoder)}<button
            type="button"
            onclick={() => choose('dolbyVisionRpuPath')}>Choose Dolby Vision RPU</button
          >{/if}{#if value?.dolbyVisionRpuPath}<span>{value.dolbyVisionRpuPath}</span><button
            type="button"
            onclick={() => clear('dolbyVisionRpuPath')}>Remove RPU</button
          >{/if}{#if supportsDynamicHdr(encoder)}<button
            type="button"
            onclick={() => choose('hdr10PlusJsonPath')}>Choose HDR10+ JSON</button
          >{/if}{#if value?.hdr10PlusJsonPath}<span>{value.hdr10PlusJsonPath}</span><button
            type="button"
            onclick={() => clear('hdr10PlusJsonPath')}>Remove HDR10+ JSON</button
          >{/if}
      </div>{/if}
    {#if !validHdrMetadata(value)}<p role="alert">
        Enter valid HDR coordinates, luminance and light levels; maximum luminance must exceed
        minimum.
      </p>{/if}
    {#if pickerError}<p role="alert">{pickerError}</p>{/if}
  </fieldset>
</details>

<style>
  .hdr-options {
    grid-column: 1 / -1;
    border: 1px solid var(--border);
    border-radius: var(--radius);
    padding: 0.8rem;
  }
  summary {
    cursor: pointer;
    font-weight: 600;
  }
  fieldset {
    border: 0;
    padding: 0.7rem 0 0;
    display: grid;
    gap: 0.6rem;
    min-width: 0;
  }
  p {
    margin: 0;
    color: var(--muted-foreground);
    font-size: 0.8rem;
  }
  [role='alert'] {
    color: var(--destructive);
  }
  .grid {
    display: grid;
    grid-template-columns: repeat(auto-fit, minmax(min(100%, 10rem), 1fr));
    gap: 0.6rem;
  }
  .grid label {
    display: grid;
    gap: 0.3rem;
    text-transform: capitalize;
  }
  input[type='number'] {
    min-width: 0;
    width: 100%;
    padding: 0.4rem;
    color: inherit;
    background: var(--background);
    border: 1px solid var(--border);
    border-radius: var(--radius);
  }
  .dynamic {
    display: flex;
    flex-wrap: wrap;
    align-items: center;
    gap: 0.5rem;
    overflow-wrap: anywhere;
  }
  .dynamic button {
    border: 1px solid var(--border);
    border-radius: var(--radius);
    padding: 0.4rem 0.6rem;
    background: var(--background);
    color: inherit;
    cursor: pointer;
  }
</style>
