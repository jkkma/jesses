<script lang="ts">
  import {
    ArrowRight,
    AudioLines,
    Clapperboard,
    FolderOutput,
    Info,
    Play,
    RotateCcw,
  } from '@lucide/svelte';
  import { Button } from '$lib/components/ui/button';
  import { chooseEncodeDestination, isDesktop } from '$lib/ipc/client';
  import { errorMessage } from '$lib/components/shared/format';
  import EncodeOptions from '$lib/components/shared/EncodeOptions.svelte';
  import type {
    EncodeBackend,
    EncodeRequest,
    JobSnapshot,
    MediaFile,
    ToolInfo,
  } from '$lib/ipc/generated';

  let {
    file,
    tools,
    jobs,
    connected,
    onfiles,
    onstart,
    onqueue,
  }: {
    file: MediaFile | undefined;
    tools: ToolInfo[];
    jobs: JobSnapshot[];
    connected: boolean;
    onfiles: () => void;
    onstart: (request: EncodeRequest) => Promise<void>;
    onqueue: (request: EncodeRequest) => Promise<void>;
  } = $props();
  let videoIndex = $state<number | undefined>();
  let crf = $state<number | undefined>(30);
  let preset = $state(4);
  let backend = $state<EncodeBackend>('svtAv1');
  let workers = $state<number | undefined>(2);
  let filmGrain = $state<number | undefined>(0);
  let hdr10Fallback = $state(false);
  let included = $state<number[]>([]);
  let destination = $state('');
  let error = $state<string | null>(null);
  let submitting = $state(false);
  const desktop = isDesktop();
  const terminal = (state: string) =>
    ['succeeded', 'failed', 'canceled', 'interrupted'].includes(state);
  const active = $derived(jobs.find((job) => !terminal(job.state)));
  const videos = $derived(file?.streams.filter((stream) => stream.kind === 'video') ?? []);
  const copiedStreams = $derived(file?.streams.filter((stream) => stream.kind !== 'video') ?? []);
  const usableSource = $derived(!!file && !file.id.startsWith('jesses-synthetic'));
  const toolsReady = $derived(
    ['ffmpeg', 'ffprobe', 'svt-av1', ...(backend === 'av1an' ? ['av1an'] : [])].every((id) =>
      tools.some((tool) => tool.id === id && tool.available),
    ),
  );
  const disabled = $derived(!desktop || !usableSource || submitting);
  const validSettings = $derived(
    typeof crf === 'number' &&
      Number.isInteger(crf) &&
      crf >= 1 &&
      crf <= 63 &&
      Number.isInteger(preset) &&
      preset >= 0 &&
      preset <= 13 &&
      typeof filmGrain === 'number' &&
      Number.isInteger(filmGrain) &&
      filmGrain >= 0 &&
      filmGrain <= 50 &&
      (backend !== 'av1an' ||
        (typeof workers === 'number' &&
          Number.isInteger(workers) &&
          workers >= 1 &&
          workers <= 32)),
  );
  const canQueue = $derived(
    !disabled &&
      connected &&
      toolsReady &&
      videos.some((stream) => stream.index === videoIndex) &&
      validSettings &&
      !!destination.trim(),
  );
  const canStart = $derived(canQueue && !active);

  function reset(source: MediaFile | undefined) {
    videoIndex = source?.streams.find((stream) => stream.kind === 'video')?.index;
    crf = 30;
    preset = 4;
    backend = 'svtAv1';
    workers = 2;
    filmGrain = 0;
    hdr10Fallback = false;
    included =
      source?.streams.filter((stream) => stream.kind !== 'video').map((stream) => stream.index) ??
      [];
    destination =
      source && !source.id.startsWith('jesses-synthetic')
        ? source.path.replace(/\.[^./\\]+$/, '') + '_av1.mkv'
        : '';
    error = null;
  }
  $effect(() => {
    reset(file);
  });
  function toggle(index: number) {
    included = included.includes(index)
      ? included.filter((value) => value !== index)
      : [...included, index];
  }
  async function chooseOutput() {
    try {
      const selected = await chooseEncodeDestination(destination);
      if (selected) destination = selected;
    } catch (cause) {
      error = errorMessage(cause);
    }
  }
  async function start(queue = false) {
    if (
      !(queue ? canQueue : canStart) ||
      !file ||
      videoIndex === undefined ||
      crf === undefined ||
      filmGrain === undefined
    )
      return;
    submitting = true;
    error = null;
    try {
      const copies = copiedStreams
        .filter((stream) => included.includes(stream.index))
        .sort((a, b) => Number(a.kind === 'attachment') - Number(b.kind === 'attachment'));
      await (queue ? onqueue : onstart)({
        source: {
          inputPath: file.path,
          outputPath: destination.trim(),
          streamIndices: [videoIndex, ...copies.map((stream) => stream.index)],
        },
        settings: {
          videoStreamIndex: videoIndex,
          crf,
          preset,
          backend,
          workers: backend === 'av1an' ? workers! : 2,
          filmGrain,
          hdr10Fallback,
        },
      });
    } catch (cause) {
      error = errorMessage(cause);
    } finally {
      submitting = false;
    }
  }
