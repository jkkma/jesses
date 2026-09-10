import { invoke, isTauri } from '@tauri-apps/api/core';
import { getCurrentWebview } from '@tauri-apps/api/webview';
import { open } from '@tauri-apps/plugin-dialog';
import type { MediaFile, ToolInfo } from './generated';

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

export async function subscribeDrop(handler: (paths: string[]) => void): Promise<() => void> {
  if (!isDesktop()) return () => {};
  return getCurrentWebview().onDragDropEvent((event) => {
    if (event.payload.type === 'drop') handler(event.payload.paths);
  });
}
