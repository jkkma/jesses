import type { BorderSettings, CropSettings, MediaStream, VideoFraming } from '$lib/ipc/generated';

// Must match media-runtime's framing validation until these values are exported with the IPC contract.
export const FRAME_MIN_DIMENSION = 2;
export const FRAME_MAX_DIMENSION = 16384;
export const FRAME_MAX_PIXELS = 260_000_000;

export type FramingDraft = {
  crop: { [K in keyof CropSettings]: number | undefined };
  resizeEnabled: boolean;
  resizeWidth: number | undefined;
  resizeMode: 'legacyWidth' | 'fit' | 'width' | 'height' | 'percent' | 'exact';
  resizeHeight: number | undefined;
  resizePercent: number | undefined;
  resizeFill: 'pad' | 'crop' | 'stretch';
  resizeModulus: number | undefined;
  resizeAllowUpscale: boolean;
  resizeCorrectAspect: boolean;
  bordersEnabled: boolean;
  borders: { [K in keyof BorderSettings]: number | undefined };
  borderRatio: string;
  borderRatioWidth: number | undefined;
  borderRatioHeight: number | undefined;
};

export const cropEdges = ['top', 'right', 'bottom', 'left'] as const;

export function defaultFraming(): VideoFraming {
  return {
    crop: { top: 0, right: 0, bottom: 0, left: 0 },
    resizeWidth: null,
    borders: { top: 0, right: 0, bottom: 0, left: 0 },
  };
}

export function defaultFramingDraft(): FramingDraft {
  return {
    crop: { ...defaultFraming().crop },
    resizeEnabled: false,
    resizeWidth: undefined,
    resizeMode: 'legacyWidth',
    resizeHeight: 1080,
    resizePercent: 50,
    resizeFill: 'pad',
    resizeModulus: 2,
    resizeAllowUpscale: false,
    resizeCorrectAspect: true,
    bordersEnabled: false,
    borders: { ...defaultFraming().borders },
    borderRatio: '',
    borderRatioWidth: 16,
    borderRatioHeight: 9,
  };
}

export function copyFramingDraft(draft: FramingDraft): FramingDraft {
  return { ...draft, crop: { ...draft.crop }, borders: { ...draft.borders } };
}

const evenDimension = (value: number) =>
  Number.isInteger(value) &&
  value >= FRAME_MIN_DIMENSION &&
  value <= FRAME_MAX_DIMENSION &&
  value % 2 === 0;
const validEdge = (value: number | undefined) =>
  value !== undefined && Number.isSafeInteger(value) && value >= 0 && value % 2 === 0;

