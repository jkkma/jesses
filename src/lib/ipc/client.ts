import { Channel, invoke, isTauri } from '@tauri-apps/api/core';
import { getCurrentWebview } from '@tauri-apps/api/webview';
import { listen } from '@tauri-apps/api/event';
import { open, save, type OpenDialogOptions } from '@tauri-apps/plugin-dialog';
import type {
  Av1anResourceRequest,
  Av1anRecoveryEntry,
  Av1anRecoveryAction,
  Av1anRecoveryActionResult,
  Av1anResourceEstimate,
  ImageRequest,
  ImageResult,
  OutputPathRequest,
  OutputPathResolution,
  UtilityRequest,
  UtilityResult,
  UtilityCapabilities,
  UtilityBatchRequest,
  UtilityBatchResult,
  UtilityProgress,
  CompletionOptions,
  CompletionStatus,
  SavedJobInspection,
  AnalysisReport,
  AnalysisExportFormat,
  AutoCropRequest,
  AutoCropResult,
  BitrateRequest,
  BitrateResult,
  QualityRequest,
  QualityResult,
  LoudnessRequest,
  LoudnessResult,
  BatchEncodePreview,
  BatchEncodeRequest,
  BatchOutputPathRequest,
  BatchOutputPaths,
  CommandEditRequest,
  CommandEditReview,
  CommandEditConfirmation,
  CommandEditResult,
  EncodeRequest,
  EncodeBackend,
  VideoEncoder,
  EncoderParameterCatalog,
  EncoderParameterPreset,
  EncoderParameterPresetKey,
  EncodeCommandPlan,
  FolderScanRequest,
  FolderScanResult,
  FramePreviewRequest,
  FramePreviewResult,
  JobSnapshot,
  MediaFile,
  MuxRequest,
  RemuxRequest,
  ToolInfo,
  UserPreferences,
  SavePreferencesRequest,
  PreferenceImportPreview,
} from './generated';

export async function estimateAv1anResources(
  request: Av1anResourceRequest,
): Promise<Av1anResourceEstimate | null> {
  if (!isTauri()) return null;
  return invoke<Av1anResourceEstimate>('estimate_av1an_resources', { request });
}

export function readAv1anGrainTable(path: string): Promise<string> {
  return invoke<string>('read_av1an_grain_table', { path });
}

export function makeAv1anGrainPreset(preset: string, signal?: AbortSignal): Promise<string> {
  return analyzeSource('make_av1an_grain_preset', preset, signal);
}

