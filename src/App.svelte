<script lang="ts">
  import { onMount } from 'svelte';
  import {
    ArrowRight,
    Check,
    ChevronDown,
    ChevronUp,
    CircleAlert,
    FilePlus2,
    Files,
    Film,
    FlaskConical,
    FolderOpen,
    HardDrive,
    LoaderCircle,
    Monitor,
    Plus,
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
  import ToolsPanel from '$lib/features/tools/ToolsPanel.svelte';
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
    startEncode,
    enqueueEncode,
    enqueueEncodeBatch,
    cancelAllJobs,
    cancelJob,
  } from '$lib/ipc/client';
  import type {
    EncodeRequest,
    JobSnapshot,
    RemuxRequest,
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

  type View = 'files' | 'convert' | 'av1an' | 'batch' | 'remux' | 'tools';
  type LogEntry = { id: number; time: string; level: 'info' | 'error'; message: string };
  const desktop = isDesktop();
  const sampleId = 'jesses-synthetic-preview';
  let view = $state<View>('files');
  let files = $state<MediaFile[]>([]);
  let selectedId = $state<string | null>(null);
  let tools = $state<ToolInfo[]>([]);
  let toolsLoading = $state(desktop);
  let toolsError = $state<string | null>(null);
  let importing = $state(false);
  let importingName = $state('');
  let recursiveImport = $state(false);
  let importNotice = $state<string | null>(null);
  let importGeneration = 0;
  let importQueue: string[] = [];
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
  const terminalJob = (state: string) =>
    ['succeeded', 'failed', 'canceled', 'interrupted'].includes(state);
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

  async function stopJob(id: string) {
    const job = await cancelJob(id);
    jobs = jobs.map((entry) => (entry.id === id && !terminalJob(entry.state) ? job : entry));
  }
  async function submitEncode(request: EncodeRequest) {
    const job = await startEncode(request);
    if (!jobs.some((entry) => entry.id === job.id)) jobs = [job, ...jobs];
    addLog(`${request.settings.encoder === 'x264' ? 'H.264 / x264' : 'AV1'} encode job submitted.`);
  }
  async function queueEncode(request: EncodeRequest) {
    const job = await enqueueEncode(request);
    if (!jobs.some((entry) => entry.id === job.id)) jobs = [job, ...jobs];
    addLog(
      `${request.settings.encoder === 'x264' ? 'H.264 / x264' : 'AV1'} encode added to the queue.`,
    );
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
  const totalSize = $derived(files.reduce((total, file) => total + Number(file.sizeBytes), 0));
  const hasSample = $derived(files.some((file) => file.id === sampleId));
  const availableTools = $derived(tools.filter((tool) => tool.available).length);

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

  async function refreshTools() {
    if (!desktop) return;
    toolsLoading = true;
    toolsError = null;
    try {
      tools = await getCapabilities();
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

  function queueImportPaths(paths: string[]) {
    const candidates = paths.filter(
      (path) =>
        path !== activePath &&
        !importQueue.includes(path) &&
        !files.some((file) => file.path === path),
    );
    importQueue.push(...new Set(candidates));
  }

  function beginImport() {
    const generation = ++importGeneration;
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
    activePath = null;
    importing = false;
    importingName = '';
  }

  function stopImport() {
    ++importGeneration;
    importQueue = [];
    activePath = null;
    importing = false;
    importingName = '';
    importNotice =
      'Stopped importing. Completed files are kept. The current scan or probe may finish in the background.';
    addLog('Stopped importing; pending files and late results are discarded.');
  }

  async function drainImport(generation: number) {
    try {
      while (generation === importGeneration && importQueue.length) {
        const path = importQueue.shift()!;
        activePath = path;
        importingName = fileName(path);
        try {
          const media = await probeMedia(path);
          if (generation !== importGeneration) return;
          if (!files.some((file) => file.id === media.id)) {
            files = [...files, media];
            selectedId = media.id;
            addLog(`Imported ${media.name} · ${media.streams.length} streams.`);
          }
        } catch (error) {
          if (generation !== importGeneration) return;
          const message = errorMessage(error);
          importErrors = [...importErrors, { name: importingName, message }];
          addLog(`Could not import ${importingName}: ${message}`, 'error');
        }
      }
    } finally {
      finishImport(generation);
    }
  }

  async function importPaths(paths: string[]) {
    if (!desktop || !paths.length) return;
    if (importing) {
      queueImportPaths(paths);
      return;
    }
    const generation = beginImport();
    queueImportPaths(paths);
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
      importingName = `Scanning ${fileName(path)}`;
      const scan = await scanMediaFolder({ path, recursive });
      if (generation !== importGeneration) return;
      importErrors = scan.errors.map((error) => ({
        name: error.path ?? 'Folder scan',
        message: error.message,
      }));
      importNotice = `${scan.paths.length} media files found. ${scan.skippedCount} entries skipped.${scan.truncated ? ' Scan truncated: the 500-file or 10,000-entry limit was reached. Add a smaller folder to find the remaining files.' : ''}`;
      queueImportPaths(scan.paths);
      await drainImport(generation);
    } catch (error) {
      if (generation !== importGeneration) return;
      const message = errorMessage(error);
      importErrors = [...importErrors, { name: 'Folder import', message }];
      addLog(`Could not import folder: ${message}`, 'error');
    } finally {
      finishImport(generation);
    }
  }

  function removeFile(id: string) {
    const file = files.find((entry) => entry.id === id);
    files = files.filter((entry) => entry.id !== id);
    if (selectedId === id) selectedId = files[0]?.id ?? null;
    if (file) addLog(`Removed ${file.name} from the workspace.`);
  }

  function clearFiles() {
    files = [];
    selectedId = null;
    addLog('Source list cleared.');
  }

  function loadSample() {
    if (files.some((file) => file.id === sampleId)) {
      selectedId = sampleId;
      view = 'files';
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
    view = 'files';
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
      void refreshTools();
      void connectJobs();
      void subscribeDrop((paths) => {
        void importPaths(paths);
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
    <nav aria-label="Workspace">
      <button
        type="button"
        class:active={view === 'files'}
        aria-current={view === 'files' ? 'page' : undefined}
        onclick={() => (view = 'files')}
        ><Files size={16} aria-hidden="true" />Files<span class="nav-count"
          >{files.length.toString().padStart(2, '0')}</span
        ></button
      >
      <button
        type="button"
        class:active={view === 'convert'}
        aria-current={view === 'convert' ? 'page' : undefined}
        onclick={() => (view = 'convert')}
        ><SlidersHorizontal size={16} aria-hidden="true" />Quick Convert</button
      >
      <button
        type="button"
        class:active={view === 'av1an'}
        aria-current={view === 'av1an' ? 'page' : undefined}
        onclick={() => (view = 'av1an')}>av1an</button
      >
      <button
        type="button"
        class:active={view === 'batch'}
        aria-current={view === 'batch' ? 'page' : undefined}
        onclick={() => (view = 'batch')}>Batch encode</button
      >
      <button
        type="button"
        class:active={view === 'remux'}
        aria-current={view === 'remux' ? 'page' : undefined}
        onclick={() => (view = 'remux')}>Remux</button
      >
      <button
        type="button"
        class:active={view === 'tools'}
        aria-current={view === 'tools' ? 'page' : undefined}
        onclick={() => (view = 'tools')}
        ><Wrench size={16} aria-hidden="true" />Tools & settings</button
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
        <button type="button" class="text-button" onclick={loadSample}
          ><FlaskConical size={13} aria-hidden="true" />{hasSample
            ? 'View sample'
            : 'Load sample'}</button
        >
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
            <p>Inspect your media. Start with a source.</p>
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
              bind:checked={recursiveImport}
              disabled={!desktop || importing}
            /> Include subfolders</label
          >
          <span class="small-muted">Folder scans stop at 500 media files or 10,000 entries.</span>
          {#if importing}<Button variant="outline" onclick={stopImport}>Stop import</Button>{/if}
        </div>
        {#if importNotice}<div class="notice" role="status"><p>{importNotice}</p></div>{/if}
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
                    {#each files as file (file.id)}
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
                          ><span class="table-codec">{video ? displayCodec(video.codec) : '—'}</span
                          ></td
                        ><td class="audio-column"
                          ><span class="table-codec">{audio ? displayCodec(audio.codec) : '—'}</span
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
                  <span class="corner top-left"></span><span class="corner top-right"
                  ></span><FilePlus2 size={42} strokeWidth={1.05} aria-hidden="true" /><span
                    class="corner bottom-left"
                  ></span><span class="corner bottom-right"></span>
                </div>
                <h2>{importing ? 'Reading your media…' : 'Your media starts here.'}</h2>
                <p>
                  {importing
                    ? importingName
                    : desktop
                      ? 'Drop files anywhere in this window, or choose them from your computer.'
                      : 'Add a video or audio file to explore its format, details, and individual streams.'}
                </p>
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
        {tools}
        {jobs}
        connected={jobsConnected}
        onfiles={() => (view = 'files')}
        onstart={submitRemux}
      />
    </div>
    {#if view === 'convert' || view === 'av1an' || view === 'batch' || view === 'remux'}<JobStatus
        job={currentJob}
        {jobs}
        oncancel={stopJob}
        onstop={stopQueue}
      />{/if}
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
  .folder-import-options {
    display: flex;
    align-items: center;
    flex-wrap: wrap;
    gap: 14px;
    margin: 0 0 16px;
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
  :global(.workspace-nav nav) {
    flex-wrap: wrap;
  }
  :global(.workspace-nav) {
    min-height: 49px;
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
</style>
