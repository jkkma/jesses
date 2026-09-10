import { Channel, invoke, isTauri } from '@tauri-apps/api/core';
import { getCurrentWebview } from '@tauri-apps/api/webview';
import { open, save } from '@tauri-apps/plugin-dialog';
import type {
  BatchEncodePreview,
  BatchEncodeRequest,
  EncodeRequest,
  FolderScanRequest,
  FolderScanResult,
  JobSnapshot,
  MediaFile,
  RemuxRequest,
  ToolInfo,
} from './generated';

export function isDesktop(): boolean {
  return isTauri();
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
    filters: [{ name: 'Matroska', extensions: ['mkv'] }],
  });
}

export async function startRemux(request: RemuxRequest): Promise<JobSnapshot> {
  requireDesktop();
  return invoke<JobSnapshot>('start_remux', { request });
}

export async function chooseEncodeDestination(defaultPath: string): Promise<string | null> {
  requireDesktop();
  return save({
    title: 'Save AV1 encode',
    defaultPath,
    filters: [{ name: 'Matroska', extensions: ['mkv'] }],
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
