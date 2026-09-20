<script lang="ts">
  import type { MediaStream } from '$lib/ipc/generated';
  import {
    cropEdges,
    framingDimensions,
    framingSummary,
    selectedFraming,
    type FramingDraft,
  } from './framing-options';

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
  const configured = $derived(
    cropEdges.some((edge) => draft.crop[edge] !== 0) || draft.resizeEnabled || draft.bordersEnabled,
  );
  const summary = $derived(
    configured && dimensions.error
      ? `Needs attention · ${dimensions.error}`
      : framingSummary(selectedFraming(draft)),
  );
  const readNumber = (input: HTMLInputElement) =>
    input.value === '' ? undefined : input.valueAsNumber;
</script>

<details class="framing-options">
  <summary>
    <span class="summary-content">
      <span>Crop, resize & borders</span>
      <small class:summary-error={configured && !!dimensions.error}>{summary}</small>
    </span>
  </summary>
  <fieldset {disabled} aria-describedby={`${idPrefix}-framing-help`}>
    <p id={`${idPrefix}-framing-help`} class="small-muted">
      Crop in even pixels, resize while keeping the picture's aspect ratio, then add black borders.
      Automatic picture height rounds to the nearest even pixel, with ties rounded up.
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
        <label for={`${idPrefix}-resize-width`}>Picture width (pixels)</label>
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
        <p>Width before borders, 64–8192 even pixels. Height is automatic.</p>
      </div>
    {/if}
    <label class="resize-choice" for={`${idPrefix}-borders`}>
      <input
        id={`${idPrefix}-borders`}
        type="checkbox"
        checked={draft.bordersEnabled}
        onchange={(event) => onchange({ ...draft, bordersEnabled: event.currentTarget.checked })}
      />Add black borders
    </label>
    {#if draft.bordersEnabled}
      <div class="crop-fields">
        {#each cropEdges as edge}
          <div class="field">
            <label for={`${idPrefix}-border-${edge}`}>Border {edge} (pixels)</label>
            <input
              id={`${idPrefix}-border-${edge}`}
              type="number"
              min="0"
              max="8192"
              step="2"
              required
              value={draft.borders[edge] ?? ''}
              oninput={(event) =>
                onchange({
                  ...draft,
                  borders: { ...draft.borders, [edge]: readNumber(event.currentTarget) },
                })}
            />
          </div>
        {/each}
      </div>
      <p class="border-help small-muted">
        Even pixels per edge. Final dimensions must stay within 8192 × 8192.
      </p>
    {/if}
    <p class="dimensions small-muted" aria-live="polite" aria-label="Video dimensions">
      {#if dimensions.error}
        <span class="disabled-reason">{dimensions.error}</span>
      {:else}
        Source {stream?.width} × {stream?.height} → Cropped {dimensions.croppedWidth} × {dimensions.croppedHeight}
        {#if draft.bordersEnabled}
          → Picture {dimensions.pictureWidth} × {dimensions.pictureHeight}
        {/if}
        → Output {dimensions.width} × {dimensions.height}
      {/if}
    </p>
  </fieldset>
</details>

<style>
  .framing-options {
    min-width: 0;
    margin: 10px 0 0;
    border: 1px solid var(--border);
    border-radius: 8px;
  }
  summary {
    padding: 11px 12px;
    cursor: pointer;
    font-size: 13px;
    font-weight: 600;
  }
  .summary-content {
    display: inline-flex;
    justify-content: space-between;
    gap: 12px;
    width: calc(100% - 1.4rem);
    vertical-align: middle;
  }
  .summary-content small {
    color: var(--muted-foreground);
    font-size: 11px;
    font-weight: 400;
    text-align: right;
  }
  .summary-error {
    color: var(--destructive);
  }
  fieldset {
    min-width: 0;
    margin: 0;
    padding: 0 12px 12px;
    border: 0;
  }
  .crop-fields {
    display: grid;
    grid-template-columns: repeat(auto-fit, minmax(min(100%, 8rem), 1fr));
    gap: 10px 12px;
    margin-top: 10px;
  }
  .crop-fields .field,
  .resize-width {
    min-width: 0;
  }
  .crop-fields input,
  .resize-width input {
    width: min(100%, 9rem);
    min-width: 0;
  }
  .resize-choice {
    display: flex;
    align-items: center;
    gap: 8px;
    margin: 11px 0;
    font-size: 12px;
  }
  .resize-choice input {
    width: 16px;
    height: 16px;
    accent-color: #ad5326;
  }
  .dimensions {
    margin-top: 10px;
    overflow-wrap: anywhere;
  }
  .border-help {
    margin-top: 8px;
  }
  @media (max-width: 560px) {
    .summary-content {
      display: inline-grid;
    }
    .summary-content small {
      text-align: left;
    }
  }
</style>
