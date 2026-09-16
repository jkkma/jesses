<script lang="ts">
  import AdvancedEncoderOptions from '$lib/components/shared/AdvancedEncoderOptions.svelte';
  import CommandPlanPreview from '$lib/components/shared/CommandPlanPreview.svelte';
  import { parameterError, parameterSummary } from '$lib/components/shared/encoder-parameters';
  import type { EncoderParameter } from '$lib/ipc/generated';
  import Av1anOptionsControl from '$lib/components/shared/Av1anOptions.svelte';
  import {
    defaultAv1an,
    copyAv1an,
    selectedAv1an,
    av1anError,
    type Av1anDraft,
  } from '$lib/components/shared/av1an-options';
  import TemporalOptions from '$lib/components/shared/TemporalOptions.svelte';
  import {
    defaultTemporal,
    temporalError,
    selectedTemporal,
    type TemporalDraft,
  } from '$lib/components/shared/temporal-options';
  import RateControlOptions from '$lib/components/shared/RateControlOptions.svelte';
  import {
    defaultRate,
    selectedRate,
    validRate,
    rateEncoderError,
    rateSummary,
    type RateDraft,
  } from '$lib/components/shared/rate-control-options';
  import ContainerOptions from '$lib/components/shared/ContainerOptions.svelte';
  import type { ContainerFormat } from '$lib/ipc/generated';
  import { untrack } from 'svelte';
  import { preferences } from '$lib/preferences.svelte';
  import { FolderOutput, Info, ListChecks, LoaderCircle } from '@lucide/svelte';
  import { Button } from '$lib/components/ui/button';
  import { chooseOutputFolder, isDesktop, previewEncodeBatch } from '$lib/ipc/client';
  import { errorMessage, fileName, formatDuration } from '$lib/components/shared/format';
  import EncodeOptions from '$lib/components/shared/EncodeOptions.svelte';
  import AudioOptions from '$lib/components/shared/AudioOptions.svelte';
  import SubtitleOptions from '$lib/components/shared/SubtitleOptions.svelte';
  import ToneMapOptions from '$lib/components/shared/ToneMapOptions.svelte';
  import {
    defaultToneMap,
    selectedToneMap,
    toneMapError,
    toneMapSummary,
    type ToneMapDraft,
  } from '$lib/components/shared/tone-map-options';
  import {
    defaultSubtitles,
    selectedSubtitles,
    subtitleError,
    subtitleSummary,
  } from '$lib/components/shared/subtitle-options';
  import type { SubtitleTrackSettings } from '$lib/ipc/generated';
  import FramingOptions from '$lib/components/shared/FramingOptions.svelte';
  import TrimOptions from '$lib/components/shared/TrimOptions.svelte';
  import {
    defaultTrim,
    selectedTrim,
    trimError,
    trimSummary,
    type TrimDraft,
  } from '$lib/components/shared/trim-options';
  import SourcePreview from '$lib/components/shared/SourcePreview.svelte';
  import {
    defaultFramingDraft,
    validFramingDraft,
    framingSummary,
    selectedFraming,
    type FramingDraft,
  } from '$lib/components/shared/framing-options';
  import {
    audioSummary,
    defaultAudio,
    selectedAudio,
    validAudio,
    type AudioTrackDraft,
  } from '$lib/components/shared/audio-options';
  import {
    encoderOptions,
    requiredEncoderTools,
    knownHdr,
    encoderChoices,
    isSvtEncoder,
    validForkSettings,
    type HdrTune,
  } from '$lib/components/shared/encoder-options';
  import type {
    BatchEncodePreview,
    BatchEncodeRequest,
    EncodeRequest,
    EncodeBackend,
    MediaFile,
    ToolInfo,
    VideoEncoder,
  } from '$lib/ipc/generated';

  let {
    files,
    tools,
    connected,
    onfiles,
    onqueue,
  }: {
    files: MediaFile[];
    tools: ToolInfo[];
    connected: boolean;
    onfiles: () => void;
    onqueue: (requests: EncodeRequest[]) => Promise<void>;
  } = $props();
  type Draft = {
    temporal: TemporalDraft;
    identity: string;
    version: number;
    selected: boolean;
    video: number | undefined;
    copies: number[];
    audio: AudioTrackDraft[];
    subtitles: SubtitleTrackSettings[];
    framing: FramingDraft;
    trim: TrimDraft;
    toneMap: ToneMapDraft;
  };
  const desktop = isDesktop();
  let drafts = $state<Record<string, Draft>>({});
  // Details own no encode settings; closing them can dispose costly controls
  // while their drafts and reviewed request remain in this parent.
  let expandedEpisodes = $state<Record<string, boolean>>({});
  let nextVersion = 0;
  let parameters = $state<EncoderParameter[]>([]);
  let rate = $state(defaultRate());
  let av1an = $state(defaultAv1an());
  let crf = $state<number | undefined>(30);
  let preset = $state(2);
  let backend = $state<EncodeBackend>('standalone');
  let selectedEncoder = $state<VideoEncoder>('svtAv1Hdr');
  let av1anEncoder = $state<VideoEncoder>('svtAv1Hdr');
  const encoder = $derived(backend === 'av1an' ? av1anEncoder : selectedEncoder);
  const options = $derived(encoderOptions(encoder));
  let workers = $state<number | undefined>(2);
  let filmGrain = $state<number | undefined>(0);
  let hdr10Fallback = $state(false);
  let lineartPsyBias = $state<number | undefined>(0);
  let texturePsyBias = $state<number | undefined>(0);
  let hdrTune = $state<HdrTune>('filmGrain');
  let outputDirectory = $state('');
  let outputContainer = $state<ContainerFormat>('matroska');
  let defaultFolderInitialized = false;
  let outputDirectoryTouched = false;
  $effect(() => {
    const loaded = preferences.loaded;
    const folder = preferences.value.general.defaultOutputDirectory;
    if (loaded && !defaultFolderInitialized) {
      defaultFolderInitialized = true;
      untrack(() => {
        if (!outputDirectoryTouched && !outputDirectory) outputDirectory = folder;
      });
    }
  });
  let preview = $state<{ key: string; result: BatchEncodePreview } | null>(null);
  let previewing = $state(false);
  let previewGeneration = 0;
  let submitting = $state(false);
  let choosingOutput = $state(false);
  let error = $state<string | null>(null);
  let submissionNotice = $state<string | null>(null);
  type CommonSettings = {
    parameters: EncoderParameter[];
    rate: RateDraft;
    av1an: Av1anDraft;
    crf: number | undefined;
    preset: number;
    workers: number | undefined;
    filmGrain: number | undefined;
    hdr10Fallback: boolean;
    lineartPsyBias: number | undefined;
    texturePsyBias: number | undefined;
    hdrTune: HdrTune;
  };
  const savedSettings = new Map<string, CommonSettings>();
  const savedDrafts = new Map<string, Record<string, Draft>>();
  let activeWorkflow: string | null = null;

  $effect(() => {
    const currentFiles = files;
    const currentWorkflow = `${backend}:${encoder}`;
    const prior = untrack(() => {
      if (activeWorkflow === currentWorkflow) return drafts;
      if (activeWorkflow !== null) {
        savedDrafts.set(activeWorkflow, drafts);
        savedSettings.set(activeWorkflow, {
          parameters: parameters.map((value) => ({ ...value })),
          rate: { ...rate },
          av1an: copyAv1an(av1an),
          crf,
          preset,
          workers,
          filmGrain,
          hdr10Fallback,
          lineartPsyBias,
          texturePsyBias,
          hdrTune,
        });
      }
      const priorSettings = savedSettings.get(currentWorkflow);
      if (priorSettings) {
        parameters = priorSettings.parameters.map((value) => ({ ...value }));
        rate = { ...priorSettings.rate };
        av1an = copyAv1an(priorSettings.av1an);
        ({
          crf,
          preset,
          workers,
          filmGrain,
          hdr10Fallback,
          lineartPsyBias,
          texturePsyBias,
          hdrTune,
        } = priorSettings);
      } else resetSettings();
      activeWorkflow = currentWorkflow;
      return savedDrafts.get(currentWorkflow) ?? {};
    });
    let selectedCount = currentFiles.filter(
      (file) => prior[file.id]?.selected && prior[file.id]?.identity === JSON.stringify(file),
    ).length;
    const next = Object.fromEntries(
      currentFiles.map((file) => {
        const identity = JSON.stringify(file);
        const existing = prior[file.id];
        if (existing?.identity === identity) return [file.id, existing];
        const video = file.streams.find((stream) => stream.kind === 'video')?.index;
        const selected =
          video !== undefined && !file.id.startsWith('jesses-synthetic') && selectedCount < 100;
        if (selected) selectedCount++;
        return [
          file.id,
          {
            identity,
            version: ++nextVersion,
            selected,
            video,
            copies: file.streams
              .filter((stream) => !['video', 'data'].includes(stream.kind))
              .map((stream) => stream.index),
            audio: defaultAudio(file.streams),
            subtitles: defaultSubtitles(file.streams),
            framing: defaultFramingDraft(),
            trim: defaultTrim(),
            toneMap: defaultToneMap(),
            temporal: defaultTemporal(file.streams.find((stream) => stream.index === video)),
          } satisfies Draft,
        ];
      }),
    );
    if (JSON.stringify(next) !== JSON.stringify(untrack(() => drafts))) drafts = next;
  });

  const selectedFiles = $derived(files.filter((file) => drafts[file.id]?.selected));
  $effect(() => {
    const retained = new Set(selectedFiles.map((file) => file.id));
    untrack(() => {
      for (const id of Object.keys(expandedEpisodes)) {
        if (!retained.has(id)) delete expandedEpisodes[id];
      }
    });
  });

  const toolsReady = $derived(
    requiredEncoderTools(backend, encoder).every((id) =>
      tools.some((tool) => tool.id === id && tool.available),
    ),
  );
  const rateIssue = $derived(rateEncoderError(rate, encoder));
  const validSettings = $derived(
    (backend !== 'av1an' || isSvtEncoder(encoder)) &&
      validForkSettings(encoder, lineartPsyBias, texturePsyBias, hdrTune) &&
      validRate(rate) &&
      !rateIssue &&
      (backend !== 'av1an' ||
        !av1anError(
          av1an,
          selectedFiles.some((file) =>
            knownHdr(file.streams.find((stream) => stream.index === drafts[file.id].video)),
          ),
        )) &&
      ((backend === 'av1an' && av1an.targetEnabled) ||
        rate.mode !== 'quality' ||
        (typeof crf === 'number' &&
          Number.isFinite(crf) &&
          (!isSvtEncoder(encoder) || Number.isInteger(crf * 4)) &&
          (isSvtEncoder(encoder) || Number.isInteger(crf)) &&
          crf >= options.crfMin &&
          crf <= options.crfMax)) &&
      Number.isInteger(preset) &&
      options.presets.some((choice) => choice.value === preset) &&
      (!isSvtEncoder(encoder) ||
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
  const draftKey = $derived(
    JSON.stringify({
      files,
      drafts,
      parameters,
      rate,
      av1an,
      crf,
      preset,
      backend,
      encoder,
      workers,
      filmGrain,
      hdr10Fallback,
      lineartPsyBias,
      texturePsyBias,
      hdrTune,
      outputDirectory,
      outputContainer,
    }),
  );
  const currentPreview = $derived(preview?.key === draftKey ? preview.result : null);
  const validFraming = $derived(
    selectedFiles.every((file) =>
      validFramingDraft(
        drafts[file.id].framing,
        file.streams.find(
          (stream) => stream.kind === 'video' && stream.index === drafts[file.id].video,
        ),
      ),
    ),
  );
  const ready = $derived(currentPreview?.items.filter((item) => item.request && !item.error) ?? []);
  const subtitleIssue = $derived(
    selectedFiles
      .map((file) =>
        subtitleError(
          drafts[file.id].subtitles,
          drafts[file.id].copies,
          file.streams,
          backend,
          file.streams.find((stream) => stream.index === drafts[file.id].video),
          drafts[file.id].toneMap.enabled,
        ),
      )
      .find(Boolean),
  );
  const canPreview = $derived(
    desktop &&
      toolsReady &&
      validSettings &&
      !parameterError(parameters, null, encoder) &&
      validFraming &&
      !subtitleIssue &&
      selectedFiles.every((file) => !temporalError(drafts[file.id].temporal, backend)) &&
      selectedFiles.every(
        (file) =>
          !toneMapError(
            drafts[file.id].toneMap,
            backend,
            file.streams.find((stream) => stream.index === drafts[file.id].video),
            isSvtEncoder(encoder) && hdr10Fallback,
          ),
      ) &&
      selectedFiles.every(
        (file) =>
          !trimError(
            drafts[file.id].trim,
            backend,
            drafts[file.id].audio,
            drafts[file.id].copies,
            file.streams,
          ),
      ) &&
      selectedFiles.every((file) =>
        validAudio(drafts[file.id].audio, drafts[file.id].copies, file.streams),
      ) &&
      !!outputDirectory.trim() &&
      selectedFiles.length > 0 &&
      selectedFiles.length <= 100 &&
      selectedFiles.every((file) =>
        file.streams.some(
          (stream) => stream.kind === 'video' && stream.index === drafts[file.id]?.video,
        ),
      ) &&
      !submitting &&
      !previewing,
  );
  const canQueue = $derived(
    connected && toolsReady && !!currentPreview && ready.length > 0 && !submitting && !previewing,
  );

  $effect(() => {
    draftKey;
    // Native work may finish later; its result belongs only to its original draft.
    ++previewGeneration;
    preview = null;
    previewing = false;
    error = null;
  });

  function resetSettings() {
    parameters = [];
    rate = defaultRate();
    av1an = defaultAv1an();
    crf = options.defaultCrf;
    preset = options.defaultPreset;
    workers = 2;
    filmGrain = 0;
    hdr10Fallback = false;
    lineartPsyBias = options.defaultLineartPsyBias;
    texturePsyBias = options.defaultTexturePsyBias;
    hdrTune = options.defaultHdrTune;
  }

  function updateDraft(id: string, patch: Partial<Draft>) {
    const draft = drafts[id];
    if (!draft || submitting) return;
    drafts = { ...drafts, [id]: { ...draft, ...patch } };
  }

  function selectFiles(selected: boolean) {
    if (submitting) return;
    let count = 0;
    drafts = Object.fromEntries(
      files.map((file) => {
        const draft = drafts[file.id];
        const eligible =
          selected &&
          draft?.video !== undefined &&
          !file.id.startsWith('jesses-synthetic') &&
          count < 100;
        if (eligible) count++;
        return [file.id, { ...draft, selected: eligible }];
      }),
    );
  }

  function toggleCopy(id: string, index: number) {
    const draft = drafts[id];
    if (draft)
      updateDraft(id, {
        copies: draft.copies.includes(index)
          ? draft.copies.filter((value) => value !== index)
          : [...draft.copies, index],
      });
  }

  async function chooseOutput() {
    if (!desktop || submitting || choosingOutput) return;
    const key = draftKey;
    const generation = previewGeneration;
    choosingOutput = true;
    try {
      const path = await chooseOutputFolder();
      if (path && key === draftKey && generation === previewGeneration) {
        outputDirectoryTouched = true;
        outputDirectory = path;
      }
    } catch (cause) {
      if (key === draftKey && generation === previewGeneration) error = errorMessage(cause);
    } finally {
      choosingOutput = false;
    }
  }

  async function previewBatch() {
    if (
      !canPreview ||
      (rate.mode === 'quality' && crf === undefined) ||
      (isSvtEncoder(encoder) && filmGrain === undefined)
    )
      return;
    const key = draftKey;
    const generation = ++previewGeneration;
    const request: BatchEncodeRequest = {
      ...(parameters.length ? { parameters: parameters.map((value) => ({ ...value })) } : {}),
      outputDirectory: outputDirectory.trim(),
      outputContainer,
      ...(backend === 'av1an' ? { av1anOptions: selectedAv1an(av1an) } : {}),
      ...(rate.mode === 'bitrate' || rate.mode === 'targetSize'
        ? { rateControl: selectedRate(rate) }
        : {}),
      crf:
        rate.mode === 'quality' && !(backend === 'av1an' && av1an.targetEnabled)
          ? Math.round(crf!)
          : Math.round(options.defaultCrf),
      preset: Math.max(0, preset),
      lossless: rate.mode === 'lossless',
      ...(isSvtEncoder(encoder) && rate.mode === 'quality'
        ? { svtCrfQuarterSteps: Math.round(crf! * 4) }
        : {}),
      ...(isSvtEncoder(encoder) ? { svtPreset: preset } : {}),
      backend,
      encoder,
      workers: backend === 'av1an' ? workers! : 2,
      filmGrain: isSvtEncoder(encoder) ? filmGrain! : 0,
      hdr10Fallback: isSvtEncoder(encoder) ? hdr10Fallback : false,
      lineartPsyBias: encoder === 'svtAv1FiveFish' ? lineartPsyBias! : 0,
      texturePsyBias: encoder === 'svtAv1FiveFish' ? texturePsyBias! : 0,
      hdrTune: encoder === 'svtAv1Hdr' ? hdrTune : 'visualQuality',
      inputs: selectedFiles.map((file) => {
        const draft = drafts[file.id];
        const copies = file.streams
          .filter((stream) => stream.kind !== 'video' && draft.copies.includes(stream.index))
          .sort((a, b) => Number(a.kind === 'attachment') - Number(b.kind === 'attachment'));
        return {
          ...(selectedTemporal(draft.temporal)
            ? { temporal: selectedTemporal(draft.temporal) }
            : {}),
          inputPath: file.path,
          videoStreamIndex: draft.video!,
          streamIndices: [draft.video!, ...copies.map((stream) => stream.index)],
          audio: selectedAudio(draft.audio, draft.copies),
          ...(selectedSubtitles(draft.subtitles, draft.copies).length
            ? { subtitles: selectedSubtitles(draft.subtitles, draft.copies) }
            : {}),
          framing: selectedFraming(draft.framing),
          ...(draft.trim.enabled ? { trim: selectedTrim(draft.trim) } : {}),
          ...(draft.toneMap.enabled ? { toneMap: selectedToneMap(draft.toneMap) } : {}),
        };
      }),
    };
    previewing = true;
    preview = null;
    error = null;
    submissionNotice = null;
    try {
      const result = await previewEncodeBatch(request);
      if (generation === previewGeneration && key === draftKey) preview = { key, result };
    } catch (cause) {
      if (generation === previewGeneration && key === draftKey) error = errorMessage(cause);
    } finally {
      if (generation === previewGeneration) previewing = false;
    }
  }

  async function queueReady() {
    if (!canQueue) return;
    const key = draftKey;
    const requests: EncodeRequest[] = JSON.parse(JSON.stringify(ready.map((item) => item.request)));
    const submittedDrafts = new Map(
      files
        .filter((file) => requests.some((request) => request.source.inputPath === file.path))
        .map((file) => [file.id, JSON.stringify(drafts[file.id])]),
    );
    submitting = true;
    error = null;
    try {
      await onqueue(requests);
      // A source may have changed while another tab was open. Clear only the
      // exact submitted selections, leaving new or edited drafts alone.
      drafts = Object.fromEntries(
        Object.entries(drafts).map(([id, draft]) => [
          id,
          submittedDrafts.get(id) === JSON.stringify(draft) ? { ...draft, selected: false } : draft,
        ]),
      );
      preview = null;
      submissionNotice = `${requests.length} ${requests.length === 1 ? 'file' : 'files'} added to the queue. Review any remaining selections before another batch.`;
    } catch (cause) {
      if (key === draftKey) {
        preview = null;
        error = `${errorMessage(cause)} Preview the batch again before retrying.`;
      } else {
        submissionNotice = `The previous batch was not queued: ${errorMessage(cause)} Preview the current selection again.`;
      }
    } finally {
      submitting = false;
    }
  }
</script>

<section class="batch-workspace" aria-label="Batch encode workspace">
  <div class="view-intro">
    <div>
      <span class="eyebrow">Folder workflow</span>
      <h1>Batch encode</h1>
      <p>Review your episodes, choose common settings, then queue the ready files together.</p>
    </div>
    <span class="status-label"
      >{options.name} · {isSvtEncoder(encoder)
        ? '10-bit AV1'
        : `${options.codec} · Source bit depth`}</span
    >
  </div>
  <div class="notice">
    <Info size={16} aria-hidden="true" />
    <div>
      {#if encoder === 'x264'}<p>
          Encode SDR to H.264 at the source's 8-bit or 10-bit depth, or to 10-bit SDR after explicit
          HDR tone mapping. The installed x264 build must support the chosen depth.
        </p>{:else if encoder === 'x265' || encoder === 'vp9'}<p>
          FFmpeg {encoder === 'x265' ? 'libx265' : 'libvpx-vp9'} encodes tagged SDR at 8-bit or 10-bit
          depth. HDR sources require explicit tone mapping to SDR. The runtime checks the installed encoder
          and pixel format before encoding.
        </p>{:else}<p>
          SVT encodes 10-bit AV1 from SDR or compatible HDR10 sources. HDR10 output preserves
          validated static HDR metadata. Each episode keeps its own source-processing choices.
        </p>{/if}
      {#if backend === 'standalone'}<p>
          Use each episode's settings for explicit BWDIF deinterlacing, frame-rate conversion or
          HDR/HLG-to-SDR tone mapping. Subtitles start with Copy source; supported tracks can be
          converted or burned into video. Sources need a validated constant frame rate, square
          pixels and 4:2:0 color. Mixed field order and cadence repair are not supported.
        </p>{:else}<p>
          Av1an requires progressive video with a validated constant frame rate, square pixels and
          4:2:0 color. Subtitle tracks and attachments are copied. Deinterlacing, tone mapping and
          subtitle conversion or burn-in require standalone encoding.
        </p>{/if}
    </div>
  </div>
  {#if error}<div class="notice error-notice" role="alert"><p>{error}</p></div>{/if}
  {#if submissionNotice}<div class="notice" role="status"><p>{submissionNotice}</p></div>{/if}
  <div class="batch-grid">
    <section class="panel batch-sources" aria-label="Batch source selection">
      <div class="section-heading">
        <span class="heading-with-icon"
          ><ListChecks size={16} aria-hidden="true" /><span class="eyebrow"
            >Episodes · {selectedFiles.length} selected</span
          ></span
        ><button class="text-button" type="button" onclick={onfiles}>Add or remove files</button>
      </div>
      <div class="selection-actions">
        <button
          class="text-button"
          type="button"
          disabled={!files.length || submitting}
          onclick={() => selectFiles(true)}>Select up to 100</button
        ><button
          class="text-button"
          type="button"
          disabled={!selectedFiles.length || submitting}
          onclick={() => selectFiles(false)}>Clear selection</button
        ><span class="small-muted">Maximum 100 files per batch.</span>
      </div>
      <div class="episode-list">
        {#each files as file (file.id)}
          {@const draft = drafts[file.id]}
          {@const videos = file.streams.filter((stream) => stream.kind === 'video')}
          {#if draft}
            <article class="episode" class:selected={draft.selected}>
              <label class="episode-choice"
                ><input
                  type="checkbox"
                  aria-label={`Select ${file.name}`}
                  checked={draft.selected}
                  disabled={submitting ||
                    !desktop ||
                    !videos.length ||
                    file.id.startsWith('jesses-synthetic') ||
                    (!draft.selected && selectedFiles.length >= 100)}
                  onchange={(event) =>
                    updateDraft(file.id, { selected: event.currentTarget.checked })}
                /><span
                  ><strong>{file.name}</strong><small
                    >{formatDuration(file.durationSeconds)} · {file.path}</small
                  ></span
                ></label
              >
              {#if !videos.length}<p class="disabled-reason">
                  No video stream. This source cannot be encoded.
                </p>{/if}
              {#if file.id.startsWith('jesses-synthetic')}<p class="disabled-reason">
                  Synthetic preview only. Import a local file to encode.
                </p>{/if}
              {#if !isSvtEncoder(encoder) && knownHdr(file.streams.find((stream) => stream.index === draft.video)) && !draft.toneMap.enabled}<p
                  class="disabled-reason"
                >
                  {options.name} needs SDR video. Open this episode's settings and enable explicit HDR-to-SDR
                  tone mapping, or choose an SVT build for compatible HDR10 output.
                </p>{/if}
              {#if draft.selected}
                <details
                  class="episode-tracks"
                  open={expandedEpisodes[file.id] ?? false}
                  ontoggle={(event) => {
                    expandedEpisodes[file.id] = event.currentTarget.open;
                  }}
                >
                  <summary>Video, audio & source tracks · {draft.copies.length} selected</summary>
                  {#if expandedEpisodes[file.id]}
                    <div class="field">
                      <label for={`batch-video-${draft.version}`}
                        >Video stream for {file.name}</label
                      ><select
                        id={`batch-video-${draft.version}`}
                        value={draft.video}
                        disabled={submitting}
                        onchange={(event) =>
                          updateDraft(file.id, { video: Number(event.currentTarget.value) })}
                        >{#each videos as stream (stream.index)}<option value={stream.index}
                            >#{stream.index} · {stream.codec ?? 'Unknown'}{stream.title
                              ? ` · ${stream.title}`
                              : ''}</option
                          >{/each}</select
                      >
                    </div>
                    <div class="episode-framing">
                      <TemporalOptions
                        value={drafts[file.id].temporal}
                        video={file.streams.find(
                          (stream) => stream.index === drafts[file.id].video,
                        )}
                        {backend}
                        disabled={submitting}
                        onchange={(value) => updateDraft(file.id, { temporal: value })}
                      />
                      {#if backend === 'standalone'}<ToneMapOptions
                          draft={draft.toneMap}
                          disabled={submitting || previewing}
                          error={toneMapError(
                            draft.toneMap,
                            backend,
                            file.streams.find((stream) => stream.index === draft.video),
                            isSvtEncoder(encoder) && hdr10Fallback,
                          )}
                          onchange={(next) => updateDraft(file.id, { toneMap: next })}
                        /><TrimOptions
                          idPrefix={`batch-${draft.version}`}
                          draft={draft.trim}
                          disabled={submitting}
                          error={trimError(
                            draft.trim,
                            backend,
                            draft.audio,
                            draft.copies,
                            file.streams,
                          )}
                          onchange={(next) => updateDraft(file.id, { trim: next })}
                        />{/if}
                      <FramingOptions
                        idPrefix={`batch-${draft.version}`}
                        draft={draft.framing}
                        stream={videos.find((stream) => stream.index === draft.video)}
                        disabled={submitting}
                        onchange={(next) => updateDraft(file.id, { framing: next })}
                      />
                      {#if typeof draft.video === 'number'}
                        <SourcePreview
                          {file}
                          videoStreamIndex={draft.video}
                          crop={selectedFraming(draft.framing).crop}
                          disabled={submitting || !desktop}
                          onapply={(crop) =>
                            updateDraft(file.id, {
                              framing: { ...draft.framing, crop: { ...crop } },
                            })}
                        />
                      {/if}
                    </div>
                    <div class="track-choices">
                      {#each file.streams.filter((stream) => stream.kind !== 'video') as stream (stream.index)}<label
                          ><input
                            type="checkbox"
                            aria-label={`${stream.kind === 'audio' ? 'Include audio' : 'Copy'} stream #${stream.index} from ${file.name}`}
                            checked={draft.copies.includes(stream.index)}
                            disabled={submitting}
                            onchange={() => toggleCopy(file.id, stream.index)}
                          /><span
                            >#{stream.index} · {stream.kind} · {stream.codec ??
                              'Unknown'}{stream.language
                              ? ` · ${stream.language}`
                              : ''}{stream.title ? ` · ${stream.title}` : ''}</span
                          ></label
                        >
                        {#if stream.kind === 'audio' && draft.copies.includes(stream.index)}
                          {@const settings = draft.audio.find(
                            (track) => track.streamIndex === stream.index,
                          )}
                          {#if settings}
                            <AudioOptions
                              inputPath={file.path}
                              idPrefix={`batch-${draft.version}`}
                              {stream}
                              {settings}
                              disabled={submitting}
                              onchange={(next) =>
                                updateDraft(file.id, {
                                  audio: draft.audio.map((track) =>
                                    track.streamIndex === next.streamIndex ? next : track,
                                  ),
                                })}
                            />
                          {/if}
                        {/if}
                        {#if stream.kind === 'subtitle' && draft.copies.includes(stream.index)}
                          {@const settings = draft.subtitles.find(
                            (track) => track.streamIndex === stream.index,
                          )}
                          {#if settings}<SubtitleOptions
                              toneMapped={draft.toneMap.enabled}
                              idPrefix={`batch-${draft.version}`}
                              {stream}
                              {settings}
                              {backend}
                              video={file.streams.find((stream) => stream.index === draft.video)}
                              disabled={submitting}
                              onchange={(next) =>
                                updateDraft(file.id, {
                                  subtitles: draft.subtitles.map((track) =>
                                    track.streamIndex === next.streamIndex ? next : track,
                                  ),
                                })}
                            />{/if}
                        {/if}
                      {/each}
                    </div>
                    <p class="small-muted audio-guidance">
                      Audio starts with Copy source. Choose a codec separately for each selected
                      track.
                    </p>
                  {/if}
                </details>
              {/if}
            </article>
          {/if}
        {:else}<div class="batch-empty">
            <p>Import files or a folder to prepare a batch.</p>
            <Button variant="outline" onclick={onfiles}>Choose source files</Button>
          </div>{/each}
      </div>
    </section>
    <aside class="panel batch-settings">
      <div class="section-heading">
        <span class="heading-with-icon"
          ><FolderOutput size={16} aria-hidden="true" /><span class="eyebrow">Common settings</span
          ></span
        >
      </div>
      <div class="batch-settings-content">
        <div class="setting-fields">
          {#if backend === 'standalone'}<div class="field full-width">
              <label for="batch-encoder">Video encoder</label>
              <select
                id="batch-encoder"
                bind:value={selectedEncoder}
                disabled={submitting || !desktop}
              >
                {#each encoderChoices as choice}
                  <option value={choice.value}>{choice.label}</option>
                {/each}
              </select>
            </div>{:else}<div class="field full-width">
              <label for="batch-svt-build">SVT-AV1 build</label>
              <select
                id="batch-svt-build"
                bind:value={av1anEncoder}
                disabled={submitting || !desktop}
              >
                {#each encoderChoices.filter((choice) => isSvtEncoder(choice.value)) as choice}
                  <option value={choice.value}>{choice.label}</option>
                {/each}
              </select>
            </div>{/if}
          {#if backend === 'av1an'}<Av1anOptionsControl
              idPrefix="batch-av1an"
              draft={av1an}
              disabled={submitting}
              hdr={selectedFiles.some((file) =>
                knownHdr(file.streams.find((stream) => stream.index === drafts[file.id].video)),
              )}
              framed={selectedFiles.some(
                (file) => !!framingSummary(selectedFraming(drafts[file.id].framing)),
              )}
              onchange={(value) => (av1an = value)}
            />{/if}
          {#if backend === 'standalone'}<RateControlOptions
              idPrefix="batch"
              draft={rate}
              disabled={submitting}
              onchange={(value) => (rate = value)}
            />{#if rateIssue}<p role="alert">{rateIssue}</p>{/if}{/if}
          {#if rate.mode === 'quality' && !(backend === 'av1an' && av1an.targetEnabled)}
            <div class="field">
              <label for="batch-crf">CRF</label><input
                id="batch-crf"
                type="number"
                min={options.crfMin}
                max={options.crfMax}
                step={isSvtEncoder(encoder) ? '0.25' : '1'}
                bind:value={crf}
                disabled={submitting}
              />
              <p>
                {options.crfMin}–{options.crfMax} · Lower values retain more detail
              </p>
            </div>
          {/if}
          <div class="field">
            <label for="batch-preset">Preset</label><select
              id="batch-preset"
              bind:value={preset}
              disabled={submitting}
              >{#each options.presets as choice}<option value={choice.value}>{choice.label}</option
                >{/each}</select
            >
            <p>{options.presetHelp}</p>
          </div>
          <EncodeOptions
            idPrefix="batch"
            disabled={submitting || !desktop}
            bind:backend
            {encoder}
            bind:workers
            bind:filmGrain
            bind:hdr10Fallback
            bind:lineartPsyBias
            bind:texturePsyBias
            bind:hdrTune
          />
        </div>
        <AdvancedEncoderOptions
          {encoder}
          {backend}
          value={parameters}
          disabled={submitting || !desktop}
          onchange={(value) => (parameters = value)}
        />
        <button class="text-button" type="button" disabled={submitting} onclick={resetSettings}
          >Reset batch settings</button
        >
        <div class="field">
          <label for="batch-output">Output folder</label><input
            id="batch-output"
            oninput={() => (outputDirectoryTouched = true)}
            bind:value={outputDirectory}
            disabled={!desktop || submitting}
            placeholder="Choose an existing output folder"
          />
          <ContainerOptions
            value={outputContainer}
            onchange={(value) => (outputContainer = value)}
            disabled={submitting}
          />
        </div>
        <Button
          variant="outline"
          onclick={chooseOutput}
          disabled={!desktop || submitting || choosingOutput}>Choose output folder</Button
        >
        <p class="small-muted">
          Each source gets a new filename in the selected container. The preview avoids existing
          names; files are never replaced. Names are checked again when queued.
        </p>
        <Button onclick={previewBatch} disabled={!canPreview}
          >{#if previewing}<LoaderCircle size={14} class="spinning" aria-hidden="true" />Preparing
            preview…{:else}Preview batch{/if}</Button
        >
        {#if !desktop}<p class="disabled-reason">
            Batch encoding requires the desktop app.
          </p>{:else if !toolsReady}<p class="disabled-reason">
            Install FFmpeg, FFprobe, standalone {options.name}{backend === 'av1an'
              ? ', and av1an'
              : ''}, then refresh Tools & settings.
          </p>{:else if !validRate(rate)}<p class="disabled-reason">
            Enter a valid whole-number bitrate or target size above.
          </p>{:else if !validSettings}<p class="disabled-reason">
            Use {isSvtEncoder(encoder) ? 'quarter-step' : 'whole-number'} CRF {options.crfMin}–{options.crfMax},
            a listed preset{isSvtEncoder(encoder) ? ', grain 0–50' : ''}{encoder ===
            'svtAv1FiveFish'
              ? '; lineart and texture bias 0–7'
              : ''}{backend === 'av1an' ? ', and parallel chunks 1–32' : ''}.
          </p>{:else if !validFraming}<p class="disabled-reason">
            Check crop, resize and border values in each selected episode's video settings.
          </p>{:else if subtitleIssue}<p class="disabled-reason" role="alert">
            {subtitleIssue}
          </p>{:else if !selectedFiles.every( (file) => validAudio(drafts[file.id].audio, drafts[file.id].copies, file.streams) )}<p
            class="disabled-reason"
          >
            Check the selected audio codec, channels, and bitrate. Any compatibility issue is shown
            beside its track.
          </p>{:else if !selectedFiles.length}<p class="disabled-reason">
            Select at least one episode.
          </p>{:else if !outputDirectory.trim()}<p class="disabled-reason">
            Choose an output folder before previewing.
          </p>{/if}
      </div>
    </aside>
  </div>
  <section class="panel batch-review" aria-label="Batch output preview">
    <div class="section-heading">
      <span class="eyebrow">Output preview</span><span class="small-muted"
        >{currentPreview
          ? `${ready.length} ready / ${currentPreview.items.length} reviewed`
          : 'Review required before queueing'}</span
      >
    </div>
    {#if currentPreview}<div class="batch-preview-scroll">
        <table>
          <thead><tr><th>Source</th><th>New output</th><th>Result</th></tr></thead><tbody
            >{#each currentPreview.items as item, index (`${item.inputPath}-${index}`)}<tr
                ><td title={item.inputPath}>{fileName(item.inputPath)}</td><td
                  >{item.outputPath ?? '—'}</td
                ><td
                  >{#if item.error}<span class="preview-error">{item.error.message}</span
                    >{:else if item.request}<span>Ready</span><small class="preview-audio"
                      >{rateSummary(
                        item.request.settings.rateControl,
                        item.request.settings.svtCrfQuarterSteps === undefined
                          ? item.request.settings.crf
                          : item.request.settings.svtCrfQuarterSteps / 4,
                        item.request.settings.lossless,
                      )} · {audioSummary(item.request.settings.audio)}</small
                    ><small class="preview-audio"
                      >{framingSummary(item.request.settings.framing)}{trimSummary(
                        item.request.settings.trim,
                      )}{toneMapSummary(item.request.settings.toneMap)}</small
                    >{#if item.request.settings.subtitles?.length}<small class="preview-audio"
                        >{subtitleSummary(item.request.settings.subtitles)}</small
                      >{/if}<small>{parameterSummary(item.request.settings.parameters)}</small>
                    <details>
                      <summary>Command plan</summary><CommandPlanPreview
                        request={item.request}
                        disabled={submitting}
                      />
                    </details>{:else}Unavailable{/if}</td
                ></tr
              >{/each}</tbody
          >
        </table>
      </div>{:else}<p class="preview-placeholder">
        Preview the current selection to check filenames and per-file errors. Changes to files,
        tracks, settings, or the output folder require a new preview.
      </p>{/if}
    <div class="batch-submit">
      <Button onclick={queueReady} disabled={!canQueue}
        >{submitting ? 'Queueing…' : 'Queue ready files'}</Button
      >
      <p class="small-muted">
        {connected
          ? 'Only ready rows are submitted. Each job keeps its reviewed settings and runs in order.'
          : 'Connecting to the job runtime…'}
      </p>
    </div>
  </section>
</section>

<style>
  .batch-grid {
    display: grid;
    grid-template-columns: minmax(0, 1.6fr) minmax(260px, 1fr);
    gap: 20px;
    margin-top: 20px;
  }
  .batch-sources,
  .batch-settings {
    min-width: 0;
  }
  .selection-actions,
  .batch-submit {
    display: flex;
    flex-wrap: wrap;
    gap: 12px;
    align-items: center;
    padding: 16px 20px;
  }
  .episode-list {
    max-height: 580px;
    overflow-y: auto;
  }
  .episode {
    padding: 16px 20px;
    border-top: 1px solid var(--border);
  }
  .episode.selected {
    background: color-mix(in srgb, var(--background) 40%, transparent);
  }
  .episode-choice,
  .track-choices > label {
    display: flex;
    gap: 10px;
    align-items: flex-start;
  }
  .episode-choice input,
  .track-choices > label > input {
    width: 16px;
    height: 16px;
    flex: 0 0 auto;
    margin-top: 2px;
    accent-color: #ad5326;
  }
  .episode-choice span {
    min-width: 0;
  }
  .episode-choice strong,
  .episode-choice small {
    display: block;
    overflow-wrap: anywhere;
  }
  .episode-choice strong {
    font-size: 13px;
  }
  .episode-choice small {
    margin-top: 5px;
    font-size: 11px;
    color: var(--muted-foreground);
  }
  .episode-tracks {
    margin: 12px 0 0 26px;
  }
  .episode-tracks summary {
    cursor: pointer;
    font-size: 12px;
  }
  .episode-tracks .field {
    margin-top: 14px;
  }
  .track-choices {
    display: grid;
    gap: 10px;
    margin-top: 12px;
    max-height: 240px;
    overflow-y: auto;
    font-size: 12px;
  }
  .episode-framing {
    margin-top: 18px;
  }
  .track-choices span {
    overflow-wrap: anywhere;
  }
  .audio-guidance {
    margin-top: 12px;
  }
  .batch-settings-content {
    display: grid;
    gap: 16px;
    padding: 20px;
  }
  .batch-settings-content .setting-fields {
    padding: 0;
  }
  .batch-review {
    margin-top: 20px;
  }
  .batch-preview-scroll {
    overflow-x: auto;
    max-height: 420px;
  }
  table {
    width: 100%;
    border-collapse: collapse;
    font-size: 12px;
    table-layout: fixed;
  }
  th,
  td {
    padding: 12px 16px;
    text-align: left;
    border-bottom: 1px solid var(--border);
    overflow-wrap: anywhere;
    vertical-align: top;
  }
  th {
    font-size: 10px;
    text-transform: uppercase;
    color: var(--muted-foreground);
  }
  .preview-error {
    color: #8b3328;
  }
  .preview-audio {
    display: block;
    margin-top: 6px;
    font-size: 10px;
    color: var(--muted-foreground);
  }
  .preview-placeholder,
  .batch-empty {
    padding: 24px 20px;
    font-size: 12px;
  }
  .batch-empty p {
    margin-bottom: 14px;
  }
  .batch-submit p {
    flex: 1;
    min-width: 180px;
  }
  .text-button:disabled {
    opacity: 0.5;
    cursor: default;
  }
  @media (max-width: 950px) {
    .batch-grid {
      grid-template-columns: 1fr;
    }
    .episode-list {
      max-height: 430px;
    }
  }
</style>
