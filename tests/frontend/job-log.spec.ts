import { expect, test } from '@playwright/test';
import { installSourceEncodeFixture } from './helpers/source-encode-fixture';

function job(id: string, overrides: Record<string, unknown> = {}) {
  return {
    id,
    state: 'running',
    request: { inputPath: '/media/movie.mp4', outputPath: '/media/movie.mkv', streamIndices: [0] },
    progressSeconds: 0,
    durationSeconds: 12,
    logs: [] as string[],
    error: null,
    recovery: null,
    logPath: `/logs/${id}.log`,
    ...overrides,
  };
}

// The footer says "the log has the details", so a job's own lines and its
// final error must reach the Log pane, without replaying older history.
test('job lines and the final error reach the Log pane', async ({ page }) => {
  await page.addInitScript(
    (history) => {
      sessionStorage.setItem('test-initial-jobs', JSON.stringify([history]));
    },
    job('older', {
      state: 'failed',
      logs: ['Old failure line.'],
      error: { code: 'OLD', message: 'Old failure line.', path: null },
    }),
  );
  await installSourceEncodeFixture(page);
  const log = page.getByRole('log', { name: 'Recent activity' });
  await page.evaluate(
    (current) => {
      const host = globalThis as any;
      host.__testJobs = [current, ...host.__testJobs];
      host.__emitJobs();
    },
    job('current', { logs: ['Inspecting selected streams and encoder capabilities.'] }),
  );
  await expect(log).toContainText('Inspecting selected streams and encoder capabilities.');
  await page.evaluate(() => {
    const host = globalThis as any;
    const current = host.__testJobs.find((entry: any) => entry.id === 'current');
    const message = 'The output duration differs from the selected source tracks.';
    current.logs = [
      ...current.logs,
      '[consumer] Encoding:   12/50 Frames @ 3.00 fps | 88.14 kb/s',
      'Muxing encoded video and applying selected audio settings.',
      message,
    ];
    current.state = 'failed';
    current.error = { code: 'OUTPUT_VALIDATION_FAILED', message, path: null };
    host.__emitJobs();
  });
  await expect(log).toContainText('Muxing encoded video and applying selected audio settings.');
  await expect(log.locator('.log-error')).toHaveText([
    'The output duration differs from the selected source tracks.',
    'Did not finish (OUTPUT_VALIDATION_FAILED). Job log: /logs/current.log',
  ]);
  await expect(log).not.toContainText('Frames @');
  await expect(log).not.toContainText('Old failure line.');
  // A later snapshot of the finished job adds nothing.
  await page.evaluate(() => (globalThis as any).__emitJobs());
  await expect(log.locator('.log-error')).toHaveCount(2);
});
