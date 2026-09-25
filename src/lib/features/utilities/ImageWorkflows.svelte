<script lang="ts">
  import { onDestroy } from 'svelte';
  import {
    chooseImages,
    chooseUtilityDestination,
    chooseOutputFolder,
    runImageJob,
    isDesktop,
  } from '$lib/ipc/client';
  import type { MediaFile, ImageOutput, ImageResult } from '$lib/ipc/generated';
  import { errorMessage, fileName } from '$lib/components/shared/format';
  let { files, onimport }: { files: MediaFile[]; onimport: (paths: string[]) => void } = $props();
  let mode = $state<'import' | 'export'>('import');
  let paths = $state<string[]>([]);
  let rate = $state(24);
  let denominator = $state(1);
  let input = $state('');
  let stream = $state(0);
  let start = $state(0);
  let count = $state(1);
  let format = $state<ImageOutput>('png');
  let resize = $state(false);
  let width = $state(1280);
  let directoryName = $state('frames');
  let pending = $state(false);
  let result = $state<ImageResult | null>(null);
  let error = $state<string | null>(null);
  let controller: AbortController | undefined;
  const selected = $derived(files.find((f) => f.path === input));
  $effect(() => {
    stream = selected?.streams.find((s) => s.kind === 'video')?.index ?? 0;
  });
  $effect(() => {
    if (!input && files.length) input = files[0].path;
  });
  onDestroy(() => controller?.abort());
  function move(i: number, delta: number) {
    const j = i + delta;
    if (j < 0 || j >= paths.length) return;
    const list = [...paths];
    [list[i], list[j]] = [list[j], list[i]];
    paths = list;
  }
  async function choose() {
    error = null;
    try {
      paths = await chooseImages();
    } catch (e) {
      error = errorMessage(e);
    }
  }
  async function run() {
    pending = true;
    error = null;
    result = null;
    const active = new AbortController();
    controller = active;
    try {
      if (mode === 'import') {
        if (!paths.length) throw new Error('Choose images first.');
        const output = await chooseUtilityDestination('image-sequence.mkv', ['mkv']);
        if (!output) return;
        result = await runImageJob(
          {
            operation: 'importSequence',
            paths: [...paths],
            frameRate: { numerator: rate, denominator },
            outputPath: output,
          },
          active.signal,
        );
        onimport([result.outputPath]);
      } else {
        if (!selected) throw new Error('Choose an imported video first.');
        const sequence = format.endsWith('Sequence');
        let output: string | null;
        if (sequence) {
          if (
            !/^[\p{L}\p{N}_ -]{1,80}$/u.test(directoryName) ||
            ['.', '..'].includes(directoryName)
          )
            throw new Error(
              'Use a simple new folder name with letters, numbers, spaces, underscores or hyphens.',
            );
          const parent = await chooseOutputFolder();
          output = parent ? `${parent.replace(/[\\/]$/, '')}/${directoryName}` : null;
        } else {
          const ext = format === 'jpeg' ? 'jpg' : format;
          output = await chooseUtilityDestination(`frame.${ext}`, [ext]);
        }
        if (!output) return;
        result = await runImageJob(
          {
            operation: 'export',
            inputPath: selected.path,
            streamIndex: stream,
            startFrame: start,
            frameCount: format === 'png' || format === 'jpeg' ? 1 : count,
            format,
            outputPath: output,
            width: resize ? width : null,
          },
          active.signal,
        );
      }
    } catch (e) {
      error = active.signal.aborted ? 'Image processing canceled.' : errorMessage(e);
    } finally {
      pending = false;
      controller = undefined;
    }
  }
</script>

