export function formatDuration(seconds: number | null): string {
  if (seconds === null || !Number.isFinite(seconds)) return '—';
  const total = Math.max(0, Math.round(seconds));
  const hours = Math.floor(total / 3600);
  const minutes = Math.floor((total % 3600) / 60);
  const remainder = total % 60;
  return `${hours.toString().padStart(2, '0')}:${minutes.toString().padStart(2, '0')}:${remainder.toString().padStart(2, '0')}`;
}

export function formatBytes(bytes: string): string {
  const value = Number(bytes);
  if (!Number.isFinite(value) || value < 0) return '—';
  if (value === 0) return '0 B';
  const units = ['B', 'KB', 'MB', 'GB', 'TB'];
  const power = Math.min(Math.floor(Math.log(value) / Math.log(1024)), units.length - 1);
  return `${(value / 1024 ** power).toLocaleString(undefined, { maximumFractionDigits: power < 2 ? 0 : 2 })} ${units[power]}`;
}

export function displayCodec(codec: string | null): string {
  return codec ? codec.toUpperCase() : 'Unknown';
}

export function formatFrameRate(value: string | null): string {
  if (!value) return '—';
  const [numerator, denominator = '1'] = value.split('/');
  const rate = Number(numerator) / Number(denominator);
  return Number.isFinite(rate) && rate > 0
    ? rate.toLocaleString(undefined, { maximumFractionDigits: 3 })
    : value;
}

export function errorMessage(error: unknown): string {
  if (typeof error === 'string') return error;
  if (error instanceof Error) return error.message;
  if (error && typeof error === 'object' && 'message' in error) return String(error.message);
  return 'An unexpected error occurred.';
}

export function fileName(path: string): string {
  return path.split(/[\\/]/).filter(Boolean).at(-1) ?? path;
}
