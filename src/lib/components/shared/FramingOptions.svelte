<script lang="ts">
  import type { MediaStream } from '$lib/ipc/generated';
  import {
    cropEdges,
    FRAME_MIN_DIMENSION,
    FRAME_MAX_DIMENSION,
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
    cropEdges.some((edge) => draft.crop[edge] !== 0) ||
      draft.resizeEnabled ||
      draft.bordersEnabled ||
      !!draft.borderRatio,
  );
  const summary = $derived(
    configured && dimensions.error
      ? `Needs attention · ${dimensions.error}`
      : framingSummary(selectedFraming(draft)),
  );
  const readNumber = (input: HTMLInputElement) =>
    input.value === '' ? undefined : input.valueAsNumber;
  const anamorphic = $derived.by(() => {
    const [numerator, denominator] = (stream?.sampleAspectRatio ?? '').split(/[:/]/).map(Number);
    return numerator > 0 && denominator > 0 && numerator !== denominator;
  });
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
      Crop in even pixels, resize, then add black borders. Fit and proportional modes preserve the
      picture's aspect ratio; exact size can pad, crop or stretch.
    </p>
    <div class="crop-fields">
      {#each cropEdges as edge}
        <div class="field">
          <label for={`${idPrefix}-crop-${edge}`}>Crop {edge} (pixels)</label>
          <input
            id={`${idPrefix}-crop-${edge}`}
            type="number"
            min="0"
            max={FRAME_MAX_DIMENSION}
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
            ...(event.currentTarget.checked && anamorphic && draft.resizeMode === 'legacyWidth'
              ? { resizeMode: 'width', resizeAllowUpscale: true }
              : {}),
          })}
      />Resize video
    </label>
    {#if draft.resizeEnabled}
      <div class="field resize-width">
        <label for={`${idPrefix}-resize-mode`}>Resize mode</label>
        <select
          id={`${idPrefix}-resize-mode`}
          value={draft.resizeMode}
          onchange={(event) =>
            onchange({
              ...draft,
              resizeMode: event.currentTarget.value as FramingDraft['resizeMode'],
              resizeWidth: draft.resizeWidth ?? 1920,
              resizeHeight: draft.resizeHeight ?? 1080,
            })}
        >
          <option value="legacyWidth">Picture width (automatic height)</option>
          <option value="fit">Fit inside a box</option>
          <option value="width">Width (with alignment)</option>
          <option value="height">Height</option>
          <option value="percent">Percentage</option>
          <option value="exact">Exact dimensions</option>
        </select>
      </div>
      {#if ['legacyWidth', 'fit', 'width', 'exact'].includes(draft.resizeMode)}<div
          class="field resize-width"
        >
          <label for={`${idPrefix}-resize-width`}>Picture width (pixels)</label>
          <input
            id={`${idPrefix}-resize-width`}
            type="number"
            min={FRAME_MIN_DIMENSION}
            max={FRAME_MAX_DIMENSION}
            step={draft.resizeMode === 'legacyWidth' ? 2 : 1}
            required
            value={draft.resizeWidth ?? ''}
            oninput={(event) =>
              onchange({ ...draft, resizeWidth: readNumber(event.currentTarget) })}
          />
          <p>
            Width before borders, {FRAME_MIN_DIMENSION}–{FRAME_MAX_DIMENSION} pixels{draft.resizeMode ===
            'legacyWidth'
              ? '. Height is automatic.'
              : '.'}
          </p>
        </div>{/if}
      {#if ['fit', 'height', 'exact'].includes(draft.resizeMode)}<div class="field resize-width">
          <label for={`${idPrefix}-resize-height`}>Picture height (pixels)</label><input
            id={`${idPrefix}-resize-height`}
            type="number"
            min={FRAME_MIN_DIMENSION}
            max={FRAME_MAX_DIMENSION}
            step="1"
            value={draft.resizeHeight ?? ''}
            oninput={(event) =>
              onchange({ ...draft, resizeHeight: readNumber(event.currentTarget) })}
          />
        </div>{/if}
      {#if draft.resizeMode === 'fit'}<div class="field resize-width">
          <label for={`${idPrefix}-resize-preset`}>Fit preset</label><select
            id={`${idPrefix}-resize-preset`}
            value=""
            onchange={(event) => {
              const [width, height] = event.currentTarget.value.split('x').map(Number);
              if (width && height)
                onchange({
                  ...draft,
                  resizeWidth: width,
                  resizeHeight: height,
                  resizeAllowUpscale: true,
                });
            }}
            ><option value="">Choose a preset</option><option value="3840x2160">2160p</option
            ><option value="2560x1440">1440p</option><option value="1920x1080">1080p</option><option
              value="1280x720">720p</option
            ><option value="1024x576">576p</option><option value="854x480">480p</option><option
              value="640x360">360p</option
            ></select
          >
        </div>{/if}
      {#if draft.resizeMode === 'percent'}<div class="field resize-width">
          <label for={`${idPrefix}-percent-preset`}>Scale preset</label><select
            id={`${idPrefix}-percent-preset`}
            value=""
            onchange={(event) => {
              const percent = Number(event.currentTarget.value);
              if (percent) onchange({ ...draft, resizePercent: percent });
            }}
            ><option value="">Choose a preset</option><option value="75">75% of source</option
            ><option value="50">50% of source</option><option value="25">25% of source</option
            ></select
          >
        </div>
        <div class="field resize-width">
          <label for={`${idPrefix}-resize-percent`}>Scale (%)</label><input
            id={`${idPrefix}-resize-percent`}
            type="number"
            min="1"
            max="800"
            step="1"
            value={draft.resizePercent ?? ''}
            oninput={(event) =>
              onchange({ ...draft, resizePercent: readNumber(event.currentTarget) })}
          />
        </div>{/if}
      {#if draft.resizeMode === 'exact'}<div class="field resize-width">
          <label for={`${idPrefix}-resize-fill`}>Exact size handling</label><select
            id={`${idPrefix}-resize-fill`}
            value={draft.resizeFill}
            onchange={(event) =>
              onchange({
                ...draft,
                resizeFill: event.currentTarget.value as FramingDraft['resizeFill'],
              })}
            ><option value="pad">Fit and pad with black</option><option value="crop"
              >Fill and crop edges</option
            ><option value="stretch">Stretch picture</option></select
          >
        </div>{/if}
      {#if draft.resizeMode !== 'legacyWidth'}
        <div class="field resize-width">
          <label for={`${idPrefix}-resize-modulus`}>Dimension alignment</label><input
            id={`${idPrefix}-resize-modulus`}
            type="number"
            min="2"
            max="256"
            step="2"
            value={draft.resizeModulus ?? ''}
            oninput={(event) =>
              onchange({ ...draft, resizeModulus: readNumber(event.currentTarget) })}
          />
        </div>
        {#if draft.resizeMode !== 'percent'}<label class="resize-choice"
            ><input
              type="checkbox"
              checked={draft.resizeAllowUpscale}
              onchange={(event) =>
                onchange({ ...draft, resizeAllowUpscale: event.currentTarget.checked })}
            />Allow upscaling</label
          >{/if}
        <label class="resize-choice"
          ><input
            type="checkbox"
            checked={draft.resizeCorrectAspect}
            onchange={(event) =>
              onchange({ ...draft, resizeCorrectAspect: event.currentTarget.checked })}
          />Correct non-square source pixels while resizing</label
        >
      {/if}
    {/if}
    <div class="field resize-width">
      <label for={`${idPrefix}-border-ratio`}>Pad to aspect ratio</label>
      <select
        id={`${idPrefix}-border-ratio`}
        value={draft.borderRatio}
        onchange={(event) => {
          const ratio = event.currentTarget.value;
          const [width, height] = ratio.split(':').map(Number);
          onchange({
            ...draft,
            borderRatio: ratio,
            borderRatioWidth: width || draft.borderRatioWidth,
            borderRatioHeight: height || draft.borderRatioHeight,
          });
        }}
      >
        <option value="">None</option><option value="16:9">16:9</option><option value="4:3"
          >4:3</option
        ><option value="1:1">1:1</option><option value="9:16">9:16</option><option value="64:27"
          >21:9 ultrawide (64:27)</option
        ><option value="custom">Custom ratio</option>
      </select>
    </div>
    {#if draft.borderRatio === 'custom'}<div class="crop-fields">
        <div class="field">
          <label for={`${idPrefix}-ratio-width`}>Ratio width</label><input
            id={`${idPrefix}-ratio-width`}
            type="number"
            min="1"
            max="10000"
            value={draft.borderRatioWidth ?? ''}
            oninput={(event) =>
              onchange({ ...draft, borderRatioWidth: readNumber(event.currentTarget) })}
          />
        </div>
        <div class="field">
          <label for={`${idPrefix}-ratio-height`}>Ratio height</label><input
            id={`${idPrefix}-ratio-height`}
            type="number"
            min="1"
            max="10000"
            value={draft.borderRatioHeight ?? ''}
            oninput={(event) =>
              onchange({ ...draft, borderRatioHeight: readNumber(event.currentTarget) })}
          />
        </div>
      </div>{/if}
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
              max={FRAME_MAX_DIMENSION}
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
        Even pixels per edge. Final dimensions must stay within {FRAME_MAX_DIMENSION} × {FRAME_MAX_DIMENSION}.
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
    {#if !dimensions.error && dimensions.upscaling}<p class="small-muted" role="status">
        This resize enlarges the source picture.
      </p>{/if}
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
  .resize-width {
    margin-top: 10px;
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
