import type { MediaStream } from '$lib/ipc/generated';

type PreviewStream = Pick<MediaStream, 'frameRate'> & { durationSeconds?: number | null };

/** Keep a requested frame before the selected stream's reported end. */
export function previewMaximum(
  fileDurationSeconds: number | null,
  stream: PreviewStream | undefined,
): number {
  const duration = stream?.durationSeconds ?? fileDurationSeconds;
  if (duration === null || !Number.isFinite(duration)) return 86400;
  const [numerator, denominator] = (stream?.frameRate ?? '').split('/').map(Number);
  const frameSeconds =
    numerator > 0 && denominator > 0 && Number.isFinite(denominator / numerator)
      ? denominator / numerator
      : 0.05;
  return Math.max(0, Math.min(86400, duration - frameSeconds));
}