export function runImageJob(request: ImageRequest, signal?: AbortSignal): Promise<ImageResult> {
  return analyzeSource('run_image_job', request, signal);
}
export function resolveOutputPath(request: OutputPathRequest): Promise<OutputPathResolution> {
  requireDesktop();
  return invoke<OutputPathResolution>('resolve_output_path', { request });
}
export function resolveBatchOutputPaths(
  request: BatchOutputPathRequest,
  signal?: AbortSignal,
): Promise<BatchOutputPaths> {
  if (signal) return analyzeSource('resolve_batch_output_paths', request, signal);
  requireDesktop();
  return invoke<BatchOutputPaths>('resolve_batch_output_paths', { request });
}
export async function prepareCommandEdit(
  request: CommandEditRequest,
  signal?: AbortSignal,
): Promise<CommandEditReview> {
  const review = await analyzeSource<CommandEditReview>(
    'prepare_command_edit',
    request,
    signal,
    undefined,
    true,
  );
  if (signal?.aborted) {
    await cancelCommandEdit(review.id);
    throw new DOMException('The operation was aborted.', 'AbortError');
  }
  return review;
}
export async function confirmCommandEdit(
  request: CommandEditConfirmation,
  signal?: AbortSignal,
): Promise<CommandEditResult> {
  try {
    const result = await analyzeSource<CommandEditResult>('confirm_command_edit', request, signal);
    return result;
  } catch (cause) {
    if (signal?.aborted) await cancelCommandEdit(request.id).catch(() => {});
    throw cause;
  }
}
export function cancelCommandEdit(id: string): Promise<void> {
  requireDesktop();
  return invoke<void>('cancel_command_edit', { id });
}
export function setCommandEditPaused(id: string, paused: boolean): Promise<void> {
  requireDesktop();
  return invoke<void>('set_command_edit_paused', { id, paused });
}
export function runUtility(
  request: UtilityRequest,
  signal?: AbortSignal,
  onId?: (id: string | null) => void,
  onProgress?: (progress: UtilityProgress) => void,
): Promise<UtilityResult> {
  return analyzeSource('run_utility', request, signal, onId, false, onProgress);
}
export function runUtilityBatch(
  request: UtilityBatchRequest,
  signal?: AbortSignal,
  onId?: (id: string | null) => void,
  onProgress?: (progress: UtilityProgress) => void,
): Promise<UtilityBatchResult> {
  return analyzeSource('run_utility_batch', request, signal, onId, true, onProgress);
}
export async function setMediaAnalysisPaused(id: string, paused: boolean): Promise<void> {
  requireDesktop();
  return invoke('set_media_analysis_paused', { id, paused });
}
export function inspectUtilityCapabilities(signal?: AbortSignal): Promise<UtilityCapabilities> {
  return analyzeSource('inspect_utility_capabilities', {}, signal);
}
export async function chooseImages(): Promise<string[]> {
  requireDesktop();
  const result = await openRemembered({
    multiple: true,
    directory: false,
    title: 'Choose images in sequence',
    filters: [{ name: 'Images', extensions: ['png', 'jpg', 'jpeg', 'bmp', 'tiff', 'tif', 'webp'] }],
  });
  const paths = result === null ? [] : Array.isArray(result) ? result : [result];
  rememberDirectory(paths[0], false);
  return paths;
}
export async function chooseUtilityDestination(
  defaultPath: string,
  extensions: string[],
): Promise<string | null> {
  requireDesktop();
  return save({ title: 'Save new output', defaultPath, filters: [{ name: 'Output', extensions }] });
}
export async function saveActivityLog(contents: string): Promise<string | null> {
  requireDesktop();
  const path = await save({
    title: 'Save activity log',
    defaultPath: 'jesses-activity.log',
    filters: [{ name: 'Log', extensions: ['log', 'txt'] }],
  });
  if (!path) return null;
  await invoke('save_activity_log', { path, contents });
  return path;
}
export async function openDirectory(path: string): Promise<void> {
  requireDesktop();
  return invoke('open_directory', { path });
}
export async function extractAttachment(inputPath: string, streamIndex: number): Promise<string> {
  requireDesktop();
  const id = await invoke<string>('begin_media_analysis');
  try {
    return await invoke<string>('extract_attachment', { id, inputPath, streamIndex });
  } finally {
    await invoke('cancel_media_analysis', { id }).catch(() => {});
  }
}
export function parentDirectory(path: string): string | null {
  const slash = Math.max(path.lastIndexOf('/'), path.lastIndexOf('\\'));
  if (slash < 0) return null;
  if (slash === 0 || (slash === 2 && path[1] === ':')) return path.slice(0, slash + 1);
  return path.slice(0, slash);
}
const DIRECTORY_KEY = 'jesses.last-browsed-directory';
function lastDirectory(): string | undefined {
  try {
    return localStorage.getItem(DIRECTORY_KEY) || undefined;
  } catch {
    return undefined;
  }
}
function rememberDirectory(path: string | undefined | null, directory: boolean) {
  if (!path) return;
  const folder = directory ? path : parentDirectory(path);
  if (folder)
    try {
      localStorage.setItem(DIRECTORY_KEY, folder);
    } catch {
      /* optional preference */
    }
}
async function openRemembered(options: OpenDialogOptions): Promise<string | string[] | null> {
  const defaultPath = lastDirectory();
  try {
    return await open({ ...options, defaultPath });
  } catch (error) {
    if (!defaultPath) throw error;
    try {
      localStorage.removeItem(DIRECTORY_KEY);
    } catch {
      /* optional preference */
    }
    return open(options);
  }
}
export async function chooseUtilityFile(
  title: string,
  extensions: string[],
): Promise<string | null> {
  requireDesktop();
  const result = await openRemembered({
    title,
    multiple: false,
    directory: false,
    filters: [{ name: 'Input', extensions }],
  });
  const path = Array.isArray(result) ? (result[0] ?? null) : result;
  rememberDirectory(path, false);
  return path;
}
export async function getCompletionStatus(): Promise<CompletionStatus> {
  requireDesktop();
  return invoke('get_completion_status');
}
export async function setCompletionOptions(options: CompletionOptions): Promise<CompletionStatus> {
  requireDesktop();
  return invoke('set_completion_options', { options });
}
export async function cancelFinishAction(): Promise<CompletionStatus> {
  requireDesktop();
  return invoke('cancel_finish_action');
}
export async function inspectSavedJob(
  path: string,
  svtBuild: VideoEncoder | null = null,
): Promise<SavedJobInspection> {
  requireDesktop();
  return invoke('inspect_saved_job', { path, svtBuild });
}
export async function exportSavedJob(path: string, request: EncodeRequest): Promise<string> {
  requireDesktop();
  return invoke('export_saved_job', { path, request });
}

