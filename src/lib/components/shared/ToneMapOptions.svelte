<script lang="ts">
  import type { EncodeBackend } from '$lib/ipc/generated';
  import type { ToneMapDraft } from './tone-map-options';
  let {
    draft,
    backend,
    disabled = false,
    error = null,
    onchange,
  }: {
    draft: ToneMapDraft;
    backend: EncodeBackend;
    disabled?: boolean;
    error?: string | null;
    onchange: (next: ToneMapDraft) => void;
  } = $props();
  const id = $props.id();
  const av1an = $derived(backend === 'av1an');
</script>

<fieldset {disabled} class="tone-map-options">
  <label class="tone-toggle"
    ><input
      type="checkbox"
      checked={draft.enabled}
      onchange={(event) => onchange({ ...draft, enabled: event.currentTarget.checked })}
    /> HDR / HLG to SDR</label
  >
  {#if draft.enabled}
    <div class="tone-grid">
      <label for={`${id}-route`}
        >Processing route<select
          id={`${id}-route`}
          aria-label="Processing route"
          value={draft.backend}
          onchange={(event) => {
            const route = event.currentTarget.value as ToneMapDraft['backend'];
            onchange({
              ...draft,
              backend: route,
              peakMode: route === 'gpu' ? 'measured' : draft.peakMode,
              algorithm:
                route === 'cpu' && draft.algorithm === 'spline' ? 'hable' : draft.algorithm,
            });
          }}
        >
          <option value="auto">Auto · use GPU when available</option>
          <option value="cpu">CPU</option>
          <option value="gpu" disabled={av1an}>GPU · Vulkan/libplacebo</option>
        </select></label
      >
      <label for={`${id}-curve`}
        >Tone mapping curve<select
          id={`${id}-curve`}
          aria-label="Tone mapping curve"
          value={draft.algorithm}
          onchange={(event) =>
            onchange({
              ...draft,
              algorithm: event.currentTarget.value as ToneMapDraft['algorithm'],
              peakMode: event.currentTarget.value === 'spline' ? 'measured' : draft.peakMode,
            })}
        >
          <option value="hable">Hable</option>
          <option value="mobius">Mobius</option>
          <option value="reinhard">Reinhard</option>
          <option value="spline" disabled={av1an || draft.backend === 'cpu'}>Spline · GPU</option>
        </select></label
      >
      <label for={`${id}-peak-mode`}
        >Signal peak mode<select
          id={`${id}-peak-mode`}
          aria-label="Signal peak mode"
          value={draft.peakMode}
          onchange={(event) =>
            onchange({ ...draft, peakMode: event.currentTarget.value as ToneMapDraft['peakMode'] })}
        >
          <option value="measured">Measure from source</option>
          <option value="manual" disabled={draft.backend === 'gpu' || draft.algorithm === 'spline'}
            >Use manual value</option
          >
        </select></label
      >
      <label for={`${id}-peak`}
        >Signal peak (nits)<input
          id={`${id}-peak`}
          aria-label="Signal peak (nits)"
          type="number"
          min="100"
          max="10000"
          step="1"
          value={draft.sourcePeakNits ?? ''}
          oninput={(event) =>
            onchange({
              ...draft,
              sourcePeakNits:
                event.currentTarget.value === '' ? undefined : Number(event.currentTarget.value),
            })}
        /></label
      >
    </div>
    <p>
      Tone mapping produces 100-nit BT.709, limited-range 10-bit SDR. Measured peak uses the source
      picture when possible; the nits value is its fallback. Auto may use the CPU if GPU processing
      is unavailable{av1an ? ' and always uses CPU in av1an' : ''}. GPU requires Vulkan and
      libplacebo with measured peak detection. Auto with manual peak uses CPU; Spline requires
      measured peak on standalone Auto/GPU.
    </p>
    <label class="tone-toggle"
      ><input
        type="checkbox"
        checked={draft.hdr10BaseLayer}
        onchange={(event) => onchange({ ...draft, hdr10BaseLayer: event.currentTarget.checked })}
      /> Use the compatible HDR10 base layer for Dolby Vision / HDR10+</label
    >
    <p>
      This option discards dynamic HDR and enhancement data from compatible Dolby Vision profile 7/8
      base layers. Profile 5 has no HDR10 base layer; standalone Auto/GPU checks for a capable
      rendering route. Tone mapping runs before subtitles and borders; HDR metadata is removed from
      SDR output.
    </p>
    {#if error}<p role="alert" class="tone-error">{error}</p>{/if}
  {/if}
</fieldset>

<style>
  .tone-map-options {
    border: 1px solid var(--border);
    border-radius: 8px;
    padding: 12px;
    display: grid;
    gap: 8px;
    margin: 10px 0;
  }
  label {
    display: grid;
    gap: 5px;
    font-size: 12px;
  }
  .tone-grid {
    display: grid;
    grid-template-columns: repeat(auto-fit, minmax(min(100%, 12rem), 1fr));
    gap: 10px;
  }
  .tone-toggle {
    display: flex;
    align-items: center;
    gap: 8px;
  }
  input[type='number'],
  select {
    max-width: 100%;
    border: 1px solid var(--border);
    background: var(--background);
    border-radius: 5px;
    padding: 7px;
  }
  p {
    margin: 0;
    color: var(--muted-foreground);
    font-size: 11px;
    line-height: 1.5;
  }
  .tone-error {
    color: var(--destructive);
  }
</style>
