import type { Av1anPixelFormat, MediaStream } from '$lib/ipc/generated';

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
    (stream.colorRange == null || ['tv', 'unknown', 'unspecified'].includes(stream.colorRange)) &&
    stream.colorPrimaries === 'bt2020' &&
    stream.colorTransfer === 'smpte2084' &&
    stream.colorSpace === 'bt2020nc'
  );
}

export function hdrOutputError(
  stream: MediaStream | undefined,
  supportsHdr10: boolean,
  pixelFormat: Av1anPixelFormat | undefined,
): string | null {
  if (!knownHdr(stream)) return null;
  if (stream?.colorTransfer === 'arib-std-b67')
    return 'Choose a Tone Mapping curve to convert HLG to SDR.';
  if (stream?.dolbyVisionProfile === 5)
    return 'Enable GPU Tone Mapping to render Dolby Vision profile 5 to SDR.';
  if (!supportsHdr10)
    return 'The selected encoder supports SDR output only. Enable Tone Mapping or choose an HDR10 encoder.';
  if (!preservableHdr10(stream))
    return 'Keeping HDR requires a limited-range 10-bit 4:2:0 BT.2020/PQ source. Enable Tone Mapping if this source supports it.';
  if (pixelFormat !== undefined && pixelFormat !== 'yuv420p10le')
    return 'HDR10 output requires 10-bit 4:2:0. Choose a 10-bit Color Format or enable Tone Mapping.';
  return null;
}

export function av1anDynamicHdrError(
  stream: MediaStream | undefined,
  supportsFallback: boolean,
  hdr10Fallback: boolean,
): string | null {
  const dynamic = stream?.dolbyVisionProfile != null || !!stream?.dynamicHdrFormats?.length;
  if (!dynamic) return null;
  if (!supportsFallback)
    return 'Choose an SVT-AV1 encoder and enable HDR10 fallback to discard Dolby Vision / HDR10+, or enable Tone Mapping.';
  if (!hdr10Fallback)
    return 'Enable HDR10 fallback to discard Dolby Vision / HDR10+, or enable Tone Mapping.';
  return null;
}