export async function exportAnalysis(
  report: AnalysisReport,
  format: AnalysisExportFormat,
): Promise<string | null> {
  requireDesktop();
  const outputPath = await save({
    title: 'Export analysis',
    defaultPath: `${report.kind}-analysis.${format}`,
    filters: [{ name: format === 'csv' ? 'CSV data' : 'SVG chart', extensions: [format] }],
  });
  if (outputPath === null) return null;
  return invoke<string>('export_analysis', { request: { outputPath, format, report } });
}

export async function getParameterPresets(): Promise<EncoderParameterPreset[]> {
  requireDesktop();
  return invoke('get_parameter_presets');
}
export async function saveParameterPreset(
  request: EncoderParameterPreset,
): Promise<EncoderParameterPreset[]> {
  requireDesktop();
  return invoke('save_parameter_preset', { request });
}
export async function removeParameterPreset(
  request: EncoderParameterPresetKey,
): Promise<EncoderParameterPreset[]> {
  requireDesktop();
  return invoke('remove_parameter_preset', { request });
}
export async function getPreferences(): Promise<UserPreferences> {
  requireDesktop();
  return invoke('get_preferences');
}
export async function savePreferences(request: SavePreferencesRequest): Promise<UserPreferences> {
  requireDesktop();
  return invoke('save_preferences', { request });
}
export async function rememberRecentMedia(paths: string[]): Promise<UserPreferences> {
  requireDesktop();
  return invoke('remember_recent_media', { paths });
}
export async function previewPreferenceImport(path: string): Promise<PreferenceImportPreview> {
  requireDesktop();
  return invoke('preview_preference_import', { path });
}
export async function recentPathIsFolder(path: string): Promise<boolean> {
  requireDesktop();
  return invoke('recent_path_is_folder', { path });
}
export async function choosePreferenceImport(): Promise<string | null> {
  requireDesktop();
  return open({
    multiple: false,
    directory: false,
    title: 'Review saved general preferences',
    filters: [{ name: 'JSON preferences', extensions: ['json'] }],
  });
}

export async function getStorageLocations(): Promise<[string, string][]> {
  requireDesktop();
  return invoke<[string, string][]>('get_storage_locations');
}

async function analyzeSource<T>(
  command: string,
  request: unknown,
  signal?: AbortSignal,
  onId?: (id: string | null) => void,
  allowCanceledResult = false,
  onProgress?: (progress: UtilityProgress) => void,
  context?: Record<string, unknown>,
): Promise<T> {
  requireDesktop();
  if (signal?.aborted) throw new DOMException('Source inspection was canceled.', 'AbortError');
  const id = await invoke<string>('begin_media_analysis');
  onId?.(id);
  let unlisten: (() => void) | undefined;
  const cancel = () => {
    void invoke('cancel_media_analysis', { id }).catch(() => {});
  };
  signal?.addEventListener('abort', cancel, { once: true });
  try {
    if (onProgress)
      unlisten = await listen<{ id: string; progress: UtilityProgress }>(
        'utility-progress',
        (event) => {
          if (event.payload.id === id && !signal?.aborted) onProgress(event.payload.progress);
        },
      );
    if (signal?.aborted) throw new DOMException('Source inspection was canceled.', 'AbortError');
    const result = await invoke<T>(command, { id, request, ...context });
    if (signal?.aborted && !allowCanceledResult)
      throw new DOMException('Source inspection was canceled.', 'AbortError');
    return result;
  } finally {
    try {
      await unlisten?.();
    } catch {
      /* cleanup must not hide the job result */
    }
    signal?.removeEventListener('abort', cancel);
    onId?.(null);
    cancel();
  }
}

