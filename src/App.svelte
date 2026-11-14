<script lang="ts">
  import { onMount, tick } from 'svelte';
  import { appVersion } from '$lib/app-version';
  import {
    preferences,
    forgetMedia,
    loadPreferences,
    updatePreferences,
    rememberMedia,
  } from '$lib/preferences.svelte';
  import {
    ArrowRight,
    Check,
    Clapperboard,
    ChevronDown,
    ChevronUp,
    CircleAlert,
    FilePlus2,
    Files,
    Film,
    FlaskConical,
    FolderOpen,
    Layers,
    LoaderCircle,
    Monitor,
    Plus,
    Search,
    SlidersHorizontal,
    Terminal,
    Trash2,
    Wrench,
    X,
  } from '@lucide/svelte';
  import { Button } from '$lib/components/ui/button';
  import WindowControls from '$lib/components/WindowControls.svelte';
  import FileInspector from '$lib/features/files/FileInspector.svelte';
  import SourceFileList from '$lib/features/source/FileList.svelte';
  import SourceTrackList from '$lib/features/source/TrackList.svelte';
  import SourceSettings from '$lib/features/source/SourceSettings.svelte';
  import SourceDialog from '$lib/components/shared/SourceDialog.svelte';
  import { batchRunId } from '$lib/source-workspace';
  import type {
    FileListMode,
    TrackSelection,
    TaskView,
    WorkspaceAction,
  } from '$lib/source-workspace';
  import QuickConvert from '$lib/features/convert/QuickConvert.svelte';
  import Av1an from '$lib/features/av1an/Av1an.svelte';
  import BatchEncode from '$lib/features/batch/BatchEncode.svelte';
  import Remux from '$lib/features/remux/Remux.svelte';
  import JobStatus from '$lib/components/shared/JobStatus.svelte';
  import { terminalJob } from '$lib/components/shared/job-state';
  import { liveJobProgress } from '$lib/components/shared/job-progress';
  import {
    ProgressEstimator,
    type ProgressEstimate,
  } from '$lib/components/shared/progress-estimate';
  import ToolsPanel from '$lib/features/tools/ToolsPanel.svelte';
  import SourceUtilities from '$lib/features/utilities/SourceUtilities.svelte';
  import SavedJobs from '$lib/features/utilities/SavedJobs.svelte';
  import { encoderChoices, encoderOptions } from '$lib/components/shared/encoder-options';
  import {
    chooseMediaFiles,
    chooseMediaFolder,
    importImageSequence,
    releaseImageSequence,
    scanMediaFolder,
    getCapabilities,
    isDesktop,
    beginMediaImport,
    endMediaImport,
    probeMedia,
    subscribeDrop,
    subscribeJobs,
    startRemux,
    startMux,
    startEncode,
    enqueueEncode,
    enqueueEncodeBatch,
    cancelAllJobs,
    cancelJob,
    stopJob as stopAndKeepProgress,
    resumeJob,
    discardAv1anRecovery,
    setJobPaused,
    recentPathIsFolder,
    getStorageLocations,
    saveActivityLog,
    openDirectory,
    extractAttachment,
    getCompletionStatus,
    setCompletionOptions,
    cancelFinishAction,
  } from '$lib/ipc/client';
  import { restoreAndTrackWindow } from '$lib/window-state';
  import type {
    CompletionStatus,
    EncodeRequest,
    JobSnapshot,
    RemuxRequest,
    MuxRequest,
    MediaFile,
    ToolInfo,
  } from '$lib/ipc/generated';
  import {
    displayCodec,
    errorMessage,
    fileName,
    formatBytes,
    formatDuration,
  } from '$lib/components/shared/format';

  type View = 'files' | 'tracks' | 'convert' | 'av1an' | 'batch' | 'remux' | 'tools' | 'utilities';
  type LogEntry = { id: number; time: string; level: 'info' | 'error'; message: string };
  type ImportEntry = { path: string; detectFolder: boolean };
  type ImportSession = {
    generation: number;
    folders: string[];
    completed: string[];
    ticket: string | null;
  };
  const desktop = isDesktop();
  const sampleId = 'jesses-synthetic-preview';
  let view = $state<View>('files');
  let uiStateReady = $state(false);
  let logHeight = $state(140);
  let logUserAdjusted = $state(false);
  let logActionBusy = $state(false);
  $effect(() => {
    if (!uiStateReady) return;
    try {
      localStorage.setItem('jesses.active-view', view);
      localStorage.setItem('jesses.log-panel-height', String(logHeight));
    } catch {
      /* UI state is optional. */
    }
  });
  let files = $state<MediaFile[]>([]);
  let selectedId = $state<string | null>(null);
  let selectedFileIds = $state<string[]>([]);
  let importChoices = $state<{ path: string; checked: boolean }[] | null>(null);
  let finishImportChoices:
    ((value: { paths: string[]; replace: boolean } | null) => void) | undefined;
  function chooseImportItems(paths: string[]) {
    if (!files.length || !paths.length) return Promise.resolve({ paths, replace: false });
    if (finishImportChoices) return Promise.resolve(null);
    importChoices = paths.map((path) => ({ path, checked: true }));
    return new Promise<{ paths: string[]; replace: boolean } | null>((resolve) => {
      finishImportChoices = resolve;
    });
  }
  function finishImportDialog(replace?: boolean) {
    const resolve = finishImportChoices;
    const chosen = importChoices?.filter((item) => item.checked).map((item) => item.path) ?? [];
    finishImportChoices = undefined;
    importChoices = null;
    resolve?.(replace === undefined ? null : { paths: chosen, replace });
  }
  let sequencePrompt = $state<{ path: string; text: string; error: string } | null>(null);
  let finishSequencePrompt:
    ((rate: { numerator: number; denominator: number } | null) => void) | undefined;
  let sequenceImportAbort: AbortController | undefined;
  function askSequenceRate(path: string) {
    sequencePrompt = { path, text: '30', error: '' };
    return new Promise<{ numerator: number; denominator: number } | null>((resolve) => {
      finishSequencePrompt = resolve;
    });
  }
  function finishSequenceRate(useDefault = false) {
    if (!sequencePrompt) return;
    const text = useDefault ? '30' : sequencePrompt.text.trim() || '30';
    const match = /^(\d+)(?:\/(\d+))?$/.exec(text);
    const decimal = /^\d+(?:\.\d+)?$/.test(text) ? Math.fround(Number(text)) : NaN;
    const floor = Math.floor(decimal);
    const rounded = decimal - floor === 0.5 && floor % 2 === 0 ? floor : Math.round(decimal);
    const numerator = match ? Number(match[1]) : rounded;
    const denominator = match ? Number(match[2] || 1) : 1;
    if (
      !Number.isSafeInteger(numerator) ||
      !Number.isSafeInteger(denominator) ||
      numerator < 1 ||
      denominator < 1 ||
      numerator > 2147483647 ||
      denominator > 2147483647 ||
      numerator / denominator < 0.001 ||
      numerator / denominator > 1000
    ) {
      sequencePrompt.error = 'Enter a positive frame rate, such as 30 or 24000/1001.';
      return;
    }
    const resolve = finishSequencePrompt;
    finishSequencePrompt = undefined;
    sequencePrompt = null;
    resolve?.({ numerator, denominator });
  }
  function displaySourcePath(file: MediaFile) {
    return file.source?.kind === 'imageSequence' ? file.source.directoryPath : file.path;
  }
  let mainSourceId = $state<string | null>(null);
  let fileMode = $state<FileListMode>('mux');
  let batchNameTemplate = $state('{name}');
  let trackSelection = $state<TrackSelection[]>([]);
  let noDefaultSubtitles = $state(false);
  let workspaceActions = $state<Partial<Record<TaskView, WorkspaceAction>>>({});
  let lastActionStatus = $state('');
  let lastTaskTab = $state<'convert' | 'av1an' | 'utilities'>('av1an');
  let completionStatus = $state<CompletionStatus>();
  const shutdownWhenDone = $derived(completionStatus?.options.finishAction === 'shutdown');
  let shutdownError = $state('');
  let updatingShutdown = $state(false);
  let completionGeneration = 0;
  const mainSource = $derived(files.find((file) => file.id === mainSourceId));
  const taskSource = $derived(fileMode === 'batch' ? files[0] : mainSource);
  const batchExample = $derived.by(() => {
    const first = files[0];
    if (!first) return 'Load a file to see an example.';
    const naming =
      lastTaskTab === 'utilities'
        ? { codec: '', quality: '', preset: '', extension: '' }
        : workspaceActions[lastTaskTab]?.batchNaming;
    if (!naming) return '';
    const original =
      displaySourcePath(first)
        .replace(/[\\/]$/, '')
        .split(/[\\/]/)
        .at(-1) ?? first.name;
    const dot = original.lastIndexOf('.');
    const stem =
      first.source?.kind === 'imageSequence'
        ? original
        : dot > 0
          ? original.slice(0, dot)
          : original;
    const now = new Date();
    const values: Record<string, string> = {
      name: stem,
      ext: first.source?.kind === 'imageSequence' ? '' : dot > 0 ? original.slice(dot + 1) : '',
      index: '1'.padStart(String(files.length).length, '0'),
      codec: naming.codec,
      crf: naming.quality,
      quality: naming.quality,
      preset: naming.preset,
      width: '',
      height: '',
      date: `${now.getFullYear()}-${String(now.getMonth() + 1).padStart(2, '0')}-${String(now.getDate()).padStart(2, '0')}`,
    };
    const base =
      (batchNameTemplate.trim() || '{name}')
        .replace(/\{([^{}]+)\}/g, (whole, key: string) => values[key.toLowerCase()] ?? whole)
        .replace(/[\\/:*?"<>|\u0000]/g, '_')
        .trim()
        .replace(/[. ]+$/, '') || stem;
    return `e.g. ${original} → ${base}${naming.extension ? `.${naming.extension}` : ''}`;
  });
  const action = $derived(
    view === 'convert' || view === 'av1an' || view === 'utilities'
      ? workspaceActions[view]
      : undefined,
  );
  const runningAction = $derived(
    Object.values(workspaceActions).find((candidate) => candidate?.active),
  );
  const displayAction = $derived(runningAction ?? action);
  const runBlockedReason = $derived(
    !runningAction && action && !action.canRun ? action.blockedReason : undefined,
  );
  // Image and utility actions report their own progress. Encode batches provide
  // a file label while their current job supplies the per-file progress.
  const actionProgress = $derived(
    displayAction?.active &&
      displayAction.progressJobId === undefined &&
      displayAction.progressDescription
      ? displayAction
      : undefined,
  );
  const batchFileText = $derived(
    runningAction?.progressJobId !== undefined ? runningAction.progressDescription : undefined,
  );
  const currentActionText = $derived.by(() => {
    if (actionProgress) return actionProgress.progressDescription!;
    if (liveProgress) return liveProgress.text;
    if (batchFileText) return batchFileText;
    if (displayAction?.active)
      return `Running: ${displayAction.label.replace(/^Run(?: Batch)?:\s*/, '')}...`;
    if (lastActionStatus) return lastActionStatus;
    if (!currentJob) return '';
    if (currentJob.state === 'succeeded') return 'Done.';
    if (currentJob.state === 'failed' || currentJob.state === 'interrupted')
      return 'Did not finish - the log has the details.';
    return 'Canceled.';
  });
  function registerWorkspaceAction(tab: TaskView, next: WorkspaceAction | null) {
    const previous = workspaceActions[tab];
    if (next?.active && !previous?.active) lastActionStatus = '';
    else if (previous?.active && !next?.active && next?.progressDescription)
      lastActionStatus = next.progressDescription;
    workspaceActions = { ...workspaceActions, [tab]: next ?? undefined };
  }
  function selectFiles(ids: string[]) {
    selectedFileIds = ids;
    selectedId = ids.at(-1) ?? null;
  }
  function loadTracks(ids = selectedFileIds, switchTab = true) {
    const chosen = files.filter((file) => ids.includes(file.id));
    if (!chosen.length) return;
    if (!mainSourceId) mainSourceId = chosen[0].id;
    const existing = new Set(
      trackSelection.map((row) => JSON.stringify([row.sourceId, row.streamIndex])),
    );
    // Cover art (attached pictures) is a still image, not a second picture
    // track; it starts unchecked and never counts as the source's video.
    let hasVideo = trackSelection.some((row) =>
      files
        .find((file) => file.id === row.sourceId)
        ?.streams.some(
          (stream) =>
            stream.index === row.streamIndex && stream.kind === 'video' && !stream.attachedPic,
        ),
    );
    const additions: TrackSelection[] = [];
    for (const file of chosen) {
      const alreadyHadVideo = hasVideo;
      for (const stream of file.streams) {
        const key = JSON.stringify([file.id, stream.index]);
        if (existing.has(key)) continue;
        existing.add(key);
        additions.push({
          sourceId: file.id,
          streamIndex: stream.index,
          included:
            stream.codec?.toLowerCase() !== 'unknown' &&
            !stream.attachedPic &&
            !(stream.kind === 'video' && alreadyHadVideo),
          title: stream.title ?? '',
          language: stream.language ?? '',
          defaultFlag:
            stream.kind === 'subtitle' && noDefaultSubtitles ? false : (stream.isDefault ?? null),
          forcedFlag: null,
        });
        if (stream.kind === 'video' && !stream.attachedPic) hasVideo = true;
      }
    }
    let next = [...trackSelection, ...additions];
    for (const kind of ['audio', 'subtitle']) {
      if (kind === 'subtitle' && noDefaultSubtitles) continue;
      const candidates = next.filter(
        (row) =>
          row.included &&
          files
            .find((file) => file.id === row.sourceId)
            ?.streams.some((stream) => stream.index === row.streamIndex && stream.kind === kind),
      );
      if (candidates.length && !candidates.some((row) => row.defaultFlag === true)) {
        const first = candidates[0];
        next = next.map((row) => (row === first ? { ...row, defaultFlag: true } : row));
      }
    }
    trackSelection = next;
    if (switchTab) view = 'tracks';
  }
  function changeFileMode(next: FileListMode) {
    const previous = fileMode;
    if (previous === 'mux' && next === 'batch') {
      mainSourceId = null;
      trackSelection = [];
      noDefaultSubtitles = false;
    }
    fileMode = next;
    if (previous === 'batch' && next === 'mux' && files.length === 1 && !trackSelection.length)
      loadTracks([files[0].id], false);
  }
  function moveSelectedFile(offset: number) {
    if (selectedFileIds.length !== 1) return;
    const index = files.findIndex((file) => file.id === selectedFileIds[0]);
    if (index < 0 || index + offset < 0 || index + offset >= files.length) return;
    const next = [...files];
    [next[index], next[index + offset]] = [next[index + offset], next[index]];
    files = next;
  }
  function sortFiles(sort: string) {
    const [field, direction] = sort.split(':');
    const sign = direction === 'desc' ? -1 : 1;
    files = [...files].sort((a, b) => {
      if (field === 'mtime') {
        const left = a.modifiedAtMs,
          right = b.modifiedAtMs;
        return left == null
          ? right == null
            ? 0
            : -sign
          : right == null
            ? sign
            : (left - right) * sign;
      }
      if (field === 'size')
        return BigInt(a.sizeBytes) < BigInt(b.sizeBytes)
          ? -sign
          : BigInt(a.sizeBytes) > BigInt(b.sizeBytes)
            ? sign
            : 0;
      return displaySourcePath(a).localeCompare(displaySourcePath(b)) * sign;
    });
  }
  function removeSelectedFiles() {
    for (const id of [...selectedFileIds]) removeFile(id);
    selectedFileIds = [];
  }
  async function extractSelectedAttachment(row: TrackSelection) {
    const source = files.find((file) => file.id === row.sourceId);
    if (!source) throw new Error('The selected source is no longer loaded.');
    const folder = await extractAttachment(source.path, row.streamIndex);
    addLog(`Extracted attachment from ${source.name}.`);
    await openDirectory(folder);
  }
  async function updateCompletion(action: () => Promise<CompletionStatus>) {
    if (updatingShutdown) return;
    ++completionGeneration;
    updatingShutdown = true;
    shutdownError = '';
    try {
      completionStatus = await action();
    } catch (cause) {
      shutdownError = errorMessage(cause);
    } finally {
      updatingShutdown = false;
    }
  }
  function setShutdown(checked: boolean) {
    return updateCompletion(() =>
      setCompletionOptions({
        notify: completionStatus?.options.notify ?? false,
        finishAction: checked ? 'shutdown' : 'none',
      }),
    );
  }
  function resizeLog(event: PointerEvent) {
    const startY = event.clientY;
    const height =
      document.querySelector('.log-entries')?.getBoundingClientRect().height ?? logHeight;
    logUserAdjusted = true;
    try {
      localStorage.setItem('jesses.log-height-user-adjusted', 'true');
    } catch {
      /* Optional UI state. */
    }
    function move(next: PointerEvent) {
      logHeight = Math.max(90, Math.min(400, height - (next.clientY - startY)));
    }
    function finish() {
      window.removeEventListener('pointermove', move);
      window.removeEventListener('pointerup', finish);
      window.removeEventListener('pointercancel', finish);
    }
    window.addEventListener('pointermove', move);
    window.addEventListener('pointerup', finish, { once: true });
    window.addEventListener('pointercancel', finish, { once: true });
  }
  let fileQuery = $state('');
  let tools = $state<ToolInfo[]>(
    [
      { id: 'ffmpeg', name: 'FFmpeg' },
      { id: 'ffprobe', name: 'FFprobe' },
      ...encoderChoices.map(({ value }) => {
        const { tool, name } = encoderOptions(value);
        return { id: tool, name };
      }),
      { id: 'av1an', name: 'av1an' },
    ]
      .filter(
        (tool, index, entries) => entries.findIndex((entry) => entry.id === tool.id) === index,
      )
      .map((tool) => ({ ...tool, available: false, path: null, version: null, detail: null })),
  );
  let toolsLoading = $state(desktop);
  let toolsRevision = $state(0);
  let toolsChecked = $state(false);
  let toolsError = $state<string | null>(null);
  let importing = $state(false);
  let importingName = $state('');
  let recursiveImport = $state(false);
  let recursiveTouched = false;
  let preferencesInitialized = false;
  let recentSelection = $state('');
  $effect(() => {
    const loaded = preferences.loaded;
    const recursive = preferences.value.general.recursiveImport;
    if (loaded) {
      if (preferencesInitialized || !recursiveTouched) recursiveImport = recursive;
      preferencesInitialized = true;
    }
  });
  function changeRecursive(event: Event) {
    recursiveImport = (event.currentTarget as HTMLInputElement).checked;
    recursiveTouched = true;
    if (preferences.loaded)
      void updatePreferences({
        general: { ...preferences.value.general, recursiveImport },
        recentPaths: null,
      }).catch((error) => (preferences.error = errorMessage(error)));
  }
  let importNotice = $state<string | null>(null);
  let importGeneration = 0;
  let importSession: ImportSession | null = null;
  let importQueue: ImportEntry[] = [];
  let activePath: string | null = null;
  let importErrors = $state<{ name: string; message: string }[]>([]);
  let logs = $state<LogEntry[]>([]);
  let logOpen = $state(false);
  let nextLogId = 0;
  let jobs = $state<JobSnapshot[]>([]);
  let batchJobIds = $state<Record<string, string>>({});
  let utilityBatchStatuses = $state<Record<string, import('$lib/source-workspace').BatchRowStatus>>(
    {},
  );
  let batchStatusOwner: string | null = null;
  let batchStatusFiles = new Map<string, MediaFile>();
  function reportBatchStatuses(
    runId: string,
    statuses: Record<string, import('$lib/source-workspace').BatchRowStatus>,
    begin = false,
  ) {
    if (begin) {
      batchStatusOwner = runId;
      batchJobIds = {};
      batchStatusFiles = new Map(files.map((file) => [file.id, file]));
    } else if (batchStatusOwner !== runId) return;
    utilityBatchStatuses = Object.fromEntries(
      Object.entries(statuses).filter(([id]) =>
        files.some((file) => file.id === id && file === batchStatusFiles.get(id)),
      ),
    );
  }
  const batchStatuses = $derived.by(() => {
    const result: Record<string, { glyph: string; text: string; note?: string }> = {};
    for (const [fileId, jobId] of Object.entries(batchJobIds)) {
      const job = jobs.find((item) => item.id === jobId);
      if (!job) continue;
      const source = files.find((entry) => entry.id === fileId);
      const note =
        source?.source?.kind === 'imageSequence'
          ? job.error?.message.replaceAll(source.path, source.source.directoryPath)
          : job.error?.message;
      const state = job.state;
      result[fileId] =
        state === 'queued'
          ? { glyph: '○', text: 'Queued' }
          : state === 'succeeded'
            ? { glyph: '✔', text: 'Done' }
            : state === 'failed' || state === 'interrupted'
              ? { glyph: '✕', text: 'Failed', note }
              : state === 'canceled' || state === 'stopped'
                ? { glyph: '■', text: 'Canceled' }
                : { glyph: '▶', text: 'Running…' };
    }
    return { ...result, ...utilityBatchStatuses };
  });
  let jobsConnected = $state(false);
  let jobsError = $state<string | null>(null);
  let jobsConnecting = $state(false);
  let stopJobSubscription: (() => void) | undefined;
  let jobsDisposed = false;
  const currentJob = $derived(
    jobs.find((job) => !terminalJob(job.state) && job.state !== 'queued') ??
      jobs.filter((job) => job.state === 'queued').at(-1) ??
      jobs[0],
  );
  const liveJob = $derived.by(() => {
    const id = runningAction?.progressJobId;
    const job = id === undefined ? currentJob : jobs.find((entry) => entry.id === id);
    return job && !terminalJob(job.state) ? job : undefined;
  });
  // Estimate a rate from progress seen in this window when the encoder lacks telemetry.
  const progressEstimator = new ProgressEstimator();
  let progressEstimate = $state<ProgressEstimate | null>(null);
  $effect(() => {
    const job = liveJob,
      duration = job?.durationSeconds ?? null;
    progressEstimator.observe(
      job?.state === 'running' ? `${job.id}:${job.state}` : null,
      job?.progressSeconds ?? null,
      performance.now(),
    );
    progressEstimate = progressEstimator.estimate(performance.now(), duration);
  });
  $effect(() => {
    if (liveJob?.state !== 'running') return;
    const duration = liveJob.durationSeconds ?? null;
    const timer = setInterval(() => {
      progressEstimate = progressEstimator.estimate(performance.now(), duration);
    }, 1_000);
    return () => clearInterval(timer);
  });
  const liveProgress = $derived(liveJob ? liveJobProgress(liveJob, progressEstimate) : null);
  const statusProgress = $derived(
    actionProgress
      ? { max: 1, value: actionProgress.progress ?? 0 }
      : (liveProgress ??
          (runningAction
            ? { max: 1, value: null }
            : { max: currentJob?.durationSeconds || 1, value: currentJob?.progressSeconds || 0 })),
  );
  const currentActionTitle = $derived(
    batchFileText && liveProgress ? `${batchFileText} · ${currentActionText}` : currentActionText,
  );

  // Mirror each job's own log into the Log pane: phase, tool and warning lines
  // as they arrive, then the final error. Encoder progress lines are left to
  // the progress bar, and jobs already present when the pane connects are not
  // replayed.
  const jobLogCursors = new Map<string, { last: string | undefined; finished: boolean }>();
  let jobLogsPrimed = false;
  const progressLine =
    /^(?:\[(?:consumer|producer)\]\s*)?(?:Encoding:?\s+\d+(?:\/\d+)?\s+Frames|Encoding frame\s+\d+|frame=\s*\d+|Encoding\s*$)/;
  function mirrorJobLogs(snapshot: JobSnapshot[]) {
    for (const job of snapshot) {
      const lines = job.logs ?? [];
      const finished = terminalJob(job.state);
      const cursor = jobLogCursors.get(job.id);
      if (!cursor && !jobLogsPrimed) {
        jobLogCursors.set(job.id, { last: lines.at(-1), finished });
        continue;
      }
      if (cursor?.finished) continue;
      // Job logs are a bounded window; continue after the last line already
      // shown, or show the whole window when it has moved past that line.
      const seen = cursor?.last === undefined ? -1 : lines.lastIndexOf(cursor.last);
      const failed = finished && (job.state === 'failed' || job.state === 'interrupted');
      const message = failed ? job.error?.message : undefined;
      for (const line of lines.slice(seen + 1))
        if (!progressLine.test(line) && line !== message)
          addLog(line, /\b(?:error|failed)\b/i.test(line) ? 'error' : 'info');
      if (failed) {
        if (message) addLog(message, 'error');
        addLog(
          `Did not finish${job.error?.code ? ` (${job.error.code})` : ''}.${job.logPath ? ` Job log: ${job.logPath}` : ''}`,
          'error',
        );
      }
      jobLogCursors.set(job.id, { last: lines.at(-1), finished });
    }
    jobLogsPrimed = true;
  }

  async function connectJobs() {
    if (!desktop || jobsConnecting) return;
    jobsConnecting = true;
    jobsError = null;
    jobsConnected = false;
    stopJobSubscription?.();
    try {
      const stop = await subscribeJobs((snapshot) => {
        if (jobsDisposed) return;
        jobs = snapshot;
        jobsConnected = true;
        mirrorJobLogs(snapshot);
      });
      if (jobsDisposed) stop();
      else stopJobSubscription = stop;
    } catch (error) {
      jobsError = errorMessage(error);
      addLog(`Job connection failed: ${jobsError}`, 'error');
    } finally {
      jobsConnecting = false;
    }
  }

  async function submitRemux(request: RemuxRequest) {
    const job = await startRemux(request);
    // A channel snapshot may arrive before the command reply. Never regress it.
    if (!jobs.some((entry) => entry.id === job.id)) jobs = [job, ...jobs];
    addLog('Remux job submitted.');
  }

  async function submitMux(request: MuxRequest) {
    const job = await startMux(request);
    // A channel snapshot may arrive before the command reply. Never regress it.
    if (!jobs.some((entry) => entry.id === job.id)) jobs = [job, ...jobs];
    addLog('Multi-source remux job submitted.');
  }

  async function stopJob(id: string) {
    const job = await cancelJob(id);
    jobs = jobs.map((entry) => (entry.id === id && !terminalJob(entry.state) ? job : entry));
  }
  async function keepJobProgress(id: string) {
    const before = jobs.find((entry) => entry.id === id);
    const snapshot = await stopAndKeepProgress(id);
    // The command can reply after its channel has already advanced the job.
    jobs = jobs.map((entry) => (entry.id === id && entry === before ? snapshot : entry));
    addLog('Requested a stop with progress kept.');
  }
  async function resumeSavedJob(id: string) {
    const before = jobs.find((entry) => entry.id === id);
    const snapshot = await resumeJob(id);
    if (jobs.find((entry) => entry.id === id) === before) {
      // New queued work is stored first and displayed in reverse queue order.
      // A resumed job joins the tail behind jobs that were already waiting.
      jobs = [snapshot, ...jobs.filter((entry) => entry.id !== id)];
    }
    addLog('Saved job submitted for resume.');
  }
  async function discardSavedAv1anProgress(id: string) {
    const before = jobs.find((entry) => entry.id === id);
    const snapshot = await discardAv1anRecovery(id);
    jobs = jobs.map((entry) => (entry.id === id && entry === before ? snapshot : entry));
    addLog('Saved AV1AN progress discarded.');
  }
  async function pauseLiveJob(id: string, paused: boolean) {
    const before = jobs.find((entry) => entry.id === id);
    const snapshot = await setJobPaused(id, paused);
    jobs = jobs.map((entry) => (entry.id === id && entry === before ? snapshot : entry));
    addLog(paused ? 'Paused live av1an workers.' : 'Continued live av1an workers.');
  }
  async function activateRecoverySource(path: string) {
    if (importing) throw new Error('Wait for the current import to finish before resuming.');
    const beforeFiles = files,
      beforeMain = mainSourceId,
      generation = ++importGeneration;
    const source = await probeMedia(path);
    if (generation !== importGeneration || files !== beforeFiles || mainSourceId !== beforeMain)
      throw new Error('The source list changed while the saved input was loading.');
    if (source.path !== path)
      throw new Error('The saved input path did not resolve to the same source.');
    clearFiles();
    files = [source];
    selectedId = source.id;
    selectedFileIds = [source.id];
    loadTracks([source.id], false);
    await tick();
    await tick();
    if ((fileMode === 'batch' ? files[0]?.path : mainSource?.path) !== path)
      throw new Error('The saved input could not be made the current source.');
  }
  function acceptRecoveryJobs(before: JobSnapshot[], after: JobSnapshot[]) {
    for (const snapshot of after) {
      const prior = before.find((job) => job.id === snapshot.id),
        current = jobs.find((job) => job.id === snapshot.id);
      if (current !== prior) continue;
      if (current) jobs = jobs.map((job) => (job.id === snapshot.id ? snapshot : job));
      else jobs = [snapshot, ...jobs];
    }
  }
  async function submitEncode(request: EncodeRequest) {
    const job = await startEncode(request);
    if (!jobs.some((entry) => entry.id === job.id)) jobs = [job, ...jobs];
    addLog(`${encoderOptions(request.settings.encoder).name} encode job submitted.`);
  }
  function acceptReviewedJob(job: JobSnapshot) {
    // Subscription updates may arrive before the command reply.
    if (!jobs.some((entry) => entry.id === job.id)) jobs = [job, ...jobs];
  }
  async function queueEncode(request: EncodeRequest) {
    const job = await enqueueEncode(request);
    if (!jobs.some((entry) => entry.id === job.id)) jobs = [job, ...jobs];
    addLog(`${encoderOptions(request.settings.encoder).name} encode added to the queue.`);
  }
  async function queueBatch(requests: EncodeRequest[]) {
    const statusId = batchRunId('encode');
    batchStatusOwner = statusId;
    batchStatusFiles = new Map(files.map((file) => [file.id, file]));
    const submitted = await enqueueEncodeBatch(requests);
    if (batchStatusOwner === statusId) {
      utilityBatchStatuses =
        requests[0]?.settings.backend === 'av1an'
          ? Object.fromEntries(
              files
                .filter((entry) => entry.source?.kind === 'imageSequence')
                .map((entry) => [
                  entry.id,
                  {
                    glyph: '✕',
                    text: 'Failed',
                    note: 'AV1AN does not support image sequence folders.',
                  },
                ]),
            )
          : {};
      batchJobIds = Object.fromEntries(
        submitted
          .map((job) => {
            const file = files.find(
              (entry) =>
                entry === batchStatusFiles.get(entry.id) &&
                pathKey(entry.path) === pathKey(job.request.inputPath),
            );
            return file ? [file.id, job.id] : null;
          })
          .filter((row): row is [string, string] => row !== null),
      );
    }
    // Keep authoritative channel updates that beat the command response.
    const missing = submitted.filter((job) => !jobs.some((entry) => entry.id === job.id));
    jobs = [...missing.reverse(), ...jobs];
    addLog(`${submitted.length} encodes added to the queue.`);
  }
  async function stopQueue() {
    const snapshots = await cancelAllJobs();
    jobs = jobs.map((entry) =>
      terminalJob(entry.state) ? entry : (snapshots.find((job) => job.id === entry.id) ?? entry),
    );
  }
  const selectedFile = $derived(files.find((file) => file.id === selectedId));
  const visibleFiles = $derived(
    files.filter((file) =>
      `${file.name} ${file.path}`
        .toLocaleLowerCase()
        .includes(fileQuery.trim().toLocaleLowerCase()),
    ),
  );
  const totalSize = $derived(files.reduce((total, file) => total + Number(file.sizeBytes), 0));
  const hasSample = $derived(files.some((file) => file.id === sampleId));
  const availableTools = $derived(tools.filter((tool) => tool.available).length);
  const sourceRequired = $derived(
    ((view === 'convert' || view === 'av1an' || view === 'remux') && !selectedFile) ||
      ((view === 'batch' || view === 'utilities') && files.length === 0),
  );

  function addLog(message: string, level: LogEntry['level'] = 'info') {
    for (const file of files)
      if (file.source?.kind === 'imageSequence')
        message = message.replaceAll(file.path, file.source.directoryPath);
    logs = [
      ...logs.slice(-199),
      {
        id: nextLogId++,
        time: new Date().toLocaleTimeString(undefined, { hour12: false }),
        level,
        message,
      },
    ];
  }

  function errorCode(error: unknown) {
    return error && typeof error === 'object' && 'code' in error ? String(error.code) : null;
  }

  async function refreshTools() {
    if (!desktop) return;
    toolsRevision++;
    toolsLoading = true;
    toolsError = null;
    try {
      tools = await getCapabilities();
      toolsChecked = true;
      addLog(
        `Tool check complete: ${tools.filter((tool) => tool.available).length} of ${tools.length} available.`,
      );
    } catch (error) {
      toolsError = errorMessage(error);
      addLog(`Tool detection failed: ${toolsError}`, 'error');
    } finally {
      toolsLoading = false;
    }
  }

  function pathKey(path: string) {
    const windows = path
      .replace(/^\\\\\?\\UNC\\/i, '\\\\')
      .replace(/^\\\\\?\\/i, '')
      .replaceAll('/', '\\');
    if (/^(?:[a-z]:\\|\\\\)/i.test(windows)) return windows.replace(/\\+$/, '').toLowerCase();
    return path.replace(/\/+$/, '');
  }

  function queueImportPaths(paths: string[], detectFolders = false) {
    const activeKey = activePath ? pathKey(activePath) : null;
    const queued = new Set(importQueue.map((entry) => pathKey(entry.path)));
    const loaded = new Set(files.map((file) => pathKey(displaySourcePath(file))));
    for (const path of paths) {
      const key = pathKey(path);
      if (key === activeKey || queued.has(key) || loaded.has(key)) continue;
      queued.add(key);
      importQueue.push({ path, detectFolder: detectFolders });
    }
  }

  function beginImport() {
    fileQuery = '';
    const generation = ++importGeneration;
    importSession = { generation, folders: [], completed: [], ticket: null };
    importQueue = [];
    activePath = null;
    importing = true;
    importNotice = null;
    importErrors = [];
    view = 'files';
    return generation;
  }

  async function releaseImport(ticket: string | null | undefined) {
    if (!ticket) return;
    try {
      await endMediaImport(ticket);
    } catch (error) {
      addLog(`Could not release the media import: ${errorMessage(error)}`, 'error');
    }
  }

  async function reserveImport(generation: number) {
    const session = importSession;
    if (session?.generation !== generation) return false;
    const ticket = await beginMediaImport();
    if (importSession !== session) {
      await releaseImport(ticket);
      return false;
    }
    session.ticket = ticket;
    return true;
  }

  async function finishImport(generation: number) {
    if (importSession?.generation !== generation) return;
    const ticket = importSession.ticket;
    const recent = takeImportRecents(generation);
    activePath = null;
    importing = false;
    importingName = '';
    if (fileMode === 'mux' && files.length === 1 && !mainSourceId) loadTracks([files[0].id], false);
    try {
      if (recent.length) await rememberMedia(recent);
    } finally {
      await releaseImport(ticket);
    }
  }

  function stopImport() {
    sequenceImportAbort?.abort();
    finishSequencePrompt?.(null);
    finishSequencePrompt = undefined;
    sequencePrompt = null;
    void releaseImport(importSession?.ticket);
    const recent = takeImportRecents(importGeneration);
    ++importGeneration;
    importQueue = [];
    activePath = null;
    importing = false;
    importingName = '';
    importNotice = 'Stopped importing. Completed files are kept.';
    addLog('Stopped importing; pending files and late results are discarded.');
    if (recent.length) void rememberMedia(recent);
  }

  function recordImportRecent(generation: number, kind: 'folders' | 'completed', path: string) {
    if (importSession?.generation !== generation) return;
    if (!importSession[kind].some((existing) => pathKey(existing) === pathKey(path)))
      importSession[kind].push(path);
  }

  function takeImportRecents(generation: number) {
    if (importSession?.generation !== generation) return [];
    const recent = [...importSession.folders, ...importSession.completed].slice(0, 15);
    importSession = null;
    return recent;
  }

  async function drainImport(generation: number) {
    try {
      while (generation === importGeneration && importQueue.length) {
        const entry = importQueue.shift()!;
        const path = entry.path;
        activePath = path;
        importingName = fileName(path);
        try {
          const folder = entry.detectFolder && (await recentPathIsFolder(path));
          if (generation !== importGeneration) return;
          if (folder) {
            const frameRate = await askSequenceRate(path);
            if (generation !== importGeneration || !frameRate) return;
            const controller = new AbortController();
            sequenceImportAbort = controller;
            const media = await importImageSequence(
              { directoryPath: path, frameRate },
              controller.signal,
            );
            if (sequenceImportAbort === controller) sequenceImportAbort = undefined;
            if (generation !== importGeneration) return;
            recordImportRecent(generation, 'folders', displaySourcePath(media));
            const existing = files.find(
              (item) => pathKey(displaySourcePath(item)) === pathKey(displaySourcePath(media)),
            );
            if (existing) {
              selectedId = existing.id;
              selectedFileIds = [existing.id];
              releaseSequence(media);
            } else {
              files = [...files, media];
              addLog(`Imported ${media.name} · ${media.streams.length} streams.`);
              selectedId = media.id;
              selectedFileIds = [media.id];
            }
            continue;
          }
          const media = await probeMedia(path, importSession?.ticket ?? undefined);
          if (generation !== importGeneration) return;
          recordImportRecent(generation, 'completed', media.path);
          if (!files.some((file) => file.id === media.id)) {
            files = [...files, media];
            addLog(`Imported ${media.name} · ${media.streams.length} streams.`);
          }
          selectedId = media.id;
          selectedFileIds = [media.id];
        } catch (error) {
          if (generation !== importGeneration) return;
          const message = errorMessage(error);
          importErrors = [...importErrors, { name: importingName, message }];
          addLog(`Could not import ${importingName}: ${message}`, 'error');
        }
      }
    } finally {
      await finishImport(generation);
    }
  }

  async function importPaths(paths: string[], detectFolders = false) {
    if (!desktop || !paths.length) return;
    if (importing) {
      queueImportPaths(paths, detectFolders);
      return;
    }
    const generation = beginImport();
    try {
      if (!(await reserveImport(generation))) return;
      const selection = await chooseImportItems(paths);
      if (generation !== importGeneration || !selection?.paths.length) return;
      if (selection.replace) clearFiles();
      queueImportPaths(selection.paths, detectFolders);
      await drainImport(generation);
    } catch (error) {
      if (generation !== importGeneration) return;
      const message = errorMessage(error);
      importErrors = [...importErrors, { name: 'Media import', message }];
      addLog(message, 'error');
    } finally {
      await finishImport(generation);
    }
  }

  async function addFiles() {
    if (!desktop || importing) return;
    const generation = beginImport();
    importingName = 'Choosing files';
    try {
      if (!(await reserveImport(generation))) return;
      const paths = await chooseMediaFiles();
      if (generation !== importGeneration) return;
      const selection = await chooseImportItems(paths);
      if (generation !== importGeneration || !selection?.paths.length) return;
      if (selection.replace) clearFiles();
      queueImportPaths(selection.paths);
      await drainImport(generation);
    } catch (error) {
      if (generation !== importGeneration) return;
      const message = errorMessage(error);
      importErrors = [...importErrors, { name: 'File picker', message }];
      addLog(`Could not open file picker: ${message}`, 'error');
    } finally {
      await finishImport(generation);
    }
  }

  async function addFolder() {
    if (!desktop || importing) return;
    const generation = beginImport();
    importingName = 'Choosing folder';
    try {
      if (!(await reserveImport(generation))) return;
      const paths = await chooseMediaFolder();
      if (generation !== importGeneration || !paths.length) return;
      const selection = await chooseImportItems(paths);
      if (generation !== importGeneration || !selection?.paths.length) return;
      if (selection.replace) clearFiles();
      queueImportPaths(selection.paths, true);
      await drainImport(generation);
    } catch (error) {
      if (generation !== importGeneration) return;
      const message = errorMessage(error);
      importErrors = [...importErrors, { name: 'Folder import', message }];
      addLog(`Could not import folder: ${message}`, 'error');
    } finally {
      await finishImport(generation);
    }
  }

  async function importFolder(path: string, recursive: boolean, generation: number) {
    await scanImportFolder(path, recursive, generation);
    if (generation !== importGeneration) return;
    recordImportRecent(generation, 'folders', path);
    await drainImport(generation);
  }

  async function scanImportFolder(path: string, recursive: boolean, generation: number) {
    importingName = `Scanning ${fileName(path)}`;
    const scan = await scanMediaFolder({ path, recursive });
    if (generation !== importGeneration) return;
    importErrors = [
      ...importErrors,
      ...scan.errors.map((error) => ({
        name: error.path ?? 'Folder scan',
        message: error.message,
      })),
    ];
    importNotice = `${scan.paths.length} media files found. ${scan.skippedCount} entries skipped.${scan.truncated ? ' Scan truncated: the 500-file or 10,000-entry limit was reached. Add a smaller folder to find the remaining files.' : ''}`;
    queueImportPaths(scan.paths);
    addLog(
      `Scanned ${fileName(path)}: ${scan.paths.length} media files found, ${scan.skippedCount} entries skipped.`,
    );
  }

  async function openRecent() {
    if (!desktop || importing || !recentSelection) return;
    const path = recentSelection;
    const generation = beginImport();
    importingName = fileName(path);
    try {
      if (!(await reserveImport(generation))) return;
      const folder = await recentPathIsFolder(path);
      if (generation !== importGeneration) return;
      if (folder) {
        const existing = files.find((item) => pathKey(displaySourcePath(item)) === pathKey(path));
        if (existing) {
          selectFiles([existing.id]);
          await rememberMedia([path]);
          return;
        }
        queueImportPaths([path], true);
        await drainImport(generation);
      } else {
        const key = pathKey(path);
        const existing = files.find((file) => pathKey(displaySourcePath(file)) === key);
        if (existing) {
          selectFiles([existing.id]);
          await rememberMedia([existing.path]);
          return;
        }
        queueImportPaths([path]);
        await drainImport(generation);
      }
    } catch (error) {
      if (generation === importGeneration) {
        const message = errorMessage(error);
        importErrors = [...importErrors, { name: fileName(path), message }];
        addLog(message, 'error');
        if (errorCode(error) === 'RECENT_MEDIA_UNAVAILABLE') {
          try {
            await forgetMedia(path);
            if (recentSelection === path) recentSelection = '';
          } catch (forgetError) {
            const forgetMessage = errorMessage(forgetError);
            preferences.error = forgetMessage;
            addLog(`Could not remove unavailable recent media: ${forgetMessage}`, 'error');
          }
        }
      }
    } finally {
      await finishImport(generation);
    }
  }

  function removeFile(id: string) {
    const file = files.find((entry) => entry.id === id);
    files = files.filter((entry) => entry.id !== id);
    const { [id]: _removedBatchJob, ...remainingBatchJobs } = batchJobIds;
    batchJobIds = remainingBatchJobs;
    const { [id]: _removedUtilityStatus, ...remainingUtilityStatuses } = utilityBatchStatuses;
    utilityBatchStatuses = remainingUtilityStatuses;
    selectedFileIds = selectedFileIds.filter((entry) => entry !== id);
    trackSelection = trackSelection.filter((track) => track.sourceId !== id);
    if (mainSourceId === id) {
      mainSourceId = null;
      if (fileMode === 'mux' && files.length) {
        mainSourceId = files[0].id;
        if (!trackSelection.some((row) => row.sourceId === mainSourceId))
          loadTracks([mainSourceId], false);
      }
    }
    if (!files.length) {
      fileQuery = '';
      noDefaultSubtitles = false;
    }
    if (selectedId === id) selectedId = files[0]?.id ?? null;
    if (file) {
      addLog(`Removed ${file.name} from the workspace.`);
      if (file.source?.kind === 'imageSequence') releaseSequence(file);
    }
  }

  function clearFiles() {
    const removed = files;
    fileQuery = '';
    files = [];
    for (const file of removed) if (file.source?.kind === 'imageSequence') releaseSequence(file);
    batchJobIds = {};
    utilityBatchStatuses = {};
    batchStatusOwner = null;
    batchStatusFiles.clear();
    selectedId = null;
    selectedFileIds = [];
    mainSourceId = null;
    trackSelection = [];
    noDefaultSubtitles = false;
    addLog('Source list cleared.');
  }

  function releaseSequence(file: MediaFile) {
    if (
      file.source?.kind !== 'imageSequence' ||
      files.some((loaded) => pathKey(loaded.path) === pathKey(file.path))
    )
      return;
    void releaseImageSequence({
      inputPath: file.path,
      sequenceFingerprint: file.source.sequenceFingerprint,
    }).catch(() =>
      addLog(
        `${file.name}: the managed sequence is retained because it is still in use or cannot be released.`,
        'error',
      ),
    );
  }

  function loadSample(nextView: View = 'files') {
    fileQuery = '';
    if (files.some((file) => file.id === sampleId)) {
      selectedId = sampleId;
      view = nextView;
      return;
    }
    const baseStream = {
      width: null,
      height: null,
      frameRate: null,
      sampleRate: null,
      channels: null,
      language: null,
      title: null,
    };
    const sample: MediaFile = {
      id: sampleId,
      path: 'Synthetic sample · no file on disk',
      name: 'Coastal walk.mkv',
      sizeBytes: '1247805440',
      durationSeconds: 754.8,
      format: 'matroska,webm',
      streams: [
        {
          ...baseStream,
          index: 0,
          kind: 'video',
          codec: 'h264',
          width: 3840,
          height: 2160,
          frameRate: '24000/1001',
        },
        {
          ...baseStream,
          index: 1,
          kind: 'audio',
          codec: 'aac',
          sampleRate: 48000,
          channels: 2,
          language: 'eng',
          title: 'Main audio',
        },
        {
          ...baseStream,
          index: 2,
          kind: 'subtitle',
          codec: 'subrip',
          language: 'eng',
          title: 'English subtitles',
        },
      ],
    };
    files = [...files, sample];
    selectedId = sample.id;
    view = nextView;
    addLog('Loaded synthetic interface sample. No media file was read.');
  }

  let shiftHeld = false;
  function handleKeydown(event: KeyboardEvent) {
    shiftHeld = event.shiftKey;
    if (desktop && (event.ctrlKey || event.metaKey) && event.key.toLowerCase() === 'o') {
      event.preventDefault();
      void addFiles();
    }
  }

  onMount(() => {
    addLog(
      desktop
        ? 'jesses desktop workspace ready.'
        : 'Browser preview ready. Local file access requires the desktop app.',
    );
    try {
      const savedView = localStorage.getItem('jesses.active-view');
      if (
        savedView &&
        ['files', 'tracks', 'convert', 'av1an', 'tools', 'utilities'].includes(savedView)
      )
        view = savedView as View;
      else if (savedView === 'batch') view = 'convert';
      else if (savedView === 'remux') view = 'tracks';
      const savedHeight = Number(localStorage.getItem('jesses.log-panel-height'));
      if (Number.isFinite(savedHeight) && savedHeight >= 90 && savedHeight <= 400)
        logHeight = savedHeight;
      logUserAdjusted = localStorage.getItem('jesses.log-height-user-adjusted') === 'true';
    } catch {
      /* Storage may be unavailable in a restricted webview. */
    }
    uiStateReady = true;
    let disposed = false;
    let unlisten: (() => void) | undefined;
    let stopWindowTracking: (() => void) | undefined;
    let completionTimer: ReturnType<typeof setInterval> | undefined;
    if (desktop) {
      let readingCompletion = false;
      async function refreshCompletion() {
        if (readingCompletion || updatingShutdown) return;
        const generation = completionGeneration;
        readingCompletion = true;
        try {
          const status = await getCompletionStatus();
          if (!disposed && generation === completionGeneration) completionStatus = status;
        } catch {
          // Keep the last confirmed state until the next poll.
        } finally {
          readingCompletion = false;
        }
      }
      void refreshCompletion();
      completionTimer = setInterval(refreshCompletion, 1000);
      void restoreAndTrackWindow()
        .then((stop) => {
          if (disposed) stop();
          else stopWindowTracking = stop;
        })
        .catch(() => {
          /* Window state is optional. */
        });
      void loadPreferences();
      void refreshTools();
      void connectJobs();
      void subscribeDrop((paths) => {
        void importPaths(paths, true);
      })
        .then((stop) => {
          if (disposed) stop();
          else unlisten = stop;
        })
        .catch((error) => {
          addLog(`File drop listener unavailable: ${errorMessage(error)}`, 'error');
        });
    }
    return () => {
      disposed = true;
      clearInterval(completionTimer);
      ++importGeneration;
      const importTicket = importSession?.ticket;
      importSession = null;
      void releaseImport(importTicket);
      unlisten?.();
      stopWindowTracking?.();
      jobsDisposed = true;
      stopJobSubscription?.();
    };
  });

  function toggleLog() {
    logOpen = !logOpen;
    try {
      localStorage.setItem('jesses.log-panel-open', String(logOpen));
    } catch {
      /* The panel still works without persistence. */
    }
  }
  async function exportActivityLog() {
    if (logActionBusy || !logs.length) return;
    logActionBusy = true;
    try {
      const contents =
        logs
          .map((entry) => `${entry.time} [${entry.level.toUpperCase()}] ${entry.message}`)
          .join('\n') + '\n';
      const path = await saveActivityLog(contents);
      if (path) addLog(`Activity log saved to ${path}.`);
    } catch (error) {
      addLog(`Could not save activity log: ${errorMessage(error)}`, 'error');
    } finally {
      logActionBusy = false;
    }
  }
  async function copyActivityLog() {
    if (!logs.length) return;
    try {
      const contents =
        logs
          .map((entry) => `${entry.time} [${entry.level.toUpperCase()}] ${entry.message}`)
          .join('\n') + '\n';
      let copied = false;
      try {
        if (navigator.clipboard) {
          await navigator.clipboard.writeText(contents);
          copied = true;
        }
      } catch {
        /* Try the webview fallback below. */
      }
      if (!copied) {
        const previousFocus = document.activeElement as HTMLElement | null;
        const text = document.createElement('textarea');
        text.value = contents;
        text.style.position = 'fixed';
        text.style.opacity = '0';
        document.body.appendChild(text);
        text.select();
        try {
          copied = document.execCommand('copy');
        } finally {
          text.remove();
          previousFocus?.focus();
        }
      }
      if (!copied) throw new Error('Clipboard access was denied.');
      addLog('Activity log copied.');
    } catch (error) {
      addLog(`Could not copy activity log: ${errorMessage(error)}`, 'error');
    }
  }
  async function openLogFolder() {
    if (logActionBusy) return;
    logActionBusy = true;
    try {
      const folder = (await getStorageLocations()).find(([label]) => label === 'Job logs')?.[1];
      if (!folder) throw new Error('Job log folder is unavailable.');
      await openDirectory(folder);
    } catch (error) {
      addLog(`Could not open the job log folder: ${errorMessage(error)}`, 'error');
    } finally {
      logActionBusy = false;
    }
  }
</script>

<svelte:window
  onkeydown={handleKeydown}
  onkeyup={(event) => {
    shiftHeld = event.shiftKey;
  }}
  onblur={() => {
    shiftHeld = false;
  }}
  ondragover={(event) => event.preventDefault()}
  ondrop={(event) => event.preventDefault()}
/>

<div class="app-shell" class:compact-utilities={view === 'utilities' && !logUserAdjusted}>
  <header class="app-header" data-tauri-drag-region={desktop ? '' : undefined}>
    <div class="brand" data-tauri-drag-region={desktop ? '' : undefined}>
      <img
        src="/app-icon.png"
        width="20"
        height="20"
        alt=""
        draggable="false"
        data-tauri-drag-region={desktop ? '' : undefined}
      />
      <div class="brand-wordmark" data-tauri-drag-region={desktop ? '' : undefined}>jesses</div>
      <span class="version" data-tauri-drag-region={desktop ? '' : undefined}>v{appVersion}</span>
    </div>
    {#if desktop}
      <WindowControls onerror={(message) => addLog(message, 'error')} />
    {/if}
  </header>
  <div class="work-area">
    <nav class="workspace-nav" aria-label="Workspace workflows">
      {#each [{ id: 'files', label: 'File List' }, { id: 'tracks', label: 'Track List' }, { id: 'av1an', label: 'AV1AN' }, { id: 'convert', label: 'Quick Convert' }, { id: 'utilities', label: 'Utilities' }, { id: 'tools', label: 'Settings' }] as tab}
        <button
          type="button"
          class:active={view === tab.id}
          aria-current={view === tab.id ? 'page' : undefined}
          onclick={() => {
            view = tab.id as View;
            if (tab.id === 'convert' || tab.id === 'av1an') lastTaskTab = tab.id;
            else if (tab.id === 'utilities' && workspaceActions.utilities?.label !== 'Run')
              lastTaskTab = 'utilities';
          }}>{tab.label}</button
        >
      {/each}
    </nav>
    <main>
      {#if view === 'files'}
        <SourceFileList
          {files}
          selectedIds={selectedFileIds}
          primary={selectedFile}
          mode={fileMode}
          {batchNameTemplate}
          {batchExample}
          statuses={batchStatuses}
          recentPaths={preferences.value.recentPaths}
          {importing}
          {desktop}
          onselect={selectFiles}
          onmode={changeFileMode}
          onname={(value) => (batchNameTemplate = value)}
          onadd={addFiles}
          onfolder={addFolder}
          onrecent={(path) => {
            recentSelection = path;
            void openRecent();
          }}
          onload={() => loadTracks()}
          onremove={removeSelectedFiles}
          onmove={moveSelectedFile}
          onsort={sortFiles}
        />
      {:else if view === 'tracks'}
        <SourceTrackList
          {files}
          tracks={trackSelection}
          mainFile={mainSource}
          mode={fileMode}
          {noDefaultSubtitles}
          onchange={(next, disableDefaultSubtitles) => {
            trackSelection = next;
            if (disableDefaultSubtitles !== undefined) noDefaultSubtitles = disableDefaultSubtitles;
          }}
          onextract={extractSelectedAttachment}
        />
      {:else if view === 'tools'}
        <SourceSettings
          onreset={() => {
            workspaceActions.convert?.reset?.();
            workspaceActions.av1an?.reset?.();
          }}
        />
      {/if}
      {#if (view === 'convert' || view === 'av1an') && jobsError}<p role="alert">
          {jobsError}
          <button type="button" onclick={connectJobs} disabled={jobsConnecting}>Reconnect</button>
        </p>{/if}
      <div hidden={view !== 'convert'}>
        <QuickConvert
          {files}
          file={taskSource}
          {tools}
          {toolsRevision}
          {toolsLoading}
          {jobs}
          tracks={trackSelection}
          {fileMode}
          {batchNameTemplate}
          sourcePreferences={preferences.value.general}
          onlog={(message) => addLog(message)}
          registerAction={registerWorkspaceAction}
          onbatchqueue={queueBatch}
          onbatchstatus={reportBatchStatuses}
          onreviewjob={acceptReviewedJob}
          ontracklist={() => {
            lastActionStatus = 'Canceled.';
            view = 'tracks';
          }}
          isShiftHeld={() => shiftHeld}
          connected={jobsConnected}
          onfiles={() => (view = 'files')}
          onstart={submitEncode}
          onqueue={queueEncode}
        />
      </div>
      <div hidden={view !== 'av1an'}>
        <Av1an
          {files}
          file={taskSource}
          {tools}
          {toolsRevision}
          {toolsLoading}
          {jobs}
          tracks={trackSelection}
          {fileMode}
          {batchNameTemplate}
          sourcePreferences={preferences.value.general}
          onlog={(message) => addLog(message)}
          registerAction={registerWorkspaceAction}
          onbatchqueue={queueBatch}
          onbatchstatus={reportBatchStatuses}
          onreviewjob={acceptReviewedJob}
          isShiftHeld={() => shiftHeld}
          onrecoverysource={activateRecoverySource}
          onrecoveryjobs={acceptRecoveryJobs}
          connected={jobsConnected}
          onfiles={() => (view = 'files')}
          onstart={submitEncode}
          onqueue={queueEncode}
        />
      </div>
      <div hidden={view !== 'utilities'}>
        <SourceUtilities
          {files}
          file={taskSource}
          tracks={trackSelection}
          {fileMode}
          registerAction={registerWorkspaceAction}
          onbatchstatus={(statuses, runId, begin) => reportBatchStatuses(runId, statuses, begin)}
          onlog={(message, error) => addLog(message, error ? 'error' : 'info')}
        />
      </div>
    </main>
  </div>
  <div
    class="log-splitter"
    role="slider"
    aria-label="Resize log"
    aria-orientation="horizontal"
    aria-valuenow={logHeight}
    aria-valuemin="90"
    aria-valuemax="400"
    tabindex="0"
    onpointerdown={resizeLog}
    onkeydown={(event) => {
      if (event.key === 'ArrowUp') {
        event.preventDefault();
        logUserAdjusted = true;
        try {
          localStorage.setItem('jesses.log-height-user-adjusted', 'true');
        } catch {
          /* Optional UI state. */
        }
        logHeight = Math.min(400, logHeight + 10);
      } else if (event.key === 'ArrowDown') {
        event.preventDefault();
        logUserAdjusted = true;
        try {
          localStorage.setItem('jesses.log-height-user-adjusted', 'true');
        } catch {
          /* Optional UI state. */
        }
        logHeight = Math.max(90, logHeight - 10);
      }
    }}
  ></div>
  <section class="log-panel expanded" aria-label="Activity log">
    <div class="log-bar">
      <strong>LOG</strong>
      <div class="log-bar-right">
        <button type="button" onclick={copyActivityLog} disabled={!logs.length}>Copy</button><button
          type="button"
          onclick={exportActivityLog}
          disabled={!logs.length || logActionBusy}>Save…</button
        ><button type="button" onclick={openLogFolder} disabled={logActionBusy}>Log Folder</button
        ><button type="button" onclick={() => (logs = [])} disabled={!logs.length}>Clear</button>
      </div>
    </div>
    <div
      id="activity-log"
      class="log-entries"
      role="log"
      aria-label="Recent activity"
      aria-live="polite"
      style:height={`${logHeight}px`}
    >
      {#each logs as entry (entry.id)}<div
          class="log-entry"
          class:log-error={entry.level === 'error'}
        >
          <span>{entry.message}</span>
        </div>{/each}
    </div>
  </section>
  {#if completionStatus && completionStatus.options.finishAction !== 'none'}
    <aside class="finish-action">
      <span role="status" aria-label="Finish action">
        {completionStatus.options.finishAction === 'shutdown'
          ? 'Computer shutdown'
          : 'Close jesses'}
        {completionStatus.secondsRemaining === null
          ? ` armed for ${completionStatus.armedJobs} job${completionStatus.armedJobs === 1 ? '' : 's'}.`
          : ` (${completionStatus.secondsRemaining} seconds remaining).`}
      </span>
      <button
        type="button"
        onclick={() => updateCompletion(cancelFinishAction)}
        disabled={updatingShutdown}
      >
        Cancel finish action
      </button>
    </aside>
  {/if}
  <footer class="status-bar">
    <div class="current-action">
      <div aria-label="Current action" title={currentActionTitle}>
        {#if batchFileText && liveProgress}<span>{batchFileText}</span>{/if}
        <span>{currentActionText}</span>
      </div>
      {#if statusProgress.value === null}<progress
          aria-label="Task progress"
          aria-valuetext={currentActionTitle}
        ></progress>{:else}<progress
          aria-label="Task progress"
          aria-valuetext={currentActionTitle}
          max={statusProgress.max}
          value={statusProgress.value}
        ></progress>{/if}
    </div>
    <label
      ><input
        type="checkbox"
        checked={shutdownWhenDone}
        disabled={!desktop || updatingShutdown}
        onchange={(event) => {
          const input = event.currentTarget;
          void setShutdown(input.checked).then(() => {
            input.checked = shutdownWhenDone;
          });
        }}
      /> Shutdown when done</label
    >
    {#if displayAction?.active}<button
        type="button"
        disabled={!displayAction.pause}
        onclick={() => displayAction?.pause?.()}>{displayAction.paused ? 'Resume' : 'Pause'}</button
      ><button type="button" disabled={!displayAction.stop} onclick={() => displayAction?.stop?.()}
        >Stop</button
      >
    {:else if currentJob && ['preparing', 'running', 'paused', 'finalizing'].includes(currentJob.state)}<button
        type="button"
        onclick={() => pauseLiveJob(currentJob.id, currentJob.state !== 'paused')}
        >{currentJob.state === 'paused' ? 'Resume' : 'Pause'}</button
      ><button type="button" onclick={stopQueue}>Stop</button>{/if}
    {#if runBlockedReason}<span
        id="run-blocked-reason"
        class="run-blocked"
        role="alert"
        title={runBlockedReason}>{runBlockedReason}</span
      >{/if}
    <button
      type="button"
      class="run-button"
      aria-describedby={runBlockedReason ? 'run-blocked-reason' : undefined}
      disabled={!action?.canRun || !!runningAction || !desktop}
      onclick={(event) =>
        action?.run({ editCommand: event.shiftKey, isShiftHeld: () => shiftHeld })}
      >{action?.label ?? 'Run'}</button
    >
  </footer>
  {#if shutdownError || completionStatus?.error}<p role="alert">
      {shutdownError || completionStatus?.error}
    </p>{/if}
</div>

<SourceDialog
  title={`Import ${importChoices?.length ?? 0} File${importChoices?.length === 1 ? '' : 's'}`}
  open={importChoices !== null}
  onclose={() => finishImportDialog()}
  width={640}
>
  <p>Uncheck any files you don't want to import.</p>
  <div class="import-choices">
    {#each importChoices ?? [] as item}
      <label><input type="checkbox" bind:checked={item.checked} /> {fileName(item.path)}</label>
    {/each}
  </div>
  <div class="import-actions">
    <button onclick={() => finishImportDialog(true)}>Replace file list</button><button
      onclick={() => finishImportDialog(false)}>Add to file list</button
    >
  </div>
</SourceDialog>
<SourceDialog
  title="Enter Frame Rate"
  open={sequencePrompt !== null}
  onclose={() => finishSequenceRate(true)}
  width={440}
>
  {#if sequencePrompt}
    <p>
      Please enter a frame rate to use for the image sequence '{fileName(sequencePrompt.path).slice(
        0,
        80,
      )}'.
    </p>
    <input
      aria-label="Frame rate"
      bind:value={sequencePrompt.text}
      onkeydown={(event) => {
        if (event.key === 'Enter') finishSequenceRate();
      }}
    />
    {#if sequencePrompt.error}<p role="alert">{sequencePrompt.error}</p>{/if}
    <div class="import-actions"><button onclick={() => finishSequenceRate()}>OK</button></div>
  {/if}
</SourceDialog>

<style>
  .import-choices {
    min-height: 220px;
    max-height: 55vh;
    overflow: auto;
    display: grid;
    align-content: start;
    gap: 6px;
  }
  .import-actions {
    display: flex;
    justify-content: end;
    gap: 8px;
    margin-top: 12px;
  }
  .app-shell {
    height: 100vh;
    min-height: 680px;
    padding: 0 10px 10px;
    gap: 8px;
    background: var(--background);
    color: var(--foreground);
  }
  .app-header {
    height: 32px;
    min-height: 32px;
    margin: 0 -10px -4px;
    padding: 0 0 0 10px;
    display: flex;
    align-items: center;
    justify-content: space-between;
    border: 0;
    background: transparent;
  }
  .app-header[data-tauri-drag-region] {
    user-select: none;
    -webkit-app-region: drag;
  }
  .brand {
    align-items: center;
    gap: 8px;
  }
  .brand img {
    width: 20px;
    height: 20px;
    border-radius: var(--radius);
  }
  .brand-wordmark {
    color: var(--foreground);
    font-size: 21px;
    font-weight: 650;
    letter-spacing: -0.9px;
  }
  .version {
    color: var(--muted-foreground);
    font-size: 10px;
  }
  .work-area {
    min-height: 260px;
    flex: 1;
    display: flex;
    flex-direction: column;
    padding: 8px;
    border-radius: var(--radius);
    background: var(--card);
    overflow: hidden;
  }
  .workspace-nav {
    min-height: 35px;
    gap: 20px;
    align-items: flex-start;
    padding: 0 8px;
    overflow-x: auto;
  }
  .workspace-nav button {
    height: 29px;
    padding: 0;
    border: 0;
    border-bottom: 2px solid transparent;
    border-radius: 0;
    background: transparent;
    color: var(--muted-foreground);
    font-size: 13px;
  }
  .workspace-nav button:hover {
    background: transparent;
    color: var(--foreground);
  }
  .workspace-nav button.active {
    color: var(--foreground);
    border-color: var(--primary);
    background: transparent;
  }
  main {
    display: flex;
    flex: 1;
    min-height: 0;
    overflow: auto;
    padding: 0 4px;
  }
  main > div:not([hidden]) {
    flex: 1;
    min-width: 0;
    min-height: 0;
    display: flex;
    flex-direction: column;
    overflow: hidden;
  }
  .log-panel {
    flex: 0 0 auto;
    padding: 0;
    border: 0;
    background: transparent;
  }
  .log-splitter {
    height: 5px;
    flex: 0 0 5px;
    border-radius: var(--radius);
    background: var(--rule);
    cursor: ns-resize;
    touch-action: none;
  }
  .log-bar {
    min-height: 30px;
    display: flex;
    justify-content: space-between;
    align-items: center;
    padding: 0;
    border: 0;
  }
  .log-bar strong {
    font-size: 10px;
    letter-spacing: 0.08em;
    color: var(--muted-foreground);
  }
  .log-bar-right {
    display: flex;
    gap: 5px;
  }
  .log-bar button,
  .status-bar button,
  .finish-action button {
    padding: 0.38rem 0.7rem;
    border: 0;
    border-radius: var(--radius);
    background: var(--secondary);
    color: var(--secondary-foreground);
    font-size: 12px;
  }
  .log-bar button:disabled,
  .status-bar button:disabled,
  .finish-action button:disabled {
    opacity: 0.5;
  }
  .log-entries {
    min-height: 90px;
    max-height: 400px;
    overflow: auto;
    padding: 6px;
    background: var(--background);
    color: var(--foreground);
    border: 0;
    border-radius: var(--radius);
    font:
      11px Consolas,
      monospace;
  }
  @media (max-width: 1000px) and (max-height: 800px) {
    .compact-utilities .log-entries {
      min-height: 36px;
      height: 36px !important;
    }
  }
  .log-entry {
    display: block;
    padding: 0;
    line-height: 1.35;
  }
  .log-entry.log-error {
    color: var(--destructive);
  }
  .status-bar {
    min-height: 32px;
    padding: 0;
    display: flex;
    gap: 10px;
    align-items: center;
    background: transparent;
    border: 0;
    color: var(--foreground);
    font-size: 13px;
  }
  .finish-action {
    display: flex;
    align-items: center;
    justify-content: space-between;
    gap: 10px;
    padding: 6px 8px;
    border: 1px solid var(--primary);
    font-size: 13px;
  }
  .current-action {
    flex: 1;
    min-width: 0;
    display: flex;
    flex-direction: column;
    gap: 3px;
  }
  .current-action span {
    display: block;
    min-height: 12px;
    font-size: 11px;
    line-height: 12px;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  .status-bar progress {
    width: 100%;
    height: 8px;
    accent-color: var(--primary);
  }
  /* A step without a measurable fraction (preparation, final checks) must
     still look busy; the native indeterminate bar can render as a bare track. */
  .status-bar progress:indeterminate {
    appearance: none;
    border: 0;
    background:
      linear-gradient(90deg, transparent, var(--primary), transparent) -40% 0 / 40% 100% no-repeat,
      var(--rule);
    animation: status-progress-sweep 1.4s ease-in-out infinite;
  }
  .status-bar progress:indeterminate::-webkit-progress-bar {
    background: transparent;
  }
  @keyframes status-progress-sweep {
    to {
      background-position:
        140% 0,
        0 0;
    }
  }
  @media (prefers-reduced-motion: reduce) {
    .status-bar progress:indeterminate {
      animation: none;
      background: repeating-linear-gradient(135deg, var(--primary) 0 6px, var(--rule) 6px 12px);
    }
  }
  .status-bar label {
    white-space: nowrap;
    display: flex;
    gap: 0.4rem;
    align-items: center;
  }
  .status-bar .run-button {
    min-width: 120px;
    color: var(--primary-foreground);
    background: var(--primary);
    font-weight: 600;
  }
  .status-bar .run-blocked {
    max-width: 46ch;
    color: var(--destructive);
    font-size: 11px;
    font-weight: 600;
    line-height: 1.3;
    overflow: hidden;
    display: -webkit-box;
    -webkit-line-clamp: 2;
    line-clamp: 2;
    -webkit-box-orient: vertical;
  }
  /* Paper shell and menu selection stay compact at desktop and laptop widths. */
  .app-shell {
    background-color: #c9c1a9;
    background-image:
      repeating-linear-gradient(0deg, transparent 0 31px, #7278660b 31px 32px),
      repeating-linear-gradient(90deg, transparent 0 31px, #7278660b 31px 32px);
  }
  .brand-wordmark {
    letter-spacing: 0.11em;
    text-transform: uppercase;
    font-size: 16px;
    font-family: 'Century Gothic', 'Avenir Next', 'DejaVu Sans', sans-serif;
  }
  .version {
    letter-spacing: 0.06em;
  }
  .work-area {
    border: 1px solid var(--border);
    border-radius: 0;
    background: var(--card);
  }
  .workspace-nav {
    gap: 4px;
  }
  .workspace-nav button {
    position: relative;
    padding: 0 9px 0 16px;
    border: 0;
    letter-spacing: 0.045em;
    font-size: 12px;
  }
  .workspace-nav button::before {
    content: '';
    position: absolute;
    left: 6px;
    width: 4px;
    height: 4px;
    background: var(--border);
  }
  .workspace-nav button:hover {
    background: var(--secondary);
  }
  .workspace-nav button.active {
    background: var(--primary);
    color: var(--primary-foreground);
  }
  .workspace-nav button.active::before {
    background: var(--primary-foreground);
  }
  .log-splitter {
    border-radius: 0;
    background: repeating-linear-gradient(90deg, var(--border) 0 36px, transparent 36px 39px);
  }
  .log-bar strong {
    letter-spacing: 0.13em;
  }
  .log-entries {
    border: 1px solid var(--border);
    border-radius: 0;
    background: #d6d0b8;
  }
  .log-bar button,
  .status-bar button {
    border-radius: 0;
  }
  .status-bar .run-button {
    letter-spacing: 0.035em;
  }
</style>