<details class="image-workflows">
  <summary>
    <span>Images and sequences</span>
    <small>Import a sequence or export frames and GIFs</small>
  </summary>
  <section class="image-body" aria-label="Images and sequences">
    <div class="image-fields operation-fields">
      <label
        >Operation<select bind:value={mode} disabled={pending}
          ><option value="import">Import an image sequence</option><option value="export"
            >Export images or GIF</option
          ></select
        ></label
      >
    </div>
    {#if mode === 'import'}
      <p>
        Choose same-sized images of one format. Review their presentation order, then save a
        lossless video for Quick Convert or Batch.
      </p>
      <div class="image-actions">
        <button type="button" onclick={choose} disabled={pending || !isDesktop()}
          >Choose images</button
        >
        <div class="image-fields rate-fields">
          <label class="compact-field"
            >Frame-rate numerator<input
              type="number"
              min="1"
              max="120000"
              bind:value={rate}
              disabled={pending}
            /></label
          ><label class="compact-field"
            >Frame-rate denominator<input
              type="number"
              min="1"
              max="100000"
              bind:value={denominator}
              disabled={pending}
            /></label
          >
        </div>
      </div>
      <ol class="sequence-list">
        {#each paths as path, i}<li>
            <span>{fileName(path)}</span><button
              type="button"
              aria-label={`Move image ${i + 1} up`}
              disabled={pending || i === 0}
              onclick={() => move(i, -1)}>↑</button
            ><button
              type="button"
              aria-label={`Move image ${i + 1} down`}
              disabled={pending || i === paths.length - 1}
              onclick={() => move(i, 1)}>↓</button
            ><button
              type="button"
              aria-label={`Remove image ${i + 1}`}
              disabled={pending}
              onclick={() => (paths = paths.filter((_, index) => index !== i))}>Remove</button
            >
          </li>{/each}
      </ol>
    {:else}
      <div class="image-fields export-fields">
        <label class="source-field"
          >Source<select bind:value={input} disabled={pending}
            ><option value="">Choose a source</option>{#each files as file}<option value={file.path}
                >{file.name}</option
              >{/each}</select
          ></label
        >
        <label
          >Video stream<select bind:value={stream} disabled={pending}
            >{#each selected?.streams.filter((s) => s.kind === 'video') ?? [] as s}<option
                value={s.index}>Stream {s.index} · {s.width} × {s.height}</option
              >{/each}</select
          ></label
        >
        <label
          >Output<select bind:value={format} disabled={pending}
            ><option value="png">PNG image</option><option value="jpeg">JPEG image</option><option
              value="pngSequence">PNG sequence</option
            ><option value="jpegSequence">JPEG sequence</option><option value="gif"
              >Animated GIF</option
            ></select
          ></label
        >
        <label class="compact-field"
          >First frame (from zero)<input
            type="number"
            min="0"
            step="1"
            bind:value={start}
            disabled={pending}
          /></label
        >
        {#if !['png', 'jpeg'].includes(format)}<label class="compact-field"
            >Frame count<input
              type="number"
              min="1"
              max="10000"
              step="1"
              bind:value={count}
              disabled={pending}
            /></label
          >{/if}
        <label class="checkbox-field"
          ><input type="checkbox" bind:checked={resize} disabled={pending} />Resize width</label
        >{#if resize}<label class="compact-field"
            >Width<input
              type="number"
              min="1"
              max="8192"
              step="1"
              bind:value={width}
              disabled={pending}
            /></label
          >{/if}
        {#if format.endsWith('Sequence')}<label class="source-field"
            >New sequence folder<input bind:value={directoryName} disabled={pending} /></label
          >{/if}
      </div>
      <p class="small-muted">
        Exports use a new destination. Existing files or folders are never replaced. Convert HDR to
        SDR before image export; GIF supports a 256-color palette and centisecond timing.
      </p>
    {/if}
    <div class="actions">
      <button
        class="primary-action"
        type="button"
        onclick={run}
        disabled={pending || !isDesktop() || (mode === 'import' ? paths.length === 0 : !selected)}
        >{pending
          ? 'Processing…'
          : mode === 'import'
            ? 'Save and import sequence'
            : 'Export'}</button
      >
      {#if pending}<button type="button" onclick={() => controller?.abort()}
          >Cancel image job</button
        >{/if}
    </div>
    {#if error}<p role="alert">{error}</p>{/if}
    {#if result}<div role="status">
        <p>
          Saved {result.frameCount} frame{result.frameCount === 1 ? '' : 's'} at {result.width} × {result.height}.
        </p>
        <p class="path">{result.outputPath}</p>
        {#each result.notes as note}<p>{note}</p>{/each}
      </div>{/if}
  </section>
</details>

<style>
  .image-workflows {
    border: 1px solid #c0b7aa;
    background: var(--card);
    margin: 12px 0;
  }
  summary {
    display: flex;
    align-items: baseline;
    gap: 10px;
    padding: 12px 14px;
    cursor: pointer;
    font-weight: 600;
  }
  summary small {
    color: var(--muted-foreground);
    font-size: 0.7rem;
    font-weight: 400;
  }
  summary::after {
    content: '›';
    margin-left: auto;
    transition: transform 100ms ease;
  }
  .image-workflows[open] > summary::after {
    transform: rotate(90deg);
  }
  .image-body {
    padding: 14px;
    border-top: 1px solid #c0b7aa;
  }
  .image-fields {
    display: flex;
    flex-wrap: wrap;
    align-items: end;
    gap: 12px;
    margin: 12px 0;
  }
  .operation-fields {
    margin-top: 0;
  }
  .operation-fields label {
    flex: 0 1 20rem;
  }
  .export-fields > label {
    flex: 1 1 13rem;
  }
  .export-fields > .source-field {
    flex-basis: 20rem;
  }
  .export-fields > .compact-field {
    flex: 0 1 9rem;
  }
  .export-fields > .checkbox-field {
    flex: 0 1 auto;
  }
  .rate-fields {
    margin: 0;
  }
  .compact-field {
    flex: 0 1 9rem;
  }
  .checkbox-field {
    flex: 0 1 auto;
    flex-direction: row;
    align-items: center;
    padding-block: 0.55rem;
  }
  .image-actions,
  .actions {
    display: flex;
    align-items: end;
    flex-wrap: wrap;
    gap: 8px 12px;
  }
  label {
    display: flex;
    flex-direction: column;
    gap: 0.35rem;
    min-width: 0;
  }
  input,
  select,
  button {
    font: inherit;
    padding: 0.5rem;
    border: 1px solid #9b8c7a;
    background: var(--background);
    max-width: 100%;
  }
  input[type='number'],
  .compact-field input {
    width: 9rem;
  }
  .primary-action {
    background: var(--primary);
    border-color: var(--primary);
    color: var(--primary-foreground);
  }
  button:disabled {
    cursor: not-allowed;
    opacity: 0.55;
  }
  button {
    cursor: pointer;
  }
  input[type='checkbox'] {
    width: 1rem;
    height: 1rem;
  }
  .sequence-list {
    max-height: 18rem;
    overflow: auto;
    padding-left: 2rem;
  }
  .sequence-list li {
    margin: 0.4rem 0;
    display: flex;
    align-items: center;
    gap: 0.5rem;
  }
  .sequence-list span {
    flex: 1;
    min-width: 0;
    overflow-wrap: anywhere;
  }
  .path {
    overflow-wrap: anywhere;
  }
  p {
    max-width: 90ch;
  }
  @media (max-width: 800px) {
    summary {
      align-items: flex-start;
      flex-direction: column;
      gap: 3px;
    }
    .export-fields > label {
      flex-basis: 12rem;
    }
    .export-fields > .compact-field {
      flex-basis: 9rem;
    }
    .export-fields > .checkbox-field {
      flex-basis: auto;
    }
    .export-fields > .source-field {
      flex-basis: 100%;
    }
  }
</style>
