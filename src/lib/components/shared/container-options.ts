import type { ContainerFormat } from '$lib/ipc/generated';

export function destinationContainer(path: string): ContainerFormat {
  const extension = path.split('.').pop()?.toLowerCase();
  return extension === 'mp4' || extension === 'mov' || extension === 'webm'
    ? extension
    : 'matroska';
}

export function containerDestination(path: string, container: ContainerFormat): string {
  if (!path) return path;
  const extension = container === 'matroska' ? 'mkv' : container;
  return path.replace(/\.[^./\\]+$/, '') + '.' + extension;
}