export function previewFrame(
  request: FramePreviewRequest,
  signal?: AbortSignal,
): Promise<FramePreviewResult> {
  return analyzeSource('preview_frame', request, signal);
}

export function detectCrop(
  request: AutoCropRequest,
  signal?: AbortSignal,
): Promise<AutoCropResult> {
  return analyzeSource('detect_crop', request, signal);
}

export function snapKeyframe(
  request: { inputPath: string; videoStreamIndex: number; positionSeconds: number },
  signal?: AbortSignal,
): Promise<{ positionSeconds: number | null; sourceFingerprint: string }> {
  return analyzeSource('snap_keyframe', request, signal);
}

export function detectFields(
  request: { inputPath: string; videoStreamIndex: number },
  signal?: AbortSignal,
): Promise<{
  order: 'progressive' | 'topFirst' | 'bottomFirst' | 'undetermined';
  telecined: boolean;
  sampledFrames: number;
  topFirstFrames: number;
  bottomFirstFrames: number;
  progressiveFrames: number;
  undeterminedFrames: number;
  fieldGapRatio: number | null;
  sourceFingerprint: string;
  evidence: string;
}> {
  return analyzeSource('detect_fields', request, signal);
}

export function readColorMetadata(
  request: import('./generated').ColorMetadataInspectionRequest,
  signal?: AbortSignal,
  onId?: (id: string | null) => void,
): Promise<import('./generated').ColorMetadataReport> {
  return analyzeSource('read_color_metadata', request, signal, onId);
}

export async function resolveDeinterlace(
  request: import('./generated').DeinterlaceExportRequest,
  automatic: boolean,
  signal?: AbortSignal,
  onId?: (id: string | null) => void,
): Promise<{ request?: import('./generated').DeinterlaceExportRequest | null; message: string }> {
  requireDesktop();
  if (signal?.aborted) throw new DOMException('Source inspection was canceled.', 'AbortError');
  const id = await invoke<string>('begin_media_analysis');
  onId?.(id);
  const cancel = () => {
    void invoke('cancel_media_analysis', { id }).catch(() => {});
  };
  signal?.addEventListener('abort', cancel, { once: true });
  try {
    if (signal?.aborted) throw new DOMException('Source inspection was canceled.', 'AbortError');
    const result = await invoke<{
      request?: import('./generated').DeinterlaceExportRequest | null;
      message: string;
    }>('resolve_deinterlace', { id, request, automatic });
    if (signal?.aborted) throw new DOMException('Source inspection was canceled.', 'AbortError');
    return result;
  } finally {
    signal?.removeEventListener('abort', cancel);
    onId?.(null);
    cancel();
  }
}

export function getEncoderParameters(
  encoder: VideoEncoder,
  backend: EncodeBackend,
  signal?: AbortSignal,
): Promise<EncoderParameterCatalog> {
  return analyzeSource('get_encoder_parameters', { encoder, backend }, signal);
}

export function previewEncodePlan(
  request: EncodeRequest,
  signal?: AbortSignal,
): Promise<EncodeCommandPlan> {
  return analyzeSource('preview_encode_plan', request, signal);
}

export function isDesktop(): boolean {
  return isTauri();
}

export async function setJobPaused(id: string, paused: boolean): Promise<JobSnapshot> {
  requireDesktop();
  return invoke<JobSnapshot>('set_job_paused', { id, paused });
}

export function analyzeQuality(
  request: QualityRequest,
  signal?: AbortSignal,
  onId?: (id: string | null) => void,
): Promise<QualityResult> {
  return analyzeSource('analyze_quality', request, signal, onId);
}

export function analyzeBitrate(
  request: BitrateRequest,
  signal?: AbortSignal,
  onId?: (id: string | null) => void,
  sourceChart = false,
): Promise<BitrateResult> {
  return analyzeSource('analyze_bitrate', request, signal, onId, false, undefined, { sourceChart });
}

export function measureLoudness(
  request: LoudnessRequest,
  signal?: AbortSignal,
): Promise<LoudnessResult> {
  return analyzeSource('measure_loudness', request, signal);
}

function requireDesktop() {
  if (!isDesktop()) throw new Error('Open the jesses desktop app to import local media.');
}

