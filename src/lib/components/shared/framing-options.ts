import type { CropSettings, MediaStream, VideoFraming } from '$lib/ipc/generated';

export type FramingDraft = {
  crop: { [K in keyof CropSettings]: number | undefined };
  resizeEnabled: boolean;
  resizeWidth: number | undefined;
};

export const cropEdges = ['top', 'right', 'bottom', 'left'] as const;

export function defaultFraming(): VideoFraming {
  return { crop: { top: 0, right: 0, bottom: 0, left: 0 }, resizeWidth: null };
}

export function defaultFramingDraft(): FramingDraft {
  return { crop: { ...defaultFraming().crop }, resizeEnabled: false, resizeWidth: undefined };
}

export function copyFramingDraft(draft: FramingDraft): FramingDraft {
  return { ...draft, crop: { ...draft.crop } };
}

const evenDimension = (value: number) =>
  Number.isInteger(value) && value >= 64 && value <= 8192 && value % 2 === 0;

export function framingDimensions(draft: FramingDraft, stream: MediaStream | undefined) {
  if (
    cropEdges.some((edge) => {
      const value = draft.crop[edge];
      return value === undefined || !Number.isInteger(value) || value < 0 || value % 2 !== 0;
    })
  )
    return { error: 'Crop values must be even whole numbers, zero or greater.' };
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
  const width = draft.resizeEnabled ? draft.resizeWidth : croppedWidth;
  if (width === undefined || !evenDimension(width))
    return { error: 'Output width must be an even whole number between 64 and 8192 pixels.' };
  // Matches the integer rounding used by the encoder: nearest even, ties upward.
  const height = draft.resizeEnabled
    ? 2 * Math.floor((croppedHeight * width + croppedWidth) / (2 * croppedWidth))
    : croppedHeight;
  if (!evenDimension(height))
    return {
      error:
        'Automatic output height must be between 64 and 8192 pixels. Adjust the width or crop.',
    };
  return { croppedWidth, croppedHeight, width, height, error: null };
}

export function selectedFraming(draft: FramingDraft): VideoFraming {
  return {
    crop: { ...(draft.crop as CropSettings) },
    resizeWidth: draft.resizeEnabled ? draft.resizeWidth! : null,
  };
}

export function validFramingDraft(draft: FramingDraft, stream: MediaStream | undefined): boolean {
  // Untouched framing leaves source compatibility to the native validator, so
  // one unsupported source can still receive its own batch preview error.
  return (
    (!draft.resizeEnabled && cropEdges.every((edge) => draft.crop[edge] === 0)) ||
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
    parts.push(`Width ${framing.resizeWidth} px · Automatic height`);
  return parts.join(' · ') || 'Source dimensions';
}
