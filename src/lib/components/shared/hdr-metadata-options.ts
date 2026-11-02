import type { HdrMetadataSettings, VideoEncoder } from '$lib/ipc/generated';

export function supportsHdrMetadata(encoder: VideoEncoder): boolean {
  return ['svtAv1', 'svtAv1FiveFish', 'svtAv1Hdr', 'aomAv1', 'x265Standalone', 'x265'].includes(
    encoder,
  );
}
export function supportsDynamicHdr(encoder: VideoEncoder): boolean {
  return encoder === 'svtAv1' || encoder === 'svtAv1FiveFish' || encoder === 'svtAv1Hdr';
}
export function validHdrMetadata(value: HdrMetadataSettings | undefined): boolean {
  if (!value) return true;
  const integer = (n: number, max: number) => Number.isInteger(n) && n >= 0 && n <= max;
  const m = value.masteringDisplay;
  return (
    (!m ||
      ([m.red, m.green, m.blue, m.whitePoint].every(
        (p) => integer(p.x, 50000) && integer(p.y, 50000),
      ) &&
        integer(m.maxLuminanceTenThousandths, 0xffffffff) &&
        integer(m.minLuminanceTenThousandths, 0xffffffff) &&
        m.maxLuminanceTenThousandths > m.minLuminanceTenThousandths)) &&
    (!value.contentLight ||
      (integer(value.contentLight.maxContent, 65535) &&
        integer(value.contentLight.maxAverage, 65535)))
  );
}
