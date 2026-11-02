import type { HdrMetadataSettings } from '$lib/ipc/generated';
export type RawHdr = Record<string, string>;
export function parseSourceHdr(raw: RawHdr): { value?: HdrMetadataSettings; error?: string } {
  const value: HdrMetadataSettings = {};
  if (raw['master-display']?.trim()) {
    const m =
      /^G\((\d+),(\d+)\)B\((\d+),(\d+)\)R\((\d+),(\d+)\)WP\((\d+),(\d+)\)L\((\d+),(\d+)\)$/i.exec(
        raw['master-display'].trim(),
      );
    if (!m) return { error: 'Use master-display G(x,y)B(x,y)R(x,y)WP(x,y)L(max,min).' };
    const n = m.slice(1).map(Number);
    if (
      n.slice(0, 8).some((x) => x > 50000) ||
      n[8] > 0xffffffff ||
      n[9] > 0xffffffff ||
      n[8] <= n[9]
    )
      return { error: 'Mastering display coordinates or luminance are out of range.' };
    value.masteringDisplay = {
      green: { x: n[0], y: n[1] },
      blue: { x: n[2], y: n[3] },
      red: { x: n[4], y: n[5] },
      whitePoint: { x: n[6], y: n[7] },
      maxLuminanceTenThousandths: n[8],
      minLuminanceTenThousandths: n[9],
    };
  }
  if (raw['max-cll']?.trim()) {
    const m = /^(\d+),(\d+)$/.exec(raw['max-cll'].trim());
    if (!m || +m[1] > 65535 || +m[2] > 65535)
      return { error: 'Use max-cll as MaxCLL,MaxFALL (0–65535).' };
    value.contentLight = { maxContent: +m[1], maxAverage: +m[2] };
  }
  if (raw['dolby-vision-rpu']?.trim()) value.dolbyVisionRpuPath = raw['dolby-vision-rpu'].trim();
  if (raw['hdr10plus-json']?.trim()) value.hdr10PlusJsonPath = raw['hdr10plus-json'].trim();
  return Object.keys(value).length ? { value } : {};
}
