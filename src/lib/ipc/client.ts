import { Channel, invoke, isTauri } from '@tauri-apps/api/core';
import { getCurrentWebview } from '@tauri-apps/api/webview';
import { open, save } from '@tauri-apps/plugin-dialog';
import type {
  Av1anResourceRequest,
  Av1anResourceEstimate,
  ImageRequest,
  ImageResult,
  UtilityRequest,
  UtilityResult,
  UtilityCapabilities,
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
export function runUtility(request: UtilityRequest, signal?: AbortSignal): Promise<UtilityResult> {
  return analyzeSource('run_utility', request, signal);
}
export function inspectUtilityCapabilities(signal?: AbortSignal): Promise<UtilityCapabilities> {
  return analyzeSource('inspect_utility_capabilities', {}, signal);
}
export async function chooseImages(): Promise<string[]> {
  requireDesktop();
  const result = await open({
    multiple: true,
    directory: false,
    title: 'Choose images in sequence',
    filters: [{ name: 'Images', extensions: ['png', 'jpg', 'jpeg', 'bmp', 'tiff', 'tif', 'webp'] }],
  });
  return result === null ? [] : Array.isArray(result) ? result : [result];
}
export async function chooseUtilityDestination(
  defaultPath: string,
  extensions: string[],
): Promise<string | null> {
  requireDesktop();
  return save({ title: 'Save new output', defaultPath, filters: [{ name: 'Output', extensions }] });
}
export async function chooseUtilityFile(
  title: string,
  extensions: string[],
): Promise<string | null> {
  requireDesktop();
  const result = await open({
    title,
    multiple: false,
    directory: false,
    filters: [{ name: 'Input', extensions }],
  });
  return Array.isArray(result) ? (result[0] ?? null) : result;
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
export async function inspectSavedJob(path: string): Promise<SavedJobInspection> {
  requireDesktop();
  return invoke('inspect_saved_job', { path });
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
): Promise<T> {
  requireDesktop();
  if (signal?.aborted) throw new DOMException('Source inspection was canceled.', 'AbortError');
  const id = await invoke<string>('begin_media_analysis');
  const cancel = () => {
    void invoke('cancel_media_analysis', { id }).catch(() => {});
  };
  signal?.addEventListener('abort', cancel, { once: true });
  try {
    if (signal?.aborted) throw new DOMException('Source inspection was canceled.', 'AbortError');
    const result = await invoke<T>(command, { id, request });
    if (signal?.aborted) throw new DOMException('Source inspection was canceled.', 'AbortError');
    return result;
  } finally {
    signal?.removeEventListener('abort', cancel);
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
): Promise<QualityResult> {
  return analyzeSource('analyze_quality', request, signal);
}

export function analyzeBitrate(
  request: BitrateRequest,
  signal?: AbortSignal,
): Promise<BitrateResult> {
  return analyzeSource('analyze_bitrate', request, signal);
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
  const paths = await open({
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
  return paths === null ? [] : Array.isArray(paths) ? paths : [paths];
}

export async function probeMedia(path: string): Promise<MediaFile> {
  requireDesktop();
  return invoke<MediaFile>('probe_media', { path });
}

export async function chooseMediaFolder(): Promise<string | null> {
  requireDesktop();
  return open({ directory: true, multiple: false, title: 'Add media folder to jesses' });
}

export async function chooseOutputFolder(): Promise<string | null> {
  requireDesktop();
  return open({ directory: true, multiple: false, title: 'Choose batch output folder' });
}

export async function scanMediaFolder(request: FolderScanRequest): Promise<FolderScanResult> {
  requireDesktop();
  return invoke<FolderScanResult>('scan_media_folder', { request });
}

export async function previewEncodeBatch(request: BatchEncodeRequest): Promise<BatchEncodePreview> {
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
    filters: [{ name: 'Media containers', extensions: ['mkv', 'mp4', 'mov', 'webm'] }],
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

export async function discardAv1anRecovery(id: string): Promise<JobSnapshot> {
  requireDesktop();
  return invoke<JobSnapshot>('discard_av1an_recovery', { id });
}

export async function subscribeJobs(handler: (jobs: JobSnapshot[]) => void): Promise<() => void> {
  requireDesktop();
  let disposed = false;
  const channel = new Channel<JobSnapshot[]>();
  channel.onmessage = (jobs) => {
    if (!disposed) handler(jobs);
  };
  await invoke('subscribe_jobs', { channel });
  return () => {
    disposed = true;
  };
}

export async function subscribeDrop(handler: (paths: string[]) => void): Promise<() => void> {
  if (!isDesktop()) return () => {};
  return getCurrentWebview().onDragDropEvent((event) => {
    if (event.payload.type === 'drop') handler(event.payload.paths);
  });
}
