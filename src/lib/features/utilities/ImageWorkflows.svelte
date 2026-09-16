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

<section aria-label="Images and sequences">
  <h2>Images and sequences</h2>
  <div class="image-fields">
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
      Choose same-sized images of one format. Review their presentation order, then save a lossless
      video for Quick Convert or Batch.
    </p>
    <button type="button" onclick={choose} disabled={pending || !isDesktop()}>Choose images</button>
    <div class="image-fields">
      <label
        >Frame-rate numerator<input
          type="number"
          min="1"
          max="120000"
          bind:value={rate}
          disabled={pending}
        /></label
      ><label
        >Frame-rate denominator<input
          type="number"
          min="1"
          max="100000"
          bind:value={denominator}
          disabled={pending}
        /></label
      >
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
    <div class="image-fields">
      <label
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
      <label
        >First frame (from zero)<input
          type="number"
          min="0"
          step="1"
          bind:value={start}
          disabled={pending}
        /></label
      >
      {#if !['png', 'jpeg'].includes(format)}<label
          >Frame count<input
            type="number"
            min="1"
            max="10000"
            step="1"
            bind:value={count}
            disabled={pending}
          /></label
        >{/if}
      <label><input type="checkbox" bind:checked={resize} disabled={pending} />Resize width</label
      >{#if resize}<label
          >Width<input
            type="number"
            min="1"
            max="8192"
            step="1"
            bind:value={width}
            disabled={pending}
          /></label
        >{/if}
      {#if format.endsWith('Sequence')}<label
          >New sequence folder<input bind:value={directoryName} disabled={pending} /></label
        >{/if}
    </div>
    <p class="small-muted">
      Exports use a new destination. Existing files or folders are never replaced. Convert HDR to
      SDR before image export; GIF supports a 256-color palette and centisecond timing.
    </p>
  {/if}
  <button
    class="primary-action"
    type="button"
    onclick={run}
    disabled={pending || !isDesktop() || (mode === 'import' ? paths.length === 0 : !selected)}
    >{pending ? 'Processing…' : mode === 'import' ? 'Save and import sequence' : 'Export'}</button
  >
  {#if pending}<button type="button" onclick={() => controller?.abort()}>Cancel image job</button
    >{/if}
  {#if error}<p role="alert">{error}</p>{/if}
  {#if result}<div role="status">
      <p>
        Saved {result.frameCount} frame{result.frameCount === 1 ? '' : 's'} at {result.width} × {result.height}.
      </p>
      <p class="path">{result.outputPath}</p>
      {#each result.notes as note}<p>{note}</p>{/each}
    </div>{/if}
</section>

<style>
  section {
    padding: 1.25rem;
    border: 1px solid #c0b7aa;
    background: #e3dacc;
    margin: 1rem 0;
  }
  .image-fields {
    display: flex;
    flex-wrap: wrap;
    gap: 1rem;
    margin: 1rem 0;
  }
  label {
    display: flex;
    flex-direction: column;
    gap: 0.35rem;
  }
  input,
  select,
  button {
    font: inherit;
    padding: 0.5rem;
    border: 1px solid #9b8c7a;
    background: #f0eee6;
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
    margin-right: 0.4rem;
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
    overflow-wrap: anywhere;
  }
  .path {
    overflow-wrap: anywhere;
  }
  p {
    max-width: 90ch;
  }
</style>
