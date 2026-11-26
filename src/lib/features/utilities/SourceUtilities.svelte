<script lang="ts">
  import { onDestroy, untrack } from 'svelte';
  import { batchRunId } from '$lib/source-workspace';
  import SourceDialog from '$lib/components/shared/SourceDialog.svelte';
  import SharedCutDialog from '$lib/components/shared/SharedCutDialog.svelte';
  import {
    analyzeBitrate,
    analyzeQuality,
    chooseUtilityFile,
    inspectUtilityCapabilities,
    readColorMetadata,
    isDesktop,
    runUtility,
    runUtilityBatch,
    resolveDeinterlace,
    setMediaAnalysisPaused,
  } from '$lib/ipc/client';
  import { copyTextWithFeedback } from '$lib/components/shared/clipboard';
  import {
    errorMessage,
    fileName,
    formatBytes,
    formatDuration,
  } from '$lib/components/shared/format';
  import type {
    BitrateResult,
    CrfLadderRequest,
    DeinterlaceExportMethod,
    FieldOrder,
    GrainRequest,
    LadderEncoder,
    LadderMetric,
    MediaFile,
    QualityAlignment,
    QualityMetric,
    QualityRequest,
    QualityVmafModel,
    QtgmcPreset,
    UtilityRequest,
    UtilityResult,
    UtilityProgress,
  } from '$lib/ipc/generated';
  import type {
    FileListMode,
    RegisterWorkspaceAction,
    TrackSelection,
    BatchRowStatus,
  } from '$lib/source-workspace';
  import contentPresets from '$lib/content-presets.json';
  let {
    files,
    file,
    tracks,
    fileMode,
    registerAction,
    onbatchstatus,
    onlog,
    onswitchtomux,
  }: {
    files: MediaFile[];
    file?: MediaFile;
    tracks: TrackSelection[];
    fileMode: FileListMode;
    registerAction: RegisterWorkspaceAction;
    onbatchstatus: (
      statuses: Record<string, BatchRowStatus>,
      runId: string,
      begin?: boolean,
    ) => void;
    onlog: (message: string, error?: boolean) => void;
    onswitchtomux?: () => void;
  } = $props();
  type Task =
    | 'bitrates'
    | 'metrics'
    | 'color'
    | 'deinterlace'
    | 'cadence'
    | 'grain'
    | 'ladder'
    | 'cut'
    | 'concat'
    | 'chart'
    | 'ocr';
  type Dialog =
    | 'metrics'
    | 'color'
    | 'deinterlace'
    | 'grain'
    | 'ladder'
    | 'cut'
    | 'ladderResult'
    | 'chartResult'
    | null;
  const cards: { id: Task; label: string; description: string; configure?: boolean }[] = [
    {
      id: 'bitrates',
      label: 'Read Bitrates',
      description: 'Calculates stream size and average bitrate for each selected track.',
    },
    {
      id: 'metrics',
      label: 'Get Metrics',
      description: 'Calculate quality metrics like VMAF, SSIM and PSNR between two loaded files.',
      configure: true,
    },
    {
      id: 'color',
      label: 'Transfer Color Metadata',
      description: 'Copy color properties and HDR metadata from one file to another.',
      configure: true,
    },
    {
      id: 'deinterlace',
      label: 'Deinterlace Video',
      description:
        'Exports a deinterlaced copy - QTGMC over the whole file into a near-lossless MKV, audio and subtitles copied across. Has its own settings, separate from the encode tabs.',
      configure: true,
    },
    {
      id: 'cadence',
      label: 'Repair Frame Cadence',
      description:
        "Removes the duplicate frames a bad capture padded a file with, writing a constant-rate copy that is still interlaced. No settings - the recording's own length is the answer. Refuses a file whose frame count already matches it.",
    },
    {
      id: 'grain',
      label: 'Film Grain (AV1)',
      description:
        'Measures a grain table off any source, or reads, writes and strips the film grain in a finished AV1 file without re-encoding it.',
      configure: true,
    },
    {
      id: 'ladder',
      label: 'Sample Encodes',
      description:
        'Encodes short sections from across the source at several CRFs, then reports the bitrate, the size a whole file would come to, and a VMAF score for each.',
      configure: true,
    },
    {
      id: 'cut',
      label: 'Cut Video',
      description:
        'Keep only a chosen section of the video, copied out without re-encoding. Pick the start and end point while watching the frame you are on.',
      configure: true,
    },
    {
      id: 'concat',
      label: 'Concatenate Into Single MKV',
      description: 'Merge all loaded files into a single MKV without re-encoding.',
    },
    {
      id: 'chart',
      label: 'Show Bitrate Chart',
      description: 'Samples the bitrate across the whole video and plots it.',
    },
    {
      id: 'ocr',
      label: 'OCR Bitmap Subtitles',
      description: 'Converts selected bitmap-based subtitle tracks into text subtitles.',
    },
  ];
  let task = $state<Task | null>(null),
    dialog = $state<Dialog>(null),
    activeTask = $state<Task | null>(null),
    runningLabel = $state(''),
    paused = $state(false),
    pausing = $state(false),
    analysisId = $state<string | null>(null),
    progress = $state<UtilityProgress | null>(null);
  let completedStatus = $state('');
  let ladderCopied = $state(false);
  let ladderCopyTimer: ReturnType<typeof setTimeout> | undefined;
  const pending = $derived(activeTask !== null);
  let unsupportedBatchTask = $state<Task | null>(null);
  const batchReasons: Partial<Record<Task, string>> = {
    concat:
      'It joins the whole file list into one output, which is the opposite of running per file.',
    metrics:
      'It compares two loaded files against each other, so a single file means nothing to it.',
    chart:
      'It opens a chart window for the file it analysed, and a queue would raise one per file.',
    ladder:
      'It ends in a results window, which a queue would raise once per file with nobody there - and a ladder is read per source anyway, a CRF that suits one file being the wrong one for the next.',
  };
  let abort: AbortController | undefined;
  let metricsEncoded = $state(''),
    metricsReference = $state('');
  let metricsVmaf = $state(true),
    metricsSsim = $state(false),
    metricsPsnr = $state(false),
    alignment = $state<QualityAlignment>('none'),
    subsample = $state(1),
    vmafModel = $state<QualityVmafModel>('standard');
  let colorSource = $state(''),
    colorTarget = $state(''),
    copyColor = $state(true),
    copyHdr = $state(true);
  let deinterlace = $state<'automatic' | 'disabled' | 'qtgmc' | 'bwdif' | 'yadif'>('qtgmc'),
    doubleRate = $state(true),
    qtgmcPreset = $state<QtgmcPreset>('verySlow');
  let grainOp = $state<'measureSource' | 'extract' | 'apply' | 'remove'>('measureSource'),
    grainSource = $state<'table' | 'preset' | 'photonNoise'>('table'),
    grainTable = $state(''),
    grainPreset = $state('Super8'),
    grainPresets = $state([
      'Super8',
      'MaxMid',
      '16mm',
      '16mm-1',
      '16mm-2',
      '16mm-3',
      'Classic35',
      'Classic35-1',
      'Classic35-2',
      'Classic35-3',
      'Modern35',
      'Modern35-1',
      'Modern35-2',
      'Modern35-3',
    ]),
    denoise = $state(4),
    keepDenoised = $state(false),
    iso = $state(400),
    chroma = $state(false);
  let grainAvailable = $state<boolean | null>(null);
  let ladderEncoder = $state<LadderEncoder>('av1'),
    ladderPreset = $state('4'),
    ladderPixel = $state('yuv420p10le'),
    ladderContent = $state(''),
    ladderCrfs = $state(''),
    sampleCount = $state(3),
    sampleSeconds = $state(10),
    ladderMetric = $state<LadderMetric>('vmaf'),
    ladderModel = $state<QualityVmafModel>('standard'),
    keepSamples = $state(false);
  $effect(() => {
    if (['h264Nvenc', 'hevcNvenc'].includes(ladderEncoder))
      untrack(() => {
        ladderEncoder = 'av1';
        ladderPreset = '4';
        ladderPixel = 'yuv420p10le';
        ladderContent = '';
        ladderCrfs = '';
      });
  });
  let cutStart = $state(0),
    cutEnd = $state(0),
    cutConfigured = $state(false);
  const cutValue = $derived({
    enabled: cutConfigured,
    mode: 'time' as const,
    startFrame: 0,
    endFrameExclusive: undefined,
    startMilliseconds: Math.round(cutStart * 1000),
    endMilliseconds: Math.round(cutEnd * 1000),
  });
  let ladderResult = $state<UtilityResult | null>(null),
    chartResult = $state<BitrateResult | null>(null);
  let chartStart = $state<number | null>(null),
    chartEnd = $state<number | null>(null),
    chartDragging = $state(false);
  const selectedChartPoints = $derived(
    chartResult && chartStart !== null && chartEnd !== null
      ? chartResult.points.filter(
          (_, index) =>
            index >= Math.min(chartStart!, chartEnd!) && index <= Math.max(chartStart!, chartEnd!),
        )
      : [],
  );
  const CHART_VB_WIDTH = 640,
    CHART_PLOT_LEFT = 56,
    CHART_PLOT_RIGHT = 624,
    CHART_PLOT_WIDTH = CHART_PLOT_RIGHT - CHART_PLOT_LEFT;
  function chartX(index: number, count: number) {
    return CHART_PLOT_LEFT + (index * CHART_PLOT_WIDTH) / Math.max(1, count - 1);
  }
  function chartIndex(event: PointerEvent) {
    const svg = (event.currentTarget as HTMLElement).querySelector('svg') as SVGSVGElement | null;
    const ctm = svg?.getScreenCTM();
    let svgX: number;
    if (svg && ctm) {
      const point = svg.createSVGPoint();
      point.x = event.clientX;
      point.y = event.clientY;
      svgX = point.matrixTransform(ctm.inverse()).x;
    } else {
      const rect = (svg ?? (event.currentTarget as HTMLElement)).getBoundingClientRect();
      svgX = ((event.clientX - rect.left) / Math.max(1, rect.width)) * CHART_VB_WIDTH;
    }
    const last = Math.max(0, (chartResult?.points.length ?? 1) - 1);
    const ratio = (svgX - CHART_PLOT_LEFT) / CHART_PLOT_WIDTH;
    return Math.max(0, Math.min(last, Math.round(ratio * last)));
  }
  let dialogMetricsEncoded = $state(''),
    dialogMetricsReference = $state('');
  let dialogVmaf = $state(true),
    dialogSsim = $state(false),
    dialogPsnr = $state(false),
    dialogAlignment = $state<QualityAlignment>('none'),
    dialogSubsample = $state(1),
    dialogVmafModel = $state<QualityVmafModel>('standard');
  let dialogColorSource = $state(''),
    dialogColorTarget = $state(''),
    dialogCopyColor = $state(true),
    dialogCopyHdr = $state(true);
  const storedKey = 'jesses.source-utilities.v1';
  function readStored(): Record<string, any> {
    try {
      const value = JSON.parse(localStorage.getItem(storedKey) ?? '{}');
      return value && typeof value === 'object' && !Array.isArray(value) ? value : {};
    } catch {
      return {};
    }
  }
  function member(value: unknown, choices: readonly string[], fallback: string): string {
    return typeof value === 'string' && choices.includes(value) ? value : fallback;
  }
  function integer(value: unknown, min: number, max: number, fallback: number) {
    return typeof value === 'number' && Number.isInteger(value) && value >= min && value <= max
      ? value
      : fallback;
  }
  function restoreUtilitySettings() {
    const data = readStored();
    const grain = data.grain && typeof data.grain === 'object' ? data.grain : {};
    grainOp = member(
      grain.operation,
      ['measureSource', 'extract', 'apply', 'remove'],
      grainOp,
    ) as typeof grainOp;
    grainSource = member(
      grain.source,
      ['table', 'preset', 'photonNoise'],
      grainSource,
    ) as typeof grainSource;
    grainTable = typeof grain.table === 'string' && grain.table.length <= 4096 ? grain.table : '';
    grainPreset =
      typeof grain.preset === 'string' && grain.preset.length <= 256 ? grain.preset : grainPreset;
    denoise = integer(grain.denoise, 1, 16, 4);
    iso = integer(grain.iso, 1, 409600, 400);
    keepDenoised = grain.keepDenoised === true;
    chroma = grain.chroma === true;
    const deint = data.deinterlace && typeof data.deinterlace === 'object' ? data.deinterlace : {};
    deinterlace = member(
      deint.mode,
      ['automatic', 'disabled', 'qtgmc', 'bwdif', 'yadif'],
      deinterlace,
    ) as typeof deinterlace;
    qtgmcPreset = member(
      deint.preset,
      ['placebo', 'verySlow', 'slower', 'slow', 'medium', 'fast', 'faster', 'veryFast'],
      qtgmcPreset,
    ) as QtgmcPreset;
    doubleRate = typeof deint.doubleRate === 'boolean' ? deint.doubleRate : true;
    const ladder = data.ladder && typeof data.ladder === 'object' ? data.ladder : {};
    ladderEncoder = member(
      ladder.encoder,
      ['av1', 'aomAv1', 'hevc', 'h264', 'vp9'],
      ladderEncoder,
    ) as LadderEncoder;
    resetLadderEncoder();
    if (typeof ladder.preset === 'string' && ladderSpeeds.includes(ladder.preset))
      ladderPreset = ladder.preset;
    if (typeof ladder.pixel === 'string' && ladderFormats.includes(ladder.pixel))
      ladderPixel = ladder.pixel;
    if (typeof ladder.content === 'string' && presets.some((item) => item.name === ladder.content))
      ladderContent = ladder.content;
    ladderCrfs = typeof ladder.crfs === 'string' && ladder.crfs.length <= 256 ? ladder.crfs : '';
    sampleCount = integer(ladder.sampleCount, 1, 8, 3);
    sampleSeconds = integer(ladder.sampleSeconds, 2, 120, 10);
    ladderMetric = member(
      ladder.metric,
      ['vmaf', 'ssimulacra2', 'butteraugliInf', 'xpsnrWeighted', 'none'],
      ladderMetric,
    ) as LadderMetric;
    ladderModel = member(
      ladder.model,
      ['standard', 'negative', 'fourK'],
      ladderModel,
    ) as QualityVmafModel;
    keepSamples = ladder.keepSamples === true;
  }
  function saveUtilitySettings(section: 'grain' | 'deinterlace' | 'ladder') {
    const data = readStored();
    data[section] =
      section === 'grain'
        ? {
            operation: grainOp,
            source: grainSource,
            table: grainTable,
            preset: grainPreset,
            denoise,
            keepDenoised,
            iso,
            chroma,
          }
        : section === 'deinterlace'
          ? { mode: deinterlace, preset: qtgmcPreset, doubleRate }
          : {
              encoder: ladderEncoder,
              preset: ladderPreset,
              pixel: ladderPixel,
              content: ladderContent,
              crfs: ladderCrfs,
              sampleCount,
              sampleSeconds,
              metric: ladderMetric,
              model: ladderModel,
              keepSamples,
            };
    try {
      localStorage.setItem(storedKey, JSON.stringify(data));
    } catch (e) {
      onlog(`Could not save utility settings: ${errorMessage(e)}`, true);
    }
    dialog = null;
  }
  const labels: Record<Task, string> = {
    bitrates: 'Bitrate reading',
    metrics: 'Metrics calculation',
    color: 'Color data transfer',
    deinterlace: 'Deinterlace pass',
    cadence: 'Cadence repair',
    grain: 'Film grain edit',
    ladder: 'CRF ladder',
    cut: 'Lossless cut',
    concat: 'Concatenation',
    chart: 'Bitrate chart',
    ocr: 'Subtitle OCR',
  };
  const mainVideo = $derived(file?.streams.find((item) => item.kind === 'video'));
  const configuredColorSource = $derived(files.find((item) => item.path === colorSource));
  const colorTargetFile = $derived(
    fileMode === 'batch' ? undefined : files.find((item) => item.path === colorTarget),
  );
  const colorDonorMissing = $derived(
    fileMode !== 'batch' && !!colorTargetFile && !configuredColorSource,
  );
  const activeColorSource = $derived(
    configuredColorSource ?? (colorTargetFile ? undefined : (file ?? files[0])),
  );
  const utilityBlocked = $derived.by<{ reason: string; action?: () => void } | null>(() => {
    if (!isDesktop()) return { reason: 'Open jesses in the desktop app to run utilities.' };
    if (!task) return { reason: 'Pick a utility above to continue.' };
    if (fileMode === 'batch' && batchReasons[task]) {
      return {
        reason: `${labels[task]} cannot be run per file. Switch to Muxing Mode to use it.`,
        action: onswitchtomux,
      };
    }
    if (!files.length) return { reason: 'Load a file in File List to run this utility.' };
    if (task === 'metrics') {
      if (files.length < 2)
        return { reason: 'Load at least two files in File List to compare quality.' };
      if (!metricsVmaf && !metricsSsim && !metricsPsnr)
        return {
          reason: 'Select at least one metric in Quality Metrics.',
          action: () => choose('metrics', true),
        };
      if (
        !files.some((item) => item.path === metricsEncoded) ||
        !files.some((item) => item.path === metricsReference)
      )
        return {
          reason: 'Choose the source and encoded files in Quality Metrics.',
          action: () => choose('metrics', true),
        };
      return null;
    }
    if (task === 'concat') {
      if (files.length < 2)
        return { reason: 'Load at least two files in File List to concatenate.' };
      if (files.length > 512)
        return { reason: 'Concatenation supports at most 512 files at once.' };
      return null;
    }
    if (task === 'color') {
      if (fileMode === 'batch') {
        if (!files.some((item) => item.streams.some((s) => s.kind === 'video')))
          return { reason: 'Load a file with a video stream to read color metadata.' };
        return null;
      }
      if (colorDonorMissing)
        return {
          reason: `The configured source video for ${colorTargetFile!.name} was removed. Choose a source video in Transfer Color Metadata.`,
          action: () => choose('color', true),
        };
      if (!activeColorSource?.streams.some((s) => s.kind === 'video'))
        return { reason: 'Choose a source file with a video stream for color metadata.' };
      if (colorTargetFile && !colorTargetFile.streams.some((s) => s.kind === 'video'))
        return {
          reason: 'Choose a target file with a video stream for color metadata.',
          action: () => choose('color', true),
        };
      if (colorTargetFile && activeColorSource && colorTargetFile.path === activeColorSource.path)
        return {
          reason: 'Choose a different target video than the source.',
          action: () => choose('color', true),
        };
      return null;
    }
    if (fileMode !== 'batch' && !file)
      return { reason: 'Load tracks for a file in File List to select the active source.' };
    return null;
  });
  const eligible = $derived(!utilityBlocked && !pending);
  const currentActionLabel = $derived(
    task === 'color'
      ? fileMode === 'batch'
        ? 'Run Batch: Read color metadata'
        : colorTargetFile
          ? `Run: Rewrite metadata in ${colorTargetFile.name}`
          : 'Run: Read color metadata'
      : task
        ? `${fileMode === 'batch' ? 'Run Batch' : 'Run'}: ${labels[task]}`
        : 'Run',
  );
  $effect(() => {
    const next = {
      run,
      canRun: eligible,
      blockedReason: pending ? undefined : utilityBlocked?.reason,
      openBlockedSetting: pending ? undefined : utilityBlocked?.action,
      label: pending && runningLabel ? runningLabel : currentActionLabel,
      active: pending,
      paused,
      progress: progress
        ? Math.min(
            1,
            Math.max(
              0,
              (progress.completedRows +
                (progress.durationSeconds && progress.progressSeconds != null
                  ? Math.min(1, progress.progressSeconds / progress.durationSeconds)
                  : 0)) /
                Math.max(1, progress.rowCount),
            ),
          )
        : 0,
      progressDescription: pending ? progress?.message : completedStatus,
      pause: togglePause,
      stop: stop,
    };
    untrack(() => registerAction('utilities', next));
  });
  onDestroy(() => {
    abort?.abort();
    clearTimeout(ladderCopyTimer);
    registerAction('utilities', null);
  });
  function choose(id: Task, configure = false) {
    task = id;
    if (id === 'cut' && (configure || !cutConfigured) && !file) {
      onlog('You need to load a file into the file list to use this utility.', true);
      return;
    }
    if (id === 'metrics') {
      if (files.length < 2) {
        onlog('Load at least two files to compare quality.', true);
        return;
      }
      const sorted = files.toSorted((a, b) => Number(a.sizeBytes) - Number(b.sizeBytes));
      const haveBoth =
        files.some((item) => item.path === metricsEncoded) &&
        files.some((item) => item.path === metricsReference);
      dialogMetricsEncoded = haveBoth ? metricsEncoded : sorted[0].path;
      dialogMetricsReference = haveBoth ? metricsReference : sorted.at(-1)!.path;
      dialogVmaf = metricsVmaf;
      dialogSsim = metricsSsim;
      dialogPsnr = metricsPsnr;
      dialogAlignment = alignment;
      dialogSubsample = subsample;
      dialogVmafModel = vmafModel;
      dialog = 'metrics';
    } else if (id === 'color' && configure) {
      if (!files.length) {
        onlog('Load at least one source file.', true);
        return;
      }
      const resolvedTarget = files.some((item) => item.path === colorTarget) ? colorTarget : '';
      const fallbackSource =
        (files
          .toSorted((a, b) => Number(b.sizeBytes) - Number(a.sizeBytes))
          .find((item) => item.path !== resolvedTarget)?.path ??
          files[0]?.path) ||
        '';
      const resolvedSource = files.some((item) => item.path === colorSource)
        ? colorSource
        : file && file.path !== resolvedTarget
          ? file.path
          : fallbackSource;
      dialogColorSource = fileMode === 'batch' ? (file?.path ?? resolvedSource) : resolvedSource;
      dialogColorTarget =
        fileMode === 'batch' || resolvedTarget === dialogColorSource ? '' : resolvedTarget;
      dialogCopyColor = copyColor;
      dialogCopyHdr = copyHdr;
      dialog = 'color';
    } else if (configure && ['deinterlace', 'grain', 'ladder', 'cut'].includes(id)) {
      if (id === 'cut' && !cutConfigured) {
        cutStart = 0;
        cutEnd = file?.durationSeconds ?? 0;
      }
      dialog = id as Dialog;
      if (id === 'grain')
        void inspectUtilityCapabilities()
          .then((cap) => {
            if (cap.grainPresets.length) grainPresets = cap.grainPresets;
            grainAvailable =
              cap.dependencies.find((dependency) => dependency.id === 'grav1synth')?.available ??
              null;
          })
          .catch(() => {});
    } else if (id === 'cut' && !cutConfigured) {
      cutStart = 0;
      cutEnd = file?.durationSeconds ?? 0;
      dialog = 'cut';
    }
  }
  function stop() {
    abort?.abort();
  }
  async function togglePause() {
    if (!analysisId || pausing) return;
    pausing = true;
    try {
      await setMediaAnalysisPaused(analysisId, !paused);
      paused = !paused;
    } catch (e) {
      onlog(errorMessage(e), true);
    } finally {
      pausing = false;
    }
  }
  async function copyLadder() {
    if (ladderResult?.kind !== 'crfLadder') return;
    const result = ladderResult.result;
    const lines = [
      'CRF\tVideo bitrate\tPer minute\tWhole file\tOf source\tScore\tEncode time',
      ...result.rungs.map((row) =>
        [
          row.crf,
          `${row.bitrateKbps.toFixed(1)} kb/s`,
          formatBytes(row.bytesPerMinute),
          formatBytes(row.projectedSizeBytes),
          Number(result.sourceSizeBytes) > 0
            ? `${((Number(row.projectedSizeBytes) / Number(result.sourceSizeBytes)) * 100).toFixed(1)}%`
            : '—',
          row.score?.toFixed(3) ?? '—',
          `${row.encodeSeconds.toFixed(1)}s`,
        ].join('\t'),
      ),
    ];
    const copied = await copyTextWithFeedback(lines.join('\n'), {
      success: 'Copied sample encode results.',
      fail: 'Could not copy sample results.',
    });
    if (!copied) {
      onlog('Could not copy sample results: Clipboard access was denied.', true);
      return;
    }
    ladderCopied = true;
    clearTimeout(ladderCopyTimer);
    ladderCopyTimer = setTimeout(() => {
      ladderCopied = false;
    }, 2000);
  }
  function fileVideo(entry: MediaFile | undefined) {
    return entry?.streams.find((stream) => stream.kind === 'video')?.index ?? 0;
  }
  function scopedFile(row: TrackSelection) {
    return files.find((entry) => entry.id === row.sourceId);
  }
  function selectedTracks(entry: MediaFile) {
    return fileMode === 'batch'
      ? entry.streams.map((stream) => ({
          sourceId: entry.id,
          streamIndex: stream.index,
          included: true,
          title: '',
          language: stream.language ?? '',
          defaultFlag: null,
          forcedFlag: null,
        }))
      : tracks.filter((row) => row.included && row.sourceId === entry.id);
  }
  const presets = $derived(
    ladderEncoder === 'h264'
      ? contentPresets.x264
      : ladderEncoder === 'hevc'
        ? contentPresets.x265
        : ladderEncoder === 'av1'
          ? contentPresets.svtAv1
          : [],
  );
  const ladderSpeeds = $derived(
    ladderEncoder === 'h264' || ladderEncoder === 'hevc'
      ? ['veryslow', 'slower', 'slow', 'medium', 'fast', 'faster', 'veryfast', 'superfast']
      : ladderEncoder === 'h264Nvenc'
        ? ['p5', 'p4', 'p3', 'p2', 'p1']
        : ladderEncoder === 'hevcNvenc'
          ? ['p7', 'p6', 'p5', 'p4', 'p3', 'p2', 'p1']
          : Array.from({ length: ladderEncoder === 'av1' ? 9 : 7 }, (_, i) => String(i)),
  );
  const ladderFormats = $derived(
    ladderEncoder === 'av1'
      ? ['yuv420p', 'yuv420p10le']
      : ladderEncoder === 'h264Nvenc'
        ? ['yuv420p', 'yuv444p']
        : ladderEncoder === 'hevcNvenc'
          ? ['yuv420p', 'yuv444p', 'p010le']
          : ladderEncoder === 'vp9'
            ? [
                'yuv420p',
                'yuv422p',
                'yuva420p',
                'yuv444p',
                'yuv420p10le',
                'yuv422p10le',
                'yuv444p10le',
              ]
            : ['yuv420p', 'yuv422p', 'yuv444p', 'yuv420p10le', 'yuv422p10le', 'yuv444p10le'],
  );
  function pixelLabel(format: string) {
    const channels = format.startsWith('yuva') ? 'YUVA' : 'YUV';
    const sampling = format.includes('422') ? '4:2:2' : format.includes('444') ? '4:4:4' : '4:2:0';
    return `${channels} - ${sampling} - ${format.includes('10') || format === 'p010le' ? 10 : 8} bit`;
  }
  function grainWarning() {
    if (grainAvailable === false)
      return 'grav1synth is not bundled with this build and is not on your PATH, so none of these can run.';
    if (!file || !mainVideo) return '';
    if (grainOp !== 'measureSource')
      return mainVideo.codec?.toLowerCase() === 'av1'
        ? ''
        : 'This file is not AV1, so this operation cannot run on it - the film grain it edits lives in an AV1 bitstream. Measure works on any source.';
    const [numerator, denominator] = (mainVideo.frameRate ?? '').split('/').map(Number);
    const rate = denominator > 0 ? numerator / denominator : numerator;
    if (!file.durationSeconds || !rate || !mainVideo.width || !mainVideo.height)
      return 'Measuring reads every frame of the source twice, single-threaded, and writes a lossless intermediate the length of the video.';
    const estimate =
      (mainVideo.width * mainVideo.height * Math.floor(file.durationSeconds * rate)) / 7_200_000;
    return `Measuring alone is approximately ${formatDuration(estimate)} for this file, single-threaded, plus a lossless intermediate the length of the video. The AV1AN tab's Grain Synthesis row does this and the encode in one run.`;
  }
  function ladderDescription() {
    if (!file)
      return 'No file is loaded. The settings are saved either way, and applied to whichever file is loaded when this runs.';
    const duration = file.durationSeconds ?? 0;
    if (duration <= 0)
      return `'${file.name}' cannot be sampled: it reports no duration, so there is nowhere to place a section. Remux it to MKV first - the Concatenate utility does that for a single file. The settings are saved either way.`;
    const sections =
      duration <= sampleSeconds
        ? 1
        : Math.max(1, Math.min(sampleCount, Math.floor(duration / sampleSeconds)));
    const runs = sections * parsedCrfs().length;
    const scoring = {
      vmaf: 'VMAF',
      ssimulacra2: 'SSIMULACRA2',
      butteraugliInf: 'Butteraugli',
      xpsnrWeighted: 'XPSNR',
      none: 'no quality metric',
      psnr: 'PSNR',
      ssim: 'SSIM',
    }[ladderMetric];
    return `${file.name} — ${formatDuration(duration)}, sampled in ${sections} section${sections === 1 ? '' : 's'}. ${runs} sample encode${runs === 1 ? '' : 's'} at preset ${ladderPreset}, ${ladderPixel}, scored with ${scoring}. A CRF is only meaningful for the encoder and settings that produced it; the AV1AN tab drives different binaries, so treat this as a starting point there.`;
  }
  function ladderDefaults() {
    const values: Record<LadderEncoder, [string, string, string]> = {
      av1: ['4', 'yuv420p10le', '25,30,35'],
      aomAv1: ['6', 'yuv420p10le', '15,20,25'],
      hevc: ['medium', 'yuv420p', '18,22,26'],
      h264: ['slow', 'yuv420p', '14,18,22'],
      vp9: ['3', 'yuv420p', '19,24,29'],
      hevcNvenc: ['p7', 'yuv420p', '18,22,26'],
      h264Nvenc: ['p5', 'yuv420p', '14,18,22'],
    };
    return values[ladderEncoder];
  }
  function resetLadderEncoder() {
    [ladderPreset, ladderPixel] = ladderDefaults();
    ladderContent = '';
  }
  function resetLadder() {
    ladderEncoder = 'av1';
    resetLadderEncoder();
    ladderCrfs = '';
    sampleCount = 3;
    sampleSeconds = 10;
    ladderMetric = 'vmaf';
    ladderModel = 'standard';
    keepSamples = false;
  }
  restoreUtilitySettings();
  function parsedCrfs() {
    const maximum = ['h264', 'hevc', 'h264Nvenc', 'hevcNvenc'].includes(ladderEncoder) ? 51 : 63;
    const parsed = [
      ...new Set(
        ladderCrfs
          .split(/[,;\s/]+/)
          .filter((token) => /^\+?\d+$/.test(token))
          .map(Number)
          .filter((value) => Number.isSafeInteger(value) && value <= 2147483647)
          .map((value) => Math.min(maximum, value)),
      ),
    ]
      .sort((a, b) => a - b)
      .slice(0, 8);
    return parsed.length ? parsed : ladderDefaults()[2].split(',').map(Number);
  }
  function configureText(id: Task) {
    if (id === 'color') {
      if (!files.length) return 'Configure…';
      if (colorDonorMissing && colorTargetFile) return `Choose source → ${colorTargetFile.name}`;
      if (fileMode === 'batch' || !colorTargetFile)
        return `Read · ${activeColorSource?.name ?? files[0].name}`;
      return `Rewrite · ${activeColorSource?.name ?? files[0].name} → ${colorTargetFile.name}`;
    }
    if (id === 'grain')
      return grainOp === 'measureSource'
        ? `Measure · denoise ${denoise}${keepDenoised ? ' · keep video' : ''}`
        : grainOp === 'extract'
          ? 'Extract table'
          : grainOp === 'remove'
            ? 'Remove grain'
            : `Apply · ${grainSource === 'preset' ? grainPreset || 'no preset picked' : grainSource === 'photonNoise' ? `ISO ${iso}` : grainTable.split(/[\\/]/).at(-1) || 'no table picked'}`;
    if (id === 'deinterlace') {
      const name = {
        automatic: 'Automatic',
        disabled: 'Disabled',
        qtgmc: 'QTGMC',
        bwdif: 'Bwdif',
        yadif: 'Yadif',
      }[deinterlace];
      return deinterlace === 'automatic' || deinterlace === 'qtgmc'
        ? `${name} · ${qtgmcPreset[0].toUpperCase()}${qtgmcPreset.slice(1)}`
        : name;
    }
    if (id === 'ladder') {
      const values = parsedCrfs();
      const label = {
        av1: 'SVT-AV1',
        aomAv1: 'AOM-AV1',
        hevc: 'x265',
        h264: 'x264',
        vp9: 'VPX',
        hevcNvenc: 'NVIDIA NVENC',
        h264Nvenc: 'NVIDIA NVENC',
      }[ladderEncoder];
      return `${label} · ${values.length <= 3 ? `CRF ${values.join(', ')}` : `${values.length} CRFs, ${values[0]}-${values.at(-1)}`}`;
    }
    return id === 'cut' && cutConfigured
      ? `${formatDuration(cutStart)} - ${formatDuration(cutEnd)}`
      : 'Configure…';
  }
  function outputRequests(entry: MediaFile): UtilityRequest[] {
    const video = fileVideo(entry);
    if (task === 'cut')
      return [
        {
          kind: 'keyframeCut',
          request: {
            inputPath: entry.path,
            outputPath: '',
            startSeconds: cutStart,
            endSeconds: cutEnd,
          },
        },
      ];
    if (task === 'concat')
      return [
        { kind: 'concat', request: { inputPaths: files.map((item) => item.path), outputPath: '' } },
      ];
    if (task === 'cadence')
      return [
        {
          kind: 'cadenceRepairExport',
          request: { inputPath: entry.path, outputPath: '', videoStreamIndex: video },
        },
      ];
    if (task === 'deinterlace')
      return [
        {
          kind: 'deinterlaceExport',
          request: {
            inputPath: entry.path,
            outputPath: '',
            videoStreamIndex: video,
            method: (deinterlace === 'automatic'
              ? 'qtgmc'
              : deinterlace) as DeinterlaceExportMethod,
            mode: doubleRate ? 'bob' : 'frame',
            fieldOrder: (entry.streams.find((s) => s.index === video)?.fieldOrder === 'bb'
              ? 'bottomFirst'
              : 'topFirst') as FieldOrder,
            qtgmcPreset,
          },
        },
      ];
    if (task === 'ocr')
      return selectedTracks(entry)
        .filter((row) => {
          const stream = entry.streams.find((s) => s.index === row.streamIndex);
          return (
            stream?.kind === 'subtitle' &&
            ['hdmv_pgs_subtitle', 'dvd_subtitle', 'dvb_subtitle', 'xsub'].includes(
              stream.codec ?? '',
            )
          );
        })
        .map((row) => ({
          kind: 'subtitleOcr',
          request: {
            inputPath: entry.path,
            subtitleStreamIndex: row.streamIndex,
            language:
              /^[a-z]{3}$/i.test(row.language) && row.language.toLowerCase() !== 'und'
                ? row.language.toLowerCase()
                : 'eng',
            outputPath: '',
          },
        }));
    if (task === 'grain') {
      const request: GrainRequest =
        grainOp === 'measureSource'
          ? {
              operation: 'measureSource',
              sourcePath: entry.path,
              outputTablePath: '',
              denoiseStrength: Math.max(1, Math.min(16, Math.trunc(denoise || 4))),
              ...(keepDenoised ? { denoisedOutputPath: '' } : {}),
            }
          : grainOp === 'extract'
            ? { operation: 'extract', inputPath: entry.path, outputTablePath: '' }
            : grainOp === 'remove'
              ? { operation: 'remove', inputPath: entry.path, outputPath: '' }
              : {
                  operation: 'rewriteHeaders',
                  inputPath: entry.path,
                  outputPath: '',
                  source:
                    grainSource === 'table'
                      ? { kind: 'table', tablePath: grainTable }
                      : grainSource === 'preset'
                        ? { kind: 'preset', preset: grainPreset }
                        : {
                            kind: 'photonNoise',
                            iso: Math.max(1, Math.min(409600, Math.trunc(iso || 400))),
                            chroma,
                          },
                };
      return [{ kind: 'grain', request }];
    }
    if (task === 'ladder') {
      const request: CrfLadderRequest = {
        inputPath: entry.path,
        videoStreamIndex: video,
        encoder: ladderEncoder,
        preset: ladderPreset,
        pixelFormat: ladderPixel,
        crfs: parsedCrfs(),
        sampleCount: Math.max(1, Math.min(8, Math.trunc(sampleCount || 3))),
        sampleSeconds: Math.max(2, Math.min(120, Math.trunc(sampleSeconds || 10))),
        metric: ladderMetric,
        recommendationThreshold: null,
        vmafModel: ladderModel,
        ...(ladderContent
          ? {
              contentPreset: ladderContent,
              parameters: presets.find((item) => item.name === ladderContent)?.parameters ?? [],
            }
          : {}),
        ...(keepSamples ? { keepSamplesDirectory: '' } : {}),
      };
      return [{ kind: 'crfLadder', request }];
    }
    return [];
  }
  async function run() {
    if (!task || pending) return;
    const runTask = task;
    const needsActiveFile = !['metrics', 'concat', 'color'].includes(runTask);
    if (!files.length || (needsActiveFile && !file)) {
      onlog('No input file loaded. Please load one first (File List).', true);
      return;
    }
    const activeSource = file ?? files[0];
    const batch = fileMode === 'batch';
    if (batch && ['metrics', 'ladder', 'concat', 'chart'].includes(runTask)) {
      unsupportedBatchTask = runTask;
      return;
    }
    const sourceSize = (bytes: string) => {
      const value = Number(bytes);
      if (!Number.isFinite(value) || value < 0) return 'N/A B';
      if (value === 0) return '0B';
      const units = ['B', 'KB', 'MB', 'GB', 'TB', 'PB', 'EB'];
      const power = Math.min(Math.floor(Math.log(value) / Math.log(1024)), units.length - 1);
      return `${(value / 1024 ** power).toLocaleString(undefined, { maximumFractionDigits: 1, useGrouping: false })} ${units[power]}`;
    };
    const startedAt = performance.now();
    let outputSummary = '';
    let batchInputBytes = 0n,
      batchOutputBytes = 0n;
    activeTask = runTask;
    runningLabel = currentActionLabel;
    completedStatus = '';
    paused = false;
    progress = null;
    const active = new AbortController();
    abort = active;
    const statusRunId = batchRunId('utility');
    let statuses: Record<string, BatchRowStatus> = batch
      ? Object.fromEntries(files.map((entry) => [entry.id, { glyph: '○', text: 'Queued' }]))
      : {};
    let runningId: string | null = null;
    function mark(id: string, state: 'running' | 'done' | 'failed' | 'skipped', note?: string) {
      if (!batch) return;
      statuses = {
        ...statuses,
        [id]:
          state === 'running'
            ? { glyph: '▶', text: 'Running…' }
            : state === 'done'
              ? { glyph: '✔', text: 'Done' }
              : state === 'failed'
                ? { glyph: '✕', text: 'Failed', note }
                : { glyph: '–', text: 'Skipped', note },
      };
      runningId = state === 'running' ? id : runningId === id ? null : runningId;
      onbatchstatus(statuses, statusRunId);
    }
    if (batch) onbatchstatus(statuses, statusRunId, true);
    try {
      if (runTask === 'bitrates' || runTask === 'chart') {
        const rows =
          runTask === 'chart'
            ? [
                {
                  sourceId: activeSource.id,
                  streamIndex: fileVideo(activeSource),
                  included: true,
                  title: '',
                  language: '',
                  defaultFlag: null,
                  forcedFlag: null,
                },
              ]
            : batch
              ? files.flatMap(selectedTracks)
              : tracks.filter((row) => row.included);
        if (!rows.length) throw new Error('Select tracks in the Track List first.');
        const remaining = new Map(
          files.map((entry) => [entry.id, rows.filter((row) => row.sourceId === entry.id).length]),
        );
        const totals = new Map<
          string,
          Record<'video' | 'audio' | 'subtitle', { bytes: bigint; kbps: number }>
        >();
        const sourceBitrate = (kbps: number) =>
          kbps > 10240 ? `${(kbps / 1024).toFixed(2).replace(/\.?0+$/, '')} mbps` : `${kbps} kbps`;
        const percent = (bytes: bigint, sourceBytes: string) => {
          const size = Number(sourceBytes);
          return size > 0 ? ((Number(bytes) * 100) / size).toFixed(1) : '0.0';
        };
        if (runTask === 'bitrates')
          onlog('Analyzing streams... This can take a few minutes on slower hard drives.');
        for (const row of rows) {
          const source = scopedFile(row);
          if (!source) continue;
          mark(source.id, 'running');
          const result = await analyzeBitrate(
            { inputPath: source.path, streamIndex: row.streamIndex, windowSeconds: 1 },
            active.signal,
            (id) => (analysisId = id),
            runTask === 'chart',
          );
          if (runTask === 'chart') {
            if (!result.points.length)
              throw new Error(
                'No frames could be read from this file, so there is no bitrate to chart. The log has ffprobe’s own output.',
              );
            chartResult = result;
            chartStart = null;
            chartEnd = null;
            dialog = 'chartResult';
          } else {
            const kind =
              source.streams.find((stream) => stream.index === row.streamIndex)?.kind ?? 'unknown';
            const bytes = BigInt(result.packetBytes);
            const kbps = (result.averageMegabitsPerSecond ?? 0) * 1000;
            const bitrate = kbps > 1 ? sourceBitrate(Math.round(kbps)) : `${kbps.toFixed(1)} kbps`;
            onlog(
              `Stream #${row.streamIndex} (${kind}) - Bitrate: ${bitrate} - Size: ${sourceSize(result.packetBytes)} (${percent(bytes, source.sizeBytes)}%)`,
            );
            const sum = totals.get(source.id) ?? {
              video: { bytes: 0n, kbps: 0 },
              audio: { bytes: 0n, kbps: 0 },
              subtitle: { bytes: 0n, kbps: 0 },
            };
            if (kind === 'video' || kind === 'audio' || kind === 'subtitle') {
              sum[kind].bytes += bytes;
              sum[kind].kbps += Math.round(kbps);
            }
            totals.set(source.id, sum);
          }
          const left = (remaining.get(source.id) ?? 1) - 1;
          remaining.set(source.id, left);
          if (left === 0) {
            const sum = totals.get(source.id);
            if (sum)
              for (const kind of ['video', 'audio', 'subtitle'] as const) {
                const name = kind[0].toUpperCase() + kind.slice(1);
                onlog(
                  `Total ${name} Bitrate: ${sourceBitrate(sum[kind].kbps)} - Total ${name} Size: ${sourceSize(sum[kind].bytes.toString())} (${percent(sum[kind].bytes, source.sizeBytes)}%)`,
                );
              }
            mark(source.id, 'done');
          }
        }
      } else if (runTask === 'metrics') {
        const reference = files.find((item) => item.path === metricsReference),
          encoded = files.find((item) => item.path === metricsEncoded);
        if (!reference || !encoded)
          throw new Error('Configure the source and encoded files first.');
        const metrics: QualityMetric[] = [
          ...(metricsVmaf ? ['vmaf' as const] : []),
          ...(metricsSsim ? ['ssim' as const] : []),
          ...(metricsPsnr ? ['psnr' as const] : []),
        ];
        if (!metrics.length) throw new Error('Select at least one metric.');
        for (const metric of metrics) {
          const request = {
            referencePath: reference.path,
            referenceStreamIndex: fileVideo(reference),
            candidatePath: encoded.path,
            candidateStreamIndex: fileVideo(encoded),
            referenceStartFrame: 0,
            candidateStartFrame: 0,
            frameCount: 0,
            fullSource: true,
            metric,
            options: { alignment, vmafModel, subsample, fixFrameRate: false },
          } as QualityRequest;
          const result = await analyzeQuality(request, active.signal, (id) => (analysisId = id));
          onlog(`${metric.toUpperCase()}: ${result.score?.toFixed(4) ?? '∞'}`);
        }
      } else if (runTask === 'color' && (batch || !colorTargetFile)) {
        const sources = batch
          ? files.filter((item) => item.streams.some((s) => s.kind === 'video'))
          : activeColorSource
            ? [activeColorSource]
            : [];
        if (!sources.length) throw new Error('Load a file with a video stream first.');
        for (const source of sources) {
          mark(source.id, 'running');
          onlog(
            batch
              ? `Batch Processing Mode cannot transfer color data, so printing ${source.name}'s metadata only. Switch to Muxing Mode to transfer it.`
              : files.length < 2
                ? `Only one file is loaded, so there is nothing to transfer to - printing ${source.name}'s metadata only.`
                : `No target file is picked, so nothing will be written - printing ${source.name}'s metadata only. Pick one with Configure… on this utility's card to transfer the data into it.`,
          );
          const result = await readColorMetadata(
            { inputPath: source.path, videoStreamIndex: fileVideo(source) },
            active.signal,
            (id) => (analysisId = id),
          );
          onlog(
            source.source?.kind === 'imageSequence'
              ? result.summary.replace(
                  /^Color metadata: .*?(?= \(video stream )/,
                  `Color metadata: ${source.name}`,
                )
              : result.summary,
          );
          mark(source.id, 'done');
        }
      } else {
        if (runTask === 'deinterlace' && deinterlace === 'disabled') {
          onlog('Deinterlacing is disabled.');
          return;
        }
        let requests: UtilityRequest[];
        if (runTask === 'color') {
          const donor = configuredColorSource;
          const target = colorTargetFile;
          if (!donor || !target || donor.path === target.path)
            throw new Error('Choose the source and target video.');
          const request = {
            inputPath: target.path,
            inputVideoStreamIndex: fileVideo(target),
            metadataSourcePath: donor.path,
            metadataSourceVideoStreamIndex: fileVideo(donor),
            outputPath: '',
            copyColor,
            copyHdr,
            inPlace: true,
          };
          const what =
            request.copyColor && request.copyHdr
              ? 'color space and HDR data'
              : request.copyColor
                ? 'color space'
                : request.copyHdr
                  ? 'HDR data'
                  : 'nothing';
          onlog(`Transferring ${what} from ${donor.name} to ${target.name}.`);
          const report = await readColorMetadata(
            {
              inputPath: request.metadataSourcePath,
              videoStreamIndex: request.metadataSourceVideoStreamIndex,
            },
            active.signal,
            (id) => (analysisId = id),
          );
          onlog(report.summary);
          requests = [{ kind: 'colorMetadataTransfer', request }];
        } else {
          let sources = batch && runTask !== 'concat' ? files : [activeSource];
          if (['cut', 'concat', 'grain', 'cadence', 'deinterlace', 'ladder'].includes(runTask)) {
            const unsupported = (runTask === 'concat' ? files : sources).filter(
              (entry) => entry.source?.kind === 'imageSequence',
            );
            if (unsupported.length && (runTask === 'concat' || !batch))
              throw new Error('This utility does not support image sequence folders.');
            for (const entry of unsupported) {
              onlog(`${entry.name}: this utility does not support image sequence folders.`, true);
              mark(entry.id, 'failed', 'This utility does not support image sequence folders.');
            }
            sources = sources.filter((entry) => entry.source?.kind !== 'imageSequence');
            if (!sources.length) return;
          }
          requests = sources.flatMap(outputRequests);
        }
        if (runTask === 'deinterlace') {
          const resolved: UtilityRequest[] = [];
          const automatic = deinterlace === 'automatic';
          for (const item of requests) {
            if (item.kind !== 'deinterlaceExport') continue;
            const result = await resolveDeinterlace(
              item.request,
              automatic,
              active.signal,
              (id) => (analysisId = id),
            );
            onlog(result.message);
            if (result.request)
              resolved.push({ kind: 'deinterlaceExport', request: result.request });
          }
          requests = resolved;
          if (!requests.length) return;
        }
        if (!requests.length)
          throw new Error(
            runTask === 'ocr'
              ? 'Select at least one bitmap subtitle track in the Track List.'
              : 'This utility has no valid input.',
          );
        function requestPath(item: UtilityRequest): string {
          if (item.kind === 'grain')
            return item.request.operation === 'measure' ||
              item.request.operation === 'measureSource'
              ? item.request.sourcePath
              : item.request.inputPath;
          if (item.kind === 'concat') return item.request.inputPaths[0] ?? '';
          return item.request.inputPath;
        }
        const rowStates = requests.map(
          () => 'queued' as 'queued' | 'running' | 'succeeded' | 'failed' | 'skipped',
        );
        function updateRow(index: number, state: (typeof rowStates)[number], note?: string) {
          rowStates[index] = state;
          const entry = files.find((candidate) => candidate.path === requestPath(requests[index]));
          if (!entry) return;
          const group = rowStates.filter((_, i) => requestPath(requests[i]) === entry.path);
          if (group.includes('failed')) mark(entry.id, 'failed', note);
          else if (group.includes('running')) mark(entry.id, 'running');
          else if (group.every((item) => item === 'succeeded')) mark(entry.id, 'done');
          else if (group.every((item) => item === 'succeeded' || item === 'skipped'))
            mark(entry.id, 'skipped', note);
        }
        function recordOutput(result: UtilityResult, request: UtilityRequest) {
          // Report only completed native measurements on the source's routes.
          let bytes: string | undefined, outputPath: string | undefined;
          if (
            ['cut', 'deinterlace', 'cadence', 'grain'].includes(runTask) &&
            result.kind === 'artifact'
          ) {
            bytes = result.result.sizeBytes;
            outputPath = result.result.outputPath;
          } else if (runTask === 'grain' && result.kind === 'grainTable') {
            bytes = result.result.sizeBytes;
            outputPath = result.result.outputPath;
          } else if (runTask === 'ladder' && result.kind === 'crfLadder') {
            bytes = result.result.samplesSizeBytes;
            outputPath = result.result.samplesDirectory;
          }
          if (bytes === undefined || !outputPath) return;
          const source = files.find((entry) => entry.path === requestPath(request));
          try {
            const output = BigInt(bytes);
            if (output <= 0n) return;
            const input = BigInt(source?.sizeBytes ?? '0');
            let size = sourceSize(output.toString());
            if (input > 0n) {
              const difference = output - input;
              const rounded =
                ((difference < 0n ? -1n : 1n) *
                  ((difference < 0n ? -difference : difference) * 100n * 2n + input)) /
                (input * 2n);
              size = `${sourceSize(input.toString())} → ${size} (${rounded > 0n ? '+' : ''}${rounded}%)`;
              batchInputBytes += input;
              batchOutputBytes += output;
            }
            outputSummary = `${fileName(outputPath)}: ${size}`;
            onlog(`Output: ${outputSummary}`);
          } catch {
            /* An unavailable size is not a failed utility. */
          }
        }
        const update = (p: UtilityProgress) => {
          progress = p;
          if (p.rowIndex !== null && requests[p.rowIndex])
            updateRow(p.rowIndex, p.status as (typeof rowStates)[number], p.message);
        };
        if (requests.length === 1) {
          const result = await runUtility(
            requests[0],
            active.signal,
            (id) => (analysisId = id),
            update,
          );
          onlog(result.result.message);
          recordOutput(result, requests[0]);
          if (batch) updateRow(0, 'succeeded');
          if (result.kind === 'crfLadder') {
            ladderResult = result;
            dialog = 'ladderResult';
          }
        } else {
          const batch = await runUtilityBatch(
            { requests },
            active.signal,
            (id) => (analysisId = id),
            update,
          );
          for (const row of batch.entries) {
            updateRow(
              row.index,
              row.error ? 'failed' : row.result ? 'succeeded' : 'skipped',
              row.error?.message,
            );
            onlog(row.error?.message ?? row.result?.result.message ?? 'No result.', !!row.error);
            if (row.result) recordOutput(row.result, requests[row.index]);
          }
        }
      }
    } catch (e) {
      if (runningId)
        mark(
          runningId,
          active.signal.aborted ? 'skipped' : 'failed',
          active.signal.aborted ? 'Stopped by the user.' : errorMessage(e),
        );
      if (!active.signal.aborted) {
        completedStatus = 'Did not finish - the log has the details.';
        onlog(errorMessage(e), true);
      } else {
        completedStatus = 'Canceled.';
        onlog('Stopped.');
      }
    } finally {
      if (batch) {
        for (const [id, status] of Object.entries(statuses))
          if (status.text === 'Queued' || status.text === 'Running…')
            mark(id, 'skipped', 'The batch did not complete this file.');
      }
      const milliseconds = Math.max(0, Math.floor(performance.now() - startedAt));
      const seconds = Math.floor(milliseconds / 1000);
      const elapsed =
        seconds >= 3600
          ? `${String(Math.floor(seconds / 3600) % 24).padStart(2, '0')}:${String(Math.floor(seconds / 60) % 60).padStart(2, '0')}:${String(seconds % 60).padStart(2, '0')}`
          : seconds >= 60
            ? `${String(Math.floor(seconds / 60)).padStart(2, '0')}:${String(seconds % 60).padStart(2, '0')}`
            : seconds >= 1
              ? `${seconds}s`
              : `${milliseconds}ms`;
      onlog(
        `${active.signal.aborted || completedStatus ? 'Stopped after' : 'Done - Finished task in'} ${elapsed}.`,
      );
      if (!completedStatus) {
        if (batch) {
          const rows = Object.values(statuses),
            done = rows.filter((row) => row.text === 'Done').length,
            failed = rows.filter((row) => row.text === 'Failed').length,
            skipped = rows.filter(
              (row) => row.text === 'Canceled' || row.text === 'Skipped',
            ).length;
          let total = '';
          if (batchInputBytes > 0n && batchOutputBytes > 0n) {
            const difference = batchOutputBytes - batchInputBytes;
            const rounded =
              ((difference < 0n ? -1n : 1n) *
                ((difference < 0n ? -difference : difference) * 200n + batchInputBytes)) /
              (batchInputBytes * 2n);
            total = ` Total size: ${sourceSize(batchInputBytes.toString())} → ${sourceSize(batchOutputBytes.toString())} (${rounded > 0n ? '+' : ''}${rounded}%).`;
          }
          const counts = `${done}/${files.length} finished${failed ? `, ${failed} failed` : ''}${skipped ? `, ${skipped} not run` : ''}`;
          onlog(
            `Queue: ${counts}${active.signal.aborted ? ' (Stopped)' : ''}. Total time: ${elapsed}.${total}`,
          );
          completedStatus = active.signal.aborted
            ? `Batch stopped - ${counts}.`
            : `Batch done - ${counts} in ${elapsed}.${total}`;
        } else
          completedStatus = active.signal.aborted
            ? 'Canceled.'
            : `Done${outputSummary ? ` - ${outputSummary}` : ''} - finished in ${elapsed}.`;
      }
      activeTask = null;
      runningLabel = '';
      analysisId = null;
      paused = false;
      abort = undefined;
    }
  }
</script>

<section class="source-utilities" aria-label="Utilities">
  <div class="utilities-intro">
    <p>
      Pick a utility, then press Run. Utilities are shortcuts for actions that normally need long
      CLI commands.
    </p>
    {#if fileMode === 'batch'}
      <span class="batch-indicator" role="status">
        Batch Mode: runs per file (multi-file utilities require Muxing Mode)
      </span>
    {/if}
  </div>
  <div class="cards">
    {#each cards as card}<article class:chosen={task === card.id} class="card">
        <div class="card-head">
          <button
            type="button"
            class="card-select"
            aria-pressed={task === card.id}
            onclick={() => choose(card.id)}
          >
            <strong>{card.label}</strong>
          </button>
          {#if fileMode === 'batch' && batchReasons[card.id]}
            <span class="card-badge">Muxing Mode only</span>
          {/if}
          {#if card.configure}
            {@const label = configureText(card.id)}
            <button
              type="button"
              class="card-configure"
              aria-label={label === 'Configure…'
                ? `Configure… (${card.label})`
                : `${label} — Configure ${card.label}`}
              onclick={() => choose(card.id, true)}
            >
              {label}
            </button>
          {/if}
        </div>
        <small>{card.description}</small>
      </article>{/each}
  </div>
</section>
<SourceDialog
  title="jesses - Warning"
  open={unsupportedBatchTask !== null}
  onclose={() => (unsupportedBatchTask = null)}
>
  {#if unsupportedBatchTask}<p>{labels[unsupportedBatchTask]} cannot be run per file.</p>
    <p>{batchReasons[unsupportedBatchTask]}</p>
    <p>Switch the File List tab to Muxing Mode to use it.</p>{/if}
  <div class="actions">
    <button type="button" onclick={() => (unsupportedBatchTask = null)}>OK</button>
  </div>
</SourceDialog>
<SourceDialog title="Quality Metrics" open={dialog === 'metrics'} onclose={() => (dialog = null)}>
  <div class="form">
    <label for="metric-encoded">Encoded (Low Quality)</label><select
      id="metric-encoded"
      bind:value={dialogMetricsEncoded}
    >
      {#each files as item}<option value={item.path}>{item.name}</option>{/each}
    </select><label for="metric-reference">Reference (Source)</label><select
      id="metric-reference"
      bind:value={dialogMetricsReference}
    >
      {#each files as item}<option value={item.path}>{item.name}</option>{/each}
    </select>
  </div>
  <h3>METRICS</h3>
  <div class="inline">
    <label><input type="checkbox" bind:checked={dialogVmaf} /> VMAF</label><label>
      <input type="checkbox" bind:checked={dialogSsim} /> SSIM
    </label><label><input type="checkbox" bind:checked={dialogPsnr} /> PSNR</label>
  </div>
  <div class="form">
    <label for="metric-align">Alignment</label><select
      id="metric-align"
      bind:value={dialogAlignment}
    >
      <option value="none">None</option><option value="cropReference">
        Auto-Crop reference
      </option><option value="resizeReference">Resize reference</option><option
        value="cropAndResizeReference"
      >
        Auto-Crop and resize reference
      </option>
    </select><label for="metric-subsample">Subsampling</label><select
      id="metric-subsample"
      bind:value={dialogSubsample}
    >
      {#each Array.from({ length: 32 }, (_, i) => i + 1) as n}<option value={n}>
          {n === 1 ? '1 (Every frame)' : `${n} (Every ${n} frames)`}
        </option>{/each}
    </select><label for="metric-model">VMAF Model</label><select
      id="metric-model"
      bind:value={dialogVmafModel}
    >
      <option value="standard">vmaf_v0.6.1 (Default)</option><option value="negative">
        vmaf_v0.6.1neg (No Enhancement Gain)
      </option><option value="fourK">vmaf_4k_v0.6.1 (4K)</option>
    </select>
  </div>
  <div class="actions">
    <button type="button" onclick={() => (dialog = null)}>Cancel</button>
    <button
      type="button"
      onclick={() => {
        metricsEncoded = dialogMetricsEncoded;
        metricsReference = dialogMetricsReference;
        metricsVmaf = dialogVmaf;
        metricsSsim = dialogSsim;
        metricsPsnr = dialogPsnr;
        alignment = dialogAlignment;
        subsample = dialogSubsample;
        vmafModel = dialogVmafModel;
        dialog = null;
      }}
    >
      OK
    </button>
  </div>
</SourceDialog>
<SourceDialog
  title="Transfer Color Metadata"
  open={dialog === 'color'}
  onclose={() => (dialog = null)}
>
  <div class="form">
    <label for="color-source">Source Video</label><select
      id="color-source"
      bind:value={dialogColorSource}
      onchange={() => {
        if (dialogColorTarget === dialogColorSource) dialogColorTarget = '';
      }}
    >
      {#each fileMode === 'batch' && file ? [file] : files as item}<option value={item.path}>
          {item.name}
        </option>{/each}
    </select><label for="color-target">Target Video</label><select
      id="color-target"
      bind:value={dialogColorTarget}
      disabled={fileMode === 'batch'}
    >
      <option value="">(none - only read the source's metadata)</option>{#each files as item}<option
          value={item.path}
          disabled={item.path === dialogColorSource}
        >
          {item.name}{item.path === dialogColorSource ? ' (same as source)' : ''}
        </option>{/each}
    </select>
  </div>
  {#if fileMode !== 'batch'}<p class:overwrite-warning={!!dialogColorTarget}>
      {#if dialogColorTarget}
        Picking a target rewrites that file in place — running will overwrite {fileName(
          dialogColorTarget,
        )} with the selected metadata from {fileName(dialogColorSource)}.
      {:else}
        With no target picked, this utility only prints the source's metadata. Picking one rewrites
        that file in place.
      {/if}
    </p>{/if}
  <h3>DATA TO TRANSFER</h3>
  <div class="check-stack">
    <label>
      <input type="checkbox" bind:checked={dialogCopyColor} /> Color space (transfer, primaries, matrix,
      range)
    </label>
    <label>
      <input type="checkbox" bind:checked={dialogCopyHdr} /> HDR metadata (mastering display, MaxCLL/MaxFALL)
    </label>
  </div>
  {#if fileMode === 'batch'}<p>
      In Batch Processing Mode this utility can only read metadata. Switch to Muxing Mode to
      transfer it.
    </p>{/if}
  <div class="actions">
    <button type="button" onclick={() => (dialog = null)}>Cancel</button>
    <button
      type="button"
      onclick={() => {
        colorSource = dialogColorSource;
        colorTarget = dialogColorTarget === dialogColorSource ? '' : dialogColorTarget;
        copyColor = dialogCopyColor;
        copyHdr = dialogCopyHdr;
        dialog = null;
      }}
    >
      OK
    </button>
  </div>
</SourceDialog>
<SourceDialog
  title="Configure Deinterlace Video"
  open={dialog === 'deinterlace'}
  onclose={() => (dialog = null)}
>
  <p>This utility's settings are separate from the encode tabs.</p>
  <h3>SOURCE</h3>
  <p>{file?.name ?? 'No input file is loaded.'}</p>
  <div class="form">
    <label for="deinterlace-method">Deinterlacer</label><select
      id="deinterlace-method"
      bind:value={deinterlace}
    >
      <option value="automatic">Automatic</option><option value="disabled">Disabled</option><option
        value="qtgmc"
      >
        QTGMC (VapourSynth)
      </option><option value="bwdif">Bwdif (FFmpeg)</option><option value="yadif">
        Yadif (FFmpeg)
      </option>
    </select>{#if deinterlace === 'qtgmc' || deinterlace === 'automatic'}<label
        for="deinterlace-preset"
      >
        QTGMC preset
      </label><select id="deinterlace-preset" bind:value={qtgmcPreset}>
        <option value="placebo">Placebo</option><option value="verySlow">Very Slow</option><option
          value="slower"
        >
          Slower
        </option><option value="slow">Slow</option><option value="medium">Medium</option><option
          value="fast"
        >
          Fast
        </option><option value="faster">Faster</option><option value="veryFast">Very Fast</option>
      </select>{/if}
  </div>
  <label class="dialog-check">
    <input type="checkbox" bind:checked={doubleRate} disabled={deinterlace === 'disabled'} /> One frame
    per field
  </label>
  <h3>RESULT</h3>
  <p>
    Written beside the source as a near-lossless x264 MKV, with audio and subtitles copied across.
  </p>
  <p class="live-note">Changes apply immediately; OK saves them as defaults.</p>
  <div class="actions">
    <button
      type="button"
      onclick={() => {
        deinterlace = 'qtgmc';
        qtgmcPreset = 'verySlow';
        doubleRate = true;
      }}
    >
      Reset
    </button><button type="button" onclick={() => saveUtilitySettings('deinterlace')}>OK</button>
  </div>
</SourceDialog>
<SourceDialog
  title="Configure Film Grain (AV1)"
  open={dialog === 'grain'}
  onclose={() => (dialog = null)}
>
  <h3>SOURCE</h3>
  <p>{file?.name ?? 'No input file is loaded.'}</p>
  <h3>OPERATION</h3>
  <div class="form">
    <label for="grain-operation">Do</label><select id="grain-operation" bind:value={grainOp}>
      <option value="measureSource">Measure a grain table from this file</option><option
        value="extract"
      >
        Extract the grain table it already has
      </option><option value="apply">Apply grain to this file</option><option value="remove">
        Remove all grain from this file
      </option>
    </select>{#if grainOp === 'measureSource'}<label for="grain-denoise">Denoise strength</label>
      <div class="inline">
        <input
          id="grain-denoise"
          type="number"
          min="1"
          max="16"
          bind:value={denoise}
          onblur={() => (denoise = Math.max(1, Math.min(16, Math.trunc(denoise || 4))))}
        /><label>
          <input type="checkbox" bind:checked={keepDenoised} /> Keep the denoised video too
        </label>
      </div>{:else if grainOp === 'apply'}<label for="grain-source">Grain from</label><select
        id="grain-source"
        bind:value={grainSource}
      >
        <option value="table">Grain table file</option><option value="preset">
          Film stock preset
        </option><option value="photonNoise">Photon noise (ISO)</option>
      </select>{#if grainSource === 'table'}<span></span>
        <div class="inline">
          <input
            aria-label="Grain table file"
            bind:value={grainTable}
            placeholder="Path to a .tbl grain table"
          /><button
            type="button"
            aria-label="Browse grain table"
            title="Browse grain table"
            onclick={async () => {
              const path = await chooseUtilityFile('Pick a film grain table', ['tbl']);
              if (path) grainTable = path;
            }}
          >
            …
          </button>
        </div>{:else if grainSource === 'preset'}<span></span><select
          aria-label="Film stock preset"
          bind:value={grainPreset}
        >
          {#each grainPresets as item}<option value={item}>{item}</option>{/each}
        </select>{:else}<span></span>
        <div class="inline">
          <input
            aria-label="ISO"
            type="number"
            min="1"
            max="409600"
            step="100"
            bind:value={iso}
            onblur={() => (iso = Math.max(1, Math.min(409600, Math.trunc(iso || 400))))}
          /><label><input type="checkbox" bind:checked={chroma} /> Chroma</label>
        </div>{/if}{/if}
  </div>
  <h3>RESULT</h3>
  <p>
    {grainOp === 'measureSource'
      ? `Denoises the source with hqdn3d=${denoise}:${denoise * 0.75}:0:0, measures what came out against the original, and writes '<name>.grain.tbl' beside it${keepDenoised ? ", keeping the denoised video as '<name>_denoised.mkv'" : ''}. Works on any source.`
      : grainOp === 'extract'
        ? "Reads the grain table out of an AV1 file that already synthesises grain, into '<name>.grain.tbl'. Takes seconds; it only reads headers. A file with no grain in it has no table to give."
        : grainOp === 'apply'
          ? "Writes the grain into the AV1 headers of a copy, '<name>_grain', without re-encoding anything. The picture is untouched and the file is barely larger; every AV1 decoder regenerates the grain at playback."
          : "Strips every grain header out of a copy, '<name>_nograin', without re-encoding anything. What goes is the grain the decoder was adding, not anything that was coded."}
  </p>
  {#if grainWarning()}<h3>WARNING</h3>
    <p>{grainWarning()}</p>{/if}
  <p class="live-note">Changes apply immediately; OK saves them as defaults.</p>
  <div class="actions">
    <button
      type="button"
      onclick={() => {
        grainOp = 'measureSource';
        grainSource = 'table';
        grainTable = '';
        grainPreset = grainPresets[0] ?? 'Super8';
        denoise = 4;
        keepDenoised = false;
        iso = 400;
        chroma = false;
      }}
    >
      Reset
    </button><button type="button" onclick={() => saveUtilitySettings('grain')}>OK</button>
  </div>
</SourceDialog>
<SourceDialog
  title="Configure Sample Encodes"
  open={dialog === 'ladder'}
  onclose={() => (dialog = null)}
>
  <h3>SOURCE</h3>
  <p>{file?.name ?? 'The settings will apply to the file loaded when Run is pressed.'}</p>
  <h3>ENCODER</h3>
  <div class="form">
    <label for="ladder-encoder">Encoder</label><select
      id="ladder-encoder"
      bind:value={ladderEncoder}
      onchange={resetLadderEncoder}
    >
      <option value="av1">AV1 (SVT-AV1)</option><option value="aomAv1">AV1 (AOM-AV1)</option><option
        value="hevc"
      >
        H.265 / HEVC (x265)
      </option><option value="h264">H.264 / AVC (x264)</option><option value="vp9">
        VP9 (VPX)
      </option>
    </select><label for="ladder-preset">Preset</label><select
      id="ladder-preset"
      bind:value={ladderPreset}
    >
      {#each ladderSpeeds as speed}<option value={speed}>{speed}</option>{/each}
    </select><label for="ladder-format">Color Format</label><select
      id="ladder-format"
      bind:value={ladderPixel}
    >
      {#each ladderFormats as format}<option value={format}>{pixelLabel(format)}</option>{/each}
    </select>{#if presets.length}<label for="ladder-content">Content preset</label><select
        id="ladder-content"
        bind:value={ladderContent}
      >
        <option value="">None</option>{#each presets as preset}<option value={preset.name}>
            {preset.name}
          </option>{/each}
      </select>{/if}
  </div>
  <h3>LADDER</h3>
  <div class="form">
    <label for="ladder-crfs">CRF values</label><input
      id="ladder-crfs"
      bind:value={ladderCrfs}
      placeholder={ladderDefaults()[2]}
    /><label for="ladder-samples">Samples</label>
    <div class="inline">
      <input
        id="ladder-samples"
        type="number"
        min="1"
        max="8"
        bind:value={sampleCount}
        onblur={() => (sampleCount = Math.max(1, Math.min(8, Math.trunc(sampleCount || 3))))}
      />
      of
      <input
        aria-label="Seconds per sample"
        type="number"
        min="2"
        max="120"
        bind:value={sampleSeconds}
        onblur={() => (sampleSeconds = Math.max(2, Math.min(120, Math.trunc(sampleSeconds || 10))))}
      /> seconds
    </div>
    <label for="ladder-metric">Score with</label>
    <div class="inline">
      <select id="ladder-metric" bind:value={ladderMetric}>
        <option value="vmaf">VMAF</option><option value="ssimulacra2">SSIMULACRA2</option><option
          value="butteraugliInf"
        >
          Butteraugli
        </option><option value="xpsnrWeighted">XPSNR</option><option value="none">
          Nothing (size only)
        </option>
      </select><select
        aria-label="VMAF Model"
        bind:value={ladderModel}
        disabled={ladderMetric !== 'vmaf'}
      >
        <option value="standard">vmaf_v0.6.1 (Default)</option><option value="negative">
          vmaf_v0.6.1neg (No Enhancement Gain)
        </option><option value="fourK">vmaf_4k_v0.6.1 (4K)</option>
      </select>
    </div>
  </div>
  <label class="dialog-check">
    <input type="checkbox" bind:checked={keepSamples} /> Keep the sample encodes beside the source
  </label>
  <h3>RESULT</h3>
  <p>{ladderDescription()}</p>
  <p class="live-note">Changes apply immediately; OK saves them as defaults.</p>
  <div class="actions">
    <button type="button" onclick={resetLadder}>Reset</button><button
      type="button"
      onclick={() => saveUtilitySettings('ladder')}
    >
      OK
    </button>
  </div>
</SourceDialog>
<SharedCutDialog
  open={dialog === 'cut'}
  {file}
  videoStreamIndex={mainVideo?.index}
  value={cutValue}
  purpose="utility"
  videoCopied={true}
  onclose={() => (dialog = null)}
  onconfirm={(value) => {
    cutStart = (value.startMilliseconds ?? 0) / 1000;
    cutEnd = (value.endMilliseconds ?? 0) / 1000;
    cutConfigured = value.enabled;
    dialog = null;
  }}
/>
<SourceDialog
  title="Sample Encodes"
  open={dialog === 'ladderResult'}
  closedby="any"
  onclose={() => (dialog = null)}
  width={760}
>
  {#if ladderResult?.kind === 'crfLadder'}<h3>CRF LADDER</h3>
    <!-- svelte-ignore a11y_no_noninteractive_tabindex (The scrollable table needs keyboard access.) -->
    <div class="table-scroll" tabindex="0" role="region" aria-label="CRF ladder results">
      <table>
        <caption class="visually-hidden">Sample encode CRF ladder results</caption>
        <thead>
          <tr>
            <th scope="col">CRF</th>
            <th scope="col">Video bitrate</th>
            <th scope="col">Per minute</th>
            <th scope="col">Whole file</th>
            <th scope="col">Of source</th>
            {#if ladderResult.result.metric !== 'none'}<th scope="col">Score</th>{/if}
            <th scope="col">Encode time</th>
          </tr>
        </thead><tbody>
          {#each ladderResult.result.rungs as row}<tr>
              <td>{row.crf}</td><td>{row.bitrateKbps.toFixed(1)} kb/s</td><td>
                {formatBytes(row.bytesPerMinute)}
              </td><td>{formatBytes(row.projectedSizeBytes)}</td><td>
                {Number(ladderResult.result.sourceSizeBytes) > 0
                  ? `${((Number(row.projectedSizeBytes) / Number(ladderResult.result.sourceSizeBytes)) * 100).toFixed(1)}%`
                  : '—'}
              </td>{#if ladderResult.result.metric !== 'none'}<td>
                  {row.score?.toFixed(3) ?? '—'}
                </td>{/if}<td>{row.encodeSeconds.toFixed(1)}s</td>
            </tr>{/each}
        </tbody>
      </table>
    </div>{/if}
  <div class="actions">
    <button type="button" onclick={copyLadder}>{ladderCopied ? 'Copied' : 'Copy'}</button><button
      type="button"
      onclick={() => (dialog = null)}
    >
      Close
    </button>
  </div>
</SourceDialog>
<SourceDialog
  title="Bitrate Chart"
  open={dialog === 'chartResult'}
  closedby="any"
  onclose={() => (dialog = null)}
  width={760}
>
  {#if chartResult}
    {@const peak = Math.max(1, chartResult.peakWindowMegabitsPerSecond)}
    {@const minTime = chartResult.points[0]?.startSeconds ?? 0}
    {@const maxTime = chartResult.points.at(-1)?.startSeconds ?? minTime}
    {@const midTime = minTime + (maxTime - minTime) / 2}
    {@const rangeSummary = selectedChartPoints.length
      ? (() => {
          const first = selectedChartPoints[0];
          const last = selectedChartPoints.at(-1)!;
          const minK = (
            Math.min(...selectedChartPoints.map((p) => p.megabitsPerSecond)) * 1000
          ).toFixed(0);
          const maxK = (
            Math.max(...selectedChartPoints.map((p) => p.megabitsPerSecond)) * 1000
          ).toFixed(0);
          const avgK = (
            (selectedChartPoints.reduce((sum, p) => sum + p.megabitsPerSecond, 0) /
              selectedChartPoints.length) *
            1000
          ).toFixed(0);
          return `Selected ${formatDuration(first.startSeconds)} to ${formatDuration(last.startSeconds)} (${selectedChartPoints.length} samples) · Min ${minK}k · Max ${maxK}k · Average ${avgK}k`;
        })()
      : ''}
    <p>{rangeSummary || 'Drag to select a range. Double click to reset.'}</p>
    <div
      role="slider"
      tabindex="0"
      aria-label="Bitrate chart; drag to select a range, press Escape to reset"
      aria-valuemin="0"
      aria-valuemax={Math.max(0, chartResult.points.length - 1)}
      aria-valuenow={chartEnd ?? 0}
      aria-valuetext={rangeSummary || 'No range selected'}
      onpointerdown={(event) => {
        chartStart = chartIndex(event);
        chartEnd = chartStart;
        chartDragging = true;
        event.currentTarget.setPointerCapture(event.pointerId);
      }}
      onpointermove={(event) => {
        if (chartDragging) chartEnd = chartIndex(event);
      }}
      onpointerup={() => (chartDragging = false)}
      onpointercancel={() => (chartDragging = false)}
      ondblclick={() => {
        chartStart = null;
        chartEnd = null;
      }}
      onkeydown={(event) => {
        if (event.key === 'Escape') {
          event.preventDefault();
          event.stopPropagation();
          chartStart = null;
          chartEnd = null;
        } else if (event.key === 'ArrowLeft' || event.key === 'ArrowRight') {
          event.preventDefault();
          const next = Math.max(
            0,
            Math.min(
              chartResult!.points.length - 1,
              (chartEnd ?? 0) + (event.key === 'ArrowRight' ? 1 : -1),
            ),
          );
          if (!event.shiftKey || chartStart === null) chartStart = next;
          chartEnd = next;
        }
      }}
    >
      <svg viewBox="0 0 640 260" aria-hidden="true">
        <line x1="56" y1="16" x2="624" y2="16" stroke="var(--rule)" stroke-dasharray="2 3" />
        <line x1="56" y1="118" x2="624" y2="118" stroke="var(--rule)" stroke-dasharray="2 3" />
        <line x1="56" y1="220" x2="624" y2="220" stroke="var(--border)" />
        <line x1="56" y1="16" x2="56" y2="220" stroke="var(--border)" />
        <text x="50" y="20" text-anchor="end" class="chart-axis">{peak.toFixed(1)}M</text>
        <text x="50" y="122" text-anchor="end" class="chart-axis">{(peak / 2).toFixed(1)}M</text>
        <text x="50" y="224" text-anchor="end" class="chart-axis">0M</text>
        <text x={CHART_PLOT_LEFT} y="242" text-anchor="start" class="chart-axis">
          {formatDuration(minTime)}
        </text>
        <text
          x={CHART_PLOT_LEFT + CHART_PLOT_WIDTH / 2}
          y="242"
          text-anchor="middle"
          class="chart-axis"
        >
          {formatDuration(midTime)}
        </text>
        <text x={CHART_PLOT_RIGHT} y="242" text-anchor="end" class="chart-axis">
          {formatDuration(maxTime)}
        </text>
        {#if chartStart !== null && chartEnd !== null}<rect
            x={chartX(Math.min(chartStart, chartEnd), chartResult.points.length)}
            y="16"
            width={Math.max(
              2,
              chartX(Math.max(chartStart, chartEnd), chartResult.points.length) -
                chartX(Math.min(chartStart, chartEnd), chartResult.points.length),
            )}
            height="204"
            fill="currentColor"
            opacity="0.14"
          />{/if}<polyline
          fill="none"
          stroke="currentColor"
          stroke-width="1.5"
          points={chartResult.points
            .map(
              (p, i) =>
                `${chartX(i, chartResult!.points.length)},${220 - (p.megabitsPerSecond * 204) / peak}`,
            )
            .join(' ')}
        />
      </svg>
    </div>{/if}
  <div class="actions">
    <button type="button" onclick={() => (dialog = null)}>Close</button>
  </div>
</SourceDialog>

<style>
  .source-utilities {
    width: 100%;
    flex: 1;
    min-height: 0;
    overflow-y: auto;
    padding: 2px 0;
    container-type: inline-size;
  }
  .utilities-intro {
    display: flex;
    flex-wrap: wrap;
    align-items: center;
    justify-content: space-between;
    gap: 6px 12px;
    margin-bottom: 8px;
    max-width: 1280px;
  }
  .utilities-intro > p {
    color: var(--muted-foreground);
    font-size: 0.6906rem;
  }
  .batch-indicator {
    padding: 2px 8px;
    border: 1px solid var(--border);
    background: var(--secondary);
    color: var(--foreground);
    font-size: 0.6906rem;
    font-weight: 600;
  }
  .cards {
    display: grid;
    grid-template-columns: repeat(3, minmax(0, 1fr));
    gap: 8px;
    max-width: 1280px;
  }
  @container (inline-size <= 960px) {
    .cards {
      grid-template-columns: repeat(2, minmax(0, 1fr));
    }
  }
  @media (max-width: 1000px) and (max-height: 800px) {
    .cards {
      gap: 5px;
    }
    .cards .card {
      min-height: 0;
      padding: 6px 8px;
    }
    .cards .card small {
      margin-top: 3px;
      line-height: 1.2;
    }
    .cards .card .card-configure {
      padding: 4px 7px;
    }
  }
  .card {
    position: relative;
    min-height: 84px;
    padding: 10px;
    border: 1px solid var(--border);
    background: #dfdac5;
  }
  .card:hover {
    background: var(--secondary);
  }
  .card.chosen {
    background: var(--accent);
    border-color: var(--primary);
    border-inline-start: 4px solid var(--primary);
  }
  @media (forced-colors: active) {
    .card.chosen {
      border-color: Highlight;
    }
  }
  .card-head {
    display: flex;
    justify-content: space-between;
    align-items: center;
    min-height: 28px;
    gap: 6px;
  }
  .card-badge {
    position: relative;
    z-index: 1;
    padding: 2px 6px;
    border: 1px solid var(--border);
    background: var(--secondary);
    color: var(--muted-foreground);
    font-size: 0.6256rem;
    white-space: nowrap;
  }
  .card-select {
    flex: 1;
    min-width: 0;
    padding: 0;
    border: 0;
    background: transparent;
    color: var(--foreground);
    text-align: left;
    cursor: pointer;
  }
  .card-select::after {
    content: '';
    position: absolute;
    inset: 0;
  }
  .card-select:focus-visible {
    outline: 0;
  }
  .card:has(.card-select:focus-visible) {
    outline: 2px solid var(--ring);
    outline-offset: 2px;
  }
  .card-head strong {
    letter-spacing: 0.045em;
  }
  .card-configure {
    position: relative;
    z-index: 1;
    flex-shrink: 0;
  }
  .card small {
    display: block;
    margin-top: 6px;
    color: var(--muted-foreground);
    font-size: 0.6906rem;
  }
  .form {
    display: grid;
    grid-template-columns: 180px minmax(0, 1fr);
    align-items: center;
    gap: 6px;
    margin: 8px 0;
  }
  select,
  input:not([type='checkbox']) {
    min-width: 0;
    max-width: 100%;
    padding: 5px 7px;
    border: 1px solid var(--border);
    background: var(--background);
    color: inherit;
  }
  input[type='number'] {
    width: 11ch;
  }
  .check-stack {
    display: grid;
    gap: 6px;
  }
  .dialog-check {
    display: flex;
    align-items: center;
    gap: 6px;
    margin-top: 8px;
  }
  .overwrite-warning {
    color: var(--destructive);
    font-weight: 600;
  }
  .live-note {
    margin-top: 8px;
    color: var(--muted-foreground);
    font-size: 0.6906rem;
  }
  .chart-axis {
    fill: var(--muted-foreground);
    font-size: 0.6875rem;
  }
  button {
    padding: 5px 10px;
    border: 1px solid var(--border);
    background: var(--secondary);
    color: var(--foreground);
    cursor: pointer;
  }
  button:hover:not(:disabled) {
    background: var(--accent);
  }
  .card-select:hover:not(:disabled) {
    background: transparent;
  }
  .inline {
    display: flex;
    gap: 7px;
    align-items: center;
    flex-wrap: wrap;
  }
  .actions {
    display: flex;
    justify-content: flex-end;
    gap: 6px;
    margin-top: 16px;
  }
  h3 {
    margin: 14px 0 5px;
    font-size: 0.6906rem;
    letter-spacing: 0.08em;
  }
  .table-scroll {
    overflow-x: auto;
  }
  table {
    width: 100%;
    border-collapse: collapse;
  }
  th,
  td {
    text-align: left;
    border-bottom: 1px solid var(--border);
    padding: 5px;
  }
  svg {
    width: 100%;
    height: 240px;
    color: var(--primary);
  }
</style>