export async function chooseMediaFiles(): Promise<string[]> {
  requireDesktop();
  const paths = await openRemembered({
    multiple: true,
    directory: false,
    title: 'Add media to jesses',
    filters: [
      {
        name: 'Media files',
        extensions: [
          'mkv',
          'mp4',
          'mov',
          'webm',
          'avi',
          'm4v',
          'ts',
          'm2ts',
          'mpg',
          'mpeg',
          'mxf',
          'vob',
          'wav',
          'flac',
          'mp3',
          'aac',
          'm4a',
          'ogg',
          'opus',
          'srt',
          'ass',
          'ssa',
          'vtt',
          'png',
          'jpg',
          'jpeg',
          'webp',
          'gif',
        ],
      },
      { name: 'All files', extensions: ['*'] },
    ],
  });
  const result = paths === null ? [] : Array.isArray(paths) ? paths : [paths];
  rememberDirectory(result[0], false);
  return result;
}

export async function beginMediaImport(): Promise<string> {
  requireDesktop();
  const id = await invoke<string>('begin_media_analysis', { forImport: true });
  try {
    await invoke('claim_media_import', { id });
    return id;
  } catch (error) {
    await endMediaImport(id).catch(() => {});
    throw error;
  }
}

export function endMediaImport(id: string): Promise<void> {
  requireDesktop();
  return invoke('cancel_media_analysis', { id });
}

export async function probeMedia(path: string, importId?: string): Promise<MediaFile> {
  requireDesktop();
  return invoke<MediaFile>('probe_media', { path, importId });
}

export async function chooseMediaFolder(): Promise<string[]> {
  requireDesktop();
  const path = await openRemembered({
    directory: true,
    multiple: true,
    title: 'Add an image sequence folder',
  });
  const paths = Array.isArray(path) ? path : path ? [path] : [];
  if (paths.length) rememberDirectory(paths[0], true);
  return paths;
}

export function importImageSequence(
  request: import('./generated').ImageSequenceImportRequest,
  signal?: AbortSignal,
): Promise<import('./generated').MediaFile> {
  return analyzeSource('import_image_sequence', request, signal);
}
export function releaseImageSequence(
  request: import('./generated').ImageSequenceReleaseRequest,
): Promise<void> {
  requireDesktop();
  return invoke('release_image_sequence', { request });
}

export async function chooseOutputFolder(): Promise<string | null> {
  requireDesktop();
  const path = await openRemembered({
    directory: true,
    multiple: false,
    title: 'Choose batch output folder',
  });
  if (Array.isArray(path)) return null;
  rememberDirectory(path, true);
  return path;
}

export async function scanMediaFolder(request: FolderScanRequest): Promise<FolderScanResult> {
  requireDesktop();
  return invoke<FolderScanResult>('scan_media_folder', { request });
}

export async function previewEncodeBatch(
  request: BatchEncodeRequest,
  signal?: AbortSignal,
): Promise<BatchEncodePreview> {
  if (signal) return analyzeSource('preview_encode_batch', request, signal);
  requireDesktop();
  return invoke<BatchEncodePreview>('preview_encode_batch', { request });
}

export async function enqueueEncodeBatch(requests: EncodeRequest[]): Promise<JobSnapshot[]> {
  requireDesktop();
  return invoke<JobSnapshot[]>('enqueue_encode_batch', { requests });
}

export async function getCapabilities(): Promise<ToolInfo[]> {
  requireDesktop();
  return invoke<ToolInfo[]>('get_capabilities');
}

export async function chooseRemuxDestination(defaultPath: string): Promise<string | null> {
  requireDesktop();
  return save({
    title: 'Save remuxed media',
    defaultPath,
    filters: [
      { name: 'Media containers', extensions: ['mkv', 'mp4', 'mov', 'webm', 'm4a', 'ogg'] },
    ],
  });
}

export async function startRemux(request: RemuxRequest): Promise<JobSnapshot> {
  requireDesktop();
  return invoke<JobSnapshot>('start_remux', { request });
}

export async function startMux(request: MuxRequest): Promise<JobSnapshot> {
  requireDesktop();
  return invoke<JobSnapshot>('start_mux', { request });
}

export async function chooseEncodeDestination(defaultPath: string): Promise<string | null> {
  requireDesktop();
  return save({
    title: 'Save encoded video',
    defaultPath,
    filters: [{ name: 'Media containers', extensions: ['mkv', 'mp4', 'mov', 'webm'] }],
  });
}

