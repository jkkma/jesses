import type { MediaStream } from '$lib/ipc/generated';

export function knownHdr(stream: MediaStream | undefined): boolean {
  return (
    !!stream?.hdrFormat ||
    ['smpte2084', 'arib-std-b67'].includes(stream?.colorTransfer ?? '') ||
    !!stream?.dynamicHdrFormats?.length
  );
}

export function preservableHdr10(stream: MediaStream | undefined): boolean {
  return (
    stream?.pixelFormat === 'yuv420p10le' &&
    stream.colorRange === 'tv' &&
    stream.colorPrimaries === 'bt2020' &&
    stream.colorTransfer === 'smpte2084' &&
    stream.colorSpace === 'bt2020nc'
  );
}
