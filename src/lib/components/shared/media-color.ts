import type { MediaStream } from '$lib/ipc/generated';

export function knownHdr(stream: MediaStream | undefined): boolean {
  return (
    !!stream?.hdrFormat ||
    ['smpte2084', 'arib-std-b67'].includes(stream?.colorTransfer ?? '') ||
    !!stream?.dynamicHdrFormats?.length
  );
}
