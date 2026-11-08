<script lang="ts">
  import AdvancedEncoderOptions from './AdvancedEncoderOptions.svelte';
  import {
    geometryReadouts,
    resizePresetLabels,
    resizeSourceDescription,
    resizeDialogDescription,
  } from './source-geometry';
  import { parseSourceHdr } from './source-advanced';
  import SourceDialog from './SourceDialog.svelte';
  import EditCommandDialog from './EditCommandDialog.svelte';
  import SharedCutDialog from './SharedCutDialog.svelte';
  import { onDestroy, tick } from 'svelte';
  import {
    previewEncodeBatch,
    listAv1anRecoveries,
    av1anRecoveryAction,
    detectCrop,
    resolveOutputPath,
    resolveBatchOutputPaths,
    prepareCommandEdit,
    confirmCommandEdit,
    cancelCommandEdit,
    setCommandEditPaused,
    waitForJobTerminal,
    setJobPaused,
    resolveDeinterlace,
    readAv1anGrainTable,
    makeAv1anGrainPreset,
    inspectUtilityCapabilities,
    chooseUtilityFile,
  } from '$lib/ipc/client';
  import { batchRunId } from '$lib/source-workspace';
  import type {
    FileListMode,
    TrackSelection,
    RegisterWorkspaceAction,
    BatchRowStatus,
  } from '$lib/source-workspace';
  import CommandPlanPreview from './CommandPlanPreview.svelte';
  import { parameterError } from './encoder-parameters';
  import type { EncoderParameter } from '$lib/ipc/generated';
  import type { EncoderParameterCatalog } from '$lib/ipc/generated';
  import Av1anOptionsControl from './Av1anOptions.svelte';
  import Av1anResources from './Av1anResources.svelte';
  import Av1anGrain from './Av1anGrain.svelte';
  import Av1anFilters from './Av1anFilters.svelte';
  import OutputPixelFormat from './OutputPixelFormat.svelte';
  import { allowedOutputPixelFormats } from './output-pixel-format';
  import type { Av1anPixelFormat } from '$lib/ipc/generated';
  import {
    defaultAv1anAudio,
    readAv1anPreferences,
    saveAv1anAudioState,
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
  import { untrack, type Snippet } from 'svelte';
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
    storableSubtitleMode,
    subtitleError,
    subtitleSummary,
  } from './subtitle-options';
  import {
    adjustedOutputStem,
    adjustedOutputTitle,
    type OutputPictureChange,
  } from './output-title';
  import type { SubtitleTrackSettings } from '$lib/ipc/generated';
  import { terminalJob } from './job-state';
  import FramingOptions from './FramingOptions.svelte';
  import TrimOptions from './TrimOptions.svelte';
  import ToneMapOptions from './ToneMapOptions.svelte';
  import RateControlOptions from './RateControlOptions.svelte';
  import HdrMetadataOptions from './HdrMetadataOptions.svelte';
  import {
    supportsHdrMetadata,
    supportsDynamicHdr,
    validHdrMetadata,
  } from './hdr-metadata-options';
  import type { HdrMetadataSettings } from '$lib/ipc/generated';
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
    preservableHdr10,
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
    ContainerFormat,
    ImageOutput,
    ImageExportMetadata,
    ImageSubtitleSource,
    OutputTarget,
    CommandEditRequest,
    CommandEditResult,
    ImageRequest,
  } from '$lib/ipc/generated';

  let {
    backend,
    file: mainFile,
    files,
    tools,
    toolsRevision = 0,
    toolsLoading = false,
    jobs,
    connected,
    onfiles,
    onstart,
    onqueue,
    sourcePicker,
    tracks = [],
    fileMode = 'mux',
    batchNameTemplate = '{name}_{codec}',
    registerAction,
    onbatchqueue,
    sourcePreferences,
    onlog,
    onrecoverysource,
    onrecoveryjobs,
    onbatchstatus,
    onreviewjob,
    ontracklist,
    isShiftHeld: readShiftState,
  }: {
    backend: EncodeBackend;
    file: MediaFile | undefined;
    files: MediaFile[];
    tools: ToolInfo[];
    toolsRevision?: number;
    toolsLoading?: boolean;
    jobs: JobSnapshot[];
    connected: boolean;
    onfiles: () => void;
    onstart: (request: EncodeRequest) => Promise<void>;
    onqueue: (request: EncodeRequest) => Promise<void>;
    sourcePicker?: Snippet;
    tracks?: TrackSelection[];
    fileMode?: FileListMode;
    batchNameTemplate?: string;
    registerAction?: RegisterWorkspaceAction;
    onbatchqueue?: (requests: EncodeRequest[]) => Promise<void>;
    sourcePreferences?: import('$lib/ipc/generated').GeneralPreferences;
    onlog?: (message: string) => void;
    onrecoverysource?: (path: string) => Promise<void>;
    onrecoveryjobs?: (before: JobSnapshot[], after: JobSnapshot[]) => void;
    onbatchstatus?: (
      runId: string,
      statuses: Record<string, BatchRowStatus>,
      begin?: boolean,
    ) => void;
    onreviewjob?: (job: JobSnapshot) => void;
    ontracklist?: () => void;
    isShiftHeld?: () => boolean;
  } = $props();
  // Quick Convert encodes the checked video source; the loaded main file remains
  // the source of the fork's subtitle-burn selector and metadata defaults.
  const file = $derived.by(() => {
    if (backend === 'av1an' || fileMode === 'batch') return mainFile;
    const video = tracks.find(
      (row) =>
        row.included &&
        files
          .find((source) => source.id === row.sourceId)
          ?.streams.some((stream) => stream.index === row.streamIndex && stream.kind === 'video'),
    );
    return (video ? files.find((source) => source.id === video.sourceId) : undefined) ?? mainFile;
  });
  let videoIndex = $state<number | undefined>();
  let videoMode = $state<'encode' | 'copy' | 'drop'>('encode');
  let selectedEncoder = $state<VideoEncoder>('svtAv1');
  // Editable historical drafts receive a visible CPU default; stored jobs are
  // still admitted with their original settings and reject hardware encoding.
  $effect(() => {
    if (['h264Nvenc', 'hevcNvenc'].includes(selectedEncoder))
      untrack(() => {
        selectedEncoder = 'svtAv1';
        crf = 30;
        preset = 4;
        outputPixelFormat = 'yuv420p10le';
        rate = defaultRate();
      });
  });
  let parameters = $state<EncoderParameter[]>([]);
  let parameterCatalogs = $state<Record<string, EncoderParameterCatalog>>({});
  $effect(() => {
    toolsRevision;
    untrack(() => {
      parameterCatalogs = {};
    });
  });
  let rate = $state(defaultRate());
  let hdrMetadata = $state<HdrMetadataSettings | undefined>();
  let rawHdr = $state<Record<string, string>>({});
  let gridGrain = $state<{ path: string; table: string } | null>(null),
    gridGrainError = $state(''),
    gridGrainPending = $state(false),
    gridGrainGeneration = 0;
  function updateRaw(value: Record<string, string>) {
    if ((value['fgs-table'] ?? '') !== (rawHdr['fgs-table'] ?? '')) {
      gridGrain = null;
      gridGrainError = '';
      gridGrainPending = false;
      ++gridGrainGeneration;
    }
    rawHdr = value;
  }
  async function resolveGridResource(name: string, path: string) {
    if (name !== 'fgs-table') return;
    const wanted = path.trim(),
      generation = ++gridGrainGeneration;
    gridGrain = null;
    gridGrainError = '';
    if (!wanted) {
      gridGrainPending = false;
      return;
    }
    gridGrainPending = true;
    try {
      const table = await readAv1anGrainTable(wanted);
      if (generation === gridGrainGeneration && rawHdr['fgs-table']?.trim() === wanted)
        gridGrain = { path: wanted, table };
    } catch (cause) {
      if (generation === gridGrainGeneration) gridGrainError = errorMessage(cause);
    } finally {
      if (generation === gridGrainGeneration) gridGrainPending = false;
    }
  }

  const parsedHdr = $derived(parseSourceHdr(rawHdr));
  $effect(() => {
    const parsed = parsedHdr;
    untrack(() => (hdrMetadata = parsed.value));
  });
  const initialAv1anPreferences = untrack(() => readAv1anPreferences(true));
  let av1an = $state(defaultAv1an(true, true));
  let av1anGrain = $state<Av1anGrainSettings | undefined>();
  let av1anFilters = $state<string[]>(
    untrack(() => backend) === 'av1an' ? [...initialAv1anPreferences.filters] : [],
  );
  let outputPixelFormat = $state<Av1anPixelFormat | undefined>('yuv420p10le');
  let crf = $state<number | undefined>(30);
  let preset = $state(4);
  let lastPreferenceSaveProblem = '';
  function reportPreferenceSave(problem?: string) {
    if (problem && problem !== lastPreferenceSaveProblem) onlog?.(problem);
    lastPreferenceSaveProblem = problem ?? '';
  }
  let workerBaseline = initialAv1anPreferences.workerBaseline;
  let workers = $state<number | undefined>(initialAv1anPreferences.workers);
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
  // The suggested output stem before picture adjustments, while the path is
  // unedited. Typing or browsing for a path clears it.
  let suggestedStem = $state<string | null>(null);
  // Prevent a slow filesystem lookup or picker from overwriting a newer edit.
  let destinationRevision = 0;
  let suggestionPending = $state(false);
  let suggestionTask: Promise<void> | undefined;
  let error = $state<string | null>(null);
  let submitting = $state(false);
  let imageReviewId = $state<string | null>(null),
    imagePaused = $state(false),
    imagePausePending = $state(false);
  let commandDialogOpen = $state(false),
    commandDialogText = $state(''),
    commandDialogProgram = $state<'ffmpeg' | 'av1an'>('ffmpeg');
  let finishCommandDialog: ((value: string | null) => void) | undefined;
  function editCommand(program: 'ffmpeg' | 'av1an', command: string): Promise<string | null> {
    commandDialogProgram = program;
    commandDialogText = command;
    commandDialogOpen = true;
    return new Promise((resolve) => {
      finishCommandDialog = resolve;
    });
  }
  function closeCommandDialog(confirm = false) {
    const value = confirm ? commandDialogText.trim() : '';
    commandDialogOpen = false;
    const finish = finishCommandDialog;
    finishCommandDialog = undefined;
    finish?.(value || null);
  }
  onDestroy(() => {
    imageAbort?.abort();
    closeCommandDialog();
  });
  async function toggleImagePause(id: string, paused: boolean) {
    if (imagePausePending || imageReviewId !== id) return;
    imagePausePending = true;
    try {
      await setCommandEditPaused(id, !paused);
      if (imageReviewId === id) {
        imagePaused = !paused;
        error = null;
      }
    } catch (cause) {
      if (imageReviewId === id && !imageAbort?.signal.aborted) {
        error = errorMessage(cause);
        onlog?.(`Could not ${paused ? 'resume' : 'pause'} image output: ${error}`);
      }
    } finally {
      if (imageReviewId === id && !imageAbort?.signal.aborted) imagePausePending = false;
    }
  }
  async function reviewAndRun(
    request: CommandEditRequest,
    signal: AbortSignal,
    shouldEdit: () => boolean = () => true,
  ): Promise<CommandEditResult | null> {
    let id: string | undefined, trackedImageId: string | undefined;
    try {
      const review = await prepareCommandEdit(request, signal);
      id = review.id;
      if (review.job) onreviewjob?.(review.job);
      if (signal.aborted) return null;
      if (request.kind === 'image') {
        trackedImageId = review.id;
        imageReviewId = review.id;
        imagePaused = false;
        imagePausePending = false;
      }
      let args: string | null = review.arguments;
      if (shouldEdit()) {
        const cancelDialog = () => closeCommandDialog();
        signal.addEventListener('abort', cancelDialog, { once: true });
        try {
          args = await editCommand(
            review.executableLabel.toLowerCase().includes('av1an') ? 'av1an' : 'ffmpeg',
            review.arguments,
          );
        } finally {
          signal.removeEventListener('abort', cancelDialog);
        }
      }
      if (!args || signal.aborted) return null;
      const result = await confirmCommandEdit({ id: review.id, arguments: args }, signal);
      id = undefined;
      if (result.kind === 'encode') onreviewjob?.(result.job);
      return result;
    } finally {
      if (trackedImageId && imageReviewId === trackedImageId) {
        imageReviewId = null;
        imagePaused = false;
        imagePausePending = false;
      }
      if (id) await cancelCommandEdit(id).catch(() => {});
    }
  }
  let temporal = $state({
    ...defaultTemporal(),
    resizeFilter: 'bicubic',
    qtgmcPreset: 'verySlow',
  } as TemporalDraft);
  const quickTabs = [
    { id: 'video', label: 'Video' },
    { id: 'audio', label: 'Audio' },
    { id: 'subtitles', label: 'Subtitles' },
    { id: 'metadata', label: 'Metadata' },
    { id: 'advanced', label: 'Advanced' },
  ] as const;
  const av1anTabs = [
    { id: 'video', label: 'Video' },
    { id: 'audio', label: 'Audio & Tracks' },
    { id: 'av1an', label: 'Av1an Options' },
    { id: 'advanced', label: 'Advanced' },
  ] as const;
  let settingsTab = $state('video');
  let dialog = $state<'crop' | 'resize' | 'trim' | 'audio' | null>(null);
  let framingBackup: FramingDraft | undefined,
    temporalBackup: TemporalDraft | undefined,
    resizeChoiceBackup = '',
    audioBackup: AudioTrackDraft[] | undefined;
  function openEdit(kind: 'crop' | 'resize' | 'audio') {
    framingBackup = copyFramingDraft(framing);
    temporalBackup = { ...temporal };
    resizeChoiceBackup = resizeChoice;
    audioBackup = audio.map((t) => ({ ...t }));
    perTrackBackup = perTrackRows.map((t) => ({ ...t }));
    if (kind === 'audio') preparePerTrack();
    if (kind === 'crop') clampCropOnOpen();
    dialog = kind;
  }
  function cancelEdit() {
    if (framingBackup) framing = copyFramingDraft(framingBackup);
    if (temporalBackup) temporal = { ...temporalBackup };
    if (dialog === 'resize') resizeChoice = resizeChoiceBackup;
    if (dialog === 'audio' && audioBackup) {
      audio = audioBackup.map((t) => ({ ...t }));
      perTrackRows = perTrackBackup.map((t) => ({ ...t }));
      if (configuredAudioIdentity !== audioIdentity()) audioConfig = 'same';
    }
    dialog = null;
  }

  let cropMode = $state('disabled');
  let cropReduced = $state(false);
  function clampCropOnOpen() {
    const crop = { ...framing.crop };
    cropReduced = false;
    for (const [a, b, total] of [
      ['left', 'right', selectedVideo?.width],
      ['top', 'bottom', selectedVideo?.height],
    ] as const) {
      if (!total) continue;
      const limit = Math.max(0, total - 2),
        x = Math.max(0, crop[a] ?? 0),
        y = Math.max(0, crop[b] ?? 0);
      if (x + y > limit) {
        crop[a] = Math.floor((x * limit) / (x + y));
        crop[b] = limit - crop[a]!;
        cropReduced = true;
      }
    }
    framing = { ...framing, crop };
  }
  function editCrop(edge: 'left' | 'right' | 'top' | 'bottom', value: number) {
    const crop = { ...framing.crop },
      horizontal = edge === 'left' || edge === 'right',
      other =
        edge === 'left' ? 'right' : edge === 'right' ? 'left' : edge === 'top' ? 'bottom' : 'top',
      total = horizontal ? selectedVideo?.width : selectedVideo?.height,
      limit = total ? Math.max(0, total - 2) : 16384;
    crop[edge] = Math.min(limit, Math.max(0, Math.trunc(value || 0)));
    if (total) crop[other] = Math.min(crop[other] ?? 0, limit - crop[edge]!);
    cropReduced = false;
    framing = { ...framing, crop };
  }
  function alignedFraming(
    draft: FramingDraft,
    stream: import('$lib/ipc/generated').MediaStream | undefined,
    active = true,
  ) {
    const copy = copyFramingDraft(draft);
    if (!active) {
      copy.crop = { left: 0, right: 0, top: 0, bottom: 0 };
      return copy;
    }
    for (const [a, b, total] of [
      ['left', 'right', stream?.width],
      ['top', 'bottom', stream?.height],
    ] as const) {
      const first = copy.crop[a] ?? 0,
        far = copy.crop[b] ?? 0;
      if (total) {
        const offset = Math.min(
          Math.ceil(first / 2) * 2,
          Math.max(0, Math.floor((total - 2) / 2) * 2),
        );
        const size = Math.max(2, Math.floor((total - offset - far) / 2) * 2);
        copy.crop[a] = offset;
        copy.crop[b] = total - offset - size;
      } else {
        copy.crop[a] = Math.ceil(first / 2) * 2;
        copy.crop[b] = Math.ceil(far / 2) * 2;
      }
    }
    return copy;
  }

  let resizeChoice = $state('none');
  let audioCodec = $state<AudioTrackDraft['codec']>(
    untrack(() => backend) === 'av1an' ? (initialAv1anPreferences.audio?.codec ?? 'opus') : 'opus',
  );
  let audioChannels = $state<AudioTrackDraft['channels']>(
    untrack(() => backend) === 'av1an'
      ? initialAv1anPreferences.audio?.codec === 'copy'
        ? 'preserve'
        : (initialAv1anPreferences.audio?.channels ?? 'stereo')
      : 'stereo',
  );
  let audioBitrate = $state(
    untrack(() => backend) === 'av1an' ? (initialAv1anPreferences.audio?.bitrateKbps ?? 128) : 128,
  );
  let audioConfig = $state('same');
  let configuredAudioIdentity: string | null = null;
  const audioIdentity = () =>
    mainFile ? JSON.stringify([mainFile.path, mainFile.sizeBytes]) : null;
  type PerTrackRow = {
    sourceId: string;
    sourcePath: string;
    streamIndex: number;
    channels: number;
    bitrate: number;
  };
  let perTrackRows = $state<PerTrackRow[]>([]),
    perTrackBackup: PerTrackRow[] = [];
  const channelCount = (value: AudioTrackDraft['channels'], source: number | null) =>
    typeof value === 'object'
      ? value.explicit.count
      : value === 'mono'
        ? 1
        : value === 'stereo'
          ? 2
          : value === 'surround51'
            ? 6
            : value === 'surround71'
              ? 8
              : (source ?? 2);
  const audioMultiplier = (n: number) => (n === 1 ? 0.5 : n > 6 ? 2.5 : n > 4 ? 2 : 1);
  function preparePerTrack() {
    const rows = tracks.length
      ? tracks.map((row) => ({
          source: files.find((f) => f.id === row.sourceId),
          index: row.streamIndex,
        }))
      : (file?.streams ?? []).map((stream) => ({ source: file, index: stream.index }));
    perTrackRows = rows.flatMap(({ source, index }) => {
      const stream = source?.streams.find((s) => s.index === index);
      if (!source || stream?.kind !== 'audio') return [];
      const previous =
        configuredAudioIdentity === audioIdentity()
          ? perTrackRows.find((row) => row.sourcePath === source.path && row.streamIndex === index)
          : undefined;
      const count = channelCount(audioChannels, stream.channels);
      return [
        previous
          ? { ...previous, sourceId: source.id }
          : {
              sourceId: source.id,
              sourcePath: source.path,
              streamIndex: index,
              channels: count,
              bitrate: Math.round(audioBitrate * audioMultiplier(count)),
            },
      ];
    });
  }
  function audioFor(source: MediaFile, streamIndex: number) {
    const stream = source.streams.find((s) => s.index === streamIndex);
    const row =
      audioConfig === 'perTrack' && configuredAudioIdentity === audioIdentity()
        ? perTrackRows.find((r) => r.sourcePath === source.path && r.streamIndex === streamIndex)
        : undefined;
    const count = row?.channels;
    const channels: AudioTrackDraft['channels'] =
      audioCodec === 'copy'
        ? 'preserve'
        : count === undefined
          ? audioChannels
          : {
              explicit: {
                count,
                layout:
                  count === stream?.channels && !!stream.channelLayout ? 'source' : 'ffmpegDefault',
              },
            };
    const bitrate =
      row?.bitrate ??
      Math.round(audioBitrate * audioMultiplier(channelCount(channels, stream?.channels ?? null)));
    return {
      streamIndex,
      codec: audioCodec,
      channels,
      bitrateKbps: bitrate,
      ...(loudness !== 'off' && audioCodec !== 'copy'
        ? {
            normalization: { target: `lufs${loudness.slice(1)}` as 'lufs14' | 'lufs16' | 'lufs23' },
          }
        : {}),
    };
  }

  let loudness = $state('off');
  let fps = $state('');
  let deinterlaceChoice = $state<'automatic' | 'disabled' | 'qtgmc' | 'bwdif' | 'yadif'>(
    untrack(() => backend) === 'av1an' ? 'automatic' : 'qtgmc',
  );
  let deinterlaceDouble = $state(true);
  let subtitleMode = $state<SubtitleTrackSettings['mode']>('copy');
  let burnIndex = $state('');
  const activeBurn = $derived(videoMode === 'encode' && backend === 'standalone' ? burnIndex : '');
  function burnLabel(stream: import('$lib/ipc/generated').MediaStream, index: number) {
    const trim = (value: string, limit: number) =>
      value.length > limit ? value.slice(0, limit) + '…' : value;
    const codec = (stream.codec ?? '').toLowerCase();
    const nice = codec.startsWith('hdmv_pgs')
      ? 'PGS'
      : codec.startsWith('subrip')
        ? 'SRT'
        : codec.startsWith('dvd_subtitle')
          ? 'DVD Subtitles'
          : codec === 'webvtt'
            ? 'WebVTT'
            : codec === 'dvb_subtitle'
              ? 'DVB Subtitles'
              : codec.length < 5
                ? codec.toUpperCase()
                : codec.replace(/\b\w/g, (c) => c.toUpperCase());
    const number = index + (sourcePreferences?.zeroIndexedStreams ? 0 : 1);
    return `${['#' + String(number).padStart(2, '0'), trim((stream.language ?? '').toUpperCase(), 6), trim(stream.title ?? '', 25)].filter(Boolean).join(' - ')} (${trim(nice, 12)})`;
  }

  let grainMode = $state('disabled');
  const gridGrainIssue = $derived(
    rawHdr['fgs-table']?.trim()
      ? gridGrainError ||
          (gridGrainPending
            ? 'Reading grain table…'
            : !gridGrain
              ? 'Enter a valid grain table path and leave the field.'
              : null)
      : null,
  );

  let grainPath = $state(''),
    grainPreset = $state(''),
    grainPresets = $state<string[]>([]),
    grainDenoise = $state(false),
    grainStrength = $state(4);
  async function loadGrainTable(path: string) {
    try {
      const table = await readAv1anGrainTable(path);
      av1anGrain = { table, denoise: grainDenoise, denoiseStrength: grainStrength };
      filmGrain = 0;
    } catch (cause) {
      error = errorMessage(cause);
    }
  }
  async function chooseGrainTable() {
    const path = await chooseUtilityFile('Choose AV1 grain table', ['tbl', 'txt']);
    if (path) {
      grainPath = path;
      await loadGrainTable(path);
    }
  }
  async function loadGrainPresets() {
    try {
      const capabilities = await inspectUtilityCapabilities();
      grainPresets = capabilities.grainPresets;
      grainPreset ||= grainPresets[0] ?? '';
      if (grainPreset) await generateGrainPreset();
    } catch (cause) {
      error = errorMessage(cause);
    }
  }
  async function generateGrainPreset() {
    try {
      const table = await makeAv1anGrainPreset(grainPreset);
      av1anGrain = { table, denoise: grainDenoise, denoiseStrength: grainStrength };
      filmGrain = 0;
    } catch (cause) {
      error = errorMessage(cause);
    }
  }
  function updateGrain() {
    av1anGrain = {
      table: grainMode === 'encoder' ? null : (av1anGrain?.table ?? null),
      denoise: grainDenoise,
      denoiseStrength: grainStrength,
    };
  }

  let outputContainer = $state<ContainerFormat>('matroska');
  let videoChoice = $state('svtAv1');
  let imageQuality = $state(128);
  let imagePixel = $state<import('$lib/ipc/generated').ImagePixelFormat>('rgb24');
  let imageAbort: AbortController | undefined,
    imageProgress = $state(0),
    imageProgressDescription = $state(''),
    batchCurrentJobId = $state<string | null>(null);
  const batchCurrentJob = $derived(jobs.find((job) => job.id === batchCurrentJobId));
  const isImage = $derived(['gif', 'png', 'jpg'].includes(videoChoice));
  let metadataSource = $state('');
  let chapterSource = $state('');
  let applyMetadata = $state(true);
  let metadataEdits = $state<Record<string, { title: string; language: string }>>({});
  let outputMetadataEdits = $state<Record<string, { title: string; language: string }>>({});
  function outputMetadataValue(source: MediaFile) {
    return (
      outputMetadataEdits[source.id] ?? {
        title: adjustedOutputTitle(source.title ?? '', outputPictureChange),
        language: source.language ?? '',
      }
    );
  }
  function editOutputMetadata(
    source: MediaFile,
    patch: Partial<{ title: string; language: string }>,
  ) {
    outputMetadataEdits = {
      ...outputMetadataEdits,
      [source.id]: { ...outputMetadataValue(source), ...patch },
    };
  }
  $effect(() => {
    const live = new Set(files.map((source) => source.id));
    untrack(() => {
      const next = Object.fromEntries(
        Object.entries(outputMetadataEdits).filter(([id]) => live.has(id)),
      );
      if (Object.keys(next).length !== Object.keys(outputMetadataEdits).length)
        outputMetadataEdits = next;
    });
  });
  let metadataOptionsKey = '';
  $effect(() => {
    const options =
        fileMode === 'batch'
          ? files.length
            ? ['current']
            : []
          : files.map((source) => source.path),
      key = JSON.stringify([fileMode, options]);
    untrack(() => {
      if (key === metadataOptionsKey) return;
      metadataOptionsKey = key;
      metadataSource = options[0] ?? '';
      chapterSource = options[0] ?? '';
    });
  });
  const metadataKey = (row: TrackSelection) => `${row.sourceId}:${row.streamIndex}`;
  function metadataValue(row: TrackSelection) {
    return metadataEdits[metadataKey(row)] ?? { title: row.title, language: row.language };
  }
  function editMetadata(row: TrackSelection, patch: Partial<{ title: string; language: string }>) {
    metadataEdits = { ...metadataEdits, [metadataKey(row)]: { ...metadataValue(row), ...patch } };
  }
  let copySubs = $state(
      untrack(() => backend) === 'av1an' ? initialAv1anPreferences.copySubs : true,
    ),
    copyData = $state(
      untrack(() => backend) === 'av1an' ? initialAv1anPreferences.copyData : false,
    ),
    copyAttachments = $state(
      untrack(() => backend) === 'av1an' ? initialAv1anPreferences.copyAttachments : true,
    );
  let stripAudio = $state(
      untrack(() => backend) === 'av1an' ? initialAv1anPreferences.stripAudio : false,
    ),
    stripSubs = $state(false);
  let selectedFilterIndex = $state<number | null>(null);
  function selectedOptionTooltip(event: Event) {
    const element = event.target;
    if (element instanceof HTMLSelectElement)
      element.title = element.selectedOptions[0]?.text ?? '';
  }
  function optionTooltips(node: HTMLElement) {
    node.addEventListener('mouseover', selectedOptionTooltip);
    node.addEventListener('focusin', selectedOptionTooltip);
    return {
      destroy() {
        node.removeEventListener('mouseover', selectedOptionTooltip);
        node.removeEventListener('focusin', selectedOptionTooltip);
      },
    };
  }
  const tabs = $derived(backend === 'av1an' ? av1anTabs : quickTabs);
  function tabKeys(event: KeyboardEvent, index: number) {
    const next =
      event.key === 'Home'
        ? 0
        : event.key === 'End'
          ? tabs.length - 1
          : event.key === 'ArrowRight'
            ? (index + 1) % tabs.length
            : event.key === 'ArrowLeft'
              ? (index + tabs.length - 1) % tabs.length
              : -1;
    if (next < 0) return;
    event.preventDefault();
    settingsTab = tabs[next].id;
    const tablist = (event.currentTarget as HTMLElement).parentElement;
    void tick().then(() =>
      tablist?.querySelectorAll<HTMLButtonElement>('[role="tab"]')[next]?.focus(),
    );
  }
  function displaySize() {
    const [n, d] = (selectedVideo?.sampleAspectRatio ?? '1:1').split(/[:/]/).map(Number);
    const crop = activeFraming.crop;
    const width = Math.max(
        2,
        (selectedVideo?.width ?? 1920) - (crop.left ?? 0) - (crop.right ?? 0),
      ),
      height = Math.max(2, (selectedVideo?.height ?? 1080) - (crop.top ?? 0) - (crop.bottom ?? 0));
    return {
      width: Math.max(16, Math.round((width * (n > 0 && d > 0 ? n / d : 1)) / 2) * 2),
      height: Math.max(16, height),
    };
  }
  function setResize(value: string) {
    resizeChoice = value;
    if (value === 'none') {
      framing = { ...framing, resizeEnabled: false };
      return;
    }
    if (value === 'custom') {
      framing = {
        ...framing,
        resizeEnabled: true,
        resizeMode: framing.resizeEnabled ? framing.resizeMode : 'fit',
        resizeWidth: framing.resizeEnabled ? (framing.resizeWidth ?? 1920) : 1920,
        resizeHeight: framing.resizeEnabled ? (framing.resizeHeight ?? 1080) : 1080,
      };
      return;
    }
    const dimensions: Record<string, [number, number]> = {
      '2160p': [3840, 2160],
      '1440p': [2560, 1440],
      '1080p': [1920, 1080],
      '720p': [1280, 720],
      '576p': [1024, 576],
      '480p': [854, 480],
      '360p': [640, 360],
    };
    if (dimensions[value])
      framing = {
        ...framing,
        resizeEnabled: true,
        resizeMode: 'fit',
        resizeWidth: dimensions[value][0],
        resizeHeight: dimensions[value][1],
        resizePercent: 50,
        resizeFill: 'pad',
        resizeModulus: 2,
        resizeAllowUpscale: true,
        resizeCorrectAspect: true,
      };
    else
      framing = {
        ...framing,
        resizeEnabled: true,
        resizeMode: 'percent',
        resizeWidth: 1920,
        resizeHeight: 1080,
        resizePercent: Number(value),
        resizeFill: 'pad',
        resizeModulus: 2,
        resizeAllowUpscale: false,
        resizeCorrectAspect: true,
      };
    temporal = { ...temporal, resizeFilter: 'bicubic' };
  }
  function setBorder(value: string) {
    const ratios: Record<string, [number, number]> = {
      '16:9': [16, 9],
      '4:3': [4, 3],
      '1:1': [1, 1],
      '9:16': [9, 16],
      '21:9': [64, 27],
    };
    framing = {
      ...framing,
      borderRatio: value,
      ...(ratios[value]
        ? { borderRatioWidth: ratios[value][0], borderRatioHeight: ratios[value][1] }
        : {}),
    };
  }
  function setFps(value: string) {
    fps = value;
    if (!value.trim()) temporal = { ...temporal, changeRate: false };
    else {
      const parts = value.split('/').map(Number);
      const n = parts[0],
        d = parts[1] ?? 1;
      temporal = {
        ...temporal,
        changeRate: true,
        numerator: Number.isInteger(n) ? n : Math.round(n * 1000),
        denominator: Number.isInteger(n) ? d : d * 1000,
      };
    }
  }
  $effect(() => {
    const mode = subtitleMode,
      burn = videoMode === 'encode' && !chunked && file?.id === mainFile?.id ? burnIndex : '',
      source = file,
      selected = included,
      strip = stripSubs,
      container = outputContainer,
      route = backend;
    untrack(() => {
      subtitles = defaultSubtitles(source?.streams ?? []).map((track) => {
        const retained = selected.includes(track.streamIndex) && !strip;
        const burning = String(track.streamIndex) === burn;
        const stream = source?.streams.find((stream) => stream.index === track.streamIndex);
        return {
          ...track,
          mode:
            burning && !retained ? 'burnIn' : storableSubtitleMode(mode, stream, container, route),
          ...(burning && retained ? { burnIn: true } : {}),
        };
      });
    });
  });
  /** Copied MP4 text tracks that become SubRip because MKV cannot store them. */
  const convertedMovText = $derived(
    subtitles.filter(
      (track) =>
        subtitleMode === 'copy' &&
        track.mode === 'subRip' &&
        included.includes(track.streamIndex) &&
        !stripSubs,
    ).length,
  );
  function setAudio() {
    audio = (file?.streams ?? [])
      .filter((s) => s.kind === 'audio')
      .map((s) => ({
        streamIndex: s.index,
        codec: audioCodec,
        channels: audioChannels,
        bitrateKbps: audioBitrate,
      }));
  }
  $effect(() => {
    audioCodec;
    audioChannels;
    audioBitrate;
    file;
    if (audioConfig === 'same') untrack(setAudio);
  });
  $effect(() => {
    const selected = tracks
      .filter((t) => t.sourceId === file?.id && t.included)
      .map((t) => t.streamIndex);
    if (tracks.length && file)
      untrack(() => {
        included = selected;
        const row = tracks.find(
          (t) =>
            t.sourceId === file.id &&
            t.included &&
            file.streams.some((s) => s.index === t.streamIndex && s.kind === 'video'),
        );
        if (row) videoIndex = row.streamIndex;
      });
  });
  $effect(() => {
    const register = registerAction,
      mode = fileMode,
      imageMode = isImage,
      av1anMode = chunked,
      inFlight = submitting,
      liveBatchJob = batchCurrentJob,
      imageId = imageReviewId,
      imageIsPaused = imagePaused,
      imagePauseBusy = imagePausePending;
    const view = av1anMode ? 'av1an' : 'convert';
    const enabled = imageMode
      ? !disabled &&
        !active &&
        !suggestionPending &&
        connected &&
        !!destination.trim() &&
        !trimIssue &&
        (mode === 'batch'
          ? files.length > 0
          : noVideoForRun ||
            (videoIndex !== undefined &&
              framingValid &&
              !temporalError(temporal, backend) &&
              !toneMapIssue))
      : mode === 'batch'
        ? canQueue && files.length > 0
        : canStart;
    const naming = {
      ...batchNamingValues(),
      extension: outputContainer === 'matroska' ? 'mkv' : outputContainer,
    };
    const progress = imageProgress,
      progressDescription = imageMode || mode === 'batch' ? imageProgressDescription : undefined,
      paused = liveBatchJob ? liveBatchJob.state === 'paused' : imageIsPaused;
    const pause =
      liveBatchJob && ['preparing', 'running', 'paused', 'finalizing'].includes(liveBatchJob.state)
        ? async () => {
            await setJobPaused(liveBatchJob.id, !paused);
          }
        : imageId && !imagePauseBusy
          ? () => toggleImagePause(imageId, imageIsPaused)
          : undefined;
    const action = {
      run: (options?: RunOptions) => start(false, options),
      reset: resetSourceSettings,
      canRun: enabled,
      blockedReason: enabled ? undefined : (runBlockedReason ?? undefined),
      batchNaming: naming,
      label: `${mode === 'batch' ? 'Run Batch' : 'Run'}: ${av1anMode ? 'AV1AN encode' : 'Encode'}`,
      active: inFlight,
      progress,
      progressDescription,
      paused,
      pause,
      stop: () => {
        if (imageReviewId) imagePausePending = true;
        imageAbort?.abort();
      },
    };
    if (register) untrack(() => register(view, action));
  });
  type RunOptions = { editCommand?: boolean; isShiftHeld?: () => boolean };
  // Batch processing checks Shift separately as each file reaches its command,
  // as in the source fork.
  function reviewThisFile(options: RunOptions) {
    return options.isShiftHeld ? options.isShiftHeld() : !!options.editCommand;
  }
  // A single run honors Shift held at the click: preparation can take minutes,
  // and the key is rarely still down when the command is ready.
  function reviewSingleFile(options: RunOptions) {
    return !!options.editCommand || !!options.isShiftHeld?.();
  }
  function batchNamingValues() {
    const codec =
      (
        {
          svtAv1: 'av1',
          aomAv1: 'av1',
          vpxStandalone: 'vp9',
          x264: 'h264',
          x265Standalone: 'h265',
          copy: 'copy',
          drop: 'novideo',
          gif: 'gif',
          png: 'png',
          jpg: 'jpeg',
        } as Record<string, string>
      )[videoChoice] ?? '';
    const quality =
      chunked && av1an.targetEnabled
        ? av1an.target.minimumScoreTenths / 10
        : isImage
          ? imageQuality
          : rate.mode === 'bitrate'
            ? rate.bitrate
            : rate.mode === 'targetSize'
              ? rate.targetSize
              : crf;
    const presetName = options.presets.find((choice) => choice.value === preset)?.label ?? '';
    return {
      codec,
      quality: String(chunked ? (quality ?? 0) : Math.trunc(quality ?? 0)),
      preset:
        isImage || videoChoice === 'copy' || videoChoice === 'drop'
          ? ''
          : presetName.trim().toLowerCase(),
    };
  }
  function localNamingDate() {
    const d = new Date();
    return `${d.getFullYear()}-${String(d.getMonth() + 1).padStart(2, '0')}-${String(d.getDate()).padStart(2, '0')}`;
  }
  onDestroy(() => registerAction?.(backend === 'av1an' ? 'av1an' : 'convert', null));

  let recoveries = $state<import('$lib/ipc/generated').Av1anRecoveryEntry[]>([]),
    recoverySelected = $state<string | null>(null),
    recoveryDialog = $state<'list' | 'clear' | null>(null),
    recoveryBusy = $state(false),
    recoveryError = $state('');
  let recoveryReadGeneration = 0,
    recoveryDisposed = false,
    clearReviewed = $state<string[]>([]),
    clearBytes = $state('0 B');
  function recoveryFormatBytes(value: string) {
    const size = Number(value);
    if (!Number.isFinite(size) || size < 0) return 'N/A B';
    if (size === 0) return '0B';
    const units = ['B', 'KB', 'MB', 'GB', 'TB', 'PB', 'EB'];
    const place = Math.min(units.length - 1, Math.floor(Math.log(size) / Math.log(1024)));
    return `${Math.round((size / 1024 ** place) * 10) / 10} ${units[place]}`;
  }
  function recoveryAge(timestamp: number | null) {
    if (!timestamp || !Number.isFinite(timestamp)) return '???';
    const minutes = (Date.now() - timestamp) / 60000;
    return `${Math.round(minutes >= 120 ? minutes / 60 : minutes)}${minutes >= 120 ? 'h' : 'm'} ago`;
  }
  function recoveryLabel(entry: import('$lib/ipc/generated').Av1anRecoveryEntry) {
    const full = entry.job.request.inputPath.split(/[\\/]/).at(-1) ?? '',
      name = full.length > 35 ? `${full.slice(0, 35)}…` : full;
    return `${name} - ${entry.chunkCount} Chunks - ${recoveryFormatBytes(entry.encodedBytes)} - Created: ${recoveryAge(entry.createdAt)} - Last Run: ${recoveryAge(entry.lastRunAt)}`;
  }
  const recoveryBytes = $derived.by(() => {
    let total = 0n;
    for (const entry of recoveries) {
      try {
        total += BigInt(entry.encodedBytes);
      } catch {
        /* Invalid native size is omitted. */
      }
    }
    return recoveryFormatBytes(total.toString());
  });
  async function refreshRecoveries(preserveSelection = false) {
    const generation = ++recoveryReadGeneration;
    try {
      const result = await listAv1anRecoveries();
      if (recoveryDisposed || generation !== recoveryReadGeneration) return;
      recoveries = result;
      recoverySelected =
        preserveSelection && result.some((entry) => entry.job.id === recoverySelected)
          ? recoverySelected
          : (result[0]?.job.id ?? null);
    } catch (cause) {
      if (!recoveryDisposed && generation === recoveryReadGeneration)
        onlog?.(`Failed to look for resumable encodes: ${errorMessage(cause)}`);
    }
  }
  $effect(() => {
    if (chunked && connected) {
      jobs;
      untrack(() => void refreshRecoveries(true));
    }
  });
  onDestroy(() => {
    recoveryDisposed = true;
    ++recoveryReadGeneration;
  });
  async function openRecoveries() {
    recoveryError = '';
    await refreshRecoveries();
    recoveryDialog = 'list';
  }
  async function openClearTemp() {
    recoveryError = '';
    await refreshRecoveries();
    if (!recoveries.length) {
      onlog?.('There are no av1an temp files to clear.');
      return;
    }
    clearReviewed = recoveries.map((entry) => entry.job.id);
    clearBytes = recoveryBytes;
    recoveryDialog = 'clear';
  }
  function recoveryKeys(node: HTMLElement) {
    function key(event: KeyboardEvent) {
      const target = event.target as HTMLElement;
      if (event.key === 'Enter' && (target === node || target.getAttribute('role') === 'option')) {
        event.preventDefault();
        void recoveryAction('resumeSaved', event.shiftKey);
        return;
      }
      if (!['ArrowDown', 'ArrowUp', 'Home', 'End'].includes(event.key)) return;
      const ids = recoveries.map((entry) => entry.job.id);
      if (!ids.length) return;
      event.preventDefault();
      const current = ids.indexOf(recoverySelected ?? '');
      const next =
        event.key === 'Home'
          ? 0
          : event.key === 'End'
            ? ids.length - 1
            : Math.max(0, Math.min(ids.length - 1, current + (event.key === 'ArrowDown' ? 1 : -1)));
      recoverySelected = ids[next];
      node.querySelectorAll<HTMLElement>('[role=option]')[next]?.focus();
    }
    node.addEventListener('keydown', key);
    return {
      destroy() {
        node.removeEventListener('keydown', key);
      },
    };
  }
  async function recoveryAction(
    action: 'deleteEncode' | 'deleteChunks' | 'resumeSaved' | 'resumeNew' | 'clearAll',
    shiftAtClick = false,
  ) {
    if (recoveryBusy) return;
    const id = recoverySelected;
    if (action !== 'clearAll' && !id) return;
    recoveryBusy = true;
    recoveryError = '';
    let controller: AbortController | undefined;
    try {
      let request: import('$lib/ipc/generated').Av1anRecoveryAction;
      if (action === 'clearAll') request = { action, ids: [...clearReviewed] };
      else if (action === 'resumeNew') {
        const saved = recoveries.find((entry) => entry.job.id === id);
        if (!saved) throw new Error('The selected unfinished encode is no longer available.');
        const path = saved.job.request.inputPath;
        if (file?.path !== path) {
          if (!onrecoverysource)
            throw new Error('Load the saved input file before resuming with new settings.');
          await onrecoverysource(path);
          await tick();
        }
        if (file?.path !== path) throw new Error('The saved input file could not be loaded.');
        while (suggestionPending && suggestionTask) await suggestionTask;
        if (!canQueue)
          throw new Error('The current settings are not valid for the saved input file.');
        const current = currentRequest();
        if (!current)
          throw new Error('The current settings could not be built for the saved input file.');
        request = { action, id: id!, settings: structuredClone($state.snapshot(current.settings)) };
      } else request = { action, id: id! };
      if (request.action === 'resumeSaved' || request.action === 'resumeNew') {
        recoveryDialog = null;
        controller = new AbortController();
        imageAbort = controller;
        submitting = true;
        const command: CommandEditRequest =
          request.action === 'resumeSaved'
            ? { kind: 'resumeSaved', id: request.id }
            : { kind: 'resumeNew', id: request.id, settings: request.settings };
        const reviewed = await reviewAndRun(
          command,
          controller.signal,
          () => shiftAtClick || (readShiftState?.() ?? false),
        );
        if (!reviewed) {
          if (!controller.signal.aborted)
            onlog?.('The command was cleared in the edit window, so nothing was run.');
          await refreshRecoveries();
          return;
        }
        if (reviewed.kind !== 'encode')
          throw new Error('The command returned an unexpected image result.');
        await refreshRecoveries();
        return;
      }
      const before = [...jobs],
        result = await av1anRecoveryAction(request);
      onrecoveryjobs?.(before, result.jobs);
      if (action === 'clearAll') {
        recoveryDialog = null;
        onlog?.(
          `Cleared ${result.jobs.length} av1an temp folder${result.jobs.length === 1 ? '' : 's'}.`,
        );
      }
      await refreshRecoveries();
    } catch (cause) {
      if (!controller?.signal.aborted) {
        recoveryError = errorMessage(cause);
        if (action === 'resumeSaved' || action === 'resumeNew') recoveryDialog = 'list';
        onlog?.(`AV1AN recovery failed: ${recoveryError}`);
      }
    } finally {
      if (controller && imageAbort === controller) {
        imageAbort = undefined;
        submitting = false;
      }
      recoveryBusy = false;
    }
  }
  function resetSourceSettings() {
    const flags = sourcePreferences?.resetOnNewFile ?? {
      trim: true,
      frameRate: false,
      resize: false,
      crop: true,
      borders: false,
      deinterlace: true,
      toneMap: true,
      customFilters: false,
    };
    if (flags.trim) trim = defaultTrim();
    if (flags.frameRate) {
      fps = '';
      temporal = { ...temporal, changeRate: false };
    }
    if (flags.crop) {
      cropMode = 'disabled';
      framing = { ...framing, crop: { top: 0, right: 0, bottom: 0, left: 0 } };
    }
    if (flags.resize) {
      resizeChoice = 'none';
      framing = { ...framing, resizeEnabled: false };
    }
    if (flags.borders) framing = { ...framing, borderRatio: '', bordersEnabled: false };
    if (flags.deinterlace) {
      temporal = { ...temporal, deinterlace: 'off', qtgmcPreset: 'verySlow' };
      deinterlaceChoice = chunked ? 'automatic' : 'qtgmc';
      deinterlaceDouble = true;
    }
    if (flags.toneMap) toneMap = defaultToneMap();
    if (flags.customFilters) av1anFilters = [];
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
    videoMode: 'encode' | 'copy' | 'drop';
    rate: RateDraft;
    hdrMetadata: HdrMetadataSettings | undefined;
    av1an: Av1anDraft;
    av1anGrain: Av1anGrainSettings | undefined;
    av1anFilters: string[];
    outputPixelFormat: Av1anPixelFormat | undefined;
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
  const encodingVideo = $derived(chunked || videoMode === 'encode');
  // An unedited default title follows the output picture: a resized or
  // tone-mapped encode does not keep the source's "2160p ... DV.HDR10+" tokens.
  const outputPictureChange = $derived<OutputPictureChange>({
    toneMapped: encodingVideo && toneMap.enabled,
    ...(encodingVideo && framing.resizeEnabled
      ? {
          height: ['fit', 'height', 'exact'].includes(framing.resizeMode)
            ? (framing.resizeHeight ?? null)
            : null,
        }
      : {}),
  });
  // An unedited default file name follows the output picture like the title.
  $effect(() => {
    const base = suggestedStem,
      change = outputPictureChange;
    if (base !== null) untrack(() => (destination = adjustedOutputStem(base, change)));
  });
  const encoder = $derived(selectedEncoder);
  const parameterCatalog = $derived(
    !toolsLoading ? (parameterCatalogs[`${toolsRevision}:${backend}:${encoder}`] ?? null) : null,
  );
  const parameterIssue = $derived(
    parameters.length && !parameterCatalog
      ? 'Open Advanced encoder parameters to check these overrides against the installed encoder.'
      : parameterError(parameters, parameterCatalog, encoder),
  );
  const options = $derived.by(() => {
    const base = encoderOptions(encoder);
    const numbers = (end: number) =>
      Array.from({ length: end + 1 }, (_, value) => ({ label: String(value), value }));
    const speed = base.presets
      .filter((p) =>
        [
          'veryslow',
          'slower',
          'slow',
          'medium',
          'fast',
          'faster',
          'veryfast',
          'superfast',
          'ultrafast',
        ].includes(p.label),
      )
      .reverse();
    if (encoder === 'svtAv1')
      return {
        ...base,
        crfMin: 0,
        crfMax: 63,
        defaultCrf: chunked ? 35 : 30,
        defaultPreset: 4,
        presets: numbers(12),
      };
    if (encoder === 'aomAv1')
      return {
        ...base,
        crfMin: 0,
        crfMax: 63,
        defaultCrf: chunked ? 20 : 30,
        defaultPreset: 6,
        presets: numbers(chunked ? 6 : 8),
      };
    if (encoder === 'vpxStandalone')
      return {
        ...base,
        crfMin: 0,
        crfMax: chunked ? 50 : 63,
        defaultCrf: chunked ? 20 : 30,
        defaultPreset: 3,
        presets: numbers(chunked ? 6 : 8),
      };
    if (encoder === 'x264' || encoder === 'x265Standalone')
      return {
        ...base,
        crfMin: 0,
        crfMax: 51,
        defaultCrf: encoder === 'x264' ? 20 : chunked ? 20 : 22,
        defaultPreset: 5,
        presets:
          chunked && encoder === 'x265Standalone'
            ? speed.filter((p) => p.label !== 'ultrafast')
            : speed,
      };
    return base;
  });
  const sourceColors = $derived.by(() => {
    const y = (value: Av1anPixelFormat, label: string) => ({ value, label });
    const p420 = y('yuv420p', 'YUV - 4:2:0 - 8 bit'),
      p42010 = y('yuv420p10le', 'YUV - 4:2:0 - 10 bit'),
      p422 = y('yuv422p', 'YUV - 4:2:2 - 8 bit'),
      p42210 = y('yuv422p10le', 'YUV - 4:2:2 - 10 bit'),
      p444 = y('yuv444p', 'YUV - 4:4:4 - 8 bit'),
      p44410 = y('yuv444p10le', 'YUV - 4:4:4 - 10 bit');
    if (encoder === 'svtAv1') return [p420, p42010];
    if (encoder === 'vpxStandalone')
      return [
        p420,
        ...(chunked ? [y('yuva420p', 'YUVA - 4:2:0 - 8 bit')] : []),
        p444,
        p42010,
        p44410,
      ];
    return [p420, p422, p444, p42010, p42210, p44410];
  });
  const metricFields: Record<string, { min: number; max: number; step: number; default: number }> =
    {
      vmaf: { min: 10, max: 99, step: 1, default: 95 },
      ssimulacra2: { min: 30, max: 99, step: 1, default: 80 },
      butteraugli: { min: 0.5, max: 10, step: 0.1, default: 4 },
      xpsnrWeighted: { min: 20, max: 60, step: 0.5, default: 40 },
    };
  const idPrefix = $derived(chunked ? 'av1an' : 'encode');
  const active = $derived(jobs.find((job) => !terminalJob(job.state)));
  const videos = $derived(file?.streams.filter((stream) => stream.kind === 'video') ?? []);
  const selectedVideo = $derived(videos.find((stream) => stream.index === videoIndex));
  const sourceMultipleVideos = $derived(!chunked && videoMode === 'copy');
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
  function cropFits(
    draft: FramingDraft,
    stream: import('$lib/ipc/generated').MediaStream | undefined,
  ) {
    return (
      !stream?.width ||
      !stream.height ||
      (stream.width - (draft.crop.left ?? 0) - (draft.crop.right ?? 0) >= 2 &&
        stream.height - (draft.crop.top ?? 0) - (draft.crop.bottom ?? 0) >= 2)
    );
  }
  const activeFraming = $derived(alignedFraming(framing, selectedVideo, cropMode === 'manual'));
  const cropReadout = $derived.by(() => {
    const c = framing.crop;
    if (!selectedVideo?.width || !selectedVideo.height)
      return `Crop L/R/T/B: ${c.left}/${c.right}/${c.top}/${c.bottom}`;
    const aligned = alignedFraming(framing, selectedVideo).crop,
      x = aligned.left ?? 0,
      y = aligned.top ?? 0,
      w = selectedVideo.width - x - (aligned.right ?? 0),
      h = selectedVideo.height - y - (aligned.bottom ?? 0);
    const rounded =
      x !== c.left ||
      y !== c.top ||
      w !== selectedVideo.width - (c.left ?? 0) - (c.right ?? 0) ||
      h !== selectedVideo.height - (c.top ?? 0) - (c.bottom ?? 0);
    return `${w}x${h} at X ${x}, Y ${y} (source is ${selectedVideo.width}x${selectedVideo.height})${rounded ? ' · rounded to even, which is the only size 4:2:0 video has' : ''}${cropReduced ? ' · reduced to fit this file, which is smaller than the one this crop was set for' : ''}`;
  });
  const geometryStream = $derived(fileMode === 'batch' ? undefined : selectedVideo);
  const geometry = $derived(
    geometryReadouts(
      activeFraming,
      geometryStream,
      fileMode !== 'batch' && !!file,
      cropMode === 'automatic',
      chunked,
    ),
  );
  const resizeLabels = $derived(resizePresetLabels(activeFraming, geometryStream));
  const framingValid = $derived(
    validFramingDraft(activeFraming, selectedVideo) &&
      (cropMode !== 'manual' || cropFits(framing, selectedVideo)),
  );
  const toneMapIssue = $derived(
    toneMapError(toneMap, backend, selectedVideo, isSvtEncoder(encoder) && hdr10Fallback),
  );
  const trimIssue = $derived(
    trimError(
      trim,
      backend,
      audio,
      included,
      file?.streams ?? [],
      subtitles,
      chunked ? 'encode' : videoMode,
    ) ||
      (trim.enabled &&
      !encodingVideo &&
      videoMode === 'drop' &&
      !videos.length &&
      trim.mode === 'frames'
        ? 'Choose time boundaries for an audio-only source.'
        : null),
  );
  const subtitleIssue = $derived(
    subtitleError(
      subtitles,
      included,
      file?.streams ?? [],
      backend,
      selectedVideo,
      toneMap.enabled,
      outputContainer,
    ),
  );
  const rateIssue = $derived(rateEncoderError(rate, encoder));
  const hdrUnsupported = $derived(
    !isSvtEncoder(encoder) &&
      !(
        ['x265', 'x265Standalone', 'aomAv1'].includes(encoder) && preservableHdr10(selectedVideo)
      ) &&
      knownHdr(selectedVideo) &&
      !toneMap.enabled,
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
    (!chunked && fileMode === 'batch') ||
      (!encodingVideo && videoMode === 'drop') ||
      (videos.some((stream) => stream.index === videoIndex) &&
        (!chunked || videoIndex === videos[0]?.index)),
  );
  const copiedStreams = $derived(file?.streams.filter((stream) => stream.kind !== 'video') ?? []);
  const externalIssue = $derived(
    (!encodingVideo &&
    externalTracks.some((choice) => choice.subtitleMode && choice.subtitleMode !== 'copy')
      ? 'Copy or remove external subtitles when converting audio without video encoding.'
      : null) ||
      externalTrackIssue(externalTracks, files, file, {
        trimming: encodingVideo && trim.enabled,
        backend,
        video: selectedVideo,
        toneMapped: encodingVideo && toneMap.enabled,
        primaryBurnCount: encodingVideo
          ? selectedSubtitles(subtitles, included).filter((track) => track.mode === 'burnIn').length
          : 0,
      }),
  );
  const layoutDefaults = $derived(
    defaultTrackOrder(
      file,
      videoMode === 'drop' && !chunked ? undefined : videoIndex,
      included,
      externalTracks,
      files,
    ),
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
    encodingVideo
      ? movTimecodeIssue(movTimecode, files, outputContainer === 'mov', trim.enabled, temporal)
      : movTimecode
        ? 'Remove the selected MOV timecode or return to video encoding.'
        : null,
  );
  const usableSource = $derived(!!file && !file.id.startsWith('jesses-synthetic'));
  const toolsReady = $derived(
    (encodingVideo
      ? requiredEncoderTools(backend, encoder, av1an.concatMethod, true)
      : ['ffmpeg', 'ffprobe']
    ).every((id) => tools.some((tool) => tool.id === id && tool.available)) &&
      (!encodingVideo ||
        encoder !== 'aomAv1' ||
        !preservableHdr10(selectedVideo) ||
        toneMap.enabled ||
        tools.some((tool) => tool.id === 'mkvmerge' && tool.available)),
  );
  const disabled = $derived(!desktop || !usableSource || submitting);
  const dynamicHdrIssue = $derived(
    hdrMetadata &&
      (hdrMetadata.dolbyVisionRpuPath || hdrMetadata.hdr10PlusJsonPath) &&
      (trim.enabled ||
        !!selectedTemporal(temporal) ||
        framingSummary(selectedFraming(activeFraming)) !== 'Source dimensions' ||
        toneMap.enabled ||
        av1anFilters.length > 0 ||
        (chunked && (av1an.splitMethod !== 'fixedChunks' || av1an.maximumChunkFrames !== 0)))
      ? 'Dynamic HDR imports require an untrimmed source without picture or timing changes. For av1an, select fixed chunks and set maximum chunk frames to 0.'
      : null,
  );
  const grainConflict = $derived(
    chunked && isSvtEncoder(encoder) && !gridGrain
      ? av1anGrainConflict(parameters, filmGrain, av1anGrain)
      : null,
  );
  const videoSettingsIssue = $derived.by(() => {
    if (!encodingVideo) return null;
    if (chunked && !isAv1anEncoder(encoder)) return 'Choose an encoder supported by av1an.';
    if (!validRate(rate)) return 'Enter a valid whole-number bitrate or target size.';
    if (rateIssue) return rateIssue;
    if (
      !(chunked && av1an.targetEnabled) &&
      rate.mode === 'quality' &&
      !(
        typeof crf === 'number' &&
        Number.isFinite(crf) &&
        Number.isInteger(crf) &&
        crf >= options.crfMin &&
        crf <= options.crfMax
      )
    )
      return `Use ${'whole-number'} CRF ${options.crfMin}–${options.crfMax}.`;
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
    if (!encodingVideo) return null;
    if (!validForkSettings(encoder, lineartPsyBias, texturePsyBias, hdrTune))
      return 'Check encoder tuning: use lineart and texture bias 0–7 and a listed HDR tune.';
    if (
      (isSvtEncoder(encoder) || encoder === 'aomAv1') &&
      !(
        typeof filmGrain === 'number' &&
        Number.isInteger(filmGrain) &&
        filmGrain >= 0 &&
        filmGrain <= 50
      )
    )
      return 'Use film grain synthesis 0–50.';
    if (av1anGrain && (isSvtEncoder(encoder) || encoder === 'aomAv1')) {
      if (
        (av1anGrain.table !== null && av1anGrain.table.length === 0) ||
        (!gridGrain && av1anGrain.table !== null && filmGrain !== 0) ||
        !Number.isInteger(av1anGrain.denoiseStrength) ||
        av1anGrain.denoiseStrength < 1 ||
        av1anGrain.denoiseStrength > 16 ||
        (!gridGrain && !chunked && isSvtEncoder(encoder) && av1anGrain.table === null)
      )
        return 'Check the grain table and use denoise strength 1–16.';
    }
    if (chunked) {
      const issue = av1anError(
        av1an,
        knownHdr(selectedVideo) && !toneMap.enabled,
        encoder,
        outputContainer === 'matroska',
        true,
      );
      if (issue) return issue;
    }
    return null;
  });
  const customFiltersIssue = $derived(
    av1anFilters.some((filter) => filter.includes('\0'))
      ? 'Custom video filters cannot contain NUL characters.'
      : null,
  );
  const pixelFormatIssue = $derived(
    outputPixelFormat && !sourceColors.some((option) => option.value === outputPixelFormat)
      ? 'Choose an output pixel format supported by this encoder.'
      : outputPixelFormat === 'yuva420p' && !['matroska', 'webm'].includes(outputContainer)
        ? 'VP9 alpha requires a Matroska or WebM destination.'
        : null,
  );
  const validSettings = $derived(
    !videoSettingsIssue &&
      !tuningSettingsIssue &&
      (!encodingVideo || (!customFiltersIssue && !pixelFormatIssue)),
  );
  const containerIssue = $derived(
    (encodingVideo || videoMode === 'copy') && ['m4a', 'ogg'].includes(outputContainer)
      ? 'M4A and Ogg audio destinations require Audio only mode.'
      : null,
  );
  const sourceTrackIssue = $derived.by(() => {
    if (fileMode !== 'mux' || !file) return null;
    const selected = tracks.filter((row) => row.included);
    const videos = selected.filter(
      (row) =>
        files
          .find((f) => f.id === row.sourceId)
          ?.streams.find((stream) => stream.index === row.streamIndex)?.kind === 'video',
    );
    if (videos.length > 1 && !sourceMultipleVideos && videoMode !== 'drop')
      return 'This encoder supports one selected video track per encode.';
    const unsupported = selected.find((row) => {
      const kind = files
        .find((f) => f.id === row.sourceId)
        ?.streams.find((stream) => stream.index === row.streamIndex)?.kind;
      return (
        row.sourceId !== file.id &&
        !(kind === 'data' && outputContainer !== 'mov') &&
        !(kind === 'video' && !chunked && videoMode === 'drop') &&
        ![
          'audio',
          'subtitle',
          'attachment',
          ...(outputContainer === 'mov' ? ['data'] : []),
          ...(sourceMultipleVideos ? ['video'] : []),
        ].includes(kind ?? '')
      );
    });
    return unsupported
      ? 'This selected external track type cannot be included by the encoder.'
      : null;
  });
  // Track-selection and container problems are decided on other tabs, so the
  // shell repeats them beside Run instead of leaving a silently disabled button.
  const runBlockedReason = $derived(
    sourceTrackIssue ?? containerIssue ?? (encodingVideo ? subtitleIssue : null),
  );
  const noVideoForRun = $derived(
    !chunked &&
      fileMode === 'mux' &&
      tracks.length > 0 &&
      !tracks.some(
        (row) =>
          row.included &&
          files
            .find((source) => source.id === row.sourceId)
            ?.streams.some((stream) => stream.index === row.streamIndex && stream.kind === 'video'),
      ),
  );
  function sourceKnownStream(stream: import('$lib/ipc/generated').MediaStream) {
    const codec = stream.codec?.trim().toLowerCase();
    return (
      (!!codec && codec !== 'unknown') ||
      (stream.kind === 'data' &&
        ['tmcd', 'gpmd'].includes(stream.codecTag?.trim().toLowerCase() ?? ''))
    );
  }
  function logDroppedStreams(
    streams: import('$lib/ipc/generated').MediaStream[],
    container: ContainerFormat,
  ) {
    for (const kind of ['data', 'attachment'] as const) {
      const count = streams.filter(
        (stream) =>
          stream.kind === kind &&
          (kind === 'data' ? container !== 'mov' : container !== 'matroska'),
      ).length;
      if (count)
        onlog?.(
          `${count} ${kind} track${count === 1 ? '' : 's'} left out: ${(container === 'matroska' ? 'mkv' : container).toUpperCase()} cannot store ${kind} streams.`,
        );
    }
  }
  const batchWithoutVideo = $derived(
    !chunked &&
      fileMode === 'batch' &&
      (videoChoice === 'drop' ||
        !files.some((source) =>
          source.streams.some((stream) => stream.kind === 'video' && sourceKnownStream(stream)),
        )),
  );
  const canQueue = $derived(
    !disabled &&
      !suggestionPending &&
      !(
        chunked &&
        (fileMode === 'batch'
          ? files.length > 0 && files.every((entry) => entry.source?.kind === 'imageSequence')
          : file?.source?.kind === 'imageSequence')
      ) &&
      connected &&
      (noVideoForRun || batchWithoutVideo
        ? ['ffmpeg', 'ffprobe'].every((id) =>
            tools.some((tool) => tool.id === id && tool.available),
          ) &&
          !trimIssue &&
          !sourceTrackIssue &&
          validAudio(audio, included, file?.streams ?? []) &&
          !!destination.trim()
        : (!chunked && fileMode === 'batch'
            ? ['ffmpeg', 'ffprobe'].every((id) =>
                tools.some((tool) => tool.id === id && tool.available),
              )
            : toolsReady) &&
          selectedVideoSupported &&
          (!encodingVideo || !hdrUnsupported) &&
          validSettings &&
          !dynamicHdrIssue &&
          !gridGrainIssue &&
          !parsedHdr.error &&
          (!encodingVideo ||
            !hdrMetadata ||
            (supportsHdrMetadata(encoder) &&
              validHdrMetadata(hdrMetadata) &&
              knownHdr(selectedVideo) &&
              !toneMap.enabled &&
              (supportsDynamicHdr(encoder) ||
                (!hdrMetadata.dolbyVisionRpuPath && !hdrMetadata.hdr10PlusJsonPath)))) &&
          (!encodingVideo ||
            (!grainConflict && !parameterIssue && (fileMode === 'batch' || framingValid))) &&
          !trimIssue &&
          !externalIssue &&
          !containerIssue &&
          !sourceTrackIssue &&
          !layoutIssue &&
          !timecodeIssue &&
          (!encodingVideo ||
            (!temporalError(temporal, backend) && !toneMapIssue && !subtitleIssue)) &&
          validAudio(audio, included, file?.streams ?? []) &&
          !!destination.trim()),
  );
  const canStart = $derived(canQueue && !active);
  const sequenceIssue = $derived(
    chunked &&
      (fileMode === 'batch'
        ? files.length > 0 && files.every((entry) => entry.source?.kind === 'imageSequence')
        : file?.source?.kind === 'imageSequence'),
  );
  function sourceOutputBase(source: MediaFile) {
    return source.source?.kind === 'imageSequence'
      ? source.source.directoryPath.replace(/[\\/]$/, '')
      : source.path.replace(/\.[^./\\]+$/, '');
  }
  function outputTarget(): OutputTarget {
    if (isImage)
      return {
        kind: 'image',
        format:
          videoChoice === 'gif' ? 'gif' : videoChoice === 'png' ? 'pngSequence' : 'jpegSequence',
      };
    return { kind: 'container', format: outputContainer };
  }
  function targetPath(stem: string, target: OutputTarget): string {
    if (target.kind === 'container')
      return `${stem}.${target.format === 'matroska' ? 'mkv' : target.format}`;
    return target.format === 'gif'
      ? `${stem}.gif`
      : target.format === 'png'
        ? `${stem}.png`
        : target.format === 'jpeg'
          ? `${stem}.jpg`
          : stem;
  }
  async function suggestDestination(source: MediaFile) {
    const revision = ++destinationRevision,
      target = outputTarget();
    suggestionPending = true;
    suggestedStem = sourceOutputBase(source);
    destination = adjustedOutputStem(suggestedStem, outputPictureChange);
    try {
      const result = await resolveOutputPath({
        inputPath: source.path,
        defaultOutputDirectory: sourcePreferences?.defaultOutputDirectory,
        target,
      });
      if (revision === destinationRevision && mainFile?.id === source.id) {
        suggestedStem = result.outputStem;
        destination = adjustedOutputStem(result.outputStem, outputPictureChange);
      }
    } catch (cause) {
      if (revision === destinationRevision && mainFile?.id === source.id)
        onlog?.(`Could not suggest an output path: ${errorMessage(cause)}`);
    } finally {
      if (revision === destinationRevision) suggestionPending = false;
    }
  }
  function beginSuggestion(source: MediaFile) {
    suggestionTask = suggestDestination(source);
  }
  async function validateDestination() {
    if (!file || !destination.trim()) return;
    if (suggestionPending && mainFile) {
      beginSuggestion(mainFile);
      await suggestionTask;
      return;
    }
    const revision = ++destinationRevision,
      stem = destination,
      target = outputTarget(),
      inputPath = file.path;
    try {
      const result = await resolveOutputPath({ inputPath, outputStem: stem, target });
      if (revision === destinationRevision && destination === stem && file?.path === inputPath)
        destination = result.outputStem;
    } catch (cause) {
      if (revision === destinationRevision) error = errorMessage(cause);
    }
  }

  function checkedVideoIndex(source: MediaFile | undefined) {
    return (
      tracks.find(
        (row) =>
          row.sourceId === source?.id &&
          row.included &&
          source?.streams.some(
            (stream) => stream.index === row.streamIndex && stream.kind === 'video',
          ),
      )?.streamIndex ??
      (
        source?.streams.find((stream) => stream.kind === 'video' && !stream.attachedPic) ??
        source?.streams.find((stream) => stream.kind === 'video')
      )?.index
    );
  }
  function reset(
    source: MediaFile | undefined,
    usePreferences = true,
    useInfrastructurePreferences = usePreferences,
  ) {
    ++draftGeneration;
    videoIndex = checkedVideoIndex(source);
    videoMode =
      !chunked && (videoChoice === 'copy' || videoChoice === 'drop') ? videoChoice : 'encode';
    parameters = [];
    hdrMetadata = undefined;
    rate = defaultRate();
    av1an = defaultAv1an(useInfrastructurePreferences, true);
    av1anGrain = undefined;
    av1anFilters = chunked && usePreferences ? readAv1anPreferences(true).filters : [];
    outputPixelFormat =
      encoder === 'svtAv1' || encoder === 'aomAv1' || encoder === 'vpxStandalone'
        ? 'yuv420p10le'
        : 'yuv420p';
    if (chunked) av1an = { ...av1an, pixelFormat: outputPixelFormat };
    crf = options.defaultCrf;
    preset = options.defaultPreset;
    workers = chunked && useInfrastructurePreferences ? readAv1anPreferences(true).workers : 4;
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
        ? defaultAv1anAudio(source?.streams ?? [], true)
        : defaultAudio(source?.streams ?? []).map((track) => ({
            ...track,
            codec: 'opus',
            channels: 'stereo',
            bitrateKbps: 128,
          }));
    subtitles = defaultSubtitles(source?.streams ?? []);
    framing = defaultFramingDraft();
    trim = defaultTrim();
    toneMap = defaultToneMap();
    temporal = {
      ...defaultTemporal(source?.streams.find((stream) => stream.index === videoIndex)),
      resizeFilter: 'bicubic',
    };
    if (source && !source.id.startsWith('jesses-synthetic')) beginSuggestion(source);
    else {
      ++destinationRevision;
      suggestionPending = false;
      suggestedStem = null;
      destination = '';
    }
    error = null;
  }
  let lastSourceId: string | null = null,
    lastMainId: string | null = null,
    lastEncoder: VideoEncoder | undefined;
  $effect(() => {
    const source = file,
      loaded = mainFile,
      chosen = encoder;
    untrack(() => {
      const identity = source ? JSON.stringify([chosen, source]) : null;
      if (chosen !== lastEncoder) {
        if (chunked) workers = Math.max(1, workerBaseline - (chosen === 'svtAv1' ? 2 : 0));
        parameters = [];
        hdrMetadata = undefined;
        updateRaw({});
        if (rate.mode === 'quality') crf = options.defaultCrf;
        preset = options.defaultPreset;
        outputPixelFormat =
          chosen === 'svtAv1' || chosen === 'aomAv1' || chosen === 'vpxStandalone'
            ? 'yuv420p10le'
            : 'yuv420p';
        if (chunked) av1an = { ...av1an, pixelFormat: outputPixelFormat };
        lastEncoder = chosen;
      }
      if (loaded?.id !== lastMainId) {
        if (lastMainId !== null) resetSourceSettings();
        lastMainId = loaded?.id ?? null;
        metadataSource = loaded?.path ?? '';
        chapterSource = loaded?.path ?? '';
        if (loaded && !loaded.id.startsWith('jesses-synthetic')) beginSuggestion(loaded);
        else {
          ++destinationRevision;
          suggestionPending = false;
          suggestedStem = null;
          destination = '';
        }
      }
      if (source?.id !== lastSourceId) {
        lastSourceId = source?.id ?? null;
        videoIndex = checkedVideoIndex(source);
        included = tracks.some((row) => row.sourceId === source?.id)
          ? tracks
              .filter((row) => row.sourceId === source?.id && row.included)
              .map((row) => row.streamIndex)
          : (source?.streams
              .filter((stream) => stream.kind !== 'video' && stream.kind !== 'data')
              .map((stream) => stream.index) ?? []);
        audio = defaultAudio(source?.streams ?? []).map((track) => ({
          ...track,
          codec: audioCodec,
          channels: audioChannels,
          bitrateKbps: audioBitrate,
        }));
        subtitles = defaultSubtitles(source?.streams ?? []);
        temporal = {
          ...temporal,
          fieldOrder:
            source?.streams.find((stream) => stream.index === videoIndex)?.fieldOrder === 'bb'
              ? 'bottomFirst'
              : 'topFirst',
        };
        if (
          source &&
          knownHdr(source.streams.find((stream) => stream.index === videoIndex)) &&
          (sourcePreferences?.resetOnNewFile?.toneMap ?? true)
        )
          toneMap = {
            ...defaultToneMap(),
            enabled: true,
            algorithm: chunked ? 'mobius' : 'spline',
          };
      }
      ++draftGeneration;
      draftIdentity = identity;
      error = null;
    });
  });
  $effect(() => {
    if (chunked)
      reportPreferenceSave(
        saveAv1anPreferences(av1an, workers, av1anFilters, workerBaseline, true),
      );
  });
  $effect(() => {
    if (chunked)
      reportPreferenceSave(
        saveAv1anAudioState(
          { codec: audioCodec, bitrateKbps: audioBitrate, channels: audioChannels },
          { stripAudio, copySubs, copyData, copyAttachments },
          true,
        ),
      );
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
  function setTrimStart(seconds: number) {
    trim = {
      ...trim,
      enabled: true,
      mode: 'time',
      startMilliseconds: Math.round(seconds * 1000),
      endMilliseconds: trim.mode === 'time' ? trim.endMilliseconds : undefined,
    };
  }
  function setTrimEnd(seconds: number) {
    trim = {
      ...trim,
      enabled: true,
      mode: 'time',
      startMilliseconds: trim.mode === 'time' ? trim.startMilliseconds : 0,
      endMilliseconds: Math.round(seconds * 1000),
    };
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
    const revision = ++destinationRevision;
    suggestionPending = false;
    try {
      const selected = await chooseEncodeDestination(destination);
      if (selected && revision === destinationRevision && isCurrentDraft(generation, identity)) {
        // Quick Convert removes only recognised media extensions; AV1AN uses
        // Path.ChangeExtension and therefore removes any final extension.
        const match = selected.match(/\.([^./\\]+)$/);
        const known =
          match &&
          ['mp4', 'mkv', 'webm', 'mov', 'm4a', 'ogg', 'gif', 'png', 'jpg', 'jpeg'].includes(
            match[1].toLowerCase(),
          );
        destination = match && (chunked || known) ? selected.slice(0, -match[0].length) : selected;
        suggestedStem = null;
      }
    } catch (cause) {
      if (isCurrentDraft(generation, identity)) error = errorMessage(cause);
    }
  }
  // Source drops rows whose trimmed UTF-16 length is at most two, while
  // retaining every surviving string verbatim and in its original order.
  function sourceToneMap() {
    const value = selectedToneMap(toneMap);
    return value ? { ...value, sourceWorkflow: true } : undefined;
  }
  function sourceFilterRows(rows: readonly string[]): string[] {
    // .NET Trim includes NEL and excludes BOM, unlike JavaScript trim().
    const sourceSpace =
      '[\\u0009-\\u000d\\u0020\\u0085\\u00a0\\u1680\\u2000-\\u200a\\u2028\\u2029\\u202f\\u205f\\u3000]';
    const trim = new RegExp(`^${sourceSpace}+|${sourceSpace}+$`, 'g');
    return rows.filter((row) => row.replace(trim, '').length > 2);
  }
  function currentRequest(): EncodeRequest | undefined {
    const hasSelectedVideo =
      chunked ||
      (fileMode === 'batch'
        ? !batchWithoutVideo
        : tracks.length === 0 ||
          tracks.some(
            (row) =>
              row.included &&
              files
                .find((source) => source.id === row.sourceId)
                ?.streams.some(
                  (stream) => stream.index === row.streamIndex && stream.kind === 'video',
                ),
          ));
    const pictureEnabled = encodingVideo && hasSelectedVideo;
    const effectiveVideoMode = hasSelectedVideo ? videoMode : 'drop';
    if (
      !file ||
      (fileMode !== 'batch' &&
        videoIndex === undefined &&
        (pictureEnabled || effectiveVideoMode === 'copy'))
    )
      return undefined;
    const copies = copiedStreams
      .filter(
        (stream) =>
          (included.includes(stream.index) ||
            (pictureEnabled &&
              !chunked &&
              stream.kind === 'subtitle' &&
              file.id === mainFile?.id &&
              String(stream.index) === activeBurn)) &&
          !(stream.kind === 'audio' && stripAudio) &&
          !(
            stream.kind === 'subtitle' &&
            (stripSubs || (chunked && !copySubs)) &&
            !(pictureEnabled && file.id === mainFile?.id && String(stream.index) === activeBurn)
          ) &&
          !(stream.kind === 'data' && (outputContainer !== 'mov' || (chunked && !copyData))) &&
          !(
            stream.kind === 'attachment' &&
            (outputContainer !== 'matroska' || (chunked && !copyAttachments))
          ),
      )
      .sort((a, b) => Number(a.kind === 'attachment') - Number(b.kind === 'attachment'));
    const selectedIndices = [
      ...((effectiveVideoMode === 'drop' && !chunked) || videoIndex === undefined
        ? []
        : sourceMultipleVideos && tracks.length
          ? tracks
              .filter(
                (row) =>
                  row.sourceId === file.id &&
                  row.included &&
                  file.streams.some(
                    (stream) => stream.index === row.streamIndex && stream.kind === 'video',
                  ),
              )
              .map((row) => row.streamIndex)
          : [videoIndex]),
      ...copies.map((stream) => stream.index),
    ];
    const selectedPrimary = new Set(selectedIndices);
    const selectedOverrides = trackOverrides
      .filter((entry) => selectedPrimary.has(entry.streamIndex))
      .map((entry) => ({ ...entry }));
    const selectedRows = tracks.filter((row) => {
      if (
        (!row.included &&
          !(
            pictureEnabled &&
            row.sourceId === mainFile?.id &&
            String(row.streamIndex) === activeBurn
          )) ||
        (fileMode !== 'mux' && row.sourceId !== file.id)
      )
        return false;
      const kind = files
        .find((source) => source.id === row.sourceId)
        ?.streams.find((stream) => stream.index === row.streamIndex)?.kind;
      return (
        !(kind === 'audio' && stripAudio) &&
        !(
          kind === 'subtitle' &&
          (stripSubs || (chunked && !copySubs)) &&
          !(
            pictureEnabled &&
            row.sourceId === mainFile?.id &&
            String(row.streamIndex) === activeBurn
          )
        ) &&
        !(kind === 'data' && (outputContainer !== 'mov' || (chunked && !copyData))) &&
        !(
          kind === 'attachment' &&
          (outputContainer !== 'matroska' || (chunked && !copyAttachments))
        ) &&
        !(kind === 'video' && effectiveVideoMode === 'drop' && !chunked)
      );
    });
    const externalRows = selectedRows
      .filter((row) => row.sourceId !== file.id)
      .map((row) => ({ row, source: files.find((entry) => entry.id === row.sourceId) }))
      .filter((entry): entry is { row: TrackSelection; source: MediaFile } => !!entry.source);
    const selectedOrder = selectedRows.length
      ? selectedRows.map((row) => ({
          streamIndex: row.streamIndex,
          ...(row.sourceId !== file.id
            ? { inputPath: files.find((source) => source.id === row.sourceId)?.path }
            : {}),
        }))
      : selectedTrackOrder(trackOrder, layoutDefaults);
    const rowOverrides = selectedRows
      .filter((row) => row.sourceId === file.id && selectedPrimary.has(row.streamIndex))
      .map((row) => ({
        streamIndex: row.streamIndex,
        ...(applyMetadata ? metadataValue(row) : {}),
        ...(row.defaultFlag !== null ? { default: row.defaultFlag } : {}),
        ...(row.forcedFlag !== null ? { forced: row.forcedFlag } : {}),
      }));
    return {
      source: {
        inputPath: file.path,
        outputPath: targetPath(destination.trim(), outputTarget()),
        streamIndices: selectedIndices,
      },
      settings: {
        ...(sourcePreferences
          ? {
              encodingPreferences: {
                keyframeIntervalSeconds: sourcePreferences.keyframeIntervalSeconds ?? 10,
                mp4Faststart: sourcePreferences.mp4Faststart ?? true,
                fullOutputVerification: sourcePreferences.fullOutputVerification ?? false,
              },
            }
          : {}),
        ...(pictureEnabled && hdrMetadata ? { hdrMetadata } : {}),
        ...(pictureEnabled && rate.mode === 'constantQp'
          ? { constantQuantizer: rate.constantQp! }
          : {}),
        ...(pictureEnabled && parameters.length
          ? {
              parameters: parameters
                .filter((value) => !gridGrain || !['noise', 'film-grain'].includes(value.name))
                .map((value) => ({ ...value })),
            }
          : {}),
        ...(pictureEnabled && selectedTemporal(temporal)
          ? { temporal: selectedTemporal(temporal) }
          : {}),
        videoStreamIndex: videoIndex ?? 0,
        ...(!pictureEnabled ? { videoMode: effectiveVideoMode } : {}),
        ...(chunked
          ? {
              av1anOptions: {
                ...selectedAv1an(av1an, encoder, true),
                sourceWorkflow: true,
                ...(av1an.targetEnabled ? { sourceTargetDefaults: true } : {}),
              },
            }
          : {}),
        ...(pictureEnabled && !chunked && outputPixelFormat ? { outputPixelFormat } : {}),
        ...(pictureEnabled &&
        (isSvtEncoder(encoder) || encoder === 'aomAv1') &&
        (gridGrain || av1anGrain)
          ? {
              av1anGrain: gridGrain
                ? {
                    table: gridGrain.table,
                    denoise: av1anGrain?.denoise ?? false,
                    denoiseStrength: av1anGrain?.denoiseStrength ?? 4,
                  }
                : { ...av1anGrain! },
            }
          : {}),
        ...(pictureEnabled && sourceFilterRows(av1anFilters).length
          ? { av1anFilters: sourceFilterRows(av1anFilters), sourceFilters: true }
          : {}),
        ...(pictureEnabled && (rate.mode === 'bitrate' || rate.mode === 'targetSize')
          ? { rateControl: selectedRate(rate, encoder) }
          : {}),
        crf:
          pictureEnabled && rate.mode === 'quality' && !(chunked && av1an.targetEnabled)
            ? Math.round(crf!)
            : Math.round(options.defaultCrf),
        preset: Math.max(0, preset),
        // Source quality zero is distinct from the explicit native lossless
        // API; source scalar zero remains a literal quality value.
        lossless: pictureEnabled && rate.mode === 'lossless',
        ...(pictureEnabled && isSvtEncoder(encoder) && rate.mode === 'quality' && crf !== 0
          ? { svtCrfQuarterSteps: Math.round(crf! * 4) }
          : {}),
        ...(isSvtEncoder(encoder) ? { svtPreset: preset } : {}),
        backend,
        encoder,
        workers: backend === 'av1an' ? workers! : 2,
        filmGrain:
          pictureEnabled && !gridGrain && (isSvtEncoder(encoder) || encoder === 'aomAv1')
            ? filmGrain!
            : 0,
        hdr10Fallback: pictureEnabled && isSvtEncoder(encoder) ? hdr10Fallback : false,
        lineartPsyBias: pictureEnabled && encoder === 'svtAv1FiveFish' ? lineartPsyBias! : 0,
        texturePsyBias: pictureEnabled && encoder === 'svtAv1FiveFish' ? texturePsyBias! : 0,
        hdrTune: pictureEnabled && encoder === 'svtAv1Hdr' ? hdrTune : 'visualQuality',
        audio: file.streams
          .filter(
            (stream) => stream.kind === 'audio' && included.includes(stream.index) && !stripAudio,
          )
          .map((stream) => audioFor(file, stream.index)),
        ...(!chunked &&
        selectedSubtitles(
          pictureEnabled
            ? subtitles
            : subtitles.map((track) => ({
                ...track,
                burnIn: false,
                mode: track.mode === 'burnIn' ? 'copy' : track.mode,
              })),
          selectedIndices,
        ).length
          ? {
              subtitles: selectedSubtitles(
                pictureEnabled
                  ? subtitles
                  : subtitles.map((track) => ({
                      ...track,
                      burnIn: false,
                      mode: track.mode === 'burnIn' ? 'copy' : track.mode,
                    })),
                selectedIndices,
              ),
            }
          : {}),
        ...(externalRows.length
          ? {
              externalTracks: externalRows.map(({ row, source }) => ({
                inputPath: source.path,
                streamIndex: row.streamIndex,
                ...(source.streams.find((stream) => stream.index === row.streamIndex)?.kind ===
                  'audio' && audioCodec !== 'copy'
                  ? {
                      audio: (({ streamIndex: _, ...settings }) => settings)(
                        audioFor(source, row.streamIndex),
                      ),
                    }
                  : {}),
                ...(source.streams.find((stream) => stream.index === row.streamIndex)?.kind ===
                'subtitle'
                  ? pictureEnabled &&
                    source.id === mainFile?.id &&
                    String(row.streamIndex) === activeBurn
                    ? !row.included || stripSubs
                      ? { subtitleMode: 'burnIn' as const }
                      : {
                          ...(subtitleMode !== 'copy' ? { subtitleMode } : {}),
                          subtitleBurnIn: true,
                        }
                    : ((mode) => (mode !== 'copy' ? { subtitleMode: mode } : {}))(
                        storableSubtitleMode(
                          subtitleMode,
                          source.streams.find((stream) => stream.index === row.streamIndex),
                          outputContainer,
                          backend,
                        ),
                      )
                  : {}),
                ...(applyMetadata ? metadataValue(row) : {}),
                ...(row.defaultFlag !== null ? { default: row.defaultFlag } : {}),
                ...(row.forcedFlag !== null ? { forced: row.forcedFlag } : {}),
              })),
            }
          : externalTracks.length
            ? { externalTracks: requestExternalTracks(externalTracks) }
            : {}),
        ...(rowOverrides.length
          ? { trackOverrides: rowOverrides }
          : selectedOverrides.length
            ? { trackOverrides: selectedOverrides }
            : {}),
        ...(selectedOrder?.length ? { trackOrder: selectedOrder } : {}),
        ...(!chunked && !metadataSource ? { copyMetadata: false } : {}),
        ...(!chunked && !chapterSource ? { copyChapters: false } : {}),
        ...(!chunked && fileMode !== 'batch' && metadataSource
          ? { metadataSourcePath: metadataSource }
          : {}),
        ...(!chunked && fileMode !== 'batch' && chapterSource
          ? { chaptersSourcePath: chapterSource }
          : {}),
        ...(!chunked && fileMode !== 'batch' && applyMetadata && mainFile
          ? {
              outputMetadata: {
                title: outputMetadataValue(mainFile).title.trim(),
                language: outputMetadataValue(mainFile).language.trim(),
              },
            }
          : {}),
        ...(chunked && metadataDonor && metadataDonor.inputPath !== file.path
          ? { metadataSourcePath: metadataDonor.inputPath }
          : {}),
        ...(chunked && chaptersDonor && chaptersDonor.inputPath !== file.path
          ? { chaptersSourcePath: chaptersDonor.inputPath }
          : {}),
        ...(pictureEnabled && movTimecode
          ? { movTimecodeTrack: selectedMovTimecode(movTimecode, file) }
          : {}),
        framing: pictureEnabled
          ? selectedFraming(activeFraming)
          : selectedFraming(defaultFramingDraft()),
        ...(trim.enabled ? { trim: selectedTrim(trim), sourceTrim: true } : {}),
        ...(pictureEnabled && toneMap.enabled ? { toneMap: sourceToneMap() } : {}),
      },
    };
  }
  const commandRequest = $derived(canQueue ? currentRequest() : undefined);
  async function temporalForSource(
    entry: MediaFile,
    index: number,
    baseline: ReturnType<typeof selectedTemporal>,
    signal?: AbortSignal,
    setting = {
      choice: deinterlaceChoice,
      double: deinterlaceDouble,
      preset: temporal.qtgmcPreset,
      resizeFilter: temporal.resizeFilter,
    },
    outputPath = destination,
  ) {
    const video = entry.streams.find((stream) => stream.index === index);
    const relevant = !!video?.fieldOrder && !['progressive', 'unknown'].includes(video.fieldOrder);
    if (!relevant || setting.choice === 'disabled') return baseline;
    const method =
      setting.choice === 'yadif'
        ? 'yadif'
        : setting.choice === 'bwdif' || chunked
          ? 'bwdif'
          : 'qtgmc';
    const resolved = await resolveDeinterlace(
      {
        inputPath: entry.path,
        outputPath,
        videoStreamIndex: index,
        method,
        mode: !chunked && setting.double ? 'bob' : 'frame',
        fieldOrder: 'topFirst',
        qtgmcPreset: setting.preset,
      },
      setting.choice === 'automatic',
      signal,
    );
    onlog?.(resolved.message);
    if (!resolved.request) return baseline;
    const { method: actual, mode, fieldOrder, qtgmcPreset } = resolved.request;
    return {
      ...(baseline ?? { resizeFilter: setting.resizeFilter }),
      ...(actual === 'qtgmc'
        ? { qtgmc: { mode, fieldOrder, preset: qtgmcPreset } }
        : {
            deinterlace: {
              mode,
              fieldOrder,
              ...(actual === 'yadif' ? { engine: 'yadif' as const } : {}),
            },
          }),
    };
  }
  async function runBatch(base: EncodeRequest, options: RunOptions, controller: AbortController) {
    const { source: _source, settings } = structuredClone($state.snapshot(base));
    const burnOrdinal =
      mainFile?.streams
        .filter((stream) => stream.kind === 'subtitle')
        .findIndex((stream) => String(stream.index) === activeBurn) ?? -1;
    const batchState = structuredClone(
      $state.snapshot({
        files,
        audioCodec,
        audioChannels,
        audioBitrate,
        loudness,
        stripAudio,
        stripSubs,
        copySubs,
        copyData,
        copyAttachments,
        subtitleMode,
        burnOrdinal,
        cropMode,
        batchNameTemplate,
        metadataSource,
        chapterSource,
        deinterlaceChoice,
        deinterlaceDouble,
        qtgmcPreset: temporal.qtgmcPreset,
        resizeFilter: temporal.resizeFilter,
        framing: copyFramingDraft(framing),
        outputContainer,
        outputDirectory: sourcePreferences?.defaultOutputDirectory ?? '',
        naming: batchNamingValues(),
        namingDate: localNamingDate(),
        videoChoice,
        encodingVideo,
      }),
    );
    const statusId = batchRunId('encode');
    let statuses: Record<string, BatchRowStatus> = Object.fromEntries(
      batchState.files.map((entry) => [entry.id, { glyph: '○', text: 'Queued' }]),
    );
    onbatchstatus?.(statusId, statuses, true);
    const mark = (id: string, status: BatchRowStatus) => {
      if (imageAbort !== controller) return;
      statuses = { ...statuses, [id]: status };
      onbatchstatus?.(statusId, statuses);
    };
    if (chunked) {
      for (const entry of batchState.files.filter(
        (item) => item.source?.kind === 'imageSequence',
      )) {
        onlog?.(`${entry.name}: AV1AN does not support image sequence folders.`);
        mark(entry.id, {
          glyph: '✕',
          text: 'Failed',
          note: 'AV1AN does not support image sequence folders.',
        });
      }
      batchState.files = batchState.files.filter((item) => item.source?.kind !== 'imageSequence');
    }
    const inputErrors: Record<string, string> = {};
    const inputs = await Promise.all(
      batchState.files.map(async (entry) => {
        // The source reloads each file's tracks for batch, checking every known
        // stream independently of the selection made in the mux track list.
        const checked = entry.streams.filter(sourceKnownStream);
        if (!chunked) logDroppedStreams(checked, batchState.outputContainer);
        if (!checked.length)
          inputErrors[entry.id] =
            'No tracks are selected. Please select at least one track to use.';
        const chosen = checked
          .filter(
            ({ kind }) =>
              !(kind === 'video' && !chunked && batchState.videoChoice === 'drop') &&
              !(kind === 'audio' && batchState.stripAudio) &&
              !(
                kind === 'subtitle' &&
                (batchState.stripSubs || (chunked && !batchState.copySubs))
              ) &&
              !(
                kind === 'data' &&
                (batchState.outputContainer !== 'mov' || (chunked && !batchState.copyData))
              ) &&
              !(
                kind === 'attachment' &&
                (batchState.outputContainer !== 'matroska' ||
                  (chunked && !batchState.copyAttachments))
              ),
          )
          .map((stream) => stream.index);
        const vid = entry.streams.find(
          (stream) => stream.kind === 'video' && chosen.includes(stream.index),
        );
        const noPicture = !chunked && !vid;
        const burn =
          !noPicture && batchState.burnOrdinal >= 0
            ? entry.streams.filter((stream) => stream.kind === 'subtitle')[batchState.burnOrdinal]
            : undefined;
        const burnRetained = !!burn && chosen.includes(burn.index);
        if (burn && !burnRetained) chosen.push(burn.index);
        const sourceAudio = entry.streams
          .filter((stream) => stream.kind === 'audio' && chosen.includes(stream.index))
          .map((stream) => ({
            streamIndex: stream.index,
            codec: batchState.audioCodec,
            channels: batchState.audioChannels,
            bitrateKbps: Math.round(
              batchState.audioBitrate *
                audioMultiplier(channelCount(batchState.audioChannels, stream.channels)),
            ),
            ...(batchState.loudness !== 'off' && batchState.audioCodec !== 'copy'
              ? {
                  normalization: {
                    target: `lufs${batchState.loudness.slice(1)}` as 'lufs14' | 'lufs16' | 'lufs23',
                  },
                }
              : {}),
          }));
        let detected: Awaited<ReturnType<typeof detectCrop>> | null = null,
          resolvedTemporal = noPicture ? undefined : settings.temporal;
        try {
          if (vid && batchState.cropMode === 'manual' && !cropFits(batchState.framing, vid))
            throw new Error('Crop leaves less than two pixels.');
          detected =
            batchState.cropMode === 'automatic' && vid
              ? await detectCrop(
                  { inputPath: entry.path, videoStreamIndex: vid.index },
                  controller.signal,
                )
              : null;
          if (vid && batchState.cropMode === 'automatic' && !detected?.crop)
            throw new Error('Automatic crop detection did not find a crop.');
          if (vid)
            resolvedTemporal = await temporalForSource(
              entry,
              vid.index,
              settings.temporal,
              controller.signal,
              {
                choice: batchState.deinterlaceChoice,
                double: batchState.deinterlaceDouble,
                preset: batchState.qtgmcPreset,
                resizeFilter: batchState.resizeFilter,
              },
              base.source.outputPath,
            );
        } catch (cause) {
          if (controller.signal.aborted) throw cause;
          inputErrors[entry.id] = errorMessage(cause);
        }
        return {
          inputPath: entry.path,
          ...(noPicture ? { videoMode: 'drop' as const } : {}),
          videoStreamIndex: vid?.index ?? 0,
          streamIndices: chosen,
          audio: selectedAudio(sourceAudio, chosen),
          ...(!chunked || batchState.encodingVideo
            ? {
                subtitles: entry.streams
                  .filter((stream) => stream.kind === 'subtitle' && chosen.includes(stream.index))
                  .map((stream) =>
                    stream.index === burn?.index
                      ? {
                          streamIndex: stream.index,
                          mode: burnRetained ? batchState.subtitleMode : ('burnIn' as const),
                          ...(burnRetained ? { burnIn: true } : {}),
                        }
                      : {
                          streamIndex: stream.index,
                          mode: storableSubtitleMode(
                            batchState.subtitleMode,
                            stream,
                            batchState.outputContainer,
                            backend,
                          ),
                        },
                  )
                  .filter((track) => track.mode !== 'copy' || track.burnIn),
              }
            : {}),
          framing: noPicture
            ? selectedFraming(defaultFramingDraft())
            : detected?.crop
              ? { ...settings.framing, crop: detected.crop }
              : batchState.cropMode === 'manual'
                ? selectedFraming(alignedFraming(batchState.framing, vid))
                : settings.framing,
          ...(resolvedTemporal ? { temporal: resolvedTemporal } : {}),
          ...(settings.trim ? { trim: settings.trim } : {}),
          ...(!noPicture && settings.toneMap ? { toneMap: settings.toneMap } : {}),
        };
      }),
    ).catch((cause) => {
      if (controller.signal.aborted) {
        for (const entry of batchState.files)
          mark(entry.id, {
            glyph: '–',
            text: 'Skipped',
            note: 'The batch was stopped before this file ran.',
          });
        imageProgressDescription = 'Batch stopped.';
      }
      throw cause;
    });
    const {
      videoStreamIndex: _video,
      audio: _audio,
      framing: _framing,
      trim: _trim,
      temporal: _temporal,
      toneMap: _tone,
      subtitles: _subs,
      externalTracks: _external,
      trackOverrides: _overrides,
      trackOrder: _order,
      metadataSourcePath: _metadata,
      chaptersSourcePath: _chapters,
      outputMetadata: _outputMetadata,
      movTimecodeTrack: _timecode,
      ...shared
    } = settings;
    const result = await previewEncodeBatch(
      {
        ...shared,
        inputs,
        outputContainer: batchState.outputContainer,
        outputDirectory: batchState.outputDirectory,
        outputNameTemplate: batchState.batchNameTemplate,
        namingDate: batchState.namingDate,
        sourceNaming: batchState.naming,
      },
      controller.signal,
    ).catch((cause) => {
      for (const entry of batchState.files)
        mark(
          entry.id,
          controller.signal.aborted
            ? { glyph: '–', text: 'Skipped', note: 'The batch was stopped before this file ran.' }
            : { glyph: '✕', text: 'Failed', note: errorMessage(cause) },
        );
      throw cause;
    });
    for (const [index, entry] of batchState.files.entries()) {
      const item = result.items[index];
      if (controller.signal.aborted) {
        mark(entry.id, {
          glyph: '–',
          text: 'Skipped',
          note: 'The batch was stopped before this file ran.',
        });
        continue;
      }
      if (
        inputErrors[entry.id] ||
        !item ||
        item.inputPath !== entry.path ||
        item.error ||
        !item.request
      ) {
        const shown =
            entry.source?.kind === 'imageSequence' ? entry.source.directoryPath : entry.path,
          message = (
            inputErrors[entry.id] ??
            item?.error?.message ??
            'Unable to create request'
          ).replaceAll(entry.path, shown);
        mark(entry.id, { glyph: '✕', text: 'Failed', note: message });
        onlog?.(`${entry.name}: ${message}`);
        continue;
      }
      const request = item.request;
      const prepared = {
        ...request,
        settings: {
          ...request.settings,
          ...(!chunked && batchState.metadataSource === 'current'
            ? { metadataSourcePath: entry.path }
            : {}),
          ...(!chunked && batchState.chapterSource === 'current'
            ? { chaptersSourcePath: entry.path }
            : {}),
        },
      };
      imageProgress = index / batchState.files.length;
      imageProgressDescription = `File ${index + 1}/${batchState.files.length} (${entry.name})`;
      mark(entry.id, { glyph: '▶', text: 'Running…' });
      try {
        const reviewed = await reviewAndRun(
          { kind: 'encode', request: prepared },
          controller.signal,
          () => reviewThisFile(options),
        );
        if (!reviewed) {
          if (controller.signal.aborted) throw new DOMException('Canceled', 'AbortError');
          mark(entry.id, {
            glyph: '✕',
            text: 'Failed',
            note: 'The command was cleared in the edit window, so nothing was run.',
          });
          continue;
        }
        if (reviewed.kind !== 'encode')
          throw new Error('The command returned an unexpected image result.');
        const job = reviewed.job;
        batchCurrentJobId = job.id;
        const finished = await waitForJobTerminal(job.id, controller.signal);
        if (finished.state === 'succeeded') mark(entry.id, { glyph: '✔', text: 'Done' });
        else if (finished.state === 'canceled' || finished.state === 'stopped')
          mark(entry.id, { glyph: '■', text: 'Canceled', note: finished.error?.message });
        else
          mark(entry.id, {
            glyph: '✕',
            text: 'Failed',
            note: finished.error?.message ?? 'The log has the details.',
          });
      } catch (cause) {
        if (controller.signal.aborted)
          mark(entry.id, { glyph: '■', text: 'Canceled', note: 'Stopped by the user.' });
        else {
          const message = errorMessage(cause);
          mark(entry.id, { glyph: '✕', text: 'Failed', note: message });
          onlog?.(`${entry.name}: ${message}`);
        }
      } finally {
        batchCurrentJobId = null;
      }
    }
    const rows = Object.values(statuses),
      done = rows.filter((row) => row.text === 'Done').length,
      failed = rows.filter((row) => row.text === 'Failed').length,
      skipped = rows.filter((row) => row.text === 'Canceled' || row.text === 'Skipped').length;
    imageProgress = 1;
    imageProgressDescription = `Batch ${controller.signal.aborted ? 'stopped' : 'done'} - ${done}/${rows.length} finished${failed ? `, ${failed} failed` : ''}${skipped ? `, ${skipped} not run` : ''}.`;
  }
  function sourceImageMetadata(
    sources: MediaFile[],
    rows: TrackSelection[],
    current: MediaFile,
    metadataChoice: string,
    chapterChoice: string,
    includeGrid: boolean,
    container: ContainerFormat,
    audioStripped: boolean,
    subtitleStripped: boolean,
  ): ImageExportMetadata {
    const indexed = rows
      .filter((row) => row.included)
      .flatMap((row) => {
        const inputIndex = sources.findIndex((source) => source.id === row.sourceId);
        return inputIndex < 0 ? [] : [{ row, inputIndex, source: sources[inputIndex] }];
      });
    const kind = (entry: (typeof indexed)[number]) =>
      entry.source.streams.find((stream) => stream.index === entry.row.streamIndex)?.kind;
    const mapped = indexed.filter((entry) => {
      const type = kind(entry);
      return (
        !(type === 'audio' && audioStripped) &&
        !(type === 'subtitle' && subtitleStripped) &&
        !(type === 'data' && container !== 'mov') &&
        !(type === 'attachment' && container !== 'matroska')
      );
    });
    const donor = (choice: string) => {
      const index =
        choice === 'current'
          ? sources.findIndex((source) => source.id === current.id)
          : sources.findIndex((source) => source.path === choice);
      return index < 0 ? undefined : index;
    };
    const sourceInput = donor(metadataChoice),
      chaptersInput = donor(chapterChoice),
      output = outputMetadataValue(current);
    return {
      inputs: sources.map((source) => source.path),
      options: {
        ...(sourceInput === undefined ? {} : { sourceInput }),
        ...(chaptersInput === undefined ? {} : { chaptersInput }),
        attachmentInputs:
          sourceInput === undefined
            ? [
                ...new Set(
                  mapped
                    .filter((entry) => kind(entry) === 'attachment')
                    .map((entry) => entry.inputIndex),
                ),
              ]
            : [],
        ...(includeGrid
          ? {
              title: output.title.trim(),
              language: output.language.trim(),
              tracks: mapped.map((entry) => ({
                title: metadataValue(entry.row).title.trim(),
                language: metadataValue(entry.row).language.trim(),
              })),
            }
          : { tracks: [] }),
        audioDispositions: mapped
          .filter((entry) => kind(entry) === 'audio')
          .map((entry) => entry.row.defaultFlag === true),
        subtitleDispositions: mapped
          .filter((entry) => kind(entry) === 'subtitle')
          .map((entry) => entry.row.defaultFlag === true),
      },
    };
  }
  function batchImageRows(entry: MediaFile): TrackSelection[] {
    const streams = entry.streams.filter(sourceKnownStream);
    const chosen = (kind: 'audio' | 'subtitle') => {
      const candidates = streams.filter((stream) => stream.kind === kind);
      return candidates.find((stream) => stream.isDefault) ?? candidates[0];
    };
    const defaultAudio = chosen('audio'),
      defaultSubtitle = chosen('subtitle');
    return streams.map((stream) => ({
      sourceId: entry.id,
      streamIndex: stream.index,
      included: true,
      title: stream.title ?? '',
      language: stream.language ?? '',
      defaultFlag:
        stream.kind === 'audio'
          ? stream === defaultAudio
          : stream.kind === 'subtitle'
            ? stream === defaultSubtitle
            : null,
      forcedFlag: null,
    }));
  }
  async function startImplicitImage(options: RunOptions = {}) {
    if (!mainFile || !destination.trim() || suggestionPending || submitting || active) return;
    const sources = structuredClone($state.snapshot(files));
    const currentInput = sources.findIndex((source) => source.id === mainFile.id);
    const selected = structuredClone($state.snapshot(tracks.filter((row) => row.included)));
    if (
      currentInput < 0 ||
      selected.some((row) => !sources.some((source) => source.id === row.sourceId))
    ) {
      error = 'A selected input is no longer available.';
      return;
    }
    const indexed = selected.map((row) => ({
      row,
      inputIndex: sources.findIndex((source) => source.id === row.sourceId),
      source: sources.find((source) => source.id === row.sourceId)!,
    }));
    const kind = (entry: (typeof indexed)[number]) =>
      entry.source.streams.find((stream) => stream.index === entry.row.streamIndex)?.kind;
    const checked = indexed.map((entry) => ({
      inputIndex: entry.inputIndex,
      streamIndex: entry.row.streamIndex,
    }));
    const audioRows = indexed.filter((entry) => kind(entry) === 'audio');
    const audio: Extract<ImageRequest, { operation: 'implicitExport' }>['audio'] = !audioRows.length
      ? undefined
      : stripAudio
        ? { mode: 'strip' }
        : audioCodec === 'copy'
          ? { mode: 'copy' }
          : {
              mode: 'encode',
              codec: audioCodec,
              tracks: audioRows.map((entry) => {
                const value = audioFor(entry.source, entry.row.streamIndex),
                  stream = entry.source.streams.find(
                    (item) => item.index === entry.row.streamIndex,
                  );
                return {
                  source: { inputIndex: entry.inputIndex, streamIndex: entry.row.streamIndex },
                  channels: channelCount(value.channels, stream?.channels ?? null),
                  bitrateKbps: value.bitrateKbps,
                  ...(value.normalization ? { normalization: value.normalization } : {}),
                };
              }),
            };
    const current = sources[currentInput],
      metadata = sourceImageMetadata(
        sources,
        selected,
        current,
        metadataSource,
        chapterSource,
        applyMetadata,
        outputContainer,
        stripAudio,
        stripSubs,
      );
    const sourceRate = current.streams
      .find((stream) => stream.kind === 'video')
      ?.frameRate?.split('/')
      .map(Number);
    const originalRate =
      sourceRate && sourceRate[0] > 0 && sourceRate[1] > 0 ? sourceRate[0] / sourceRate[1] : 0;
    const requestedRate =
      temporal.changeRate && (temporal.denominator ?? 0) > 0
        ? (temporal.numerator ?? 0) / temporal.denominator!
        : 0;
    const keepFrameCount =
      !originalRate ||
      requestedRate <= 0.01 ||
      Math.abs(originalRate - requestedRate) <= Math.max(originalRate, requestedRate) * 0.0001;
    const format: ImageOutput =
      videoChoice === 'gif' ? 'gif' : videoChoice === 'png' ? 'pngSequence' : 'jpegSequence';
    const stem = destination,
      revision = destinationRevision,
      generation = draftGeneration,
      identity = draftIdentity;
    const request = structuredClone(
      $state.snapshot({
        operation: 'implicitExport' as const,
        inputs: metadata.inputs,
        checked,
        currentInput,
        format,
        audio,
        subtitleCodec: stripSubs
          ? ('strip' as const)
          : subtitleMode === 'ass' || subtitleMode === 'burnIn'
            ? ('copy' as const)
            : subtitleMode,
        metadata: metadata.options,
        trim: selectedTrim(trim),
        keepFrameCount,
      }),
    );
    const controller = new AbortController();
    imageAbort = controller;
    submitting = true;
    error = null;
    imageProgress = 0;
    imageProgressDescription = 'Preparing image output';
    try {
      const resolved = await resolveOutputPath({
        inputPath: current.path,
        outputStem: stem.trim(),
        target: { kind: 'image', format },
      });
      if (controller.signal.aborted) {
        imageProgressDescription = 'Canceled.';
        return;
      }
      if (
        revision === destinationRevision &&
        destination === stem &&
        isCurrentDraft(generation, identity)
      )
        destination = resolved.outputStem;
      const reviewed = await reviewAndRun(
        { kind: 'image', request: { ...request, outputPath: resolved.outputPath } },
        controller.signal,
        () => reviewSingleFile(options),
      );
      if (!reviewed) {
        imageProgressDescription = 'Canceled.';
        if (!controller.signal.aborted)
          onlog?.('The command was cleared in the edit window, so nothing was run.');
        return;
      }
      if (reviewed.kind !== 'image')
        throw new Error('The command returned an unexpected encode job.');
      imageProgress = 1;
      imageProgressDescription = 'Done.';
      onlog?.(`Exported ${current.name} to ${resolved.outputPath}.`);
    } catch (cause) {
      if (controller.signal.aborted) imageProgressDescription = 'Canceled.';
      else {
        const message = errorMessage(cause);
        imageProgressDescription = 'Did not finish - the log has the details.';
        onlog?.(message);
        if (isCurrentDraft(generation, identity)) error = message;
      }
    } finally {
      if (imageAbort === controller) imageAbort = undefined;
      submitting = false;
    }
  }
  async function startImage(options: RunOptions = {}) {
    if (
      !file ||
      (fileMode !== 'batch' && videoIndex === undefined) ||
      !destination.trim() ||
      suggestionPending ||
      submitting ||
      active
    )
      return;
    const targets = structuredClone($state.snapshot(fileMode === 'batch' ? files : [file])),
      controller = new AbortController(),
      statusId = batchRunId('image');
    imageAbort = controller;
    submitting = true;
    error = null;
    imageProgress = 0;
    const settings = structuredClone(
      $state.snapshot({
        framing: selectedFraming(activeFraming),
        temporal: selectedTemporal(temporal),
        trim: selectedTrim(trim),
        toneMap: sourceToneMap(),
        filters: sourceFilterRows(av1anFilters),
      }),
    );
    const format: ImageOutput =
      videoChoice === 'gif' ? 'gif' : videoChoice === 'png' ? 'pngSequence' : 'jpegSequence';
    const burnSource = mainFile ? structuredClone($state.snapshot(mainFile)) : undefined;
    const burnOrdinal =
      burnSource?.streams
        .filter((stream) => stream.kind === 'subtitle')
        .findIndex((stream) => String(stream.index) === activeBurn) ?? -1;
    const loadedSources = structuredClone($state.snapshot(files)),
      selectedTracks = structuredClone($state.snapshot(tracks));
    const imageMetadata = structuredClone(
      $state.snapshot(
        Object.fromEntries(targets.map((entry) => [entry.id, choiceImageMetadata(entry)])),
      ),
    );
    function choiceImageMetadata(entry: MediaFile) {
      return fileMode === 'batch'
        ? sourceImageMetadata(
            [entry],
            batchImageRows(entry),
            entry,
            metadataSource,
            chapterSource,
            false,
            outputContainer,
            stripAudio,
            stripSubs,
          )
        : sourceImageMetadata(
            loadedSources,
            selectedTracks,
            burnSource ?? entry,
            metadataSource,
            chapterSource,
            applyMetadata,
            outputContainer,
            stripAudio,
            stripSubs,
          );
    }
    const choice = structuredClone(
      $state.snapshot({
        cropMode,
        framing: copyFramingDraft(framing),
        destination,
        batchNameTemplate,
        imageQuality,
        imagePixel,
        fileMode,
        deinterlaceChoice,
        deinterlaceDouble,
        qtgmcPreset: temporal.qtgmcPreset,
        resizeFilter: temporal.resizeFilter,
        videoIndex,
        defaultOutputDirectory: sourcePreferences?.defaultOutputDirectory ?? '',
        naming: batchNamingValues(),
        namingDate: localNamingDate(),
      }),
    );
    const revision = destinationRevision;
    let imageFailed = false,
      imageCanceled = false;
    let statuses: Record<string, BatchRowStatus> = Object.fromEntries(
      targets.map((entry) => [entry.id, { glyph: '○', text: 'Queued' }]),
    );
    if (choice.fileMode === 'batch') onbatchstatus?.(statusId, statuses, true);
    function mark(id: string, status: BatchRowStatus) {
      if (choice.fileMode !== 'batch' || imageAbort !== controller) return;
      statuses = { ...statuses, [id]: status };
      onbatchstatus?.(statusId, statuses);
    }
    try {
      let batchItems: Awaited<ReturnType<typeof resolveBatchOutputPaths>>['items'] = [];
      if (choice.fileMode === 'batch') {
        try {
          const prepared = await resolveBatchOutputPaths(
            {
              inputs: targets.map((entry) => {
                const video = entry.streams.find((track) => track.kind === 'video');
                return {
                  inputPath: entry.path,
                  ...(video?.width ? { width: video.width } : {}),
                  ...(video?.height ? { height: video.height } : {}),
                };
              }),
              target: { kind: 'image', format },
              naming: choice.naming,
              outputNameTemplate: choice.batchNameTemplate,
              namingDate: choice.namingDate,
              defaultOutputDirectory: choice.defaultOutputDirectory,
            },
            controller.signal,
          );
          batchItems = prepared.items;
        } catch (cause) {
          const message = errorMessage(cause);
          imageFailed = true;
          error = message;
          onlog?.(message);
          for (const entry of targets)
            mark(
              entry.id,
              controller.signal.aborted
                ? {
                    glyph: '–',
                    text: 'Skipped',
                    note: 'The batch was stopped before this file ran.',
                  }
                : { glyph: '✕', text: 'Failed', note: message },
            );
          return;
        }
      }
      for (const [index, entry] of targets.entries()) {
        if (controller.signal.aborted) {
          mark(entry.id, {
            glyph: '–',
            text: 'Skipped',
            note: 'The batch was stopped before this file ran.',
          });
          continue;
        }
        imageProgress = index / targets.length;
        imageProgressDescription = `File ${index + 1}/${targets.length} (${entry.name})`;
        mark(entry.id, { glyph: '▶', text: 'Running…' });
        try {
          const stream = entry.streams.find(
            (stream) =>
              stream.kind === 'video' &&
              (choice.fileMode === 'batch' || stream.index === choice.videoIndex),
          );
          if (!stream) throw new Error(`${entry.name}: no video track.`);
          if (choice.cropMode === 'manual' && !cropFits(choice.framing, stream))
            throw new Error(`${entry.name}: crop leaves less than two pixels.`);
          let framingValue =
            choice.cropMode === 'manual'
              ? selectedFraming(alignedFraming(choice.framing, stream))
              : settings.framing;
          if (choice.cropMode === 'automatic') {
            const found = await detectCrop(
              { inputPath: entry.path, videoStreamIndex: stream.index },
              controller.signal,
            );
            if (!found.crop) throw new Error(`${entry.name}: automatic crop found no crop.`);
            framingValue = { ...framingValue, crop: found.crop };
          }
          const batchItem = batchItems[index];
          if (
            choice.fileMode === 'batch' &&
            (!batchItem ||
              batchItem.inputPath !== entry.path ||
              batchItem.error ||
              !batchItem.resolution)
          )
            throw new Error(
              batchItem?.error?.message ?? `Could not resolve the output for ${entry.name}.`,
            );
          const resolved =
            choice.fileMode === 'batch'
              ? batchItem.resolution!
              : await resolveOutputPath({
                  inputPath: entry.path,
                  outputStem: choice.destination.trim(),
                  target: { kind: 'image', format },
                });
          if (controller.signal.aborted) throw new Error('Canceled');
          const output = resolved.outputPath;
          if (
            choice.fileMode !== 'batch' &&
            revision === destinationRevision &&
            destination === choice.destination
          )
            destination = resolved.outputStem;
          let burn: ImageSubtitleSource | undefined;
          if (burnOrdinal >= 0) {
            const source = choice.fileMode === 'batch' ? entry : burnSource;
            const subtitle = source?.streams.filter((track) => track.kind === 'subtitle')[
              burnOrdinal
            ];
            if (source && subtitle)
              burn =
                source.path === entry.path
                  ? { source: 'primary', streamIndex: subtitle.index }
                  : { source: 'external', inputPath: source.path, streamIndex: subtitle.index };
            else
              onlog?.(`${entry.name}: the selected subtitle track is not available for burn-in.`);
          }
          const resolvedTemporal = await temporalForSource(
            entry,
            stream.index,
            settings.temporal,
            controller.signal,
            {
              choice: choice.deinterlaceChoice,
              double: choice.deinterlaceDouble,
              preset: choice.qtgmcPreset,
              resizeFilter: choice.resizeFilter,
            },
            output,
          );
          const imageRequest: ImageRequest = {
            operation: 'export',
            inputPath: entry.path,
            streamIndex: stream.index,
            fullSource: true,
            format,
            outputPath: output,
            width: null,
            ...(format === 'gif'
              ? { quality: choice.imageQuality }
              : format === 'jpegSequence'
                ? { quality: choice.imageQuality, pixelFormat: choice.imagePixel }
                : { pixelFormat: choice.imagePixel }),
            processing: {
              framing: framingValue,
              metadata: imageMetadata[entry.id],
              ...(burn ? { subtitleBurnIn: burn } : {}),
              ...(resolvedTemporal ? { temporal: resolvedTemporal } : {}),
              ...(settings.trim ? { trim: settings.trim } : {}),
              ...(settings.toneMap ? { toneMap: settings.toneMap } : {}),
              ...(settings.filters.length
                ? { filters: settings.filters, sourceFilters: true }
                : {}),
            },
          };
          const reviewed = await reviewAndRun(
            { kind: 'image', request: imageRequest },
            controller.signal,
            () => reviewThisFile(options),
          );
          if (!reviewed) {
            if (controller.signal.aborted) throw new DOMException('Canceled', 'AbortError');
            imageCanceled = true;
            imageFailed = true;
            mark(entry.id, {
              glyph: '✕',
              text: 'Failed',
              note: 'The command was cleared in the edit window, so nothing was run.',
            });
            onlog?.('The command was cleared in the edit window, so nothing was run.');
            continue;
          }
          if (reviewed.kind !== 'image')
            throw new Error('The command returned an unexpected encode job.');
          onlog?.(`Exported ${entry.name} to ${output}.`);
          mark(entry.id, { glyph: '✔', text: 'Done' });
        } catch (cause) {
          const message = errorMessage(cause);
          if (controller.signal.aborted) {
            mark(entry.id, { glyph: '■', text: 'Canceled', note: 'Stopped by the user.' });
          } else {
            imageFailed = true;
            mark(entry.id, { glyph: '✕', text: 'Failed', note: message });
            onlog?.(`${entry.name}: ${message}`);
            if (choice.fileMode !== 'batch') error = message;
          }
        }
      }
    } finally {
      if (imageAbort === controller) {
        imageAbort = undefined;
        imageProgress = 1;
        if (choice.fileMode === 'batch') {
          const rows = Object.values(statuses),
            done = rows.filter((row) => row.text === 'Done').length,
            failed = rows.filter((row) => row.text === 'Failed').length,
            skipped = rows.filter(
              (row) => row.text === 'Canceled' || row.text === 'Skipped',
            ).length;
          imageProgressDescription = `Batch ${controller.signal.aborted ? 'stopped' : 'done'} - ${done}/${targets.length} finished${failed ? `, ${failed} failed` : ''}${skipped ? `, ${skipped} not run` : ''}.`;
        } else
          imageProgressDescription =
            controller.signal.aborted || imageCanceled
              ? 'Canceled.'
              : imageFailed
                ? 'Did not finish - the log has the details.'
                : 'Done.';
        submitting = false;
      }
    }
  }
  async function start(queue = false, options: RunOptions = {}) {
    if (
      !chunked &&
      fileMode !== 'batch' &&
      tracks.length > 0 &&
      !tracks.some((row) => row.included)
    ) {
      onlog?.('No tracks are selected. Please select at least one track to use.');
      ontracklist?.();
      return;
    }
    if (!chunked && isImage && noVideoForRun) {
      await startImplicitImage(options);
      return;
    }
    if (isImage) {
      await startImage(options);
      return;
    }
    if (
      !(queue ? canQueue : canStart) ||
      !file ||
      (fileMode !== 'batch' &&
        !noVideoForRun &&
        videoIndex === undefined &&
        (encodingVideo || videoMode === 'copy')) ||
      (!noVideoForRun &&
        !batchWithoutVideo &&
        encodingVideo &&
        rate.mode === 'quality' &&
        crf === undefined) ||
      (!noVideoForRun &&
        !batchWithoutVideo &&
        encodingVideo &&
        (isSvtEncoder(encoder) || encoder === 'aomAv1') &&
        filmGrain === undefined)
    )
      return;
    const built = currentRequest();
    if (!built) return;
    if (!chunked && fileMode !== 'batch')
      logDroppedStreams(
        tracks
          .filter((row) => row.included)
          .flatMap((row) => {
            const stream = files
              .find((source) => source.id === row.sourceId)
              ?.streams.find((item) => item.index === row.streamIndex);
            return stream ? [stream] : [];
          }),
        outputContainer,
      );
    let request = structuredClone($state.snapshot(built));
    const runSource = structuredClone($state.snapshot(file));
    const runVideoIndex = request.settings.videoMode === 'drop' ? undefined : videoIndex,
      runCropMode = request.settings.videoMode === 'drop' ? 'disabled' : cropMode,
      runMode = fileMode;
    const runTarget = outputTarget(),
      runStem = destination,
      revision = destinationRevision;
    const deinterlace = {
      choice: deinterlaceChoice,
      double: deinterlaceDouble,
      preset: temporal.qtgmcPreset,
      resizeFilter: temporal.resizeFilter,
    };
    const controller = new AbortController();
    imageAbort = controller;
    submitting = true;
    error = null;
    const generation = draftGeneration;
    const identity = draftIdentity;
    try {
      if (runMode !== 'batch') {
        const resolved = await resolveOutputPath({
          inputPath: runSource.path,
          outputStem: runStem,
          target: runTarget,
        });
        request = { ...request, source: { ...request.source, outputPath: resolved.outputPath } };
        if (
          revision === destinationRevision &&
          destination === runStem &&
          isCurrentDraft(generation, identity)
        )
          destination = resolved.outputStem;
      }
      if (controller.signal.aborted) return;
      if (runMode !== 'batch' && runVideoIndex !== undefined) {
        const resolvedTemporal = await temporalForSource(
          runSource,
          runVideoIndex,
          request.settings.temporal,
          controller.signal,
          deinterlace,
          request.source.outputPath,
        );
        request = {
          ...request,
          settings: {
            ...request.settings,
            ...(resolvedTemporal ? { temporal: resolvedTemporal } : {}),
          },
        };
      }
      if (runCropMode === 'automatic' && runMode !== 'batch') {
        const detection = await detectCrop(
          { inputPath: runSource.path, videoStreamIndex: runVideoIndex! },
          controller.signal,
        );
        if (!detection.crop) throw new Error('Automatic crop detection did not find a crop.');
        request = {
          ...request,
          settings: {
            ...request.settings,
            framing: { ...request.settings.framing, crop: detection.crop },
          },
        };
      }
      if (controller.signal.aborted) return;
      if (runMode === 'batch') await runBatch(request, options, controller);
      else {
        const reviewed = await reviewAndRun({ kind: 'encode', request }, controller.signal, () =>
          reviewSingleFile(options),
        );
        if (!reviewed) {
          if (!controller.signal.aborted)
            onlog?.('The command was cleared in the edit window, so nothing was run.');
          return;
        }
        if (reviewed.kind !== 'encode')
          throw new Error('The command returned an unexpected image result.');
      }
    } catch (cause) {
      if (!controller.signal.aborted && isCurrentDraft(generation, identity))
        error = errorMessage(cause);
    } finally {
      // Keep the in-flight request locked across draft switches until its
      // authoritative reply settles, even when its draft is no longer visible.
      if (imageAbort === controller) imageAbort = undefined;
      batchCurrentJobId = null;
      submitting = false;
    }
  }
</script>

<section class="source-encode" aria-label={chunked ? 'AV1AN' : 'Quick Convert'}>
  <div
    class="tabs"
    role="tablist"
    aria-label={chunked ? 'AV1AN settings' : 'Quick Convert settings'}
  >
    {#each tabs as tab, index}<button
        role="tab"
        aria-selected={settingsTab === tab.id}
        class:active={settingsTab === tab.id}
        onkeydown={(event) => tabKeys(event, index)}
        onclick={() => (settingsTab = tab.id)}>{tab.label}</button
      >{/each}
  </div>
  <div class="tab-content">
    {#if settingsTab === 'video'}
      <div class="video-columns" use:optionTooltips>
        <div class="form">
          <label for={`${idPrefix}-codec`}>{chunked ? 'Encoder' : 'Codec'}</label>
          <select
            id={`${idPrefix}-codec`}
            bind:value={videoChoice}
            onchange={() => {
              if (isImage) {
                imageQuality = videoChoice === 'gif' ? 128 : videoChoice === 'png' ? 0 : 3;
                imagePixel = videoChoice === 'png' ? 'rgb24' : 'yuv420p';
              }
              if (videoChoice === 'copy' || videoChoice === 'drop') videoMode = videoChoice;
              else {
                videoMode = 'encode';
                if (!['gif', 'png', 'jpg'].includes(videoChoice)) {
                  if (selectedEncoder === videoChoice) {
                    if (rate.mode === 'quality') crf = options.defaultCrf;
                    preset = options.defaultPreset;
                    outputPixelFormat =
                      selectedEncoder === 'svtAv1' ||
                      selectedEncoder === 'aomAv1' ||
                      selectedEncoder === 'vpxStandalone'
                        ? 'yuv420p10le'
                        : 'yuv420p';
                    if (chunked) av1an = { ...av1an, pixelFormat: outputPixelFormat };
                  }
                  selectedEncoder = videoChoice as VideoEncoder;
                }
              }
              void validateDestination();
            }}
          >
            {#if !chunked}<option value="copy">Copy Video Without Re-Encoding</option><option
                value="drop">Disable (Strip Video)</option
              ><option value="x264">H.264 / AVC (x264)</option><option value="x265Standalone"
                >H.265 / HEVC (x265)</option
              ><option value="vpxStandalone">VP9 (VPX)</option><option value="svtAv1"
                >AV1 (SVT-AV1)</option
              ><option value="aomAv1">AV1 (AOM)</option><option value="gif"
                >GIF [Animated GIF]</option
              ><option value="png">PNG [Image Sequence]</option><option value="jpg"
                >JPEG [Image Sequence]</option
              >{:else}<option value="aomAv1">AV1 (AOM)</option><option value="svtAv1"
                >AV1 (SVT-AV1)</option
              ><option value="vpxStandalone">VP9 (VPX)</option><option value="x265Standalone"
                >H.265 / HEVC (x265)</option
              ><option value="x264">H.264 / AVC (x264)</option>{/if}
          </select>
          {#if !isImage}<label for={`${idPrefix}-container`}>Container</label><select
              id={`${idPrefix}-container`}
              bind:value={outputContainer}
              onchange={() => void validateDestination()}
              >{#if chunked}<option value="matroska">MKV</option><option value="webm">WEBM</option
                ><option value="mp4">MP4</option>{:else}<option value="mp4">MP4</option><option
                  value="matroska">MKV</option
                ><option value="webm">WEBM</option><option value="mov">MOV</option><option
                  value="m4a">M4A</option
                ><option value="ogg">OGG</option>{/if}</select
            >{/if}
          <label for={`${idPrefix}-quality-mode`}>Quality Mode</label>
          {#if chunked}<select
              id={`${idPrefix}-quality-mode`}
              value={av1an.targetEnabled ? av1an.target.metric : 'crf'}
              onchange={(event) => {
                const metric = event.currentTarget.value;
                if (metric === 'crf') crf = options.defaultCrf;
                av1an = {
                  ...av1an,
                  targetEnabled: metric !== 'crf',
                  target: {
                    ...av1an.target,
                    metric: metric === 'crf' ? 'vmaf' : (metric as Av1anDraft['target']['metric']),
                    ...(metricFields[metric]
                      ? {
                          minimumScoreTenths: metricFields[metric].default * 10,
                          maximumScoreTenths: metricFields[metric].default * 10,
                        }
                      : {}),
                  },
                };
              }}
              ><option value="crf">CRF</option><option value="vmaf">Target VMAF</option><option
                value="ssimulacra2">Target SSIMULACRA2</option
              ><option value="butteraugli">Target Butteraugli</option><option value="xpsnrWeighted"
                >Target XPSNR</option
              ></select
            >
          {:else}<select
              id={`${idPrefix}-quality-mode`}
              bind:value={rate.mode}
              onchange={() => {
                if (rate.mode === 'bitrate') rate = { ...rate, bitrate: 1500 };
                else if (rate.mode === 'targetSize') rate = { ...rate, targetSize: 50 };
                else crf = options.defaultCrf;
              }}
              disabled={!encodingVideo || isImage}
              ><option value="quality">CRF</option><option value="bitrate"
                >Target Bitrate (Kbps)</option
              ><option value="targetSize">Target Filesize (MB)</option></select
            >{/if}
          <label for={`${idPrefix}-quality`}>Quality</label>
          {#if isImage}<input
              id={`${idPrefix}-quality`}
              type="number"
              min={videoChoice === 'gif' ? 3 : 1}
              max={videoChoice === 'gif' ? 256 : 31}
              bind:value={imageQuality}
              disabled={videoChoice === 'png'}
            />{:else if chunked && av1an.targetEnabled}<input
              id={`${idPrefix}-quality`}
              type="number"
              min={metricFields[av1an.target.metric]?.min}
              max={metricFields[av1an.target.metric]?.max}
              step={metricFields[av1an.target.metric]?.step}
              value={av1an.target.minimumScoreTenths / 10}
              oninput={(event) =>
                (av1an = {
                  ...av1an,
                  target: {
                    ...av1an.target,
                    minimumScoreTenths: Math.round(event.currentTarget.valueAsNumber * 10),
                    maximumScoreTenths: Math.round(event.currentTarget.valueAsNumber * 10),
                  },
                })}
            />
          {:else if rate.mode === 'bitrate'}<input
              id={`${idPrefix}-quality`}
              type="number"
              min="10"
              max="100000"
              step="1"
              bind:value={rate.bitrate}
              disabled={!encodingVideo}
            />{:else if rate.mode === 'targetSize'}<input
              id={`${idPrefix}-quality`}
              type="number"
              min="0"
              max="8192"
              step="0.1"
              bind:value={rate.targetSize}
              disabled={!encodingVideo}
            />{:else}<input
              id={`${idPrefix}-quality`}
              type="number"
              min={options.crfMin}
              max={options.crfMax}
              step={1}
              bind:value={crf}
              disabled={!encodingVideo}
            />{/if}
          <label for={`${idPrefix}-preset`}>Preset / Effort</label><select
            id={`${idPrefix}-preset`}
            bind:value={preset}
            disabled={!encodingVideo || isImage}
            >{#each isImage || !encodingVideo ? [] : options.presets as choice}<option
                value={choice.value}>{choice.label}</option
              >{/each}</select
          >
        </div>
        <div class="form">
          <label for={`${idPrefix}-color`}>Color Format</label>{#if isImage}<select
              id={`${idPrefix}-color`}
              bind:value={imagePixel}
              disabled={videoChoice === 'gif'}
              >{#if videoChoice === 'png'}<option value="rgb24">RGB - 8 bit</option><option
                  value="rgba">RGBA - 8 bit</option
                ><option value="rgb48">RGB - 16 bit</option><option value="rgba64"
                  >RGBA - 16 bit</option
                >{:else if videoChoice === 'jpg'}<option value="yuv420p">YUV - 4:2:0 - 8 bit</option
                ><option value="yuv422p">YUV - 4:2:2 - 8 bit</option><option value="yuv444p"
                  >YUV - 4:4:4 - 8 bit</option
                >{/if}</select
            >{:else}<select
              id={`${idPrefix}-color`}
              value={chunked ? (av1an.pixelFormat ?? '') : (outputPixelFormat ?? '')}
              onchange={(event) => {
                const value = (event.currentTarget.value || undefined) as
                  Av1anPixelFormat | undefined;
                if (chunked) av1an = { ...av1an, pixelFormat: value };
                else outputPixelFormat = value;
              }}
              disabled={!encodingVideo}
              >{#each encodingVideo ? sourceColors : [] as format}<option value={format.value}
                  >{format.label}</option
                >{/each}</select
            >
          {/if}
          <label for={`${idPrefix}-fps`}>Frame Rate</label><input
            id={`${idPrefix}-fps`}
            value={fps}
            oninput={(event) => setFps(event.currentTarget.value)}
            placeholder="Keep original"
            disabled={!encodingVideo}
          />
          <span>Trim</span>
          <div class="inline">
            <button
              onclick={() => {
                dialog = 'trim';
              }}>Configure…</button
            >{#if trim.enabled}<button onclick={() => (trim = defaultTrim())}>Clear</button>{/if}
          </div>
          <label for={`${idPrefix}-crop`}>Crop</label>
          <div class="inline">
            <select id={`${idPrefix}-crop`} bind:value={cropMode} disabled={!encodingVideo}
              ><option value="disabled">Disabled</option><option value="automatic"
                >Automatic (detect black bars)</option
              ><option value="manual">Manual</option></select
            >{#if cropMode === 'manual'}<button onclick={() => openEdit('crop')}>Configure…</button
              >{/if}
          </div>
          <label for={`${idPrefix}-resize`}>Resize</label>
          <div class="inline">
            <select
              id={`${idPrefix}-resize`}
              value={resizeChoice}
              disabled={!encodingVideo}
              onchange={(event) => setResize(event.currentTarget.value)}
              >{#each resizeLabels as option}<option value={option.value}>{option.name}</option
                >{/each}</select
            >{#if resizeChoice === 'custom'}<button onclick={() => openEdit('resize')}
                >Configure…</button
              >{/if}
          </div>
          <p class="geometry-readout" aria-label="Output dimensions" title={geometry.resize}>
            {geometry.resize}
          </p>
          <label for={`${idPrefix}-borders`}>Borders</label><select
            id={`${idPrefix}-borders`}
            value={framing.borderRatio}
            disabled={!encodingVideo}
            onchange={(event) => setBorder(event.currentTarget.value)}
            ><option value="">No borders</option><option value="16:9">16:9 (Widescreen)</option
            ><option value="4:3">4:3 (Standard)</option><option value="1:1">1:1 (Square)</option
            ><option value="9:16">9:16 (Vertical)</option><option value="21:9"
              >21:9 (Ultrawide)</option
            ></select
          >
          <p class="geometry-readout" aria-label="Padding outcome" title={geometry.border}>
            {geometry.border}
          </p>
        </div>
        <div class="form">
          <label for={`${idPrefix}-grain`}>Grain Synthesis</label>
          <div class="inline">
            <select
              id={`${idPrefix}-grain`}
              bind:value={grainMode}
              disabled={!encodingVideo ||
                !(isSvtEncoder(encoder) || encoder === 'aomAv1') ||
                isImage}
              onchange={() => {
                if (grainMode === 'disabled') {
                  filmGrain = 0;
                  av1anGrain = undefined;
                } else if (grainMode === 'encoder') updateGrain();
                else {
                  grainDenoise = true;
                  if (grainMode === 'preset') void loadGrainPresets();
                }
              }}
              ><option value="disabled">No grain synthesis</option><option value="encoder"
                >Encoder analysis</option
              ><option value="table">Grain table file</option><option value="preset"
                >Film stock preset</option
              ></select
            >{#if grainMode === 'encoder'}<input
                aria-label="Grain synthesis strength"
                type="number"
                min="0"
                max="50"
                bind:value={filmGrain}
              /><label
                ><input type="checkbox" bind:checked={grainDenoise} onchange={updateGrain} /> Denoise</label
              >{:else if grainMode === 'table'}<input
                aria-label="Grain table file"
                placeholder="Path to a .tbl grain table"
                bind:value={grainPath}
                onblur={() => void loadGrainTable(grainPath)}
              /><button aria-label="Browse grain table" onclick={() => void chooseGrainTable()}
                >…</button
              >{:else if grainMode === 'preset'}<select
                aria-label="Film stock preset"
                bind:value={grainPreset}
                onchange={() => void generateGrainPreset()}
                >{#each grainPresets as choice}<option value={choice}>{choice}</option
                  >{/each}</select
              >{/if}{#if grainMode === 'table' || grainMode === 'preset'}<label
                ><input type="checkbox" bind:checked={grainDenoise} onchange={updateGrain} /> Denoise</label
              >{#if grainDenoise}<input
                  aria-label="Denoise strength"
                  type="number"
                  min="1"
                  max="16"
                  bind:value={grainStrength}
                  oninput={updateGrain}
                />{/if}{/if}
          </div>
          {#if selectedVideo && selectedVideo.fieldOrder && !['progressive', 'unknown'].includes(selectedVideo.fieldOrder)}<label
              for={`${idPrefix}-deinterlace`}>Deinterlace</label
            >
            <div class="inline">
              <select
                id={`${idPrefix}-deinterlace`}
                bind:value={deinterlaceChoice}
                disabled={!encodingVideo}
                ><option value="automatic">Automatic</option><option value="disabled"
                  >Disabled</option
                >{#if !chunked}<option value="qtgmc">QTGMC (VapourSynth)</option>{/if}<option
                  value="bwdif">Bwdif (FFmpeg)</option
                ><option value="yadif">Yadif (FFmpeg)</option></select
              >{#if !chunked && (deinterlaceChoice === 'qtgmc' || deinterlaceChoice === 'automatic')}<select
                  aria-label="QTGMC preset"
                  bind:value={temporal.qtgmcPreset}
                  ><option value="placebo">Placebo</option><option value="verySlow"
                    >Very Slow</option
                  ><option value="slower">Slower</option><option value="slow">Slow</option><option
                    value="medium">Medium</option
                  ><option value="fast">Fast</option><option value="faster">Faster</option><option
                    value="veryFast">Very Fast</option
                  ></select
                >{/if}{#if !chunked}<label
                  ><input
                    type="checkbox"
                    bind:checked={deinterlaceDouble}
                    disabled={deinterlaceChoice === 'disabled'}
                  /> One frame per field</label
                >{/if}
            </div>{/if}
          {#if knownHdr(selectedVideo)}<label for={`${idPrefix}-tone`}>Tone Mapping</label><select
              id={`${idPrefix}-tone`}
              disabled={!encodingVideo}
              value={toneMap.enabled ? toneMap.algorithm : 'off'}
              onchange={(event) =>
                (toneMap = {
                  ...toneMap,
                  enabled: event.currentTarget.value !== 'off',
                  algorithm: (event.currentTarget.value === 'off'
                    ? 'hable'
                    : event.currentTarget.value) as ToneMapDraft['algorithm'],
                })}
              ><option value="off">No tone mapping</option><option value="hable">Hable</option
              ><option value="mobius">Mobius</option><option value="reinhard">Reinhard</option
              >{#if !chunked}<option value="spline">Spline (GPU)</option>{/if}</select
            >{/if}
        </div>
      </div>
    {:else if settingsTab === 'audio'}
      <div class="form narrow">
        <label for={`${idPrefix}-audio-codec`}>Codec</label><select
          id={`${idPrefix}-audio-codec`}
          value={stripAudio ? 'strip' : audioCodec}
          onchange={(event) => {
            stripAudio = event.currentTarget.value === 'strip';
            if (!stripAudio) {
              audioCodec = event.currentTarget.value as AudioTrackDraft['codec'];
              if (audioCodec === 'copy') {
                audioChannels = 'preserve';
                audioConfig = 'same';
              }
            }
          }}
          ><option value="copy">Copy Audio Without Re-Encoding</option><option value="strip"
            >Disable (Strip Audio)</option
          ><option value="aac">AAC (Advanced Audio Coding)</option><option value="opus">Opus</option
          ><option value="vorbis">Vorbis</option><option value="eac3"
            >E-AC-3 (Dolby Digital Plus)</option
          ><option value="mp3">MP3</option><option value="flac"
            >FLAC (Free Lossless Audio Coding)</option
          ></select
        >
        {#if !chunked}<label for={`${idPrefix}-audio-config`}>Configuration</label>
          <div class="inline">
            <select
              id={`${idPrefix}-audio-config`}
              bind:value={audioConfig}
              onchange={() => {
                if (audioConfig === 'perTrack') {
                  if (
                    !file ||
                    !(
                      tracks.some((row) =>
                        files
                          .find((f) => f.id === row.sourceId)
                          ?.streams.some(
                            (stream) => stream.index === row.streamIndex && stream.kind === 'audio',
                          ),
                      ) || audio.length
                    ) ||
                    ['copy', 'flac'].includes(audioCodec)
                  ) {
                    error =
                      'Configure each track separately requires a loaded audio track and a codec with a custom bitrate.';
                    audioConfig = 'same';
                  } else openEdit('audio');
                }
              }}
              ><option value="same">Same settings for all tracks</option><option value="perTrack"
                >Configure each track separately</option
              ></select
            >{#if audioConfig === 'perTrack'}<button onclick={() => openEdit('audio')}
                >Configure…</button
              >{/if}
          </div>{/if}
        <label for={`${idPrefix}-audio-bitrate`}>Bitrate (Kbps)</label><input
          id={`${idPrefix}-audio-bitrate`}
          type="number"
          min="0"
          max="8192"
          step="8"
          bind:value={audioBitrate}
          disabled={stripAudio ||
            audioCodec === 'copy' ||
            audioCodec === 'flac' ||
            (!chunked && audioConfig === 'perTrack')}
        />
        <label for={`${idPrefix}-audio-channels`}>Channels</label><select
          id={`${idPrefix}-audio-channels`}
          bind:value={audioChannels}
          disabled={stripAudio || audioCodec === 'copy' || (!chunked && audioConfig === 'perTrack')}
          ><option value="preserve">0 (Keep original)</option>{#if !chunked}<option value="mono"
              >1 (Mono)</option
            >{/if}<option value="stereo">2 (Stereo)</option><option value="surround51"
            >6 (5.1 Surround)</option
          ><option value="surround71">8 (7.1 Surround)</option></select
        >
        {#if chunked}<span>Copy</span>
          <div class="inline">
            <label><input type="checkbox" bind:checked={copySubs} /> Subtitles</label><label
              ><input type="checkbox" bind:checked={copyData} /> Data Tracks</label
            ><label><input type="checkbox" bind:checked={copyAttachments} /> Attachments</label>
          </div>{:else}<label for="loudness">Loudness</label><select
            id="loudness"
            bind:value={loudness}
            disabled={stripAudio || audioCodec === 'copy'}
            ><option value="off">No normalization</option><option value="-14"
              >-14 LUFS (YouTube, Spotify)</option
            ><option value="-16">-16 LUFS (podcast, mobile)</option><option value="-23"
              >-23 LUFS (EBU R128 broadcast)</option
            ></select
          >{/if}
      </div>
    {:else if settingsTab === 'subtitles'}
      <div class="form narrow">
        <label for="subtitle-codec">Codec</label><select
          id="subtitle-codec"
          value={stripSubs ? 'strip' : subtitleMode}
          onchange={(event) => {
            stripSubs = event.currentTarget.value === 'strip';
            if (!stripSubs)
              subtitleMode = event.currentTarget.value as SubtitleTrackSettings['mode'];
          }}
          ><option value="copy">Copy Subtitles Without Re-Encoding</option><option value="strip"
            >Disable (Strip Subtitles)</option
          ><option value="movText">Mov_Text (3GPP Timed Text) - For MP4/MOV</option><option
            value="subRip">SRT (SubRip Text) - For MKV</option
          ><option value="webVtt">WebVTT (Web Video Text Tracks) - For WEBM</option></select
        ><label for="subtitle-burn">Burn In</label><select id="subtitle-burn" bind:value={burnIndex}
          ><option value="">Disabled</option
          >{#each mainFile?.streams.filter((s) => s.kind === 'subtitle') ?? [] as stream, index}<option
              value={String(stream.index)}>{burnLabel(stream, index)}</option
            >{/each}</select
        >{#if convertedMovText}<p class="geometry-readout subtitle-note">
            {convertedMovText === 1
              ? 'The MP4 text subtitle is converted to SubRip, because MKV cannot store MP4 text.'
              : `${convertedMovText} MP4 text subtitles are converted to SubRip, because MKV cannot store MP4 text.`}
          </p>{/if}
      </div>
    {:else if settingsTab === 'metadata'}
      <div class="metadata-head">
        <label
          >Copy Metadata From <select bind:value={metadataSource}
            ><option value="">None</option>{#if fileMode === 'batch'}<option value="current"
                >#1 - Current Input File</option
              >{:else}{#each files as entry}<option value={entry.path}>{entry.name}</option
                >{/each}{/if}</select
          ></label
        ><label
          >Copy Chapters From <select bind:value={chapterSource}
            ><option value="">None</option>{#if fileMode === 'batch'}<option value="current"
                >#1 - Current Input File</option
              >{:else}{#each files as entry}<option value={entry.path}>{entry.name}</option
                >{/each}{/if}</select
          ></label
        >
      </div>
      <label
        ><input type="checkbox" bind:checked={applyMetadata} /> Apply the titles and languages below</label
      >
      <table class="metadata-table">
        <thead><tr><th>Track</th><th>Title</th><th>Lang</th></tr></thead><tbody
          >{#if mainFile && fileMode !== 'batch'}<tr
              ><td>Output File</td><td
                ><input
                  aria-label="Title for output file"
                  disabled={!applyMetadata}
                  value={outputMetadataValue(mainFile).title}
                  oninput={(event) =>
                    editOutputMetadata(mainFile!, { title: event.currentTarget.value })}
                /></td
              ><td
                ><input
                  aria-label="Language for output file"
                  disabled={!applyMetadata}
                  value={outputMetadataValue(mainFile).language}
                  oninput={(event) =>
                    editOutputMetadata(mainFile!, { language: event.currentTarget.value })}
                /></td
              ></tr
            >{/if}{#each tracks.filter((t) => t.included) as row}<tr
              ><td
                title={`${files.find((f) => f.id === row.sourceId)?.name ?? ''} #${row.streamIndex}`}
                >{files.find((f) => f.id === row.sourceId)?.name ?? ''} #{row.streamIndex}</td
              ><td
                ><input
                  aria-label={`Title for track ${row.streamIndex}`}
                  disabled={!applyMetadata}
                  value={metadataValue(row).title}
                  oninput={(event) => editMetadata(row, { title: event.currentTarget.value })}
                /></td
              ><td
                ><input
                  aria-label={`Language for track ${row.streamIndex}`}
                  disabled={!applyMetadata}
                  value={metadataValue(row).language}
                  oninput={(event) => editMetadata(row, { language: event.currentTarget.value })}
                /></td
              ></tr
            >{/each}</tbody
        >
      </table>
    {:else if settingsTab === 'av1an'}
      <div class="video-columns av1an-infrastructure">
        <div class="form">
          <label for="split">Split Method</label><select id="split" bind:value={av1an.splitMethod}
            ><option value="fixedChunks">None</option><option value="sceneDetection"
              >Scene Change Detection</option
            ></select
          >
          <label for="chunk">Chunk Method</label><select id="chunk" bind:value={av1an.chunkMethod}
            ><option value="bestsource">BestSource</option><option value="lsmash">LSMASH</option
            ><option value="ffms2">FFMS2</option><option value="segment">Segment</option><option
              value="hybrid">Hybrid</option
            ><option value="select">Select</option></select
          >
          <label for="concat">Concat Method</label><select
            id="concat"
            bind:value={av1an.concatMethod}
            ><option value="mkvmerge">MKVMerge</option><option value="ffmpeg">FFmpeg</option
            ></select
          >
          <label for="order">Chunk Order</label><select id="order" bind:value={av1an.chunkOrder}
            ><option value="longToShort">Long-to-Short (Best CPU utilization)</option><option
              value="shortToLong">Short-to-Long (Not very useful)</option
            ><option value="sequential">Sequential (Same order as input video)</option><option
              value="random">Random (Most accurate filesize estimation)</option
            ></select
          >
          <label for="slices">Detection Slices</label><input
            id="slices"
            type="number"
            min="1"
            max="16"
            bind:value={av1an.sceneDetectionSlices}
          />
        </div>
        <div class="form">
          <label for="workers">Workers</label><input
            id="workers"
            type="number"
            min="1"
            max="64"
            bind:value={workers}
            oninput={(event) => {
              const n = event.currentTarget.valueAsNumber;
              if (Number.isInteger(n)) workerBaseline = n + (encoder === 'svtAv1' ? 2 : 0);
            }}
          /><label for="threads">Threads per Worker</label><input
            id="threads"
            type="number"
            min="1"
            max="64"
            bind:value={av1an.encoderThreads}
          />
        </div>
      </div>
    {:else}
      <h3>ENCODER ARGUMENTS</h3>
      <p>Right-click an argument for a full explanation and example values.</p>
      <AdvancedEncoderOptions
        {encoder}
        {backend}
        catalogRevision={toolsRevision}
        catalogUnavailable={toolsLoading}
        value={parameters}
        raw={rawHdr}
        onraw={updateRaw}
        onresource={(name, path) => void resolveGridResource(name, path)}
        disabled={submitting}
        onchange={(value) => (parameters = value)}
        oncatalog={(catalog, revision) => {
          if (revision === toolsRevision && !toolsLoading)
            parameterCatalogs = {
              ...parameterCatalogs,
              [`${revision}:${catalog.backend}:${catalog.encoder}`]: catalog,
            };
        }}
      />
      <div class="filter-head">
        <h3>CUSTOM VIDEO FILTERS</h3>
        <button
          onclick={() => {
            av1anFilters = [...av1anFilters, ''];
            selectedFilterIndex = av1anFilters.length - 1;
          }}>Add</button
        ><button
          onclick={() => {
            const index = selectedFilterIndex ?? av1anFilters.length - 1;
            if (index >= 0) av1anFilters = av1anFilters.filter((_, i) => i !== index);
            selectedFilterIndex = null;
          }}>Remove</button
        >
      </div>
      <table>
        <thead><tr><th>Filter</th></tr></thead><tbody
          >{#each av1anFilters as filter, index}<tr
              ><td
                ><input
                  aria-label={`Filter ${index + 1}`}
                  value={filter}
                  onfocus={() => (selectedFilterIndex = index)}
                  oninput={(event) => {
                    av1anFilters = av1anFilters.map((row, i) =>
                      i === index ? event.currentTarget.value : row,
                    );
                  }}
                /></td
              ></tr
            >{/each}</tbody
        >
      </table>
    {/if}
  </div>
  <div class="output">
    <label for={`${idPrefix}-output`}>Output Path</label><input
      id={`${idPrefix}-output`}
      bind:value={destination}
      oninput={() => {
        ++destinationRevision;
        suggestionPending = false;
        suggestedStem = null;
      }}
    /><button onclick={chooseOutput} aria-label="Browse output path">…</button>{#if chunked}<button
        onclick={() => void openRecoveries()}
        >Resume…{recoveries.length ? ` (${recoveries.length})` : ''}</button
      ><button disabled={!recoveries.length} onclick={() => void openClearTemp()}
        >Clear Temp…</button
      >{/if}
  </div>
  {#if error}<p role="alert">{error}</p>{/if}{#if sequenceIssue}<p role="alert">
      AV1AN does not support image sequence folders.
    </p>{/if}{#if sourceTrackIssue}<p role="alert">
      {sourceTrackIssue}
    </p>{/if}{#if containerIssue}<p role="alert">
      {containerIssue}
    </p>{/if}{#if encodingVideo && subtitleIssue}<p role="alert">
      {subtitleIssue}
    </p>{/if}{#if parsedHdr.error}<p role="alert">{parsedHdr.error}</p>{/if}{#if gridGrainIssue}<p
      role="alert"
    >
      {gridGrainIssue}
    </p>{/if}{#if gridGrain && grainMode !== 'disabled'}<p>
      The Advanced grain table takes precedence over grain analysis.
    </p>{/if}
</section>
<EditCommandDialog
  open={commandDialogOpen}
  program={commandDialogProgram}
  bind:value={commandDialogText}
  onconfirm={() => closeCommandDialog(true)}
  oncancel={() => closeCommandDialog()}
/>
<SourceDialog
  title="Resume AV1AN Encode"
  open={recoveryDialog === 'list'}
  onclose={() => (recoveryDialog = null)}
  width={820}
>
  <p>Unfinished av1an encodes found in the temp folder. Pick one to resume or delete.</p>
  <div
    class="recovery-list"
    role="listbox"
    aria-label="Unfinished AV1AN encodes"
    tabindex="0"
    data-dialog-initial-focus
    use:recoveryKeys
  >
    {#each recoveries as entry (entry.job.id)}<button
        role="option"
        tabindex={recoverySelected === entry.job.id ? 0 : -1}
        aria-selected={recoverySelected === entry.job.id}
        class:selected={recoverySelected === entry.job.id}
        onclick={() => (recoverySelected = entry.job.id)}>{recoveryLabel(entry)}</button
      >{/each}
  </div>
  {#if recoveryError}<p role="alert">{recoveryError}</p>{/if}
  <div class="dialog-actions">
    <button
      disabled={recoveryBusy || !recoverySelected}
      onclick={() => void recoveryAction('deleteEncode')}>Delete Encode</button
    ><button
      disabled={recoveryBusy || !recoverySelected}
      onclick={() => void recoveryAction('deleteChunks')}>Delete Chunks</button
    ><button
      disabled={recoveryBusy || !recoverySelected}
      onclick={(event) => void recoveryAction('resumeNew', event.shiftKey)}
      >Resume (New Settings)</button
    ><button
      disabled={recoveryBusy || !recoverySelected}
      onclick={(event) => void recoveryAction('resumeSaved', event.shiftKey)}
      >Resume (Saved Settings)</button
    >
  </div>
</SourceDialog>
<SourceDialog
  title="Clear all av1an temp files?"
  open={recoveryDialog === 'clear'}
  onclose={() => (recoveryDialog = null)}
>
  <p>
    Delete the temporary files of {clearReviewed.length} unfinished encode{clearReviewed.length ===
    1
      ? ''
      : 's'}?
  </p>
  <p>
    That frees {clearBytes} of encoded chunks. None of them can be resumed afterwards - each would have
    to start over from the beginning.
  </p>
  {#if recoveryError}<p role="alert">{recoveryError}</p>{/if}
  <div class="dialog-actions">
    <button disabled={recoveryBusy} onclick={() => (recoveryDialog = null)}>No</button><button
      disabled={recoveryBusy}
      onclick={() => void recoveryAction('clearAll')}>Yes</button
    >
  </div>
</SourceDialog>
<SourceDialog title="Configure Crop" open={dialog === 'crop'} onclose={cancelEdit} width={460}>
  <h3>PIXELS TO CROP OFF EACH SIDE</h3>
  <div class="form dialog-form">
    {#each ['left', 'right', 'top', 'bottom'] as edge}<label for={`crop-${edge}`}
        >{edge[0].toUpperCase() + edge.slice(1)}</label
      ><input
        id={`crop-${edge}`}
        type="number"
        min="0"
        step="2"
        value={framing.crop[edge as keyof typeof framing.crop]}
        max={selectedVideo
          ? Math.max(
              0,
              (edge === 'left' || edge === 'right'
                ? (selectedVideo.width ?? 0)
                : (selectedVideo.height ?? 0)) - 2,
            )
          : 16384}
        oninput={(event) =>
          editCrop(edge as 'left' | 'right' | 'top' | 'bottom', event.currentTarget.valueAsNumber)}
      />{/each}
  </div>
  <h3>RESULTING CROP AREA</h3>
  <p>
    {cropReadout}
  </p>
  <div class="dialog-actions">
    <button
      onclick={() => {
        framing = { ...framing, crop: { top: 0, right: 0, bottom: 0, left: 0 } };
        cropReduced = false;
      }}>Reset</button
    ><button onclick={() => (dialog = null)}>OK</button>
  </div>
</SourceDialog>
<SourceDialog title="Configure Resize" open={dialog === 'resize'} onclose={cancelEdit}>
  <h3>SOURCE</h3>
  <p>{resizeSourceDescription(activeFraming, geometryStream)}</p>
  <h3>TARGET</h3>
  <div class="form dialog-form">
    <label for="resize-mode">Mode</label><select id="resize-mode" bind:value={framing.resizeMode}
      ><option value="fit">Fit inside a box (keeps the aspect ratio)</option><option value="height"
        >Set the height (width follows)</option
      ><option value="width">Set the width (height follows)</option><option value="percent"
        >Scale by a percentage</option
      ><option value="exact">Exact size</option></select
    >
    {#if ['fit', 'width', 'exact'].includes(framing.resizeMode)}<label for="resize-width"
        >Width</label
      ><input
        id="resize-width"
        type="number"
        min="16"
        max="16384"
        step="2"
        bind:value={framing.resizeWidth}
        onblur={() =>
          (framing.resizeWidth = Math.min(
            16384,
            Math.max(16, Math.trunc(framing.resizeWidth || 16)),
          ))}
      />{/if}
    {#if ['fit', 'height', 'exact'].includes(framing.resizeMode)}<label for="resize-height"
        >Height</label
      ><input
        id="resize-height"
        type="number"
        min="16"
        max="16384"
        step="2"
        bind:value={framing.resizeHeight}
        onblur={() =>
          (framing.resizeHeight = Math.min(
            16384,
            Math.max(16, Math.trunc(framing.resizeHeight || 16)),
          ))}
      />{/if}
    {#if framing.resizeMode === 'percent'}<label for="resize-percent">Percentage</label><input
        id="resize-percent"
        type="number"
        min="1"
        max="800"
        step="5"
        bind:value={framing.resizePercent}
        onblur={() =>
          (framing.resizePercent = Math.min(
            800,
            Math.max(1, Math.trunc(framing.resizePercent || 1)),
          ))}
      />{/if}
    {#if framing.resizeMode === 'exact'}<label for="resize-fill">If it doesn't fit</label><select
        id="resize-fill"
        bind:value={framing.resizeFill}
        ><option value="pad">Letterbox with black bars</option><option value="crop"
          >Crop off the overflow</option
        ><option value="stretch">Stretch to fit</option></select
      >{/if}
  </div>
  <h3>OPTIONS</h3>
  <div class="form dialog-form">
    <label for="resize-round">Round sizes to</label><select
      id="resize-round"
      bind:value={framing.resizeModulus}
      ><option value={2}>2 (default)</option><option value={4}>4</option><option value={8}>8</option
      ><option value={16}>16</option></select
    ><label for="resize-filter">Resampler</label><select
      id="resize-filter"
      bind:value={temporal.resizeFilter}
      ><option value="bicubic">Bicubic (default)</option><option value="lanczos">Lanczos</option
      ><option value="spline">Spline</option><option value="bilinear">Bilinear</option><option
        value="area">Area</option
      ><option value="nearest">Nearest neighbor</option></select
    >
  </div>
  <label
    ><input
      type="checkbox"
      bind:checked={framing.resizeAllowUpscale}
      disabled={framing.resizeMode === 'percent' ||
        (framing.resizeMode === 'exact' && framing.resizeFill !== 'pad')}
    /> Allow upscaling past the source's own size</label
  ><br /><label
    ><input
      type="checkbox"
      bind:checked={framing.resizeCorrectAspect}
      disabled={(() => {
        const [n, d] = (selectedVideo?.sampleAspectRatio ?? '1:1').split(/[:/]/).map(Number);
        return !n || !d || n === d;
      })()}
    /> Correct non-square pixels</label
  >
  <h3>RESULT</h3>
  <p>
    {resizeDialogDescription(activeFraming, geometryStream)}
  </p>
  <div class="dialog-actions">
    <button
      onclick={() => {
        framing = {
          ...framing,
          resizeEnabled: true,
          resizeMode: 'fit',
          resizeWidth: displaySize().width,
          resizeHeight: displaySize().height,
          resizeFill: 'pad',
          resizeModulus: 2,
          resizeAllowUpscale: false,
          resizeCorrectAspect: true,
        };
        temporal = { ...temporal, resizeFilter: 'bicubic' };
      }}>Reset</button
    ><button onclick={() => (dialog = null)}>OK</button>
  </div>
</SourceDialog>
<SharedCutDialog
  open={dialog === 'trim'}
  {file}
  videoStreamIndex={videoIndex}
  value={trim}
  purpose={chunked ? 'av1an' : 'quick'}
  videoCopied={videoMode === 'copy'}
  onconfirm={(value) => {
    trim = value;
    dialog = null;
  }}
  onclose={() => (dialog = null)}
/>
<SourceDialog title="Configure Audio Tracks" open={dialog === 'audio'} onclose={cancelEdit}>
  <p>Configure channels and bitrate for each audio track.</p>
  <table>
    <thead
      ><tr><th>Track</th><th>Title</th><th>Lang</th><th>Channels</th><th>Bitrate (Kbps)</th></tr
      ></thead
    ><tbody
      >{#each perTrackRows as row, index}{@const source = files.find(
          (f) => f.id === row.sourceId,
        )}{@const stream = source?.streams.find((s) => s.index === row.streamIndex)}<tr
          ><td>#{index + 1}</td><td>{stream?.title?.slice(0, 35) || 'None'}</td><td
            >{stream?.language?.toUpperCase() ?? ''}</td
          ><td
            ><input
              aria-label={`Channels for track ${index + 1}`}
              type="number"
              min="1"
              max="24"
              step="1"
              bind:value={row.channels}
            /></td
          ><td
            ><input
              aria-label={`Bitrate for track ${index + 1}`}
              type="number"
              min="8"
              max="8192"
              step="1"
              bind:value={row.bitrate}
            /></td
          ></tr
        >{/each}</tbody
    >
  </table>
  <div class="dialog-actions">
    <button
      onclick={() => {
        perTrackRows = perTrackRows.map((row) => ({
          ...row,
          channels: Math.min(24, Math.max(1, Math.trunc(row.channels || 1))),
          bitrate: Math.min(8192, Math.max(8, Math.trunc(row.bitrate || 8))),
        }));
        configuredAudioIdentity = audioIdentity();
        dialog = null;
      }}>OK</button
    >
  </div>
</SourceDialog>

<style>
  .recovery-list {
    min-height: 220px;
    max-height: 45vh;
    overflow: auto;
    border: 1px solid var(--border);
    display: flex;
    flex-direction: column;
  }
  .recovery-list button {
    text-align: left;
    border: 0;
    background: transparent;
    padding: 6px;
    color: inherit;
  }
  .recovery-list button.selected {
    background: var(--accent);
  }
  .source-encode {
    display: flex;
    flex-direction: column;
    min-height: 0;
    height: 100%;
    gap: 8px;
  }
  .tabs {
    display: flex;
    gap: 2px;
    border-bottom: 1px solid var(--border);
    flex-wrap: wrap;
  }
  .tabs button {
    border: 1px solid transparent;
    border-bottom: 0;
    background: transparent;
    color: inherit;
    padding: 6px 13px;
    cursor: pointer;
  }
  .tabs button.active {
    border-color: var(--border);
    background: var(--background);
    font-weight: 600;
  }
  .tab-content {
    flex: 1;
    overflow: auto;
    padding: 6px 4px;
    min-height: 0;
  }
  .video-columns {
    display: grid;
    grid-template-columns: repeat(3, minmax(0, 1fr));
    align-items: start;
    gap: 12px 20px;
  }
  .form {
    display: grid;
    grid-template-columns: minmax(88px, 105px) minmax(0, 1fr);
    align-items: center;
    gap: 5px 8px;
    font-size: 13px;
    min-width: 0;
  }
  .video-columns .form > select {
    width: min(100%, 22ch);
  }
  .video-columns .form > select#split {
    width: min(100%, 26ch);
  }
  .video-columns .form > select#order {
    width: min(100%, 42ch);
  }
  .video-columns .form > input {
    width: min(100%, 14ch);
  }
  .video-columns .form > select[id$='-codec'] {
    width: min(100%, 24ch);
  }
  .video-columns .form > select[id$='-container'] {
    width: min(100%, 12ch);
  }
  .video-columns .form > select[id$='-quality-mode'] {
    width: min(100%, 23ch);
  }
  .video-columns .form > select[id$='-preset'] {
    width: min(100%, 9ch);
  }
  .video-columns .form > input[id$='-quality'] {
    width: min(100%, 8ch);
  }
  .video-columns .form > input[id$='-fps'] {
    width: min(100%, 15ch);
  }
  .video-columns .inline > select {
    width: min(100%, 22ch);
  }
  .video-columns .inline > select[id$='-deinterlace'] {
    width: 190px;
    flex: 0 0 190px;
  }
  @media (max-width: 1050px) {
    .video-columns {
      grid-template-columns: repeat(2, minmax(0, 1fr));
      gap: 10px 16px;
    }
    .video-columns.av1an-infrastructure {
      grid-template-columns: minmax(0, 1.15fr) minmax(0, 0.85fr);
    }
    .form {
      grid-template-columns: minmax(84px, 98px) minmax(0, 1fr);
    }
    .video-columns > .form:nth-child(3) {
      grid-column: 1 / -1;
    }
  }
  .geometry-readout {
    grid-column: 1 / -1;
    min-width: 0;
    margin: 0;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
    font-size: 11px;
    line-height: 12px;
    color: var(--muted-foreground);
  }
  .subtitle-note {
    margin-top: 4px;
    white-space: normal;
    line-height: 15px;
  }
  @media (max-width: 1050px) {
    .video-columns {
      row-gap: 6px;
    }
    .video-columns .form {
      row-gap: 2px;
    }
    /* Leave room for Windows native control metrics in the two-column form. */
    .video-columns select,
    .video-columns input:not([type='checkbox']),
    .video-columns button {
      padding-block: 4px;
    }
  }
  @media (max-width: 1050px) and (max-height: 750px) {
    .tab-content {
      padding-block: 4px;
    }
    .video-columns {
      row-gap: 0;
    }
    .video-columns .form {
      row-gap: 1px;
    }
  }
  .narrow {
    justify-content: start;
    grid-template-columns: 130px minmax(180px, 320px);
  }
  .inline {
    display: flex;
    align-items: center;
    flex-wrap: wrap;
    gap: 6px;
    min-width: 0;
  }
  select,
  input:not([type='checkbox']) {
    min-width: 0;
    max-width: 100%;
    padding: 5px 7px;
    border: 1px solid var(--border);
    background: var(--background);
    color: inherit;
    font: inherit;
  }
  .form input[type='number'] {
    width: 8ch;
  }
  .inline input[type='number'] {
    width: 65px;
  }
  button {
    cursor: pointer;
    color: inherit;
    background: var(--background);
    border: 1px solid var(--border);
    padding: 5px 10px;
    font: inherit;
  }
  .output {
    display: flex;
    gap: 6px;
    align-items: center;
    font-size: 13px;
  }
  .output input {
    flex: 1;
  }
  .metadata-head,
  .filter-head {
    display: flex;
    align-items: center;
    gap: 8px;
    flex-wrap: wrap;
    margin-bottom: 10px;
  }
  .filter-head h3 {
    flex: 1;
  }
  h3 {
    font-size: 12px;
    margin: 14px 0 8px;
  }
  p {
    font-size: 12px;
  }
  /* A reason that blocks Run must read as an error, not as a hint. */
  p[role='alert'] {
    color: var(--destructive);
    font-weight: 600;
  }
  table {
    width: 100%;
    border-collapse: collapse;
    margin-top: 8px;
    font-size: 12px;
  }
  th,
  td {
    text-align: left;
    border-bottom: 1px solid var(--border);
    padding: 5px;
  }
  .metadata-table {
    table-layout: fixed;
  }
  .metadata-table th:first-child {
    width: 28%;
  }
  .metadata-table th:last-child {
    width: 100px;
  }
  .metadata-table td:first-child {
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  .metadata-table input {
    width: 100%;
    min-width: 0;
  }
  .dialog-form {
    justify-content: start;
  }
  .dialog-actions {
    display: flex;
    justify-content: flex-end;
    gap: 8px;
    margin-top: 18px;
  }
  .tabs {
    gap: 2px;
  }
  .tabs button.active {
    color: var(--primary-foreground);
    background: var(--primary);
    border-color: var(--primary);
  }
  .tabs button:not(.active):hover {
    background: var(--secondary);
  }
  .recovery-list button.selected {
    background: var(--accent);
    box-shadow: inset 3px 0 var(--primary);
  }
</style>
