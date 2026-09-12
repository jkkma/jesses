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
  import AudioOptions from './AudioOptions.svelte';
  import { terminalJob } from './job-state';
  import FramingOptions from './FramingOptions.svelte';
  import {
    copyFramingDraft,
    defaultFraming,
    defaultFramingDraft,
    framingDimensions,
    framingSummary,
    selectedFraming,
    validFramingDraft,
    type FramingDraft,
  } from './framing-options';
  import {
    defaultAudio,
    validAudio,
    selectedAudio,
    audioSummary,
    type AudioTrackDraft,
  } from './audio-options';
  import {
    encoderOptions,
    requiredEncoderTools,
    presetLabel,
    sourceBitDepth,
    knownHdr,
    encoderChoices,
    isSvtEncoder,
    validForkSettings,
    forkSettingsSummary,
    type HdrTune,
  } from './encoder-options';
  import type {
    EncodeBackend,
    EncodeRequest,
    JobSnapshot,
    MediaFile,
    ToolInfo,
    VideoEncoder,
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
  let selectedEncoder = $state<VideoEncoder>('svtAv1Hdr');
  let crf = $state<number | undefined>(30);
  let preset = $state(2);
  let workers = $state<number | undefined>(2);
  let filmGrain = $state<number | undefined>(0);
  let hdr10Fallback = $state(false);
  let lineartPsyBias = $state<number | undefined>(0);
  let texturePsyBias = $state<number | undefined>(0);
  let hdrTune = $state<HdrTune>('filmGrain');
  let included = $state<number[]>([]);
  let audio = $state<AudioTrackDraft[]>([]);
  let framing = $state(defaultFramingDraft());
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
    lineartPsyBias: number | undefined;
    texturePsyBias: number | undefined;
    hdrTune: HdrTune;
    included: number[];
    audio: AudioTrackDraft[];
    framing: FramingDraft;
    destination: string;
  };
  // Each fixed workflow instance owns its source drafts. Navigation keeps both
  // instances mounted; changing a source restores that workflow's prior edits.
  const drafts = new Map<string, Draft>();
  let draftIdentity: string | null = null;
  let draftGeneration = 0;
  const desktop = isDesktop();
  const chunked = $derived(backend === 'av1an');
  const encoder = $derived(selectedEncoder);
  const options = $derived(encoderOptions(encoder));
  const idPrefix = $derived(chunked ? 'av1an' : 'encode');
  const active = $derived(jobs.find((job) => !terminalJob(job.state)));
  const videos = $derived(file?.streams.filter((stream) => stream.kind === 'video') ?? []);
  const selectedVideo = $derived(videos.find((stream) => stream.index === videoIndex));
  const framingResult = $derived(framingDimensions(framing, selectedVideo));
  const framingValid = $derived(validFramingDraft(framing, selectedVideo));
  const hdrUnsupported = $derived(encoder === 'x264' && knownHdr(selectedVideo));
  const depthLabel = $derived(
    isSvtEncoder(encoder)
      ? '10-bit'
      : sourceBitDepth(selectedVideo)
        ? `${sourceBitDepth(selectedVideo)}-bit source`
        : 'Source bit depth',
  );
  const selectedVideoSupported = $derived(
    videos.some((stream) => stream.index === videoIndex) &&
      (!chunked || videoIndex === videos[0]?.index),
  );
  const copiedStreams = $derived(file?.streams.filter((stream) => stream.kind !== 'video') ?? []);
  const usableSource = $derived(!!file && !file.id.startsWith('jesses-synthetic'));
  const toolsReady = $derived(
    requiredEncoderTools(backend, encoder).every((id) =>
      tools.some((tool) => tool.id === id && tool.available),
    ),
  );
  const disabled = $derived(!desktop || !usableSource || submitting);
  const validSettings = $derived(
    (!chunked || isSvtEncoder(encoder)) &&
      validForkSettings(encoder, lineartPsyBias, texturePsyBias, hdrTune) &&
      typeof crf === 'number' &&
      Number.isInteger(crf) &&
      crf >= options.crfMin &&
      crf <= options.crfMax &&
      Number.isInteger(preset) &&
      preset >= 0 &&
      preset < options.presets.length &&
      (encoder === 'x264' ||
        (typeof filmGrain === 'number' &&
          Number.isInteger(filmGrain) &&
          filmGrain >= 0 &&
          filmGrain <= 50)) &&
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
      !hdrUnsupported &&
      validSettings &&
      (chunked || framingValid) &&
      (chunked || validAudio(audio, included, file?.streams ?? [])) &&
      !!destination.trim(),
  );
  const canStart = $derived(canQueue && !active);

  function reset(source: MediaFile | undefined) {
    ++draftGeneration;
    videoIndex = source?.streams.find((stream) => stream.kind === 'video')?.index;
    crf = options.defaultCrf;
    preset = options.defaultPreset;
    workers = 2;
    filmGrain = 0;
    hdr10Fallback = false;
    lineartPsyBias = options.defaultLineartPsyBias;
    texturePsyBias = options.defaultTexturePsyBias;
    hdrTune = options.defaultHdrTune;
    included =
      source?.streams.filter((stream) => stream.kind !== 'video').map((stream) => stream.index) ??
      [];
    audio = defaultAudio(source?.streams ?? []);
    framing = defaultFramingDraft();
    destination =
      source && !source.id.startsWith('jesses-synthetic')
        ? source.path.replace(/\.[^./\\]+$/, '') + (chunked ? options.av1anSuffix : options.suffix)
        : '';
    error = null;
  }
  $effect(() => {
    const source = file;
    const identity = source ? JSON.stringify([encoder, source]) : null;
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
          lineartPsyBias,
          texturePsyBias,
          hdrTune,
          included: [...included],
          audio: audio.map((track) => ({ ...track })),
          framing: copyFramingDraft(framing),
          destination,
        });
      }
      const draft = identity === null ? undefined : drafts.get(identity);
      if (draft) {
        ({
          videoIndex,
          crf,
          preset,
          workers,
          filmGrain,
          hdr10Fallback,
          lineartPsyBias,
          texturePsyBias,
          hdrTune,
          destination,
        } = draft);
        included = [...draft.included];
        audio = draft.audio.map((track) => ({ ...track }));
        framing = copyFramingDraft(draft.framing);
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
      identity === (file ? JSON.stringify([encoder, file]) : null)
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
      (isSvtEncoder(encoder) && filmGrain === undefined)
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
          encoder,
          workers: backend === 'av1an' ? workers! : 2,
          filmGrain: encoder === 'x264' ? 0 : filmGrain!,
          hdr10Fallback: encoder === 'x264' ? false : hdr10Fallback,
          lineartPsyBias: encoder === 'svtAv1FiveFish' ? lineartPsyBias! : 0,
          texturePsyBias: encoder === 'svtAv1FiveFish' ? texturePsyBias! : 0,
          hdrTune: encoder === 'svtAv1Hdr' ? hdrTune : 'visualQuality',
          audio: chunked ? [] : selectedAudio(audio, included),
          framing: chunked ? defaultFraming() : selectedFraming(framing),
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
          ? 'Detect scenes and encode AV1 chunks in parallel with your selected SVT-AV1 build.'
          : 'Start with SVT-AV1-HDR, choose 5fish for anime, or use standard SVT-AV1 or x264.'}
      </p>
    </div>
    <span class="status-label"
      >{chunked ? `av1an / ${options.name}` : `Standalone ${options.name}`} · {depthLabel}</span
    >
  </div>
  <div class="notice convert-notice">
    <Info size={16} aria-hidden="true" />
    {#if encoder === 'x264'}<p>
        Encode progressive SDR to H.264 with a constant frame rate, square pixels, and 4:2:0 color.
        The source's 8-bit or 10-bit depth is retained when supported by the installed x264 build.
        HDR and interlacing are not supported. Source compatibility and encoder depth support are
        checked before encoding.
      </p>{:else}<p>
        Supports progressive SDR and compatible HDR10 video with a constant frame rate, square
        pixels, and 4:2:0 color. HDR10 preserves static HDR metadata. Rotation and interlacing are
        not supported. Source compatibility is checked before encoding.
      </p>{/if}
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
            <label for={`${idPrefix}-encoder`}>{chunked ? 'SVT-AV1 build' : 'Video encoder'}</label>
            <select id={`${idPrefix}-encoder`} bind:value={selectedEncoder} {disabled}>
              {#each encoderChoices.filter((choice) => !chunked || isSvtEncoder(choice.value)) as choice}
                <option value={choice.value}>{choice.label}</option>
              {/each}
            </select>
            <p>Each encoder keeps its own settings and output destination for this source.</p>
          </div>
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
                : `Standalone ${options.name} executable · ${depthLabel} ${options.codec} · Source frame rate`}
            </p>
          </div>
          <div class="field">
            <label for={`${idPrefix}-quality`}>Quality</label>
            <div class="input-unit">
              <input
                id={`${idPrefix}-quality`}
                type="number"
                min={options.crfMin}
                max={options.crfMax}
                step="1"
                bind:value={crf}
                {disabled}
              /><span>CRF</span>
            </div>
            <p>
              {options.crfMin}–{options.crfMax} · Lower values retain more detail
            </p>
          </div>
          <div class="field">
            <label for={`${idPrefix}-preset`}>Encoder preset</label>
            <select id={`${idPrefix}-preset`} bind:value={preset} {disabled}
              >{#each options.presets as choice}<option value={choice.value}>{choice.label}</option
                >{/each}</select
            >
            <p>{options.presetHelp}</p>
          </div>
          <EncodeOptions
            {idPrefix}
            {disabled}
            {backend}
            {encoder}
            allowBackendSelection={false}
            bind:workers
            bind:filmGrain
            bind:hdr10Fallback
            bind:lineartPsyBias
            bind:texturePsyBias
            bind:hdrTune
          />
          {#if !chunked}
            <div class="full-width">
              <FramingOptions
                {idPrefix}
                draft={framing}
                stream={selectedVideo}
                {disabled}
                onchange={(next) => (framing = next)}
              />
            </div>
          {/if}
        </div>
      </section>
      <section class="panel settings-panel">
        <div class="section-heading">
          <span class="heading-with-icon"
            ><AudioLines size={16} aria-hidden="true" /><span class="eyebrow"
              >{chunked ? 'Copy source tracks' : 'Audio & source tracks'}</span
            ></span
          >
        </div>
        <p class="copy-note">
          {chunked
            ? 'Selected audio, subtitles, and attachments are copied without encoding. Audio keeps its source codec and channels.'
            : 'Choose Copy source, Opus, or AAC for each selected audio track. Subtitles and attachments are copied.'}
        </p>
        <div class="copy-streams">
          {#each copiedStreams as stream (stream.index)}
            <label class="copy-stream"
              ><input
                type="checkbox"
                aria-label={`${stream.kind === 'audio' ? 'Include audio' : 'Copy'} stream #${stream.index}`}
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
            {#if !chunked && stream.kind === 'audio' && included.includes(stream.index)}
              {@const settings = audio.find((track) => track.streamIndex === stream.index)}
              {#if settings}
                <AudioOptions
                  {idPrefix}
                  {stream}
                  {settings}
                  {disabled}
                  onchange={(next) => {
                    audio = audio.map((track) =>
                      track.streamIndex === next.streamIndex ? next : track,
                    );
                  }}
                />
              {/if}
            {/if}
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
            <strong>{depthLabel} {options.codec}</strong><span
              >{backend === 'av1an'
                ? `av1an / ${options.name} · ${workers ?? '—'} workers`
                : `Standalone ${options.name}`} · CRF
              {crf ?? '—'} · Preset {presetLabel(encoder, preset)}{forkSettingsSummary({
                encoder,
                lineartPsyBias,
                texturePsyBias,
                hdrTune,
              })}{isSvtEncoder(encoder) ? ` · Grain ${filmGrain ?? '—'}` : ''} · MKV</span
            >
            <span
              >{chunked
                ? 'Audio copied when selected'
                : audioSummary(audio.filter((track) => included.includes(track.streamIndex)))}</span
            >
            {#if !chunked}
              <span
                >{framingResult.error
                  ? 'Check crop and resize values'
                  : framingSummary(selectedFraming(framing))}</span
              >
            {/if}
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
            Install FFmpeg, FFprobe, standalone {options.name}{backend === 'av1an'
              ? ', and av1an'
              : ''}, then refresh Tools & settings.
          </p>
        {:else if !selectedVideoSupported}<p class="disabled-reason">
            av1an requires the first video track. Use Quick Convert for another video track.
          </p>
        {:else if hdrUnsupported}<p class="disabled-reason">
            x264 supports SDR sources only. Choose SVT-AV1-HDR for compatible HDR10 video.
          </p>
        {:else if !validSettings}<p class="disabled-reason">
            Use whole numbers: CRF {options.crfMin}–{options.crfMax}, preset 0–{options.presets
              .length - 1}{isSvtEncoder(encoder) ? ', and grain 0–50' : ''}{encoder ===
            'svtAv1FiveFish'
              ? '; lineart and texture bias 0–7'
              : ''}{chunked ? '; parallel chunks 1–32' : ''}.
          </p>
        {:else if !chunked && !framingValid}<p class="disabled-reason">
            {framingResult.error}
          </p>
        {:else if !chunked && !validAudio(audio, included, file?.streams ?? [])}<p
            class="disabled-reason"
          >
            Check the bitrate range shown for each selected audio track. Use whole numbers.
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
