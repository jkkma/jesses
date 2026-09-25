<script lang="ts">
  import AdvancedEncoderOptions from './AdvancedEncoderOptions.svelte';
  import CommandPlanPreview from './CommandPlanPreview.svelte';
  import { parameterError } from './encoder-parameters';
  import type { EncoderParameter } from '$lib/ipc/generated';
  import Av1anOptionsControl from './Av1anOptions.svelte';
  import Av1anResources from './Av1anResources.svelte';
  import Av1anGrain from './Av1anGrain.svelte';
  import Av1anFilters from './Av1anFilters.svelte';
  import {
    defaultAv1anAudio,
    readAv1anPreferences,
    saveAv1anAudioPreference,
    saveAv1anPreferences,
  } from './av1an-preferences';
  import {
    defaultAv1an,
    copyAv1an,
    selectedAv1an,
    av1anError,
    av1anGrainConflict,
    type Av1anDraft,
  } from './av1an-options';
  import TemporalOptions from './TemporalOptions.svelte';
  import {
    defaultTemporal,
    temporalError,
    selectedTemporal,
    type TemporalDraft,
  } from './temporal-options';
  import ContainerOptions from '$lib/components/shared/ContainerOptions.svelte';
  import {
    destinationContainer,
    containerDestination,
  } from '$lib/components/shared/container-options';
  import { untrack, type Snippet } from 'svelte';
  import { preferredDestination } from '$lib/preferences.svelte';
  import { ArrowRight, FolderOutput, Info, Play, RotateCcw } from '@lucide/svelte';
  import { Button } from '$lib/components/ui/button';
  import { chooseEncodeDestination, isDesktop } from '$lib/ipc/client';
  import { errorMessage, formatDuration } from '$lib/components/shared/format';
  import EncodeOptions from '$lib/components/shared/EncodeOptions.svelte';
  import AudioOptions from './AudioOptions.svelte';
  import SubtitleOptions from './SubtitleOptions.svelte';
  import {
    defaultSubtitles,
    selectedSubtitles,
    subtitleError,
    subtitleSummary,
  } from './subtitle-options';
  import type { SubtitleTrackSettings } from '$lib/ipc/generated';
  import { terminalJob } from './job-state';
  import FramingOptions from './FramingOptions.svelte';
  import TrimOptions from './TrimOptions.svelte';
  import ToneMapOptions from './ToneMapOptions.svelte';
  import RateControlOptions from './RateControlOptions.svelte';
  import {
    defaultRate,
    selectedRate,
    validRate,
    rateEncoderError,
    rateSummary,
    type RateDraft,
  } from './rate-control-options';
  import {
    defaultToneMap,
    selectedToneMap,
    toneMapError,
    type ToneMapDraft,
  } from './tone-map-options';
  import { defaultTrim, selectedTrim, trimError, type TrimDraft } from './trim-options';
  import SourcePreview from './SourcePreview.svelte';
  import ExternalTracks from './ExternalTracks.svelte';
  import TrackLayout from './TrackLayout.svelte';
  import {
    invalidateMovTimecodeChoice,
    movTimecodeIssue,
    selectedMovTimecode,
    type MovTimecodeChoice,
  } from './mov-timecode';
  import {
    defaultTrackOrder,
    effectiveTrackOrder,
    invalidateSourceDonor,
    selectedTrackOrder,
    sourceDonorIssue,
    trackTagsIssue,
    type SourceDonor,
    type TrackTags,
  } from './track-layout';
  import {
    copyExternalChoices,
    externalTrackIssue,
    externalTrackSummary,
    invalidateExternalChoices,
    requestExternalTracks,
    type ExternalTrackChoice,
  } from './external-tracks';
  import {
    copyFramingDraft,
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
    isAv1anEncoder,
    isSvtEncoder,
    validForkSettings,
    forkSettingsSummary,
    type HdrTune,
  } from './encoder-options';
  import type {
    EncodeBackend,
    EncodeRequest,
    EncodeTrackOverride,
    EncodeTrackRef,
    Av1anGrainSettings,
    JobSnapshot,
    MediaFile,
    ToolInfo,
    VideoEncoder,
  } from '$lib/ipc/generated';

  let {
    backend,
    file,
    files,
    tools,
    jobs,
    connected,
    onfiles,
    onstart,
    onqueue,
    sourcePicker,
  }: {
    backend: EncodeBackend;
    file: MediaFile | undefined;
    files: MediaFile[];
    tools: ToolInfo[];
    jobs: JobSnapshot[];
    connected: boolean;
    onfiles: () => void;
    onstart: (request: EncodeRequest) => Promise<void>;
    onqueue: (request: EncodeRequest) => Promise<void>;
    sourcePicker?: Snippet;
  } = $props();
  let videoIndex = $state<number | undefined>();
  let selectedEncoder = $state<VideoEncoder>('svtAv1Hdr');
  let parameters = $state<EncoderParameter[]>([]);
  let rate = $state(defaultRate());
  let av1an = $state(defaultAv1an());
  let av1anGrain = $state<Av1anGrainSettings | undefined>();
  let av1anFilters = $state<string[]>([]);
  let crf = $state<number | undefined>(30);
  let preset = $state(2);
  let workers = $state<number | undefined>(2);
  let filmGrain = $state<number | undefined>(0);
  let hdr10Fallback = $state(false);
  let lineartPsyBias = $state<number | undefined>(0);
  let texturePsyBias = $state<number | undefined>(0);
  let hdrTune = $state<HdrTune>('filmGrain');
  let included = $state<number[]>([]);
  let externalTracks = $state<ExternalTrackChoice[]>([]);
  let trackOverrides = $state<EncodeTrackOverride[]>([]);
  let trackOrder = $state<EncodeTrackRef[]>([]);
  let metadataDonor = $state<SourceDonor | undefined>();
  let chaptersDonor = $state<SourceDonor | undefined>();
  let movTimecode = $state<MovTimecodeChoice | undefined>();
  let audio = $state<AudioTrackDraft[]>([]);
  let subtitles = $state<SubtitleTrackSettings[]>([]);
  let framing = $state(defaultFramingDraft());
  let trim = $state(defaultTrim());
  let toneMap = $state(defaultToneMap());
  let destination = $state('');
  let error = $state<string | null>(null);
  let submitting = $state(false);
  let temporal = $state(defaultTemporal());
  const settingsTabs = [
    { id: 'video', label: 'Video' },
    { id: 'audio', label: 'Audio & subtitles' },
    { id: 'filters', label: 'Filters' },
    { id: 'advanced', label: 'Advanced' },
  ] as const;
  type SettingsTab = (typeof settingsTabs)[number]['id'];
  let settingsTab = $state<SettingsTab>('video');
  let showAllSettings = $state(false);

  function selectSettingsTab(tab: SettingsTab) {
    settingsTab = tab;
    showAllSettings = false;
  }

  function handleSettingsKey(event: KeyboardEvent, index: number) {
    const offset = event.key === 'ArrowRight' ? 1 : event.key === 'ArrowLeft' ? -1 : 0;
    if (!offset && event.key !== 'Home' && event.key !== 'End') return;
    event.preventDefault();
    const next =
      event.key === 'Home'
        ? 0
        : event.key === 'End'
          ? settingsTabs.length - 1
          : (index + offset + settingsTabs.length) % settingsTabs.length;
    settingsTab = settingsTabs[next].id;
    const button = event.currentTarget as HTMLButtonElement;
    button.parentElement?.querySelectorAll<HTMLButtonElement>('[role="tab"]')[next]?.focus();
  }
  const outputFrameRateLabel = $derived(
    temporal.changeRate
      ? temporalError(temporal, 'standalone')
        ? 'Check output frame rate'
        : `${temporal.numerator}/${temporal.denominator} fps output`
      : temporal.deinterlace === 'bob'
        ? 'Double source frame rate (BWDIF bob)'
        : 'Source frame rate',
  );
  type Draft = {
    parameters: EncoderParameter[];
    temporal: TemporalDraft;
    videoIndex: number | undefined;
    rate: RateDraft;
    av1an: Av1anDraft;
    av1anGrain: Av1anGrainSettings | undefined;
    av1anFilters: string[];
    crf: number | undefined;
    preset: number;
    workers: number | undefined;
    filmGrain: number | undefined;
    hdr10Fallback: boolean;
    lineartPsyBias: number | undefined;
    texturePsyBias: number | undefined;
    hdrTune: HdrTune;
    included: number[];
    externalTracks: ExternalTrackChoice[];
    trackOverrides: EncodeTrackOverride[];
    trackOrder: EncodeTrackRef[];
    metadataDonor: SourceDonor | undefined;
    chaptersDonor: SourceDonor | undefined;
    movTimecode: MovTimecodeChoice | undefined;
    audio: AudioTrackDraft[];
    subtitles: SubtitleTrackSettings[];
    framing: FramingDraft;
    trim: TrimDraft;
    toneMap: ToneMapDraft;
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
  const sourceDetails = $derived.by(() => {
    const details: string[] = [];
    if (selectedVideo?.width && selectedVideo.height) {
      details.push(`${selectedVideo.width} × ${selectedVideo.height}`);
    }
    if (file?.durationSeconds != null && Number.isFinite(file.durationSeconds)) {
      details.push(formatDuration(file.durationSeconds));
    }
    return details.join(' · ');
  });
  const framingResult = $derived(framingDimensions(framing, selectedVideo));
  const framingValid = $derived(validFramingDraft(framing, selectedVideo));
  const toneMapIssue = $derived(
    toneMapError(toneMap, backend, selectedVideo, isSvtEncoder(encoder) && hdr10Fallback),
  );
  const trimIssue = $derived(
    trimError(trim, backend, audio, included, file?.streams ?? [], subtitles),
  );
  const subtitleIssue = $derived(
    subtitleError(
      subtitles,
      included,
      file?.streams ?? [],
      backend,
      selectedVideo,
      toneMap.enabled,
    ),
  );
  const rateIssue = $derived(rateEncoderError(rate, encoder));
  const hdrUnsupported = $derived(
    !isSvtEncoder(encoder) && knownHdr(selectedVideo) && !toneMap.enabled,
  );
  const depthLabel = $derived(
    toneMap.enabled
      ? '10-bit SDR output'
      : isSvtEncoder(encoder)
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
  const externalIssue = $derived(
    externalTrackIssue(externalTracks, files, file, {
      trimming: trim.enabled,
      backend,
      video: selectedVideo,
      toneMapped: toneMap.enabled,
      primaryBurnCount: selectedSubtitles(subtitles, included).filter(
        (track) => track.mode === 'burnIn',
      ).length,
    }),
  );
  const layoutDefaults = $derived(
    defaultTrackOrder(file, videoIndex, included, externalTracks, files),
  );
  const layoutIssue = $derived(
    sourceDonorIssue(metadataDonor, files, 'Container metadata') ||
      sourceDonorIssue(chaptersDonor, files, 'Chapters') ||
      trackOverrides
        .filter((override) =>
          effectiveTrackOrder(trackOrder, layoutDefaults).some(
            (ref) => !ref.inputPath && ref.streamIndex === override.streamIndex,
          ),
        )
        .map(trackTagsIssue)
        .find(Boolean) ||
      externalTracks.map(trackTagsIssue).find(Boolean) ||
      null,
  );
  const timecodeIssue = $derived(
    movTimecodeIssue(
      movTimecode,
      files,
      destinationContainer(destination) === 'mov',
      trim.enabled,
      temporal,
    ),
  );
  const usableSource = $derived(!!file && !file.id.startsWith('jesses-synthetic'));
  const toolsReady = $derived(
    requiredEncoderTools(backend, encoder, av1an.concatMethod).every((id) =>
      tools.some((tool) => tool.id === id && tool.available),
    ),
  );
  const disabled = $derived(!desktop || !usableSource || submitting);
  const grainConflict = $derived(
    chunked && isSvtEncoder(encoder) ? av1anGrainConflict(parameters, filmGrain, av1anGrain) : null,
  );
  const videoSettingsIssue = $derived.by(() => {
    if (chunked && !isAv1anEncoder(encoder)) return 'Choose an encoder supported by av1an.';
    if (!validRate(rate)) return 'Enter a valid whole-number bitrate or target size.';
    if (rateIssue) return rateIssue;
    if (
      !(chunked && av1an.targetEnabled) &&
      rate.mode === 'quality' &&
      !(
        typeof crf === 'number' &&
        Number.isFinite(crf) &&
        (isSvtEncoder(encoder) ? Number.isInteger(crf * 4) : Number.isInteger(crf)) &&
        crf >= options.crfMin &&
        crf <= options.crfMax
      )
    )
      return `Use ${isSvtEncoder(encoder) ? 'quarter-step' : 'whole-number'} CRF ${options.crfMin}–${options.crfMax}.`;
    if (!Number.isInteger(preset) || !options.presets.some((choice) => choice.value === preset))
      return 'Choose a listed encoder preset.';
    if (
      chunked &&
      !(typeof workers === 'number' && Number.isInteger(workers) && workers >= 1 && workers <= 64)
    )
      return 'Use 1–64 parallel chunks.';
    return null;
  });
  const tuningSettingsIssue = $derived.by(() => {
    if (!validForkSettings(encoder, lineartPsyBias, texturePsyBias, hdrTune))
      return 'Check encoder tuning: use lineart and texture bias 0–7 and a listed HDR tune.';
    if (
      isSvtEncoder(encoder) &&
      !(
        typeof filmGrain === 'number' &&
        Number.isInteger(filmGrain) &&
        filmGrain >= 0 &&
        filmGrain <= 50
      )
    )
      return 'Use film grain synthesis 0–50.';
    if (chunked) {
      const issue = av1anError(
        av1an,
        knownHdr(selectedVideo) && !toneMap.enabled,
        encoder,
        destinationContainer(destination) === 'matroska',
      );
      if (issue) return issue;
      if (
        av1anGrain &&
        !(
          (av1anGrain.table === null || av1anGrain.table.length > 0) &&
          (av1anGrain.table === null || filmGrain === 0) &&
          Number.isInteger(av1anGrain.denoiseStrength) &&
          av1anGrain.denoiseStrength >= 1 &&
          av1anGrain.denoiseStrength <= 16
        )
      )
        return 'Check the grain table and use denoise strength 1–16.';
    }
    return null;
  });
  const customFiltersIssue = $derived(
    chunked && av1anFilters.length > 16 ? 'Use at most 16 custom pixel filters.' : null,
  );
  const validSettings = $derived(
    !videoSettingsIssue && !tuningSettingsIssue && !customFiltersIssue,
  );
  const canQueue = $derived(
    !disabled &&
      connected &&
      toolsReady &&
      selectedVideoSupported &&
      !hdrUnsupported &&
      validSettings &&
      !grainConflict &&
      !parameterError(parameters, null, encoder) &&
      framingValid &&
      !trimIssue &&
      !externalIssue &&
      !layoutIssue &&
      !timecodeIssue &&
      !temporalError(temporal, backend) &&
      !toneMapIssue &&
      !subtitleIssue &&
      validAudio(audio, included, file?.streams ?? []) &&
      !!destination.trim(),
  );
  const canStart = $derived(canQueue && !active);
  const settingsIssues = $derived(
    [
      { tab: 'video' as const, message: videoSettingsIssue },
      {
        tab: 'filters' as const,
        message:
          framingResult.error ||
          trimIssue ||
          toneMapIssue ||
          temporalError(temporal, backend) ||
          customFiltersIssue,
      },
      {
        tab: 'audio' as const,
        message:
          externalIssue ||
          layoutIssue ||
          timecodeIssue ||
          subtitleIssue ||
          (!validAudio(audio, included, file?.streams ?? [])
            ? 'Check the audio codec, channels, and bitrate.'
            : null),
      },
      {
        tab: 'advanced' as const,
        message: tuningSettingsIssue || grainConflict || parameterError(parameters, null, encoder),
      },
    ].filter((issue) => issue.message),
  );

  function reset(
    source: MediaFile | undefined,
    usePreferences = true,
    useInfrastructurePreferences = usePreferences,
  ) {
    ++draftGeneration;
    videoIndex = source?.streams.find((stream) => stream.kind === 'video')?.index;
    parameters = [];
    rate = defaultRate();
    av1an = defaultAv1an(useInfrastructurePreferences);
    av1anGrain = undefined;
    av1anFilters = chunked && usePreferences ? readAv1anPreferences().filters : [];
    crf = options.defaultCrf;
    preset = options.defaultPreset;
    workers = chunked && useInfrastructurePreferences ? readAv1anPreferences().workers : 2;
    filmGrain = 0;
    hdr10Fallback = false;
    lineartPsyBias = options.defaultLineartPsyBias;
    texturePsyBias = options.defaultTexturePsyBias;
    hdrTune = options.defaultHdrTune;
    included =
      source?.streams
        .filter((stream) => !['video', 'data'].includes(stream.kind))
        .map((stream) => stream.index) ?? [];
    externalTracks = [];
    trackOverrides = [];
    trackOrder = [];
    metadataDonor = undefined;
    chaptersDonor = undefined;
    movTimecode = undefined;
    audio =
      chunked && usePreferences
        ? defaultAv1anAudio(source?.streams ?? [])
        : defaultAudio(source?.streams ?? []);
    subtitles = defaultSubtitles(source?.streams ?? []);
    framing = defaultFramingDraft();
    trim = defaultTrim();
    toneMap = defaultToneMap();
    temporal = defaultTemporal(source?.streams.find((stream) => stream.index === videoIndex));
    destination =
      source && !source.id.startsWith('jesses-synthetic')
        ? preferredDestination(
            source.path.replace(/\.[^./\\]+$/, '') +
              (chunked ? options.av1anSuffix : options.suffix),
          )
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
          parameters: parameters.map((value) => ({ ...value })),
          videoIndex,
          rate: { ...rate },
          av1an: copyAv1an(av1an),
          av1anGrain: av1anGrain ? { ...av1anGrain } : undefined,
          av1anFilters: [...av1anFilters],
          crf,
          preset,
          workers,
          filmGrain,
          hdr10Fallback,
          lineartPsyBias,
          texturePsyBias,
          hdrTune,
          included: [...included],
          externalTracks: copyExternalChoices(externalTracks),
          trackOverrides: trackOverrides.map((entry) => ({ ...entry })),
          trackOrder: trackOrder.map((entry) => ({ ...entry })),
          metadataDonor: metadataDonor ? { ...metadataDonor } : undefined,
          chaptersDonor: chaptersDonor ? { ...chaptersDonor } : undefined,
          movTimecode: movTimecode ? { ...movTimecode } : undefined,
          audio: audio.map((track) => ({ ...track })),
          subtitles: subtitles.map((track) => ({ ...track })),
          framing: copyFramingDraft(framing),
          trim: { ...trim },
          toneMap: { ...toneMap },
          temporal: { ...temporal },
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
        externalTracks = copyExternalChoices(draft.externalTracks);
        trackOverrides = draft.trackOverrides.map((entry) => ({ ...entry }));
        trackOrder = draft.trackOrder.map((entry) => ({ ...entry }));
        metadataDonor = draft.metadataDonor ? { ...draft.metadataDonor } : undefined;
        chaptersDonor = draft.chaptersDonor ? { ...draft.chaptersDonor } : undefined;
        movTimecode = draft.movTimecode ? { ...draft.movTimecode } : undefined;
        audio = draft.audio.map((track) => ({ ...track }));
        subtitles = draft.subtitles.map((track) => ({ ...track }));
        framing = copyFramingDraft(draft.framing);
        trim = { ...draft.trim };
        toneMap = { ...draft.toneMap };
        temporal = { ...draft.temporal };
        parameters = draft.parameters.map((value) => ({ ...value }));
        rate = { ...draft.rate };
        av1an = copyAv1an(draft.av1an);
        av1anGrain = draft.av1anGrain ? { ...draft.av1anGrain } : undefined;
        av1anFilters = [...draft.av1anFilters];
        error = null;
      } else {
        const previousDraftForSource =
          source !== undefined &&
          [...drafts.keys()].some((key) => {
            const [, savedSource] = JSON.parse(key) as [VideoEncoder, MediaFile];
            return JSON.stringify(savedSource) === JSON.stringify(source);
          });
        reset(source, !previousDraftForSource, chunked);
      }
      draftIdentity = identity;
    });
  });
  $effect(() => {
    if (chunked && usableSource) saveAv1anPreferences(av1an, workers, av1anFilters);
  });
  $effect(() => {
    const available = files;
    untrack(() => {
      externalTracks = invalidateExternalChoices(externalTracks, available);
      metadataDonor = invalidateSourceDonor(metadataDonor, available);
      chaptersDonor = invalidateSourceDonor(chaptersDonor, available);
      movTimecode = invalidateMovTimecodeChoice(movTimecode, available);
      for (const draft of drafts.values()) {
        draft.externalTracks = invalidateExternalChoices(draft.externalTracks, available);
        draft.metadataDonor = invalidateSourceDonor(draft.metadataDonor, available);
        draft.chaptersDonor = invalidateSourceDonor(draft.chaptersDonor, available);
        draft.movTimecode = invalidateMovTimecodeChoice(draft.movTimecode, available);
      }
    });
  });
  function toggle(index: number) {
    included = included.includes(index)
      ? included.filter((value) => value !== index)
      : [...included, index];
  }
  function updatePrimaryTags(streamIndex: number, tags: TrackTags) {
    const remaining = trackOverrides.filter((entry) => entry.streamIndex !== streamIndex);
    trackOverrides = Object.keys(tags).length
      ? [...remaining, { streamIndex, ...tags }]
      : remaining;
  }
  function updateExternalTags(inputPath: string, streamIndex: number, tags: TrackTags) {
    externalTracks = externalTracks.map((choice) => {
      if (choice.inputPath !== inputPath || choice.streamIndex !== streamIndex) return choice;
      const {
        title: _title,
        language: _language,
        default: _default,
        forced: _forced,
        ...rest
      } = choice;
      return { ...rest, ...tags };
    });
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
  function currentRequest(): EncodeRequest | undefined {
    if (!file || videoIndex === undefined) return undefined;
    const copies = copiedStreams
      .filter((stream) => included.includes(stream.index))
      .sort((a, b) => Number(a.kind === 'attachment') - Number(b.kind === 'attachment'));
    const selectedPrimary = new Set([videoIndex, ...copies.map((stream) => stream.index)]);
    const selectedOverrides = trackOverrides
      .filter((entry) => selectedPrimary.has(entry.streamIndex))
      .map((entry) => ({ ...entry }));
    const selectedOrder = selectedTrackOrder(trackOrder, layoutDefaults);
    return {
      source: {
        inputPath: file.path,
        outputPath: destination.trim(),
        streamIndices: [videoIndex, ...copies.map((stream) => stream.index)],
      },
      settings: {
        ...(parameters.length ? { parameters: parameters.map((value) => ({ ...value })) } : {}),
        ...(selectedTemporal(temporal) ? { temporal: selectedTemporal(temporal) } : {}),
        videoStreamIndex: videoIndex,
        ...(chunked ? { av1anOptions: selectedAv1an(av1an, encoder) } : {}),
        ...(chunked && isSvtEncoder(encoder) && av1anGrain
          ? { av1anGrain: { ...av1anGrain } }
          : {}),
        ...(chunked && av1anFilters.length ? { av1anFilters: [...av1anFilters] } : {}),
        ...(rate.mode === 'bitrate' || rate.mode === 'targetSize'
          ? { rateControl: selectedRate(rate) }
          : {}),
        crf:
          rate.mode === 'quality' && !(chunked && av1an.targetEnabled)
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
        audio: selectedAudio(audio, included),
        ...(selectedSubtitles(subtitles, included).length
          ? { subtitles: selectedSubtitles(subtitles, included) }
          : {}),
        ...(externalTracks.length
          ? {
              externalTracks: requestExternalTracks(externalTracks),
            }
          : {}),
        ...(selectedOverrides.length ? { trackOverrides: selectedOverrides } : {}),
        ...(selectedOrder?.length ? { trackOrder: selectedOrder } : {}),
        ...(metadataDonor && metadataDonor.inputPath !== file.path
          ? { metadataSourcePath: metadataDonor.inputPath }
          : {}),
        ...(chaptersDonor && chaptersDonor.inputPath !== file.path
          ? { chaptersSourcePath: chaptersDonor.inputPath }
          : {}),
        ...(movTimecode ? { movTimecodeTrack: selectedMovTimecode(movTimecode, file) } : {}),
        framing: selectedFraming(framing),
        ...(trim.enabled ? { trim: selectedTrim(trim) } : {}),
        ...(toneMap.enabled ? { toneMap: selectedToneMap(toneMap) } : {}),
      },
    };
  }
  const commandRequest = $derived(canQueue ? currentRequest() : undefined);
  async function start(queue = false) {
    if (
      !(queue ? canQueue : canStart) ||
      !file ||
      videoIndex === undefined ||
      (rate.mode === 'quality' && crf === undefined) ||
      (isSvtEncoder(encoder) && filmGrain === undefined)
    )
      return;
    const request = currentRequest();
    if (!request) return;
    submitting = true;
    error = null;
    const generation = draftGeneration;
    const identity = draftIdentity;
    try {
      await (queue ? onqueue : onstart)(request);
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
      <h1>{chunked ? 'av1an' : 'Quick Convert'}</h1>
      <p>
        {chunked
          ? 'Encode scenes in parallel.'
          : 'Choose your video settings, then start encoding.'}
      </p>
    </div>
    {#if sourcePicker}
      {@render sourcePicker()}
    {:else}<span class="status-label"
        >{chunked
          ? `av1an / ${options.name}`
          : `${encoder === 'x265' || encoder === 'vp9' ? 'FFmpeg' : 'Standalone'} ${options.name}`} ·
        {depthLabel}</span
      >{/if}
  </div>
  {#if error}<div class="notice error-notice" role="alert">
      <Info size={16} aria-hidden="true" />
      <p>{error}</p>
    </div>{/if}
  <div class="convert-grid">
    <div class="convert-settings">
      <div class="settings-toolbar">
        <div
          class="settings-tabs"
          role="tablist"
          aria-label="Encode settings"
          hidden={showAllSettings}
        >
          {#each settingsTabs as tab, index}
            <button
              type="button"
              role="tab"
              id={`${idPrefix}-tab-${tab.id}`}
              aria-controls={`${idPrefix}-settings-${tab.id}`}
              aria-selected={settingsTab === tab.id}
              tabindex={settingsTab === tab.id ? 0 : -1}
              onclick={() => selectSettingsTab(tab.id)}
              onkeydown={(event) => handleSettingsKey(event, index)}
            >
              {tab.label}{#if settingsIssues.some((issue) => issue.tab === tab.id)}<span
                  class="tab-warning"
                  aria-hidden="true">!</span
                >{/if}
            </button>
          {/each}
        </div>
        {#if showAllSettings}<strong>All settings</strong>{/if}
        <button
          type="button"
          class="text-button settings-view"
          aria-pressed={showAllSettings}
          onclick={() => (showAllSettings = !showAllSettings)}
          >{showAllSettings ? 'Use tabs' : 'Show all settings'}</button
        >
      </div>
      <section
        class="panel settings-panel settings-section"
        id={`${idPrefix}-settings-video`}
        role={showAllSettings ? 'region' : 'tabpanel'}
        aria-label="Video"
        hidden={!showAllSettings && settingsTab !== 'video'}
      >
        <div class="section-heading">
          <span class="eyebrow">Video settings</span>
          <button type="button" class="text-button" {disabled} onclick={() => reset(file, false)}
            ><RotateCcw size={13} aria-hidden="true" />Reset settings</button
          >
        </div>
        <div class="setting-fields">
          <div class="source-fields full-width">
            <div class="field">
              <label for={`${idPrefix}-encoder`}>Video encoder</label>
              <select id={`${idPrefix}-encoder`} bind:value={selectedEncoder} {disabled}>
                {#each encoderChoices.filter((choice) => !chunked || isAv1anEncoder(choice.value)) as choice}
                  <option value={choice.value}>{choice.label}</option>
                {/each}
              </select>
            </div>
            <div class="field">
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
                {#if !videos.length}<option value={undefined}>No video stream available</option
                  >{/if}
              </select>
              <p>
                {chunked
                  ? 'av1an encodes the first video track only.'
                  : `${['x265', 'vp9', 'h264Nvenc', 'hevcNvenc'].includes(encoder) ? 'FFmpeg' : 'Standalone'} ${options.name} · ${depthLabel} ${options.codec} · ${outputFrameRateLabel}`}
              </p>
            </div>
          </div>
          {#if !chunked}<RateControlOptions
              {idPrefix}
              draft={rate}
              {disabled}
              onchange={(value) => (rate = value)}
            />{#if rateIssue}<p role="alert">{rateIssue}</p>{/if}{/if}
          {#if rate.mode === 'quality' && !(chunked && av1an.targetEnabled)}
            <div class="field">
              <label for={`${idPrefix}-quality`}>Quality</label>
              <div class="input-unit">
                <input
                  id={`${idPrefix}-quality`}
                  type="number"
                  min={options.crfMin}
                  max={options.crfMax}
                  step={isSvtEncoder(encoder) ? '0.25' : '1'}
                  bind:value={crf}
                  {disabled}
                /><span>CRF</span>
              </div>
              <p>
                {options.crfMin}–{options.crfMax} · Lower values retain more detail
              </p>
            </div>
          {/if}
          <div class="field">
            <label for={`${idPrefix}-preset`}>Encoder preset</label>
            <select class="compact-select" id={`${idPrefix}-preset`} bind:value={preset} {disabled}
              >{#each options.presets as choice}<option value={choice.value}>{choice.label}</option
                >{/each}</select
            >
            <p>{options.presetHelp}</p>
          </div>
          {#if chunked}
            <div class="field">
              <label for={`${idPrefix}-workers`}>Parallel chunks</label>
              <input
                id={`${idPrefix}-workers`}
                type="number"
                min="1"
                max="64"
                step="1"
                bind:value={workers}
                {disabled}
              />
              <p>How many scenes to encode at once.</p>
            </div>
          {/if}
        </div>
      </section>
      <section
        class="panel settings-panel settings-section"
        id={`${idPrefix}-settings-audio`}
        role={showAllSettings ? 'region' : 'tabpanel'}
        aria-label="Audio & subtitles"
        hidden={!showAllSettings && settingsTab !== 'audio'}
      >
        <div class="section-heading"><span class="eyebrow">Audio & subtitles</span></div>
        <p class="copy-note">
          Choose actions for selected audio and subtitle tracks. Attachments are copied.
        </p>
        <div class="copy-streams">
          {#each copiedStreams as stream (stream.index)}
            <div class="source-track">
              <label class="copy-stream"
                ><input
                  type="checkbox"
                  aria-label={`${stream.kind === 'audio' ? 'Include audio' : 'Copy'} stream #${stream.index}`}
                  checked={included.includes(stream.index)}
                  {disabled}
                  onchange={() => toggle(stream.index)}
                />
                <span
                  ><strong
                    >#{stream.index} · {stream.kind} · {stream.codec ?? 'Unknown codec'}</strong
                  ><small
                    >{[stream.title, stream.language].filter(Boolean).join(' · ') ||
                      'No track title'}</small
                  ></span
                >
              </label>
              {#if stream.kind === 'audio' && included.includes(stream.index)}
                {@const settings = audio.find((track) => track.streamIndex === stream.index)}
                {#if settings}
                  <AudioOptions
                    inputPath={file!.path}
                    {idPrefix}
                    {stream}
                    {settings}
                    {disabled}
                    onchange={(next) => {
                      audio = audio.map((track) =>
                        track.streamIndex === next.streamIndex ? next : track,
                      );
                      if (chunked) saveAv1anAudioPreference(next, stream);
                    }}
                  />
                {/if}
              {/if}
              {#if stream.kind === 'subtitle' && included.includes(stream.index)}
                {@const settings = subtitles.find((track) => track.streamIndex === stream.index)}
                {#if settings}<SubtitleOptions
                    toneMapped={toneMap.enabled}
                    {idPrefix}
                    {stream}
                    {settings}
                    {backend}
                    video={selectedVideo}
                    {disabled}
                    onchange={(next) =>
                      (subtitles = subtitles.map((track) =>
                        track.streamIndex === next.streamIndex ? next : track,
                      ))}
                  />{/if}
              {/if}
            </div>
          {:else}<p class="small-muted">No additional tracks to copy.</p>{/each}
        </div>
        <ExternalTracks
          idPrefix={backend}
          {backend}
          video={selectedVideo}
          toneMapped={toneMap.enabled}
          {files}
          primary={file}
          selected={externalTracks}
          {disabled}
          issue={externalIssue}
          onchange={(next) => (externalTracks = next)}
        />
        <TrackLayout
          {files}
          primary={file}
          defaults={layoutDefaults}
          order={trackOrder}
          overrides={trackOverrides}
          external={externalTracks}
          {metadataDonor}
          {chaptersDonor}
          {movTimecode}
          outputIsMov={destinationContainer(destination) === 'mov'}
          issue={layoutIssue || timecodeIssue}
          {disabled}
          onorder={(next) => (trackOrder = next)}
          onprimarytags={updatePrimaryTags}
          onexternaltags={updateExternalTags}
          onmetadata={(next) => (metadataDonor = next)}
          onchapters={(next) => (chaptersDonor = next)}
          ontimecode={(next) => (movTimecode = next)}
        />
      </section>
      <section
        class="panel settings-panel settings-section"
        id={`${idPrefix}-settings-filters`}
        role={showAllSettings ? 'region' : 'tabpanel'}
        aria-label="Filters"
        hidden={!showAllSettings && settingsTab !== 'filters'}
      >
        <div class="section-heading"><span class="eyebrow">Filters</span></div>
        <div class="video-adjustments filter-fields">
          <TemporalOptions
            value={temporal}
            video={selectedVideo}
            {backend}
            {disabled}
            onchange={(value) => (temporal = value)}
          />
          <ToneMapOptions
            draft={toneMap}
            {backend}
            {disabled}
            error={toneMapIssue}
            onchange={(next) => (toneMap = next)}
          /><TrimOptions
            {idPrefix}
            draft={trim}
            {disabled}
            error={trimIssue}
            onchange={(next) => (trim = next)}
          />
          <FramingOptions
            {idPrefix}
            draft={framing}
            stream={selectedVideo}
            {disabled}
            onchange={(next) => (framing = next)}
          />
          {#if file && typeof videoIndex === 'number'}
            <SourcePreview
              {file}
              videoStreamIndex={videoIndex}
              crop={selectedFraming(framing).crop}
              disabled={disabled || !desktop}
              onapply={(crop) => (framing = { ...framing, crop: { ...crop } })}
            />
          {/if}
          {#if chunked}
            {#key JSON.stringify([encoder, file?.id])}
              <Av1anFilters
                value={av1anFilters}
                {disabled}
                onchange={(value) => (av1anFilters = [...value])}
              />
            {/key}
          {/if}
        </div>
      </section>
      <section
        class="panel settings-panel settings-section"
        id={`${idPrefix}-settings-advanced`}
        role={showAllSettings ? 'region' : 'tabpanel'}
        aria-label="Advanced"
        hidden={!showAllSettings && settingsTab !== 'advanced'}
      >
        <div class="section-heading"><span class="eyebrow">Advanced</span></div>
        <div class="setting-fields">
          <EncodeOptions
            {idPrefix}
            {disabled}
            {backend}
            {encoder}
            allowBackendSelection={false}
            showWorkers={false}
            bind:workers
            bind:filmGrain
            bind:hdr10Fallback
            bind:lineartPsyBias
            bind:texturePsyBias
            bind:hdrTune
            grainTableSelected={chunked && !!av1anGrain?.table}
          />
          {#if chunked && isSvtEncoder(encoder)}
            {#key JSON.stringify([encoder, file?.id])}
              <Av1anGrain
                value={av1anGrain}
                {disabled}
                onchange={(value) => {
                  av1anGrain = value ? { ...value } : undefined;
                  if (value?.table) filmGrain = 0;
                }}
              />
            {/key}
          {/if}
          {#if chunked}<Av1anOptionsControl
              {idPrefix}
              draft={av1an}
              {encoder}
              {disabled}
              attachmentSupported={destinationContainer(destination) === 'matroska'}
              hdr={knownHdr(selectedVideo) && !toneMap.enabled}
              framed={framingSummary(selectedFraming(framing)) !== 'Source dimensions'}
              onchange={(value) => (av1an = value)}
            />{/if}
          {#if chunked}<Av1anResources
              {encoder}
              {workers}
              sourceWidth={selectedVideo?.width}
              sourceHeight={selectedVideo?.height}
              outputWidth={framingResult.width}
              outputHeight={framingResult.height}
              filtered={framingSummary(selectedFraming(framing)) !== 'Source dimensions' ||
                !!selectedTemporal(temporal) ||
                toneMap.enabled ||
                trim.enabled ||
                av1anFilters.length > 0}
              floatFilter={toneMap.enabled}
              {disabled}
              onapply={(nextWorkers, threads, slices) => {
                workers = nextWorkers;
                av1an = { ...av1an, encoderThreads: threads, sceneDetectionSlices: slices };
              }}
            />{/if}
          <div class="video-adjustments full-width">
            <AdvancedEncoderOptions
              {encoder}
              {backend}
              value={parameters}
              {disabled}
              onchange={(value) => (parameters = value)}
            />
            {#if grainConflict}<p class="disabled-reason" role="alert">{grainConflict}</p>{/if}
          </div>
          <details class="compatibility-note">
            <summary>Source compatibility</summary>
            {#if encoder === 'x265' || encoder === 'vp9'}<p>
                FFmpeg {encoder === 'x265' ? 'libx265' : 'libvpx-vp9'} encodes tagged SDR at the source's
                8-bit or 10-bit depth. HDR sources require explicit tone mapping to SDR. The runtime checks
                that FFmpeg includes the selected library and pixel format before encoding.
              </p>{/if}
            {#if encoder === 'x264'}<p>
                Encode progressive SDR to H.264 with a constant frame rate, square pixels, and 4:2:0
                color. The source's 8-bit or 10-bit depth is retained when supported by the
                installed x264 build. HDR sources require tone mapping to SDR, and interlacing
                requires explicit deinterlacing. Source compatibility and encoder depth support are
                checked before encoding.
              </p>{:else if isSvtEncoder(encoder)}<p>
                Supports progressive SDR and compatible HDR10 video with a constant frame rate,
                square pixels, and 4:2:0 color. HDR10 preserves static HDR metadata. Explicit
                deinterlacing is available when needed. Rotation is not supported. Source
                compatibility is checked before encoding.
              </p>{/if}
          </details>
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
          {#if sourceDetails}<span class="source-details">{sourceDetails}</span>{/if}
        </div>
        <div class="field destination-field">
          <label for={`${idPrefix}-destination`}>Encode destination</label>
          <div class="destination-control">
            <input
              id={`${idPrefix}-destination`}
              bind:value={destination}
              {disabled}
              placeholder="Choose a new media file"
            />
            <Button
              variant="outline"
              onclick={chooseOutput}
              {disabled}
              aria-label="Choose encode destination">Browse…</Button
            >
          </div>
        </div>
        <ContainerOptions
          value={destinationContainer(destination)}
          onchange={(value) => (destination = containerDestination(destination, value))}
          compactHelp={!showAllSettings}
          {disabled}
        />
        <div class="encode-actions">
          <Button class="start-encode" onclick={() => start()} disabled={!canStart}
            ><Play size={14} aria-hidden="true" />{submitting
              ? 'Starting…'
              : 'Start encode'}</Button
          >
          <Button variant="outline" onclick={() => start(true)} disabled={!canQueue}
            >Add to queue</Button
          >
        </div>
        <details class="output-summary" open={showAllSettings}>
          <summary>Review output settings</summary>
          <p>
            <span class="summary-label">Output settings</span>
            <strong>{depthLabel} {options.codec}</strong><span
              >{backend === 'av1an'
                ? `av1an / ${options.name} · ${workers ?? '—'} workers`
                : `${['x265', 'vp9', 'h264Nvenc', 'hevcNvenc'].includes(encoder) ? 'FFmpeg' : 'Standalone'} ${options.name}`}
              · {rateSummary(selectedRate(rate), crf, rate.mode === 'lossless')} · Preset {presetLabel(
                encoder,
                preset,
              )}{forkSettingsSummary({
                encoder,
                lineartPsyBias,
                texturePsyBias,
                hdrTune,
              })}{isSvtEncoder(encoder) ? ` · Grain ${filmGrain ?? '—'}` : ''} · {{
                matroska: 'MKV',
                mp4: 'MP4',
                mov: 'MOV',
                webm: 'WebM',
              }[destinationContainer(destination)]}</span
            >
            <span
              >{audioSummary(audio.filter((track) => included.includes(track.streamIndex)))}</span
            >
            {#if selectedSubtitles(subtitles, included).length}<span
                >{subtitleSummary(selectedSubtitles(subtitles, included))}</span
              >{/if}
            {#if externalTracks.length}<span>{externalTrackSummary(externalTracks)}</span>{/if}
            {#if trim.enabled}<span>Video interval trimmed</span>{/if}
            {#if toneMap.enabled}<span>HDR to SDR tone mapping enabled</span>{/if}
            {#if selectedTemporal(temporal)}<span>Frame processing enabled</span>{/if}
            {#if av1anFilters.length}<span>{av1anFilters.length} custom pixel filters</span>{/if}
            {#if parameters.length}<span>{parameters.length} advanced encoder parameters</span>{/if}
            {#if selectedTrackOrder(trackOrder, layoutDefaults)}<span
                >Custom output track order</span
              >{/if}
            {#if trackOverrides.length}<span>Primary track labels or flags edited</span>{/if}
            {#if metadataDonor}<span
                >Metadata from {files.find((entry) => entry.path === metadataDonor?.inputPath)
                  ?.name ?? 'unavailable source'}</span
              >{/if}
            {#if chaptersDonor}<span
                >Chapters from {files.find((entry) => entry.path === chaptersDonor?.inputPath)
                  ?.name ?? 'unavailable source'}</span
              >{/if}
            {#if movTimecode}<span
                >MOV timecode from {files.find((entry) => entry.path === movTimecode?.inputPath)
                  ?.name ?? 'unavailable source'}</span
              >{/if}
            <span
              >{framingResult.error
                ? 'Check crop, resize and border values'
                : framingSummary(selectedFraming(framing))}</span
            >
          </p>
        </details>
        {#if settingsIssues.length && !showAllSettings}
          <div class="settings-issues" role="alert">
            {#each settingsIssues as issue}
              <p>{issue.message}</p>
              <button type="button" class="text-button" onclick={() => selectSettingsTab(issue.tab)}
                >Review {settingsTabs
                  .find((tab) => tab.id === issue.tab)
                  ?.label.toLowerCase()}</button
              >
            {/each}
          </div>
        {/if}
        <details class="command-details" open={showAllSettings}>
          <summary>Command preview</summary>
          <CommandPlanPreview request={commandRequest} {disabled} />
        </details>
        {#if !desktop}<p class="disabled-reason">Encoding requires the desktop app.</p>
        {:else if !connected}<p class="disabled-reason">Connecting to the job runtime…</p>
        {:else if !usableSource || !videos.length}<p class="disabled-reason">
            Choose a local source with a video stream.
          </p>
        {:else if !toolsReady}<p class="disabled-reason">
            Install FFmpeg, FFprobe, standalone {options.name}{backend === 'av1an'
              ? ', and av1an'
              : ''}{chunked && (encoder === 'x264' || av1an.concatMethod === 'mkvmerge')
              ? ', and mkvmerge'
              : ''}, then refresh Tools & settings.
          </p>
        {:else if !selectedVideoSupported}<p class="disabled-reason">
            av1an requires the first video track. Use Quick Convert for another video track.
          </p>
        {:else if hdrUnsupported}<p class="disabled-reason">
            {options.name} supports SDR sources only. Choose SVT-AV1-HDR for compatible HDR10 video.
          </p>
        {:else if !validRate(rate)}<p class="disabled-reason">
            Enter a valid whole-number bitrate or target size above.
          </p>
        {:else if !validSettings}<p class="disabled-reason">
            Use {isSvtEncoder(encoder) ? 'quarter-step' : 'whole-number'} CRF {options.crfMin}–{options.crfMax},
            a listed preset{isSvtEncoder(encoder) ? ', and grain 0–50' : ''}{encoder ===
            'svtAv1FiveFish'
              ? '; lineart and texture bias 0–7'
              : ''}{chunked ? '; parallel chunks 1–64' : ''}.
          </p>
        {:else if !framingValid}<p class="disabled-reason">
            {framingResult.error}
          </p>
        {:else if externalIssue}<p class="disabled-reason" role="alert">{externalIssue}</p>
        {:else if layoutIssue}<p class="disabled-reason" role="alert">{layoutIssue}</p>
        {:else if timecodeIssue}<p class="disabled-reason" role="alert">{timecodeIssue}</p>
        {:else if subtitleIssue}<p class="disabled-reason" role="alert">{subtitleIssue}</p>
        {:else if !validAudio(audio, included, file?.streams ?? [])}<p class="disabled-reason">
            Check the selected audio codec, channels, and bitrate. Any compatibility issue is shown
            beside its track.
          </p>
        {:else if active}<p class="disabled-reason">
            A job is active. Add this encode to the queue to run it next.
          </p>{/if}
      </div>
    </aside>
  </div>
</section>

<style>
  .settings-toolbar {
    display: flex;
    flex-wrap: wrap;
    align-items: center;
    justify-content: space-between;
    gap: 6px 12px;
    min-height: 36px;
  }
  .settings-tabs {
    display: flex;
    gap: 3px;
    flex-wrap: wrap;
  }
  .settings-tabs[hidden],
  .settings-section[hidden] {
    display: none;
  }
  .settings-tabs button {
    border: 1px solid transparent;
    border-radius: 5px;
    background: transparent;
    padding: 7px 10px;
    font-size: 12px;
    color: var(--muted-foreground);
  }
  .settings-tabs button:hover {
    background: var(--secondary);
  }
  .settings-tabs button[aria-selected='true'] {
    background: var(--card);
    border-color: var(--border);
    color: var(--foreground);
    font-weight: 600;
  }
  .settings-view {
    font-size: 11px;
  }
  .tab-warning,
  .settings-issues {
    color: var(--destructive);
  }
  .tab-warning {
    margin-left: 5px;
    font-weight: 700;
  }
  .settings-issues {
    display: grid;
    gap: 5px;
    font-size: 11px;
  }
  .settings-issues button {
    justify-self: start;
  }
  .filter-fields {
    padding: 14px;
  }
  .compatibility-note {
    grid-column: 1 / -1;
    margin-bottom: 12px;
    color: var(--muted-foreground);
    font-size: 11px;
  }
  .compatibility-note summary {
    cursor: pointer;
    width: fit-content;
    padding: 4px 0;
  }
  .compatibility-note p {
    max-width: 110ch;
    margin-top: 6px;
  }
  .source-fields {
    display: grid;
    grid-template-columns: repeat(2, minmax(0, 1fr));
    gap: 12px 16px;
  }
  .video-adjustments {
    display: grid;
    grid-template-columns: repeat(auto-fit, minmax(min(100%, 28rem), 1fr));
    align-items: start;
    gap: 8px;
    min-width: 0;
  }
  .video-adjustments > :global(*) {
    min-width: 0;
    margin: 0;
  }
  .video-adjustments > :global(details[open]),
  .video-adjustments > :global(:has(input:checked)),
  .video-adjustments > :global(:has(button[aria-expanded='true'])) {
    grid-column: 1 / -1;
  }
  .video-adjustments > :global(.trim-options) {
    padding: 11px 12px;
    border: 1px solid var(--border);
    border-radius: 8px;
  }
  .output-panel {
    position: sticky;
    top: 12px;
    min-width: 0;
  }
  .output-content {
    gap: 11px;
  }
  .output-source {
    padding: 10px 11px;
    border: 1px solid var(--rule);
    border-radius: 8px;
    background: var(--background);
  }
  .output-source .eyebrow {
    grid-column: 1;
    grid-row: 1;
    color: var(--muted-foreground);
  }
  .output-source strong {
    grid-column: 1 / -1;
    grid-row: 2;
    color: var(--foreground);
    font-size: 12px;
    line-height: 1.35;
  }
  .source-details {
    grid-column: 1 / -1;
    grid-row: 3;
    color: var(--muted-foreground);
    font-size: 10px;
    font-variant-numeric: tabular-nums;
  }
  .destination-field label {
    color: var(--foreground);
    font-size: 12px;
    font-weight: 700;
  }
  .destination-control input {
    height: 36px;
    border-radius: 7px;
    background: var(--card);
    color: var(--foreground);
    font-size: 11px;
  }
  .encode-actions {
    display: grid;
    grid-template-columns: minmax(0, 1fr) auto;
    gap: 8px;
  }
  .encode-actions :global(.start-encode) {
    width: 100%;
    min-width: 0;
    height: 38px;
    font-weight: 700;
  }
  .output-summary {
    display: block;
    padding: 10px 0 0;
    border-block: 0;
    border-top: 1px solid var(--rule);
  }
  .output-summary summary,
  .command-details > summary {
    cursor: pointer;
    font-size: 11px;
    color: var(--foreground);
  }
  .output-summary p,
  .command-details[open] > :global(section) {
    margin-top: 8px;
  }
  .output-summary .summary-label {
    color: var(--muted-foreground);
    font-size: 10px;
    font-weight: 700;
    letter-spacing: 1px;
    text-transform: uppercase;
  }
  .output-summary span:not(.summary-label) {
    font-size: 11px;
    line-height: 1.4;
  }
  .destination-control {
    display: flex;
    align-items: center;
    gap: 6px;
  }
  .destination-control input {
    min-width: 0;
    flex: 1;
  }
  .copy-note {
    padding: 10px 14px 0;
    font-size: 12px;
  }
  .copy-streams {
    display: grid;
    grid-template-columns: repeat(auto-fit, minmax(min(100%, 20rem), 1fr));
    align-items: start;
    gap: 8px 16px;
    padding: 6px 14px 14px;
    max-height: 360px;
    overflow-y: auto;
  }
  .source-track {
    min-width: 0;
  }
  .copy-stream {
    display: flex;
    align-items: center;
    gap: 8px;
    padding: 8px 0;
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
    font-size: 12px;
  }
  .copy-stream small {
    font-size: 11px;
    margin-top: 2px;
  }
  .output-source strong {
    overflow-wrap: anywhere;
  }
  .output-source {
    display: grid;
    grid-template-columns: minmax(0, 1fr) auto;
    gap: 3px 8px;
    align-items: center;
  }
  .output-source .eyebrow {
    grid-column: 1;
  }
  .output-source strong,
  .output-source .text-button {
    margin-top: 0;
  }
  .output-source .text-button {
    grid-column: 2;
    grid-row: 1;
    justify-self: end;
    align-self: center;
    font-size: 10px;
  }
  @media (max-width: 850px) {
    .encode-actions {
      grid-template-columns: minmax(0, 1fr);
    }
  }
  .text-button:disabled {
    opacity: 0.5;
    cursor: default;
  }
</style>
