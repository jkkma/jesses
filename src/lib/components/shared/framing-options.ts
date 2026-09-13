import type { BorderSettings, CropSettings, MediaStream, VideoFraming } from '$lib/ipc/generated';

export type FramingDraft = {
  crop: { [K in keyof CropSettings]: number | undefined };
  resizeEnabled: boolean;
  resizeWidth: number | undefined;
  bordersEnabled: boolean;
  borders: { [K in keyof BorderSettings]: number | undefined };
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
    bordersEnabled: false,
    borders: { ...defaultFraming().borders },
  };
}

export function copyFramingDraft(draft: FramingDraft): FramingDraft {
  return { ...draft, crop: { ...draft.crop }, borders: { ...draft.borders } };
}

const evenDimension = (value: number) =>
  Number.isInteger(value) && value >= 64 && value <= 8192 && value % 2 === 0;
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
    return { error: 'Source width and height must be even and between 64 and 8192 pixels.' };
  const crop = draft.crop as CropSettings;
  const croppedWidth = stream.width - crop.left - crop.right;
  const croppedHeight = stream.height - crop.top - crop.bottom;
  if (!evenDimension(croppedWidth) || !evenDimension(croppedHeight))
    return { error: 'Cropping must leave an even width and height between 64 and 8192 pixels.' };
  const pictureWidth = draft.resizeEnabled ? draft.resizeWidth : croppedWidth;
  if (pictureWidth === undefined || !evenDimension(pictureWidth))
    return { error: 'Picture width must be an even whole number between 64 and 8192 pixels.' };
  // Matches the integer rounding used by the encoder: nearest even, ties upward.
  const pictureHeight = draft.resizeEnabled
    ? 2 * Math.floor((croppedHeight * pictureWidth + croppedWidth) / (2 * croppedWidth))
    : croppedHeight;
  if (!evenDimension(pictureHeight))
    return {
      error:
        'Automatic picture height must be between 64 and 8192 pixels. Adjust the width or crop.',
    };
  const borders = draft.bordersEnabled
    ? (draft.borders as BorderSettings)
    : defaultFraming().borders;
  const width = pictureWidth + borders.left + borders.right;
  const height = pictureHeight + borders.top + borders.bottom;
  if (!evenDimension(width) || !evenDimension(height))
    return { error: 'Final width and height including borders must not exceed 8192 pixels.' };
  return { croppedWidth, croppedHeight, pictureWidth, pictureHeight, width, height, error: null };
}

export function selectedFraming(draft: FramingDraft): VideoFraming {
  return {
    crop: { ...(draft.crop as CropSettings) },
    resizeWidth: draft.resizeEnabled ? draft.resizeWidth! : null,
    borders: draft.bordersEnabled
      ? { ...(draft.borders as BorderSettings) }
      : defaultFraming().borders,
  };
}

export function validFramingDraft(draft: FramingDraft, stream: MediaStream | undefined): boolean {
  // Untouched framing leaves source compatibility to the native validator, so
  // one unsupported source can still receive its own batch preview error.
  return (
    (!draft.resizeEnabled &&
      cropEdges.every((edge) => draft.crop[edge] === 0) &&
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
  // Persisted snapshots created before borders were supported omit this field.
  const borders = framing.borders;
  if (borders && cropEdges.some((edge) => borders[edge] !== 0))
    parts.push(
      `Black borders top ${borders.top}, right ${borders.right}, bottom ${borders.bottom}, left ${borders.left} px`,
    );
  return parts.join(' · ') || 'Source dimensions';
}
