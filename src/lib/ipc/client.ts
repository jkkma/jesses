import { invoke, isTauri } from '@tauri-apps/api/core';
import { getCurrentWebview } from '@tauri-apps/api/webview';
import { open } from '@tauri-apps/plugin-dialog';
import type { EncodeJob, EncodeRequest, MediaFile, ToolInfo } from './generated';

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

export async function getCapabilities(): Promise<ToolInfo[]> {
  requireDesktop();
  return invoke<ToolInfo[]>('get_capabilities');
}

export async function chooseOutputDirectory(defaultPath?: string): Promise<string | null> {
  requireDesktop();
  const path = await open({
    multiple: false,
    directory: true,
    title: 'Choose encode destination',
    ...(defaultPath ? { defaultPath } : {}),
  });
  return Array.isArray(path) ? (path[0] ?? null) : path;
}

export async function startEncode(request: EncodeRequest): Promise<EncodeJob> {
  requireDesktop();
  return invoke<EncodeJob>('start_encode', { request });
}

export async function listJobs(): Promise<EncodeJob[]> {
  requireDesktop();
  return invoke<EncodeJob[]>('list_jobs');
}

export async function cancelEncode(id: string): Promise<void> {
  requireDesktop();
  return invoke<void>('cancel_encode', { id });
}

export async function subscribeDrop(handler: (paths: string[]) => void): Promise<() => void> {
  if (!isDesktop()) return () => {};
  return getCurrentWebview().onDragDropEvent((event) => {
    if (event.payload.type === 'drop') handler(event.payload.paths);
  });
}
