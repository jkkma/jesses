import type { MediaStream } from '$lib/ipc/generated';
import { copyFramingDraft, framingDimensions, type FramingDraft } from './framing-options';

const presets: {
  value: string;
  name: string;
  width?: number;
  height?: number;
  percent?: number;
}[] = [
  { value: 'none', name: 'No resizing' },
  { value: '2160p', name: '2160p (4K)', width: 3840, height: 2160 },
  { value: '1440p', name: '1440p', width: 2560, height: 1440 },
  { value: '1080p', name: '1080p (Full HD)', width: 1920, height: 1080 },
  { value: '720p', name: '720p (HD)', width: 1280, height: 720 },
  { value: '576p', name: '576p', width: 1024, height: 576 },
  { value: '480p', name: '480p', width: 854, height: 480 },
  { value: '360p', name: '360p', width: 640, height: 360 },
  { value: '75', name: '75% of the source', percent: 75 },
  { value: '50', name: '50% of the source', percent: 50 },
  { value: '25', name: '25% of the source', percent: 25 },
  { value: 'custom', name: 'Custom…' },
];
const known: [number, string][] = [
  [1, '1:1'],
  [5 / 4, '5:4'],
  [4 / 3, '4:3'],
  [1.43, '1.43:1'],
  [3 / 2, '3:2'],
  [16 / 10, '16:10'],
  [5 / 3, '1.66:1'],
  [16 / 9, '16:9'],
  [1.85, '1.85:1'],
  [1.9, '1.90:1'],
  [2, '2:1'],
  [2.2, '2.20:1'],
  [64 / 27, '21:9'],
  [2.35, '2.35:1'],
  [2.39, '2.39:1'],
  [2.76, '2.76:1'],
  [32 / 9, '32:9'],
];
const names: Record<string, string> = {
  '1:1': 'Square',
  '4:3': 'Standard',
  '1.43:1': 'IMAX film',
  '3:2': '35mm photo',
  '1.66:1': 'European widescreen',
  '16:9': 'Widescreen',
  '1.85:1': 'US widescreen',
  '1.90:1': 'IMAX Digital',
  '2:1': 'Univisium',
  '2.20:1': '70mm',
  '21:9': 'Ultrawide',
  '2.35:1': 'CinemaScope',
  '2.39:1': 'Anamorphic scope',
  '2.76:1': 'Ultra Panavision',
  '32:9': 'Super ultrawide',
};
function aspect(width: number, height: number, withName = false) {
  if (width <= 0 || height <= 0) return '';
  const portrait = height > width,
    value = portrait ? height / width : width / height;
  const nearest = known.reduce((best, row) =>
    Math.abs(row[0] - value) < Math.abs(best[0] - value) ? row : best,
  );
  if (Math.abs(nearest[0] - value) / value <= 0.015) {
    const label = portrait ? nearest[1].split(':').reverse().join(':') : nearest[1];
    return `${Math.abs(nearest[0] - value) / value > 0.005 ? '≈' : ''}${label}${withName && names[nearest[1]] ? ` (${names[nearest[1]]})` : ''}`;
  }
  let a = width,
    b = height;
  while (b) {
    [a, b] = [b, a % b];
  }
  const w = width / (a || 1),
    h = height / (a || 1);
  if (Math.max(w, h) <= 64) return `${w}:${h}`;
  return portrait
    ? `1:${Number((height / width).toFixed(2))}`
    : `${Number((width / height).toFixed(2))}:1`;
}
function sourceSize(draft: FramingDraft, stream: MediaStream) {
  const crop = draft.crop;
  return {
    width: stream.width! - (crop.left ?? 0) - (crop.right ?? 0),
    height: stream.height! - (crop.top ?? 0) - (crop.bottom ?? 0),
  };
}
function sar(stream: MediaStream | undefined) {
  const [n, d] = (stream?.sampleAspectRatio ?? '1:1').split(/[:/]/).map(Number);
  return n > 0 && d > 0 ? { n, d, value: n / d } : { n: 1, d: 1, value: 1 };
}
export function resizeSourceDescription(draft: FramingDraft, stream: MediaStream | undefined) {
  if (!stream?.width || !stream.height) return 'No video track loaded.';
  const input = sourceSize(draft, stream),
    shape = sar(stream),
    displayWidth = Math.round(input.width * shape.value),
    ratio = aspect(displayWidth, input.height, true);
  return shape.n === shape.d
    ? `${input.width}x${input.height} — ${ratio}`
    : `${input.width}x${input.height} stored, shown as ${displayWidth}x${input.height} — ${ratio}, with ${shape.n}:${shape.d} pixels`;
}
function noBorders(draft: FramingDraft) {
  return { ...copyFramingDraft(draft), borderRatio: '', bordersEnabled: false };
}
function target(draft: FramingDraft) {
  switch (draft.resizeMode) {
    case 'fit':
      return `Fit ${draft.resizeWidth}x${draft.resizeHeight}`;
    case 'width':
      return `${draft.resizeWidth}px wide`;
    case 'height':
      return `${draft.resizeHeight}p`;
    case 'percent':
      return `${draft.resizePercent}%`;
    case 'exact':
      return `${draft.resizeWidth}x${draft.resizeHeight}`;
    default:
      return 'Disabled';
  }
}
export function resizePresetLabels(draft: FramingDraft, stream: MediaStream | undefined) {
  return presets.map((p) => {
    if (p.value === 'none' || p.value === 'custom') return p;
    if (!stream?.width || !stream.height)
      return { ...p, name: p.width ? `${p.name} — fits ${p.width}x${p.height}` : p.name };
    const base = noBorders(draft);
    const next = {
      ...base,
      resizeEnabled: true,
      resizeMode: p.percent ? ('percent' as const) : ('fit' as const),
      resizeWidth: p.width ?? 1920,
      resizeHeight: p.height ?? 1080,
      resizePercent: p.percent ?? 50,
      resizeFill: 'pad' as const,
      resizeModulus: 2,
      resizeAllowUpscale: !!p.width,
      resizeCorrectAspect: true,
    };
    const result = framingDimensions(next, stream),
      input = sourceSize(base, stream);
    return {
      ...p,
      name:
        result.error || !result.width
          ? p.name
          : `${p.name} — ${result.width === input.width && result.height === input.height ? 'no change' : `${result.width}x${result.height}`}`,
    };
  });
}
export function geometryReadouts(
  draft: FramingDraft,
  stream: MediaStream | undefined,
  hasFile: boolean,
  automaticCrop: boolean,
  chunked: boolean,
) {
  const shape = sar(stream),
    valid = !!(stream?.width && stream.height),
    input = valid ? sourceSize(draft, stream!) : null;
  const base = noBorders(draft),
    result = valid ? framingDimensions(base, stream) : null;
  let resize: string;
  if (!draft.resizeEnabled) {
    if (input && shape.n !== shape.d) {
      if (chunked) {
        const width = Math.round((input.width * shape.value) / 2) * 2;
        resize = `${width}x${input.height} · ${aspect(width, input.height)} · de-squeezed from ${input.width}x${input.height}, whose pixels are ${shape.n}:${shape.d} - the encoder cannot keep the anamorphic flag`;
      } else {
        const width = Math.round(input.width * shape.value);
        resize = `${input.width}x${input.height} · kept as it is, with its ${shape.n}:${shape.d} pixel shape - so it still plays as ${aspect(width, input.height)}`;
      }
    } else resize = 'The source is encoded at its own resolution.';
  } else if (hasFile && !stream) resize = 'No video track - the resize will be skipped.';
  else if (!input)
    resize = `${target(draft)} - the size is worked out per file when the encode starts.`;
  else if (!result || result.error || !result.width || !result.height)
    resize = result?.error ?? 'Nothing configured yet - press Configure… to set a target.';
  else {
    resize = `${result.width}x${result.height} · ${aspect(result.width, result.height)}`;
    const upscale = 'larger than the source - upscaling invents no detail and costs bitrate';
    let note = '';
    const nonSquare = shape.n !== shape.d;
    if (!draft.resizeCorrectAspect && nonSquare) {
      note = `the ${shape.n}:${shape.d} pixel shape is dropped, so this plays as ${aspect(result.width, result.height)} rather than ${aspect(Math.round(input.width * shape.value), input.height)}`;
    } else if (draft.resizeMode === 'exact' && draft.resizeFill === 'stretch') {
      const sourceAspect =
          (input.width * (draft.resizeCorrectAspect ? shape.value : 1)) / input.height,
        distortion = result.width / result.height / sourceAspect,
        amount = (distortion > 1 ? distortion : 1 / distortion) - 1;
      if (amount > 0.005)
        note = `stretched ${distortion > 1 ? 'horizontally' : 'vertically'} by ${Number((amount * 100).toFixed(1))}%`;
    }
    if (!note && draft.resizeCorrectAspect && nonSquare)
      note = `de-squeezed from ${input.width}x${input.height}, whose pixels are ${shape.n}:${shape.d}${result.upscaling ? `, and ${upscale}` : ''}`;
    else if (!note && result.width === input.width && result.height === input.height)
      note = 'already this size, so it is left alone';
    else if (!note && result.upscaling) note = upscale;
    else if (
      !note &&
      !draft.resizeAllowUpscale &&
      draft.resizeMode === 'fit' &&
      (draft.resizeWidth ?? 0) > input.width * shape.value &&
      (draft.resizeHeight ?? 0) > input.height
    )
      note = 'the source is smaller than the target, and upscaling is off';
    if (note) resize += ` · ${note}`;
    if (automaticCrop)
      resize += ' · before autocrop; the final size is measured when the encode starts';
  }
  let border: string;
  if (!draft.borderRatio) border = 'The frame keeps whatever shape it has.';
  else if (hasFile && !stream) border = 'No video track - no bars will be added.';
  else if (!input || !result?.width || !result.height)
    border = `Padded out to ${draft.borderRatio} - the bars are worked out per file when the encode starts.`;
  else {
    let width = result.width,
      height = result.height,
      pixelAspect = draft.resizeEnabled ? 1 : shape.value;
    if (chunked && !draft.resizeEnabled && shape.n !== shape.d) {
      width = Math.round((width * shape.value) / 2) * 2;
      pixelAspect = 1;
    }
    const targetRatio = (draft.borderRatioWidth ?? 1) / (draft.borderRatioHeight ?? 1),
      current = (width * pixelAspect) / height;
    let frameWidth = width,
      frameHeight = height;
    if (current < targetRatio) {
      const growth = Math.round((targetRatio * height) / pixelAspect / 2) * 2 - width;
      if (growth >= 4) frameWidth += growth;
    } else if (current > targetRatio) {
      const growth = Math.round((width * pixelAspect) / targetRatio / 2) * 2 - height;
      if (growth >= 4) frameHeight += growth;
    }
    const add = frameWidth - width || frameHeight - height;
    if (frameWidth * frameHeight > 260_000_000)
      border = `${frameWidth}x${frameHeight} · too large to encode - FFmpeg will not produce a frame of ${Number(((frameWidth * frameHeight) / 1_000_000).toFixed(1))} megapixels, so nothing would be written`;
    else if (!add)
      border = `${width}x${height} · already ${draft.borderRatio}, so no bars are added`;
    else {
      const near = Math.floor(add / 4) * 2,
        far = add - near,
        bars = near === far ? `${near}px` : `${near}px and ${far}px`,
        where = frameWidth > width ? 'down the left and right' : 'along the top and bottom';
      border = `${frameWidth}x${frameHeight} · ${aspect(frameWidth, frameHeight)} · ${bars} black bars ${where}, around an unscaled ${width}x${height} picture`;
    }
    if (automaticCrop) border += ' · before autocrop; the bars are measured when the encode starts';
  }
  return { resize, border };
}

export function resizeDialogDescription(draft: FramingDraft, stream: MediaStream | undefined) {
  if (!stream?.width || !stream.height)
    return `${target(draft)} — the size is worked out per file when the encode starts.`;
  const result = framingDimensions(noBorders(draft), stream),
    line = geometryReadouts(draft, stream, true, false, false).resize;
  if (result.error || !result.width || !result.height) return result.error ?? '-';
  return line.replace(
    `${result.width}x${result.height} · ${aspect(result.width, result.height)}`,
    `${result.width}x${result.height} — ${aspect(result.width, result.height, true)}`,
  );
}
