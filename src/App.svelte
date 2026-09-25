<script lang="ts">
  import { onMount } from 'svelte';
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
    HardDrive,
    Layers,
    LoaderCircle,
    Monitor,
    Plus,
    Search,
    SlidersHorizontal,
    Terminal,
    Trash2,
    Upload,
    Wrench,
    X,
  } from '@lucide/svelte';
  import { Button } from '$lib/components/ui/button';
  import FileInspector from '$lib/features/files/FileInspector.svelte';
  import QuickConvert from '$lib/features/convert/QuickConvert.svelte';
  import Av1an from '$lib/features/av1an/Av1an.svelte';
  import BatchEncode from '$lib/features/batch/BatchEncode.svelte';
  import Remux from '$lib/features/remux/Remux.svelte';
  import JobStatus from '$lib/components/shared/JobStatus.svelte';
  import { terminalJob } from '$lib/components/shared/job-state';
  import ToolsPanel from '$lib/features/tools/ToolsPanel.svelte';
  import CompletionPanel from '$lib/features/tools/CompletionPanel.svelte';
  import Utilities from '$lib/features/utilities/Utilities.svelte';
  import SavedJobs from '$lib/features/utilities/SavedJobs.svelte';
  import { encoderChoices, encoderOptions } from '$lib/components/shared/encoder-options';
  import {
    chooseMediaFiles,
    chooseMediaFolder,
    scanMediaFolder,
    getCapabilities,
    isDesktop,
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
  } from '$lib/ipc/client';
  import type {
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

  type View = 'files' | 'convert' | 'av1an' | 'batch' | 'remux' | 'tools' | 'utilities';
  type LogEntry = { id: number; time: string; level: 'info' | 'error'; message: string };
  type ImportEntry = { path: string; detectFolder: boolean };
  type ImportSession = { generation: number; folders: string[]; completed: string[] };
  const desktop = isDesktop();
  const sampleId = 'jesses-synthetic-preview';
  let view = $state<View>('files');
  let files = $state<MediaFile[]>([]);
  let selectedId = $state<string | null>(null);
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
  async function submitEncode(request: EncodeRequest) {
    const job = await startEncode(request);
    if (!jobs.some((entry) => entry.id === job.id)) jobs = [job, ...jobs];
    addLog(`${encoderOptions(request.settings.encoder).name} encode job submitted.`);
  }
  async function queueEncode(request: EncodeRequest) {
    const job = await enqueueEncode(request);
    if (!jobs.some((entry) => entry.id === job.id)) jobs = [job, ...jobs];
    addLog(`${encoderOptions(request.settings.encoder).name} encode added to the queue.`);
  }
  async function queueBatch(requests: EncodeRequest[]) {
    const submitted = await enqueueEncodeBatch(requests);
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
    const loaded = new Set(files.map((file) => pathKey(file.path)));
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
    importSession = { generation, folders: [], completed: [] };
    importQueue = [];
    activePath = null;
    importing = true;
    importNotice = null;
    importErrors = [];
    view = 'files';
    return generation;
  }

  function finishImport(generation: number) {
    if (generation !== importGeneration) return;
    if (
      importSession?.generation === generation &&
      importSession.folders.length === 0 &&
      importSession.completed.length === 0
    )
      importSession = null;
    activePath = null;
    importing = false;
    importingName = '';
  }

  function stopImport() {
    const recent = takeImportRecents(importGeneration);
    ++importGeneration;
    importQueue = [];
    activePath = null;
    importing = false;
    importingName = '';
    importNotice =
      'Stopped importing. Completed files are kept. The current scan or probe may finish in the background.';
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
            await scanImportFolder(path, recursiveImport, generation);
            if (generation !== importGeneration) return;
            recordImportRecent(generation, 'folders', path);
            continue;
          }
          const media = await probeMedia(path);
          if (generation !== importGeneration) return;
          recordImportRecent(generation, 'completed', media.path);
          if (!files.some((file) => file.id === media.id)) {
            files = [...files, media];
            addLog(`Imported ${media.name} · ${media.streams.length} streams.`);
          }
          selectedId = media.id;
        } catch (error) {
          if (generation !== importGeneration) return;
          const message = errorMessage(error);
          importErrors = [...importErrors, { name: importingName, message }];
          addLog(`Could not import ${importingName}: ${message}`, 'error');
        }
      }
    } finally {
      const recent = takeImportRecents(generation);
      finishImport(generation);
      if (recent.length) await rememberMedia(recent);
    }
  }

  async function importPaths(paths: string[], detectFolders = false) {
    if (!desktop || !paths.length) return;
    if (importing) {
      queueImportPaths(paths, detectFolders);
      return;
    }
    const generation = beginImport();
    queueImportPaths(paths, detectFolders);
    await drainImport(generation);
  }

  async function addFiles() {
    if (!desktop || importing) return;
    const generation = beginImport();
    importingName = 'Choosing files';
    try {
      const paths = await chooseMediaFiles();
      if (generation !== importGeneration) return;
      queueImportPaths(paths);
      await drainImport(generation);
    } catch (error) {
      if (generation !== importGeneration) return;
      const message = errorMessage(error);
      importErrors = [...importErrors, { name: 'File picker', message }];
      addLog(`Could not open file picker: ${message}`, 'error');
    } finally {
      finishImport(generation);
    }
  }

  async function addFolder() {
    if (!desktop || importing) return;
    const generation = beginImport();
    const recursive = recursiveImport;
    importingName = 'Choosing folder';
    try {
      const path = await chooseMediaFolder();
      if (generation !== importGeneration || !path) return;
      await importFolder(path, recursive, generation);
    } catch (error) {
      if (generation !== importGeneration) return;
      const message = errorMessage(error);
      importErrors = [...importErrors, { name: 'Folder import', message }];
      addLog(`Could not import folder: ${message}`, 'error');
    } finally {
      finishImport(generation);
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
      const folder = await recentPathIsFolder(path);
      if (generation !== importGeneration) return;
      if (folder) await importFolder(path, recursiveImport, generation);
      else {
        const key = pathKey(path);
        const existing = files.find((file) => pathKey(file.path) === key);
        if (existing) {
          selectedId = existing.id;
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
      finishImport(generation);
    }
  }

  function removeFile(id: string) {
    const file = files.find((entry) => entry.id === id);
    files = files.filter((entry) => entry.id !== id);
    if (!files.length) fileQuery = '';
    if (selectedId === id) selectedId = files[0]?.id ?? null;
    if (file) addLog(`Removed ${file.name} from the workspace.`);
  }

  function clearFiles() {
    fileQuery = '';
    files = [];
    selectedId = null;
    addLog('Source list cleared.');
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

  function handleKeydown(event: KeyboardEvent) {
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
      logOpen = localStorage.getItem('jesses.log-panel-open') === 'true';
    } catch {
      /* Storage may be unavailable in a restricted webview. */
    }
    let disposed = false;
    let unlisten: (() => void) | undefined;
    if (desktop) {
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
      ++importGeneration;
      unlisten?.();
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
</script>

<svelte:window
  onkeydown={handleKeydown}
  ondragover={(event) => event.preventDefault()}
  ondrop={(event) => event.preventDefault()}
/>

<div class="app-shell">
  <header class="app-header">
    <div class="brand">
      <img src="/app-icon.png" width="36" height="36" alt="" />
      <div class="brand-wordmark">jesses</div>
      <span class="brand-divider"></span><span class="app-description">Media workspace</span>
    </div>
    <button
      type="button"
      class="environment-status"
      onclick={() => (view = 'tools')}
      title="View tools and environment"
    >
      {#if !desktop}<Monitor size={14} aria-hidden="true" /><span>Browser preview</span>
      {:else if toolsLoading}<LoaderCircle size={14} class="spinning" aria-hidden="true" /><span
          >Checking tools</span
        >
      {:else if toolsError}<CircleAlert size={14} aria-hidden="true" /><span>Tool check failed</span
        >
      {:else if availableTools === tools.length && tools.length > 0}<Check
          size={14}
          aria-hidden="true"
        /><span>Tools available</span>
      {:else}<CircleAlert size={14} aria-hidden="true" /><span
          >{availableTools} / {tools.length} tools available</span
        >{/if}
      <ArrowRight size={12} aria-hidden="true" />
    </button>
  </header>

  <div class="workspace-nav">
    <nav aria-label="Workspace workflows">
      <button
        type="button"
        class:active={view === 'files'}
        aria-current={view === 'files' ? 'page' : undefined}
        title="Start here: add and inspect source media"
        onclick={() => (view = 'files')}
        ><Files size={16} aria-hidden="true" />Files<span class="nav-count"
          >{files.length.toString().padStart(2, '0')}</span
        ></button
      >
      <button
        type="button"
        class:active={view === 'convert'}
        aria-current={view === 'convert' ? 'page' : undefined}
        title="Encode one source with guided settings"
        onclick={() => (view = 'convert')}
        ><SlidersHorizontal size={16} aria-hidden="true" />Quick Convert</button
      >
      <button
        type="button"
        class:active={view === 'av1an'}
        aria-current={view === 'av1an' ? 'page' : undefined}
        title="Scene-based AV1 encoding with parallel chunks"
        onclick={() => (view = 'av1an')}><Clapperboard size={16} aria-hidden="true" />av1an</button
      >
      <button
        type="button"
        class:active={view === 'batch'}
        aria-current={view === 'batch' ? 'page' : undefined}
        title="Apply one encoding recipe to multiple sources"
        onclick={() => (view = 'batch')}><Layers size={16} aria-hidden="true" />Batch encode</button
      >
      <button
        type="button"
        class:active={view === 'remux'}
        aria-current={view === 'remux' ? 'page' : undefined}
        title="Repackage streams without re-encoding"
        onclick={() => (view = 'remux')}><FolderOpen size={16} aria-hidden="true" />Remux</button
      >
      <button
        type="button"
        class:active={view === 'tools'}
        aria-current={view === 'tools' ? 'page' : undefined}
        title="Check installed tools and change app defaults"
        onclick={() => (view = 'tools')}
        ><Wrench size={16} aria-hidden="true" />Tools & settings</button
      >
      <button
        type="button"
        class:active={view === 'utilities'}
        aria-current={view === 'utilities' ? 'page' : undefined}
        title="Run focused media tasks such as cuts, joins, OCR, and analysis"
        onclick={() => (view = 'utilities')}
        ><FlaskConical size={16} aria-hidden="true" />Utilities</button
      >
    </nav>
    <span class="workspace-label">LOCAL WORKSPACE<span class="square-mark"></span></span>
  </div>

  <main>
    {#if !desktop}
      <div class="preview-notice">
        <Monitor size={15} aria-hidden="true" />
        <p>
          <strong>Browser preview.</strong> Open the desktop app to add local media and detect tools.
        </p>
        <button type="button" class="text-button" onclick={() => loadSample(view)}
          ><FlaskConical size={13} aria-hidden="true" />{hasSample
            ? 'View sample'
            : 'Load sample'}</button
        >
      </div>
    {/if}

    {#if sourceRequired}
      <div class="source-required notice" role="region" aria-label="Choose a media source">
        <Files size={18} aria-hidden="true" />
        <div class="source-required-copy">
          <strong
            >{desktop
              ? 'Choose media to begin.'
              : 'Try the sample to explore this workflow.'}</strong
          >
          <p>
            {desktop
              ? 'Add a file or folder from Files. Your source stays unchanged while you review the workflow.'
              : 'The browser preview cannot read local files, but the sample shows where each setting belongs.'}
          </p>
        </div>
        <div class="source-required-actions">
          <Button variant="outline" onclick={() => (view = 'files')}
            ><Files size={14} aria-hidden="true" />Open Files</Button
          >
          {#if !desktop}<Button onclick={() => loadSample(view)}
              ><FlaskConical size={14} aria-hidden="true" />Try sample</Button
            >{/if}
        </div>
      </div>
    {/if}

    {#if view === 'files'}
      <section class="files-workspace" aria-label="Source files">
        <div class="files-toolbar">
          <div>
            <h1>
              Source files<span class="heading-count"
                >{files.length.toString().padStart(2, '0')}</span
              >
            </h1>
            <p>Add media once, then inspect it or choose a workflow.</p>
          </div>
          <div class="toolbar-actions">
            <Button variant="outline" onclick={addFolder} disabled={!desktop || importing}
              ><FolderOpen size={15} aria-hidden="true" />Add folder</Button
            >
            <Button variant="ghost" onclick={clearFiles} disabled={!files.length || importing}
              ><Trash2 size={14} aria-hidden="true" />Clear list</Button
            ><Button
              onclick={addFiles}
              disabled={!desktop || importing}
              title={!desktop
                ? 'Local files require the jesses desktop app'
                : 'Add media files (Ctrl+O)'}
              >{#if importing}<LoaderCircle size={15} class="spinning" aria-hidden="true" />Reading
                files{:else}<Plus size={16} aria-hidden="true" />Add files{/if}</Button
            >
          </div>
        </div>
        <div class="folder-import-options">
          <label
            ><input
              type="checkbox"
              checked={recursiveImport}
              onchange={changeRecursive}
              disabled={!desktop || importing}
            /> Include subfolders</label
          >
          <span class="small-muted">Folder scans stop at 500 media files or 10,000 entries.</span>
          {#if importing}<Button variant="outline" onclick={stopImport}>Stop import</Button>{/if}
        </div>
        {#if importNotice}<div class="notice" role="status"><p>{importNotice}</p></div>{/if}
        {#if preferences.loaded && preferences.value.recentPaths.length}
          <div class="recent-media">
            <label for="recent-media-path">Recent media</label>
            <select id="recent-media-path" bind:value={recentSelection} disabled={importing}>
              <option value="">Choose a file or folder</option>
              {#each preferences.value.recentPaths as path}<option value={path}>{path}</option
                >{/each}
            </select>
            <Button variant="outline" onclick={openRecent} disabled={importing || !recentSelection}
              >Open recent</Button
            >
          </div>
        {/if}
        {#if importErrors.length}
          <div class="notice error-notice import-errors" role="alert">
            <CircleAlert size={17} aria-hidden="true" />
            <div>
              {#each importErrors as failure}<p>
                  <strong>{failure.name}:</strong>
                  {failure.message}
                </p>{/each}
            </div>
            <button
              type="button"
              class="icon-button"
              aria-label="Dismiss import errors"
              onclick={() => (importErrors = [])}><X size={15} aria-hidden="true" /></button
            >
          </div>
        {/if}
        <div class="file-columns">
          <div class="source-library-column">
            <section
              class="file-library"
              class:has-files={files.length > 0}
              aria-label="Imported media"
            >
              <div class="library-heading">
                <span class="eyebrow">Input media</span><span class="small-muted"
                  >{files.length
                    ? `${files.length} ${files.length === 1 ? 'file' : 'files'} · ${formatBytes(String(totalSize))}`
                    : 'No files added'}</span
                >
              </div>
              {#if files.length}
                <div class="library-search">
                  <label class="search-field">
                    <Search size={15} aria-hidden="true" />
                    <input
                      type="search"
                      aria-label="Search source files"
                      placeholder="Find a file by name or folder…"
                      bind:value={fileQuery}
                    />
                  </label>
                  <span class="small-muted" role="status"
                    >{visibleFiles.length} of {files.length} shown</span
                  >
                </div>
                {#if !visibleFiles.length}
                  <div class="search-empty">
                    <Search size={22} aria-hidden="true" />
                    <strong>No matching files</strong>
                    <p>Try another name or folder. Your imported files are still here.</p>
                    <button type="button" class="text-button" onclick={() => (fileQuery = '')}
                      >Clear search</button
                    >
                  </div>
                {:else}
                  <div class="file-table-scroll">
                    <table class="file-table">
                      <thead
                        ><tr
                          ><th class="name-column">Source</th><th>Duration</th><th>Video</th><th
                            class="audio-column">Audio</th
                          ><th class="size-column">Size</th><th class="remove-column"
                            ><span class="sr-only">Remove</span></th
                          ></tr
                        ></thead
                      ><tbody>
                        {#each visibleFiles as file (file.id)}
                          {@const video = file.streams.find((stream) => stream.kind === 'video')}
                          {@const audio = file.streams.find((stream) => stream.kind === 'audio')}
                          <tr class:selected={selectedId === file.id}>
                            <td class="name-cell"
                              ><button
                                type="button"
                                class="file-select"
                                aria-pressed={selectedId === file.id}
                                onclick={() => (selectedId = file.id)}
                                title={file.path}
                                ><span class="file-type-icon"
                                  ><Film size={19} strokeWidth={1.5} aria-hidden="true" /></span
                                ><span class="file-name"
                                  ><strong>{file.name}</strong><span
                                    >{file.id === sampleId
                                      ? 'Synthetic sample'
                                      : (file.format ?? 'Media file')}{video?.width && video.height
                                      ? ` · ${video.width} × ${video.height}`
                                      : ''}</span
                                  ></span
                                ></button
                              ></td
                            >
                            <td class="mono">{formatDuration(file.durationSeconds)}</td><td
                              ><span class="table-codec"
                                >{video ? displayCodec(video.codec) : '—'}</span
                              ></td
                            ><td class="audio-column"
                              ><span class="table-codec"
                                >{audio ? displayCodec(audio.codec) : '—'}</span
                              ></td
                            ><td class="mono size-column">{formatBytes(file.sizeBytes)}</td><td
                              class="remove-column"
                              ><button
                                type="button"
                                class="icon-button"
                                aria-label={`Remove ${file.name}`}
                                onclick={() => removeFile(file.id)}
                                disabled={importing}><X size={14} aria-hidden="true" /></button
                              ></td
                            >
                          </tr>
                        {/each}
                      </tbody>
                    </table>
                  </div>
                {/if}
                {#if importing}<div class="import-progress" role="status">
                    <LoaderCircle size={15} class="spinning" aria-hidden="true" />Reading {importingName}…
                  </div>{/if}
                <div class="library-bottom">
                  <span
                    ><Check size={13} aria-hidden="true" />{hasSample
                      ? 'Sample ready for inspection'
                      : 'Source metadata loaded'}</span
                  ><button type="button" class="text-button" onclick={() => (view = 'convert')}
                    >Review conversion defaults<ArrowRight size={13} aria-hidden="true" /></button
                  >
                </div>
              {:else}
                <div class="library-empty">
                  <div class="empty-file-symbol">
                    <FilePlus2 size={42} strokeWidth={1.05} aria-hidden="true" />
                  </div>
                  <h2>{importing ? 'Reading your media…' : 'Your media starts here.'}</h2>
                  <p>
                    {importing
                      ? importingName
                      : desktop
                        ? 'Drop files anywhere in this window, or choose them from your computer.'
                        : 'Add a video or audio file to explore its format, details, and individual streams.'}
                  </p>
                  <div class="empty-actions">
                    <Button
                      onclick={addFiles}
                      disabled={!desktop || importing}
                      title={!desktop
                        ? 'Local files require the jesses desktop app'
                        : 'Choose media files'}
                      >{#if importing}<LoaderCircle
                          size={15}
                          class="spinning"
                          aria-hidden="true"
                        />Reading file{:else}<FolderOpen size={15} aria-hidden="true" />Add files{/if}</Button
                    >
                    {#if !desktop}<Button variant="outline" onclick={() => loadSample()}
                        ><FlaskConical size={14} aria-hidden="true" />Try sample</Button
                      >{/if}
                  </div>
                  <span class="empty-shortcut"
                    >{desktop ? 'or press Ctrl + O' : 'Available in the desktop app'}</span
                  >
                </div>
                <div class="library-empty-footer">
                  <span><Film size={14} aria-hidden="true" />Video</span><span
                    ><HardDrive size={14} aria-hidden="true" />Audio</span
                  ><span class="supported-note">Formats supported by ffprobe</span>
                </div>
              {/if}
            </section>
            <section class="workflow-launcher" aria-label="Choose a workflow">
              <div class="workflow-launcher-heading">
                <h2>What would you like to do?</h2>
                <span
                  >{selectedFile
                    ? 'Continue with your selected source'
                    : 'Pick a workflow to get started'}</span
                >
              </div>
              <div class="workflow-shortcuts">
                <button type="button" onclick={() => (view = 'convert')}>
                  <span class="workflow-icon"
                    ><SlidersHorizontal size={19} aria-hidden="true" /></span
                  >
                  <span
                    ><strong>Convert a file</strong><small
                      >Choose a format, size, and quality.</small
                    ></span
                  >
                  <ArrowRight size={15} aria-hidden="true" />
                </button>
                <button type="button" onclick={() => (view = 'av1an')}>
                  <span class="workflow-icon"><Clapperboard size={19} aria-hidden="true" /></span>
                  <span
                    ><strong>Encode with av1an</strong><small>Encode scenes in parallel.</small
                    ></span
                  >
                  <ArrowRight size={15} aria-hidden="true" />
                </button>
                <button type="button" onclick={() => (view = 'batch')}>
                  <span class="workflow-icon"><Layers size={19} aria-hidden="true" /></span>
                  <span
                    ><strong>Convert multiple files</strong><small
                      >Apply one recipe to your file list.</small
                    ></span
                  >
                  <ArrowRight size={15} aria-hidden="true" />
                </button>
                <button type="button" onclick={() => (view = 'remux')}>
                  <span class="workflow-icon"><FolderOpen size={19} aria-hidden="true" /></span>
                  <span
                    ><strong>Repackage streams</strong><small
                      >Change containers without encoding.</small
                    ></span
                  >
                  <ArrowRight size={15} aria-hidden="true" />
                </button>
              </div>
            </section>
          </div>
          <FileInspector file={selectedFile} sample={selectedFile?.id === sampleId} />
        </div>
        <div class="workspace-hint">
          <Upload size={13} aria-hidden="true" /><span
            >{desktop
              ? 'Add multiple files at once. Select a source to see its streams.'
              : 'Try “Load sample” to explore the inspector with clearly labeled demonstration data.'}</span
          ><span class="private-label"
            ><HardDrive size={12} aria-hidden="true" />Local by design</span
          >
        </div>
      </section>
    {:else if view === 'tools'}
      <ToolsPanel
        {tools}
        {desktop}
        loading={toolsLoading}
        checked={toolsChecked}
        error={toolsError}
        onrefresh={refreshTools}
      />
    {/if}
    {#if (view === 'convert' || view === 'av1an' || view === 'batch' || view === 'remux') && jobsError}
      <div class="notice error-notice" role="alert">
        <CircleAlert size={16} aria-hidden="true" />
        <div>
          <p><strong>Job controls are unavailable.</strong> {jobsError}</p>
          <p>
            Files remains available. For a storage problem, correct it and restart jesses. Reconnect
            retries the job connection.
          </p>
        </div>
        <Button variant="outline" onclick={connectJobs} disabled={jobsConnecting}
          >Reconnect jobs</Button
        >
      </div>
    {/if}
    <div hidden={view !== 'convert'}>
      <QuickConvert
        {files}
        file={selectedFile}
        {tools}
        {jobs}
        connected={jobsConnected}
        onfiles={() => (view = 'files')}
        onstart={submitEncode}
        onqueue={queueEncode}
      />
    </div>
    <div hidden={view !== 'av1an'}>
      <Av1an
        {files}
        file={selectedFile}
        {tools}
        {jobs}
        connected={jobsConnected}
        onfiles={() => (view = 'files')}
        onstart={submitEncode}
        onqueue={queueEncode}
      />
    </div>
    <div hidden={view !== 'batch'}>
      <BatchEncode
        {files}
        {tools}
        connected={jobsConnected}
        onfiles={() => (view = 'files')}
        onqueue={queueBatch}
      />
    </div>
    <div hidden={view !== 'remux'}>
      <Remux
        file={selectedFile}
        {files}
        {tools}
        {jobs}
        connected={jobsConnected}
        onfiles={() => (view = 'files')}
        onstart={submitRemux}
        onmux={submitMux}
      />
    </div>
    <div hidden={view !== 'utilities'}>
      <Utilities {files} onimport={(paths) => void importPaths(paths)} /><SavedJobs
        {jobs}
        onqueue={queueEncode}
      />
    </div>
    <div hidden={!(view === 'convert' || view === 'av1an' || view === 'batch' || view === 'remux')}>
      <JobStatus
        job={currentJob}
        {jobs}
        oncancel={stopJob}
        onstop={stopQueue}
        onkeep={keepJobProgress}
        onresume={resumeSavedJob}
        ondiscard={discardSavedAv1anProgress}
        onpause={pauseLiveJob}
      />
    </div>
    <CompletionPanel
      settingsVisible={view === 'convert' ||
        view === 'av1an' ||
        view === 'batch' ||
        view === 'remux'}
    />
  </main>

  <section class="log-panel" class:expanded={logOpen} aria-label="Activity log">
    <div class="log-bar">
      <button
        type="button"
        class="log-toggle"
        onclick={toggleLog}
        aria-expanded={logOpen}
        aria-controls="activity-log"
        ><Terminal size={14} aria-hidden="true" /><span>Activity log</span><span class="log-count"
          >{logs.length}</span
        >{#if logOpen}<ChevronDown size={14} aria-hidden="true" />{:else}<ChevronUp
            size={14}
            aria-hidden="true"
          />{/if}</button
      >
      <div class="log-bar-right">
        {#if importing}<span role="status"
            ><LoaderCircle size={12} class="spinning" aria-hidden="true" />Reading media</span
          >{:else}<span
            ><span class="status-square"></span>{desktop ? 'Ready' : 'Preview mode'}</span
          >{/if}{#if logOpen}<button
            type="button"
            class="text-button"
            onclick={() => (logs = [])}
            disabled={!logs.length}>Clear log</button
          >{/if}
      </div>
    </div>
    {#if logOpen}<div
        id="activity-log"
        class="log-entries"
        role="log"
        aria-label="Recent activity"
        aria-live="polite"
      >
        {#each logs as entry (entry.id)}<div
            class="log-entry"
            class:log-error={entry.level === 'error'}
          >
            <time class="mono">{entry.time}</time><span class="log-level mono">{entry.level}</span
            ><span>{entry.message}</span>
          </div>{:else}<div class="log-empty">No activity to show.</div>{/each}
      </div>{/if}
  </section>
  <footer class="status-bar">
    <span
      >jesses <span class="mono">0.1.0</span><span class="footer-divider">/</span>Created by jkkma</span
    ><span>Desktop media encoding, muxing, and analysis.</span>
  </footer>
</div>

<style>
  .recent-media {
    display: flex;
    flex-wrap: wrap;
    gap: 10px;
    align-items: center;
    margin-bottom: 16px;
  }
  .recent-media select {
    flex: 1;
    min-width: 160px;
    max-width: 100%;
    padding: 8px;
    border: 1px solid var(--border);
    background: var(--background);
  }
  .folder-import-options {
    display: flex;
    align-items: center;
    flex-wrap: wrap;
    gap: 14px;
    margin: 0 0 10px;
  }
  .folder-import-options label {
    display: flex;
    align-items: center;
    gap: 8px;
    font-size: 12px;
  }
  .folder-import-options input {
    accent-color: #ad5326;
  }
  .source-required {
    align-items: center;
    margin: 8px 0;
    background: var(--accent);
    border-color: var(--border);
  }
  .source-required > :global(svg) {
    flex: 0 0 auto;
    color: var(--jesses-rust-text);
  }
  .source-required-copy {
    flex: 1 1 320px;
    min-width: 0;
  }
  .source-required-copy strong {
    display: block;
    font-size: 12px;
  }
  .source-required-copy p {
    margin-top: 3px;
    font-size: 11px;
    color: var(--muted-foreground);
  }
  .source-required-actions {
    display: flex;
    flex: 0 0 auto;
    flex-wrap: wrap;
    gap: 8px;
  }
  .empty-actions {
    display: flex;
    justify-content: center;
    flex-wrap: wrap;
    gap: 8px;
    margin-top: 24px;
  }
  .library-empty .empty-actions :global([data-slot='button']) {
    margin-top: 0;
  }
  :global(.workspace-nav nav) {
    flex-wrap: wrap;
  }
  :global(.workspace-nav) {
    min-height: 44px;
    height: auto;
  }
  :global(.toolbar-actions) {
    flex-wrap: wrap;
    justify-content: flex-end;
  }
  @media (max-width: 1000px) {
    :global(.workspace-label) {
      display: none;
    }
  }
  @media (max-width: 850px) {
    .source-required-actions {
      flex-basis: 100%;
    }
  }
</style>