export async function startEncode(request: EncodeRequest): Promise<JobSnapshot> {
  requireDesktop();
  return invoke<JobSnapshot>('start_encode', { request });
}

export async function enqueueEncode(request: EncodeRequest): Promise<JobSnapshot> {
  requireDesktop();
  return invoke<JobSnapshot>('enqueue_encode', { request });
}

export async function cancelAllJobs(): Promise<JobSnapshot[]> {
  requireDesktop();
  return invoke<JobSnapshot[]>('cancel_all_jobs');
}

export async function cancelJob(id: string): Promise<JobSnapshot> {
  requireDesktop();
  return invoke<JobSnapshot>('cancel_job', { id });
}

export async function stopJob(id: string): Promise<JobSnapshot> {
  requireDesktop();
  return invoke<JobSnapshot>('stop_job', { id });
}

export async function resumeJob(id: string): Promise<JobSnapshot> {
  requireDesktop();
  return invoke<JobSnapshot>('resume_job', { id });
}

export async function listAv1anRecoveries(): Promise<Av1anRecoveryEntry[]> {
  requireDesktop();
  return invoke<Av1anRecoveryEntry[]>('list_av1an_recoveries');
}

export async function av1anRecoveryAction(
  request: Av1anRecoveryAction,
): Promise<Av1anRecoveryActionResult> {
  requireDesktop();
  return invoke<Av1anRecoveryActionResult>('av1an_recovery_action', { request });
}

export async function discardAv1anRecovery(id: string): Promise<JobSnapshot> {
  requireDesktop();
  return invoke<JobSnapshot>('discard_av1an_recovery', { id });
}

const jobListeners = new Set<(jobs: JobSnapshot[]) => void>();
let jobSubscription: Promise<void> | null = null;
let latestJobs: JobSnapshot[] | null = null;

export async function subscribeJobs(handler: (jobs: JobSnapshot[]) => void): Promise<() => void> {
  requireDesktop();
  // Native subscribe_jobs has one generation per window. All app consumers
  // share that channel so a short-lived waiter cannot replace the main stream.
  jobListeners.add(handler);
  if (latestJobs) handler(latestJobs);
  if (!jobSubscription) {
    const channel = new Channel<JobSnapshot[]>();
    channel.onmessage = (jobs) => {
      latestJobs = jobs;
      for (const listener of [...jobListeners]) listener(jobs);
    };
    jobSubscription = invoke<void>('subscribe_jobs', { channel });
    void jobSubscription.catch(() => {
      jobSubscription = null;
      latestJobs = null;
    });
  }
  try {
    await jobSubscription;
  } catch (cause) {
    jobListeners.delete(handler);
    throw cause;
  }
  return () => {
    jobListeners.delete(handler);
  };
}

/** Wait through the existing native job channel; this never submits a job. */
export async function waitForJobTerminal(id: string, signal?: AbortSignal): Promise<JobSnapshot> {
  requireDesktop();
  let dispose = () => {};
  let settled = false;
  let resolveResult!: (job: JobSnapshot) => void;
  let rejectResult!: (cause: unknown) => void;
  const result = new Promise<JobSnapshot>((resolve, reject) => {
    resolveResult = resolve;
    rejectResult = reject;
  });
  // Abort can arrive while the native channel is being attached.
  void result.catch(() => {});
  const abort = () => {
    if (settled) return;
    settled = true;
    dispose();
    void cancelJob(id).catch(() => {});
    rejectResult(new DOMException('The operation was aborted.', 'AbortError'));
  };
  signal?.addEventListener('abort', abort, { once: true });
  if (signal?.aborted) abort();
  try {
    if (!settled) {
      dispose = await subscribeJobs((jobs) => {
        const job = jobs.find((item) => item.id === id);
        if (
          !settled &&
          job &&
          ['succeeded', 'canceled', 'failed', 'interrupted', 'stopped'].includes(job.state)
        ) {
          settled = true;
          dispose();
          resolveResult(job);
        }
      });
      if (settled) dispose();
    }
    return await result;
  } finally {
    signal?.removeEventListener('abort', abort);
    dispose();
  }
}

export async function subscribeDrop(handler: (paths: string[]) => void): Promise<() => void> {
  if (!isDesktop()) return () => {};
  return getCurrentWebview().onDragDropEvent((event) => {
    if (event.payload.type === 'drop') handler(event.payload.paths);
  });
}
