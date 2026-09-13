<script lang="ts">
  import type { ToneMapDraft } from './tone-map-options';
  let {
    draft,
    disabled = false,
    error = null,
    onchange,
  }: {
    draft: ToneMapDraft;
    disabled?: boolean;
    error?: string | null;
    onchange: (next: ToneMapDraft) => void;
  } = $props();
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
    <label
      >Signal peak (nits)<input
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
    <p>
      Hable tone mapping to 100-nit BT.709, limited-range 10-bit SDR. The signal peak controls
      highlight compression; 1000 nits is a starting value. HLG uses the 1000-nit reference display
      transfer.
    </p>
    <label class="tone-toggle"
      ><input
        type="checkbox"
        checked={draft.hdr10BaseLayer}
        onchange={(event) => onchange({ ...draft, hdr10BaseLayer: event.currentTarget.checked })}
      /> Use the compatible HDR10 base layer for Dolby Vision / HDR10+</label
    >
    <p>
      This option discards dynamic HDR and enhancement data. Compatible Dolby Vision profile 7/8
      base layers are checked before encoding. Tone mapping runs before subtitles and borders; HDR
      metadata is removed from SDR output.
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
  .tone-toggle {
    display: flex;
    align-items: center;
    gap: 8px;
  }
  input[type='number'] {
    max-width: 180px;
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
