<script lang="ts">
  import { untrack } from 'svelte';
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
    backend,
    file,
    tools,
    jobs,
    connected,
    onfiles,
    onstart,
    onqueue,
  }: {
    backend: EncodeBackend;
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
  let workers = $state<number | undefined>(2);
  let filmGrain = $state<number | undefined>(0);
  let hdr10Fallback = $state(false);
  let included = $state<number[]>([]);
  let destination = $state('');
  let error = $state<string | null>(null);
  let submitting = $state(false);
  type Draft = {
    videoIndex: number | undefined;
    crf: number | undefined;
    preset: number;
    workers: number | undefined;
    filmGrain: number | undefined;
    hdr10Fallback: boolean;
    included: number[];
    destination: string;
  };
  // Each fixed workflow instance owns its source drafts. Navigation keeps both
  // instances mounted; changing a source restores that workflow's prior edits.
  const drafts = new Map<string, Draft>();
  let draftIdentity: string | null = null;
  let draftGeneration = 0;
  const desktop = isDesktop();
  const chunked = $derived(backend === 'av1an');
  const idPrefix = $derived(chunked ? 'av1an' : 'encode');
  const terminal = (state: string) =>
    ['succeeded', 'failed', 'canceled', 'interrupted'].includes(state);
  const active = $derived(jobs.find((job) => !terminal(job.state)));
  const videos = $derived(file?.streams.filter((stream) => stream.kind === 'video') ?? []);
  const selectedVideoSupported = $derived(
    videos.some((stream) => stream.index === videoIndex) &&
      (!chunked || videoIndex === videos[0]?.index),
  );
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
      selectedVideoSupported &&
      validSettings &&
      !!destination.trim(),
  );
  const canStart = $derived(canQueue && !active);

  function reset(source: MediaFile | undefined) {
    ++draftGeneration;
    videoIndex = source?.streams.find((stream) => stream.kind === 'video')?.index;
    crf = 30;
    preset = 4;
    workers = 2;
    filmGrain = 0;
    hdr10Fallback = false;
    included =
      source?.streams.filter((stream) => stream.kind !== 'video').map((stream) => stream.index) ??
      [];
    destination =
      source && !source.id.startsWith('jesses-synthetic')
        ? source.path.replace(/\.[^./\\]+$/, '') + (chunked ? '_av1an.mkv' : '_av1.mkv')
        : '';
    error = null;
  }
  $effect(() => {
    const source = file;
    const identity = source ? JSON.stringify(source) : null;
    untrack(() => {
      ++draftGeneration;
      if (draftIdentity !== null) {
        drafts.set(draftIdentity, {
          videoIndex,
          crf,
          preset,
          workers,
          filmGrain,
          hdr10Fallback,
          included: [...included],
          destination,
        });
      }
      const draft = identity === null ? undefined : drafts.get(identity);
      if (draft) {
        ({ videoIndex, crf, preset, workers, filmGrain, hdr10Fallback, destination } = draft);
        included = [...draft.included];
        error = null;
      } else {
        reset(source);
      }
      draftIdentity = identity;
    });
  });
  function toggle(index: number) {
    included = included.includes(index)
      ? included.filter((value) => value !== index)
      : [...included, index];
  }
  function isCurrentDraft(generation: number, identity: string | null): boolean {
    return (
      generation === draftGeneration &&
      identity === draftIdentity &&
      identity === (file ? JSON.stringify(file) : null)
    );
  }
  async function chooseOutput() {
    if (disabled) return;
    const generation = draftGeneration;
    const identity = draftIdentity;
    try {
      const selected = await chooseEncodeDestination(destination);
      if (selected && isCurrentDraft(generation, identity)) destination = selected;
    } catch (cause) {
      if (isCurrentDraft(generation, identity)) error = errorMessage(cause);
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
    const generation = draftGeneration;
    const identity = draftIdentity;
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
      if (isCurrentDraft(generation, identity)) error = errorMessage(cause);
    } finally {
      // Keep the in-flight request locked across draft switches until its
      // authoritative reply settles, even when its draft is no longer visible.
      submitting = false;
    }
  }
</script>

<section
  class="convert-workspace"
  aria-label={chunked ? 'av1an workspace' : 'Quick Convert workspace'}
>
  <div class="view-intro">
    <div>
      <span class="eyebrow"
        >{chunked ? 'Scene detection & parallel chunks' : 'Standalone encoders'}</span
      >
      <h1>{chunked ? 'av1an' : 'Quick Convert'}</h1>
      <p>
        {chunked
          ? 'Detect scenes and encode AV1 chunks in parallel with av1an and SVT-AV1.'
          : 'Encode with standalone encoder executables. SVT-AV1 is currently available.'}
      </p>
    </div>
    <span class="status-label">{chunked ? 'av1an / SVT-AV1' : 'Standalone SVT-AV1'} · 10-bit</span>
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
            <label for={`${idPrefix}-video-stream`}>Video stream</label>
            <select
              id={`${idPrefix}-video-stream`}
              bind:value={videoIndex}
              disabled={disabled || !videos.length}
            >
              {#each videos as stream (stream.index)}<option
                  value={stream.index}
                  disabled={chunked && stream.index !== videos[0]?.index}
                  >#{stream.index} · {stream.codec ?? 'Unknown codec'}{stream.title
                    ? ` · ${stream.title}`
                    : ''}{chunked && stream.index !== videos[0]?.index
                    ? ' · Not supported by av1an'
                    : ''}</option
                >{/each}
              {#if !videos.length}<option value={undefined}>No video stream available</option>{/if}
            </select>
            <p>
              {chunked
                ? 'av1an encodes the first video track only.'
                : 'Standalone SVT-AV1 executable · 10-bit AV1 · Source dimensions and frame rate'}
            </p>
          </div>
          <div class="field">
            <label for={`${idPrefix}-quality`}>Quality</label>
            <div class="input-unit">
              <input
                id={`${idPrefix}-quality`}
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
            <label for={`${idPrefix}-preset`}>Encoder preset</label>
            <select id={`${idPrefix}-preset`} bind:value={preset} {disabled}
              >{#each Array.from({ length: 14 }, (_, index) => index) as value}<option {value}
                  >{value}</option
                >{/each}</select
            >
            <p>0–13 · Higher values encode faster</p>
          </div>
          <EncodeOptions
            {idPrefix}
            {disabled}
            {backend}
            allowBackendSelection={false}
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
          <label for={`${idPrefix}-destination`}>Encode destination</label><input
            id={`${idPrefix}-destination`}
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
        {:else if !selectedVideoSupported}<p class="disabled-reason">
            av1an requires the first video track. Use Quick Convert for another video track.
          </p>
        {:else if !validSettings}<p class="disabled-reason">
            Use whole numbers: CRF 1–63, preset 0–13, and grain 0–50{chunked
              ? '; parallel chunks 1–32'
              : ''}.
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
