import type { JobSnapshot } from '$lib/ipc/generated';

export const terminalJob = (state: string): boolean =>
  ['succeeded', 'failed', 'canceled', 'interrupted', 'stopped'].includes(state);

export const canKeepProgress = (job: JobSnapshot): boolean =>
  job.encodeSettings?.backend === 'av1an' &&
  ['preparing', 'running', 'finalizing'].includes(job.state);

export const canResumeJob = (job: JobSnapshot): boolean =>
  job.encodeSettings?.backend === 'av1an' &&
  !!job.recovery &&
  ['stopped', 'interrupted', 'failed', 'canceled'].includes(job.state);

export function savedProgressSummary(job: JobSnapshot): string {
  const recovery = job.recovery;
  if (!recovery) return '';
  if (recovery.phase === 'finalizing')
    return 'Encoded video is saved. Resume combines tracks and checks the output.';
  if (recovery.completedFrames > 0) {
    const completed = recovery.completedFrames.toLocaleString();
    const total = recovery.totalFrames > 0 ? ` of ${recovery.totalFrames.toLocaleString()}` : '';
    return `${completed}${total} frames kept. Resume continues with this job's saved settings.`;
  }
  return "Resume continues with this job's saved settings. Source preparation may run again.";
}
