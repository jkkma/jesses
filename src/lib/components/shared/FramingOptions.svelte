<script lang="ts">
  import type { MediaStream } from '$lib/ipc/generated';
  import { cropEdges, framingDimensions, type FramingDraft } from './framing-options';

  let {
    idPrefix,
    draft,
    stream,
    disabled = false,
    onchange,
  }: {
    idPrefix: string;
    draft: FramingDraft;
    stream: MediaStream | undefined;
    disabled?: boolean;
    onchange: (draft: FramingDraft) => void;
  } = $props();
  const dimensions = $derived(framingDimensions(draft, stream));
  const readNumber = (input: HTMLInputElement) =>
    input.value === '' ? undefined : input.valueAsNumber;
</script>

<fieldset class="framing-options" {disabled} aria-describedby={`${idPrefix}-framing-help`}>
  <legend>Crop & resize</legend>
  <p id={`${idPrefix}-framing-help`} class="small-muted">
    Crop each edge in even pixels, then optionally resize. The cropped picture keeps its aspect
    ratio; automatic height rounds to the nearest even pixel, with ties rounded up.
  </p>
  <div class="crop-fields">
    {#each cropEdges as edge}
      <div class="field">
        <label for={`${idPrefix}-crop-${edge}`}>Crop {edge} (pixels)</label>
        <input
          id={`${idPrefix}-crop-${edge}`}
          type="number"
          min="0"
          max="8192"
          step="2"
          required
          value={draft.crop[edge] ?? ''}
          oninput={(event) =>
            onchange({
              ...draft,
              crop: { ...draft.crop, [edge]: readNumber(event.currentTarget) },
            })}
        />
      </div>
    {/each}
  </div>
  <label class="resize-choice" for={`${idPrefix}-resize`}>
    <input
      id={`${idPrefix}-resize`}
      type="checkbox"
      checked={draft.resizeEnabled}
      onchange={(event) =>
        onchange({
          ...draft,
          resizeEnabled: event.currentTarget.checked,
          resizeWidth: draft.resizeWidth ?? dimensions.croppedWidth ?? stream?.width ?? undefined,
        })}
    />Resize video
  </label>
  {#if draft.resizeEnabled}
    <div class="field resize-width">
      <label for={`${idPrefix}-resize-width`}>Output width (pixels)</label>
      <input
        id={`${idPrefix}-resize-width`}
        type="number"
        min="64"
        max="8192"
        step="2"
        required
        value={draft.resizeWidth ?? ''}
        oninput={(event) => onchange({ ...draft, resizeWidth: readNumber(event.currentTarget) })}
      />
      <p>Even width, 64–8192 pixels. Height is automatic.</p>
    </div>
  {/if}
  <p class="dimensions small-muted" aria-live="polite" aria-label="Video dimensions">
    {#if dimensions.error}
      <span class="disabled-reason">{dimensions.error}</span>
    {:else}
      Source {stream?.width} × {stream?.height} → Cropped {dimensions.croppedWidth} × {dimensions.croppedHeight}
      → Output {dimensions.width} × {dimensions.height}
    {/if}
  </p>
</fieldset>

<style>
  .framing-options {
    min-width: 0;
    margin: 0;
    border: 0;
    padding: 0;
  }
  legend {
    margin-bottom: 8px;
    padding: 0;
    font-size: 12px;
    font-weight: 600;
  }
  .crop-fields {
    display: grid;
    grid-template-columns: repeat(2, minmax(0, 1fr));
    gap: 12px;
    margin-top: 14px;
  }
  .crop-fields .field,
  .resize-width {
    min-width: 0;
  }
  .crop-fields input,
  .resize-width input {
    width: 100%;
    min-width: 0;
  }
  .resize-choice {
    display: flex;
    align-items: center;
    gap: 8px;
    margin: 14px 0;
    font-size: 12px;
  }
  .resize-choice input {
    width: 16px;
    height: 16px;
    accent-color: #ad5326;
  }
  .dimensions {
    margin-top: 12px;
    overflow-wrap: anywhere;
  }
  @media (max-width: 420px) {
    .crop-fields {
      grid-template-columns: 1fr;
    }
  }
</style>
