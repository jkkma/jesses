// The default output title starts from the source title. When the encode
// changes the picture, release-style tokens such as "2160p" or "DV.HDR10+"
// would describe the source rather than the output, so they are rewritten.

const separators = '[ ._-]';
const boundaryStart = '(?<![A-Za-z0-9])';
const boundaryEnd = '(?![A-Za-z0-9+])';
const resolution = new RegExp(
  `${boundaryStart}(?:(?:4320|2160|1440|1080|720|576|480|360)[pi]|4K|8K|UHD)${boundaryEnd}`,
  'gi',
);
const hdr = new RegExp(
  `${separators}?${boundaryStart}(?:Dolby${separators}?Vision|DoVi|DV|HDR10\\+|HDR10Plus|HDR10|HDR|HLG)${boundaryEnd}`,
  'gi',
);

export type OutputPictureChange = {
  /** HDR is rendered to SDR, so HDR format tokens no longer apply. */
  toneMapped: boolean;
  /** The resize target height, or null when resized to an unknown height. */
  height?: number | null;
};

export function adjustedOutputTitle(title: string, change: OutputPictureChange): string {
  let next = title;
  if (change.height !== undefined) {
    let replaced = false;
    next = next.replace(resolution, () => {
      if (change.height == null || replaced) return '';
      replaced = true;
      return `${change.height}p`;
    });
  }
  if (change.toneMapped) next = next.replace(hdr, '');
  if (next === title) return title;
  return next
    .replace(/([ ._-])[ ._-]+/g, '$1')
    .replace(/[[(]\s*[\])]/g, '')
    .replace(/[ ._-]+(?=[\])]|$)/g, '')
    .replace(/^[ ._-]+/, '')
    .trim();
}
