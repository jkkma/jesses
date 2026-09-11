const MIN_SAMPLE_MS = 5_000;
const WINDOW_MS = 30_000;
const STALE_MS = 15_000;

type Sample = { time: number; progress: number };
export type ProgressEstimate =
  { kind: 'waiting' } | { kind: 'estimate'; speed: number; remainingSeconds: number | null };

/** Estimate only from progress observed in this view and the current job phase. */
export class ProgressEstimator {
  private key: string | null = null;
  private samples: Sample[] = [];

  observe(key: string | null, progress: number | null, time: number): void {
    if (key === null || progress === null || !Number.isFinite(progress) || progress < 0) {
      this.key = null;
      this.samples = [];
      return;
    }
    const last = this.samples.at(-1);
    if (key !== this.key || !last || time < last.time || progress < last.progress) {
      this.key = key;
      this.samples = [{ time, progress }];
      return;
    }
    if (progress === last.progress) return;
    if (time - last.time >= STALE_MS) {
      this.samples = [{ time, progress }];
      return;
    }
    // Retain the initial baseline, then at most one sample per second so a
    // noisy encoder cannot grow the observation window without bound.
    if (this.samples.length > 1 && Math.floor(time / 1_000) === Math.floor(last.time / 1_000)) {
      this.samples[this.samples.length - 1] = { time, progress };
    } else {
      this.samples.push({ time, progress });
    }
    this.trim(time);
  }

  estimate(time: number, duration: number | null): ProgressEstimate | null {
    const last = this.samples.at(-1);
    if (!last) return null;
    if (time - last.time >= STALE_MS) return { kind: 'waiting' };
    this.trim(time);
    const first = this.samples[0];
    const elapsed = time - first.time;
    const advanced = last.progress - first.progress;
    if (elapsed < MIN_SAMPLE_MS || advanced <= 0) return null;
    // Include time since the latest update so the rate ages during silence.
    const speed = advanced / (elapsed / 1_000);
    const remainingSeconds =
      duration !== null && Number.isFinite(duration) && duration > last.progress
        ? (duration - last.progress) / speed
        : null;
    return { kind: 'estimate', speed, remainingSeconds };
  }

  private trim(time: number): void {
    // Keep one sample at/before the window boundary as the rate baseline.
    while (this.samples.length > 2 && this.samples[1].time <= time - WINDOW_MS) {
      this.samples.shift();
    }
  }
}
