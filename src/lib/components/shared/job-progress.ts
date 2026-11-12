import type { JobSnapshot } from '$lib/ipc/generated';
import type { ProgressEstimate } from './progress-estimate';

// Tool output is bracket-prefixed ("[consumer] Svt[info]: ...") or indented
// ("    Last message repeated"); bookkeeping lines name files, not progress.
const toolOutput = /^(?:\s|\[)/;
const bookkeeping = /^(?:Tool|Owned temporary [^:]+|Mux log|[\w -]+ log): /;
const pauseControl = /^(?:Paused|Continued) the media job process trees\./;

/** The newest step the job reported, such as "Fingerprinting the source...". */
export function latestStep(logs: readonly string[]): string | null {
  for (let index = logs.length - 1; index >= 0; index -= 1) {
    const line = logs[index];
    if (!line.trim() || toolOutput.test(line) || bookkeeping.test(line) || pauseControl.test(line))
      continue;
    return line.trim();
  }
  return null;
}

/** m:ss, or h:mm:ss from one hour. */
export function clock(seconds: number): string {
  const total = Math.max(0, Math.round(seconds));
  const hours = Math.floor(total / 3600);
  const minutes = Math.floor((total % 3600) / 60);
  const rest = String(total % 60).padStart(2, '0');
  return hours ? `${hours}:${String(minutes).padStart(2, '0')}:${rest}` : `${minutes}:${rest}`;
}

export type LiveJobProgress = {
  text: string;
  /** Media seconds done, or null while the phase has no measurable progress. */
  value: number | null;
  max: number;
};

/**
 * What the status bar shows for a live job. Preparation and final checks can
 * take minutes without a frame count, so they show their current step and an
 * indeterminate bar instead of an empty one.
 */
export function liveJobProgress(
  job: JobSnapshot,
  estimate: ProgressEstimate | null,
): LiveJobProgress {
  const av1an = job.encodeSettings?.backend === 'av1an';
  const task = av1an ? 'AV1AN encode' : job.encodeSettings ? 'encode' : 'task';
  const duration =
    job.durationSeconds != null && Number.isFinite(job.durationSeconds) && job.durationSeconds > 0
      ? job.durationSeconds
      : null;
  const progress =
    job.progressSeconds != null && Number.isFinite(job.progressSeconds)
      ? Math.max(0, Math.min(job.progressSeconds, duration ?? Infinity))
      : null;
  const measurable =
    duration !== null && progress !== null && (job.state === 'running' || progress > 0);
  const percent = measurable ? Math.floor((progress! / duration!) * 100) : null;
  const position = measurable ? ` · ${clock(progress!)} of ${clock(duration!)}` : '';
  const step = latestStep(job.logs ?? []);
  const value = measurable ? progress : null;
  const max = duration ?? 1;
  switch (job.state) {
    case 'queued':
      return { text: `Queued: ${task}...`, value: null, max };
    case 'paused':
      return {
        text:
          percent === null && step
            ? `Paused processes; file checks may continue: ${step}`
            : `Paused${percent !== null ? ` at ${percent}%` : ''}: ${task}`,
        value: progress,
        max,
      };
    case 'running': {
      const label = av1an ? 'Encoding (AV1AN)' : job.encodeSettings ? 'Encoding' : 'Running';
      const rate =
        estimate?.kind === 'estimate'
          ? ` · ${estimate.speed.toFixed(2)}× realtime${
              estimate.remainingSeconds !== null
                ? ` · about ${clock(estimate.remainingSeconds)} left`
                : ''
            }`
          : estimate?.kind === 'waiting'
            ? ' · waiting for progress'
            : '';
      return {
        text:
          percent !== null
            ? `${label} ${percent}%${position}${rate}`
            : `${label}: ${step ?? '...'}`,
        value,
        max,
      };
    }
    case 'preparing':
    case 'finalizing': {
      const phase = job.state === 'preparing' ? 'Preparing' : 'Finishing';
      return {
        text: `${phase} ${task}${percent !== null ? ` ${percent}%` : ''}: ${step ?? '...'}`,
        value,
        max,
      };
    }
    default:
      return {
        text: `${job.state.charAt(0).toUpperCase()}${job.state.slice(1)}: ${task}...`,
        value,
        max,
      };
  }
}