export function framingDimensions(draft: FramingDraft, stream: MediaStream | undefined) {
  if (cropEdges.some((edge) => !validEdge(draft.crop[edge])))
    return { error: 'Crop values must be even whole numbers, zero or greater.' };
  if (draft.bordersEnabled && cropEdges.some((edge) => !validEdge(draft.borders[edge])))
    return { error: 'Border values must be even whole numbers, zero or greater.' };
  if (stream?.width == null || stream.height == null)
    return {
      error: 'Source dimensions are unavailable. Choose a video stream with known dimensions.',
    };
  if (!evenDimension(stream.width) || !evenDimension(stream.height))
    return {
      error: `Source width and height must be even and between ${FRAME_MIN_DIMENSION} and ${FRAME_MAX_DIMENSION} pixels.`,
    };
  const crop = draft.crop as CropSettings;
  let croppedWidth = stream.width - crop.left - crop.right;
  let croppedHeight = stream.height - crop.top - crop.bottom;
  if (!evenDimension(croppedWidth) || !evenDimension(croppedHeight))
    return {
      error: `Cropping must leave an even width and height between ${FRAME_MIN_DIMENSION} and ${FRAME_MAX_DIMENSION} pixels.`,
    };
  const rotated = Math.abs(Number(stream.rotationDegrees ?? 0)) % 180 === 90;
  if (rotated) [croppedWidth, croppedHeight] = [croppedHeight, croppedWidth];
  const [sarNum, sarDen] = (stream.sampleAspectRatio ?? '1:1').split(/[:/]/).map(Number);
  const sourceSar = sarNum > 0 && sarDen > 0 ? (rotated ? sarDen / sarNum : sarNum / sarDen) : 1;
  let pictureWidth = croppedWidth;
  let pictureHeight = croppedHeight;
  let frameWidth = croppedWidth;
  let frameHeight = croppedHeight;
  if (draft.resizeEnabled && draft.resizeMode === 'legacyWidth') {
    if (draft.resizeWidth === undefined || !evenDimension(draft.resizeWidth))
      return {
        error: `Picture width must be an even whole number between ${FRAME_MIN_DIMENSION} and ${FRAME_MAX_DIMENSION} pixels.`,
      };
    pictureWidth = frameWidth = draft.resizeWidth;
    // Matches the encoder: nearest even, ties upward.
    pictureHeight = frameHeight =
      2 * Math.floor((croppedHeight * pictureWidth + croppedWidth) / (2 * croppedWidth));
  } else if (draft.resizeEnabled) {
    const {
      resizeWidth: width,
      resizeHeight: height,
      resizePercent: percent,
      resizeModulus: modulus,
    } = draft;
    if (
      ![width, height].every(
        (value) =>
          Number.isInteger(value) && value! >= FRAME_MIN_DIMENSION && value! <= FRAME_MAX_DIMENSION,
      ) ||
      !Number.isInteger(percent) ||
      percent! < 1 ||
      percent! > 800 ||
      !Number.isInteger(modulus) ||
      modulus! < 2 ||
      modulus! > 256 ||
      modulus! % 2 !== 0
    )
      return {
        error: `Resize targets must be ${FRAME_MIN_DIMENSION}–${FRAME_MAX_DIMENSION} pixels, percentage 1–800, and alignment an even number from 2–256.`,
      };
    const m = modulus!;
    const nearest = (value: number) => Math.max(1, Math.round(value / m)) * m;
    const down = (value: number) => Math.max(1, Math.floor(value / m)) * m;
    const up = (value: number) => Math.max(1, Math.ceil(value / m)) * m;
    const sar = draft.resizeCorrectAspect ? sourceSar : 1;
    const sw = croppedWidth * sar;
    const sh = croppedHeight;
    const aspect = sw / sh;
    const fromWidth = (value: number): [number, number] => {
      const w = nearest(value);
      return [w, nearest(w / aspect)];
    };
    const fromHeight = (value: number): [number, number] => {
      const h = nearest(value);
      return [nearest(h * aspect), h];
    };
    const fit = (bw: number, bh: number, allowUpscale: boolean): [number, number] => {
      const scale = Math.min(bw / sw, bh / sh, allowUpscale ? Infinity : 1);
      let size = bw / sw <= bh / sh ? fromWidth(sw * scale) : fromHeight(sh * scale);
      if (size[0] > bw) size = fromWidth(down(bw));
      if (size[1] > bh) size = fromHeight(down(bh));
      return size;
    };
    let bw = width!;
    let bh = height!;
    if (draft.resizeMode === 'fit' && sh > sw !== bh > bw) [bw, bh] = [bh, bw];
    if (draft.resizeMode === 'fit')
      [pictureWidth, pictureHeight] = fit(bw, bh, draft.resizeAllowUpscale);
    else if (draft.resizeMode === 'width')
      [pictureWidth, pictureHeight] = fromWidth(draft.resizeAllowUpscale ? bw : Math.min(bw, sw));
    else if (draft.resizeMode === 'height')
      [pictureWidth, pictureHeight] = fromHeight(draft.resizeAllowUpscale ? bh : Math.min(bh, sh));
    else if (draft.resizeMode === 'percent')
      [pictureWidth, pictureHeight] =
        sw < sh ? fromWidth((sw * percent!) / 100) : fromHeight((sh * percent!) / 100);
    else {
      bw = nearest(bw);
      bh = nearest(bh);
      if (draft.resizeFill === 'stretch') [pictureWidth, pictureHeight] = [bw, bh];
      else if (draft.resizeFill === 'pad')
        [pictureWidth, pictureHeight] = fit(bw, bh, draft.resizeAllowUpscale);
      else if (bw / sw >= bh / sh) {
        const w = up(bw);
        [pictureWidth, pictureHeight] = [w, Math.max(up(w / aspect), bh)];
      } else {
        const h = up(bh);
        [pictureWidth, pictureHeight] = [Math.max(up(h * aspect), bw), h];
      }
    }
    frameWidth = draft.resizeMode === 'exact' ? bw : pictureWidth;
    frameHeight = draft.resizeMode === 'exact' ? bh : pictureHeight;
  }
  if (!evenDimension(pictureWidth) || !evenDimension(pictureHeight))
    return {
      error: `The resized dimensions must be even and between ${FRAME_MIN_DIMENSION} and ${FRAME_MAX_DIMENSION} pixels.`,
    };
  const borders = draft.bordersEnabled
    ? (draft.borders as BorderSettings)
    : defaultFraming().borders;
  let width = frameWidth;
  let height = frameHeight;
  if (draft.borderRatio) {
    const rw = draft.borderRatioWidth;
    const rh = draft.borderRatioHeight;
    if (![rw, rh].every((value) => Number.isInteger(value) && value! >= 1 && value! <= 10000))
      return { error: 'Border ratio terms must be whole numbers from 1 to 10000.' };
    const pixelAspect = !draft.resizeEnabled ? sourceSar : 1;
    const idealWidth = (height * rw!) / rh! / pixelAspect;
    const idealHeight = (width * pixelAspect * rh!) / rw!;
    if (idealWidth > width) {
      const growth = Math.max(0, Math.round(idealWidth / 2) * 2 - width);
      if (growth >= 4) width += growth;
    } else {
      const growth = Math.max(0, Math.round(idealHeight / 2) * 2 - height);
      if (growth >= 4) height += growth;
    }
  }
  width += borders.left + borders.right;
  height += borders.top + borders.bottom;
  if (!evenDimension(width) || !evenDimension(height))
    return {
      error: `Final width and height including borders must not exceed ${FRAME_MAX_DIMENSION} pixels.`,
    };
  if (width * height > FRAME_MAX_PIXELS)
    return { error: 'The final frame may not exceed 260 million pixels.' };
  const scaledSourceWidth = croppedWidth * (draft.resizeCorrectAspect ? sourceSar : 1);
  const upscaling =
    draft.resizeEnabled &&
    (pictureWidth > scaledSourceWidth + 1 || pictureHeight > croppedHeight + 1);
  return {
    croppedWidth,
    croppedHeight,
    pictureWidth,
    pictureHeight,
    width,
    height,
    upscaling,
    error: null,
  };
}

