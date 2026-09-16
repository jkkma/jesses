import type { JobSnapshot } from '$lib/ipc/generated';

export const terminalJob = (state: string): boolean =>
  ['succeeded', 'failed', 'canceled', 'interrupted', 'stopped'].includes(state);

export const canKeepProgress = (job: JobSnapshot): boolean =>
  !!job.encodeSettings && ['preparing', 'running', 'paused', 'finalizing'].includes(job.state);

export const canResumeJob = (job: JobSnapshot): boolean =>
  ((job.encodeSettings?.backend === 'av1an' && !!job.recovery) ||
    (job.encodeSettings?.backend === 'standalone' && !!job.standaloneRecovery)) &&
  ['stopped', 'interrupted', 'failed', 'canceled'].includes(job.state);

export function savedProgressSummary(job: JobSnapshot): string {
  const standalone = job.standaloneRecovery;
  if (job.encodeSettings?.backend === 'standalone' && standalone) {
    switch (standalone.phase) {
      case 'passOneComplete':
        return 'Pass one statistics are verified and saved. Resume starts a fresh decoder and runs pass two.';
      case 'videoComplete':
        return `All ${standalone.totalFrames.toLocaleString()} frames are verified and saved. Resume continues with timing and final muxing.`;
      case 'timingWrapComplete':
        return 'The exact-timing video wrapper is verified and saved. Resume continues with final muxing.';
      case 'finalizing':
        return 'The completed Matroska stage is verified and saved. Resume continues with final container publication.';
    }
  }
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