</script>

<section class="convert-workspace" aria-label="Quick Convert workspace">
  <div class="view-intro">
    <div>
      <span class="eyebrow">Single-file workflow</span>
      <h1>Quick Convert</h1>
      <p>Encode one video stream to AV1 and copy the other tracks you choose.</p>
    </div>
    <span class="status-label">SVT-AV1 · 10-bit</span>
  </div>
  <div class="notice convert-notice">
    <Info size={16} aria-hidden="true" />
    <p>
      Supports progressive SDR and compatible HDR10 video with a constant frame rate, square pixels,
      and 4:2:0 color. HDR10 preserves static HDR metadata. Rotation, interlacing, resizing, and
      audio encoding are not supported yet. Source compatibility is checked before encoding.
    </p>
  </div>
  {#if error}<div class="notice error-notice" role="alert">
      <Info size={16} aria-hidden="true" />
      <p>{error}</p>
    </div>{/if}
  <div class="convert-grid">
    <div class="convert-settings">
      <section class="panel settings-panel">
        <div class="section-heading">
          <span class="heading-with-icon"
            ><Clapperboard size={16} aria-hidden="true" /><span class="eyebrow">Video</span></span
          >
          <button type="button" class="text-button" {disabled} onclick={() => reset(file)}
            ><RotateCcw size={13} aria-hidden="true" />Reset settings</button
          >
        </div>
        <div class="setting-fields">
          <div class="field full-width">
            <label for="encode-video-stream">Video stream</label>
            <select
              id="encode-video-stream"
              bind:value={videoIndex}
              disabled={disabled || !videos.length}
            >
              {#each videos as stream (stream.index)}<option value={stream.index}
                  >#{stream.index} · {stream.codec ?? 'Unknown codec'}{stream.title
                    ? ` · ${stream.title}`
                    : ''}</option
                >{/each}
              {#if !videos.length}<option value={undefined}>No video stream available</option>{/if}
            </select>
            <p>SVT-AV1 · 10-bit AV1 · Source dimensions and frame rate</p>
          </div>
          <div class="field">
            <label for="encode-quality">Quality</label>
            <div class="input-unit">
              <input
                id="encode-quality"
                type="number"
                min="1"
                max="63"
                step="1"
                bind:value={crf}
                {disabled}
              /><span>CRF</span>
            </div>
            <p>1–63 · Lower values retain more detail</p>
          </div>
          <div class="field">
            <label for="encode-preset">Encoder preset</label>
            <select id="encode-preset" bind:value={preset} {disabled}
              >{#each Array.from({ length: 14 }, (_, index) => index) as value}<option {value}
                  >{value}</option
                >{/each}</select
            >
            <p>0–13 · Higher values encode faster</p>
          </div>
          <EncodeOptions
            idPrefix="encode"
            {disabled}
            bind:backend
            bind:workers
            bind:filmGrain
            bind:hdr10Fallback
          />
        </div>
      </section>
      <section class="panel settings-panel">
        <div class="section-heading">
          <span class="heading-with-icon"
            ><AudioLines size={16} aria-hidden="true" /><span class="eyebrow"
              >Copy source tracks</span
            ></span
          >
        </div>
        <p class="copy-note">
          Selected audio, subtitles, and attachments are copied without encoding. Audio keeps its
          source codec and channels.
        </p>
        <div class="copy-streams">
          {#each copiedStreams as stream (stream.index)}
            <label class="copy-stream"
              ><input
                type="checkbox"
                aria-label={`Copy stream #${stream.index}`}
                checked={included.includes(stream.index)}
                {disabled}
                onchange={() => toggle(stream.index)}
              />
              <span
                ><strong>#{stream.index} · {stream.kind} · {stream.codec ?? 'Unknown codec'}</strong
                ><small
                  >{[stream.title, stream.language].filter(Boolean).join(' · ') ||
                    'No track title'}</small
                ></span
              >
            </label>
          {:else}<p class="small-muted">No additional tracks to copy.</p>{/each}
        </div>
      </section>
    </div>
    <aside class="panel output-panel">
      <div class="section-heading">
        <span class="heading-with-icon"
          ><FolderOutput size={16} aria-hidden="true" /><span class="eyebrow">Output</span></span
        >
      </div>
      <div class="output-content">
        <div class="output-source">
          <span class="eyebrow">Source</span><strong>{file?.name ?? 'No source selected'}</strong>
          <button type="button" class="text-button" onclick={onfiles}
            >{file ? 'Change source' : 'Choose a source'}<ArrowRight
              size={13}
              aria-hidden="true"
            /></button
          >
        </div>
        <div class="field">
          <label for="encode-destination">Encode destination</label><input
            id="encode-destination"
            bind:value={destination}
            {disabled}
            placeholder="Choose a new .mkv file"
          />
        </div>
        <Button variant="outline" onclick={chooseOutput} {disabled}
          >Choose encode destination</Button
        >
        <p class="small-muted">Matroska (.mkv). Existing files are never replaced.</p>
        <div class="output-summary">
          <Clapperboard size={15} aria-hidden="true" />
          <p>
            <strong>10-bit AV1 + copied tracks</strong><span
              >{backend === 'av1an' ? `av1an · ${workers ?? '—'} workers` : 'Standalone SVT-AV1'} · CRF
              {crf ?? '—'} · Preset {preset} · Grain {filmGrain ?? '—'} · MKV</span
            >
          </p>
        </div>
        <Button class="start-encode" onclick={() => start()} disabled={!canStart}
          ><Play size={14} aria-hidden="true" />{submitting ? 'Starting…' : 'Start encode'}</Button
        >
        <Button variant="outline" onclick={() => start(true)} disabled={!canQueue}
          >Add to queue</Button
        >
        <p class="small-muted">
          Queue encodes with different sources or destinations. Jobs run one at a time.
        </p>
        {#if !desktop}<p class="disabled-reason">Encoding requires the desktop app.</p>
        {:else if !connected}<p class="disabled-reason">Connecting to the job runtime…</p>
        {:else if !usableSource || !videos.length}<p class="disabled-reason">
            Choose a local source with a video stream.
          </p>
        {:else if !toolsReady}<p class="disabled-reason">
            Install FFmpeg, FFprobe, standalone SVT-AV1{backend === 'av1an' ? ', and av1an' : ''},
            then refresh Tools & settings.
          </p>
        {:else if !validSettings}<p class="disabled-reason">
            Use whole numbers: CRF 1–63, preset 0–13, grain 0–50, and parallel chunks 1–32.
          </p>
        {:else if active}<p class="disabled-reason">
            A job is active. Add this encode to the queue to run it next.
          </p>{/if}
      </div>
    </aside>
  </div>
</section>

<style>
  .copy-note {
    padding: 16px 20px 0;
    font-size: 12px;
  }
  .copy-streams {
    padding: 12px 20px 20px;
    max-height: 360px;
    overflow-y: auto;
  }
  .copy-stream {
    display: flex;
    align-items: center;
    gap: 12px;
    padding: 12px 0;
    border-bottom: 1px solid var(--border);
  }
  .copy-stream input {
    width: 16px;
    height: 16px;
    accent-color: #ad5326;
    flex: 0 0 auto;
  }
  .copy-stream span {
    min-width: 0;
    overflow-wrap: anywhere;
  }
  .copy-stream strong,
  .copy-stream small {
    display: block;
  }
  .copy-stream strong {
    font-size: 13px;
  }
  .copy-stream small {
    font-size: 11px;
    margin-top: 4px;
  }
  .output-source strong {
    overflow-wrap: anywhere;
  }
  .text-button:disabled {
    opacity: 0.5;
    cursor: default;
  }
</style>