export function selectedFraming(draft: FramingDraft): VideoFraming {
  return {
    crop: { ...(draft.crop as CropSettings) },
    resizeWidth:
      draft.resizeEnabled && draft.resizeMode === 'legacyWidth' ? draft.resizeWidth! : null,
    ...(draft.resizeEnabled && draft.resizeMode !== 'legacyWidth'
      ? {
          resize: {
            mode: draft.resizeMode,
            width: draft.resizeWidth!,
            height: draft.resizeHeight!,
            percent: draft.resizePercent!,
            fill: draft.resizeFill,
            modulus: draft.resizeModulus!,
            allowUpscale: draft.resizeAllowUpscale,
            correctAspect: draft.resizeCorrectAspect,
          },
        }
      : {}),
    borders: draft.bordersEnabled
      ? { ...(draft.borders as BorderSettings) }
      : defaultFraming().borders,
    ...(draft.borderRatio
      ? { borderRatio: { width: draft.borderRatioWidth!, height: draft.borderRatioHeight! } }
      : {}),
  };
}

export function validFramingDraft(draft: FramingDraft, stream: MediaStream | undefined): boolean {
  // Untouched framing leaves source compatibility to the native validator, so
  // one unsupported source can still receive its own batch preview error.
  return (
    (!draft.resizeEnabled &&
      cropEdges.every((edge) => draft.crop[edge] === 0) &&
      !draft.borderRatio &&
      (!draft.bordersEnabled || cropEdges.every((edge) => draft.borders[edge] === 0))) ||
    !framingDimensions(draft, stream).error
  );
}

export function framingSummary(framing: VideoFraming | undefined): string {
  if (!framing) return 'Source dimensions';
  const cropped = cropEdges.some((edge) => framing.crop[edge] !== 0);
  const parts: string[] = [];
  if (cropped)
    parts.push(
      `Crop top ${framing.crop.top}, right ${framing.crop.right}, bottom ${framing.crop.bottom}, left ${framing.crop.left} px`,
    );
  if (framing.resizeWidth !== null)
    parts.push(`Picture width ${framing.resizeWidth} px · Automatic height`);
  const extended = framing as VideoFraming & {
    resize?: { mode: string; width: number; height: number; percent: number; fill: string };
    borderRatio?: { width: number; height: number };
  };
  if (extended.resize) {
    const resize = extended.resize;
    parts.push(
      resize.mode === 'percent'
        ? `Resize ${resize.percent}%`
        : resize.mode === 'width'
          ? `Width ${resize.width} px`
          : resize.mode === 'height'
            ? `Height ${resize.height} px`
            : `${resize.mode === 'fit' ? 'Fit' : 'Exact'} ${resize.width} × ${resize.height}${resize.mode === 'exact' ? ` · ${resize.fill}` : ''}`,
    );
  }
  if (extended.borderRatio)
    parts.push(`Pad to ${extended.borderRatio.width}:${extended.borderRatio.height}`);
  // Persisted snapshots created before borders were supported omit this field.
  const borders = framing.borders;
  if (borders && cropEdges.some((edge) => borders[edge] !== 0))
    parts.push(
      `Black borders top ${borders.top}, right ${borders.right}, bottom ${borders.bottom}, left ${borders.left} px`,
    );
  return parts.join(' · ') || 'Source dimensions';
}
