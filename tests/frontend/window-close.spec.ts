import { expect, test, type Page } from '@playwright/test';
import { installSourceEncodeFixture } from './helpers/source-encode-fixture';

function job(id: string, overrides: Record<string, unknown> = {}) {
  return {
    id,
    state: 'running',
    request: { inputPath: '/media/movie.mp4', outputPath: '/media/movie.mkv', streamIndices: [0] },
    encodeSettings: { backend: 'av1an', encoder: 'svtAv1Hdr', videoStreamIndex: 0 },
    progressSeconds: 6,
    durationSeconds: 12,
    logs: [] as string[],
    error: null,
    recovery: null,
    logPath: `/logs/${id}.log`,
    ...overrides,
  };
}

const commands = (page: Page) =>
  page.evaluate(() =>
    ((globalThis as any).__sourceCalls as { command: string }[]).map((call) => call.command),
  );

async function setJobs(page: Page, jobs: unknown[]) {
  await page.evaluate((jobs) => {
    const host = globalThis as any;
    host.__testJobs = jobs;
    host.__emitJobs();
  }, jobs);
}

// The title-bar button, Alt+F4 and the taskbar all send the same request.
const requestClose = (page: Page) =>
  page.evaluate(() => (globalThis as any).__emitEvent('tauri://close-requested'));

test('closing with no jobs stops nothing and closes at once', async ({ page }) => {
  await installSourceEncodeFixture(page);
  await expect(page.getByRole('log', { name: 'Recent activity' })).toBeVisible();
  await requestClose(page);
  await expect.poll(() => commands(page)).toContain('plugin:window|destroy');
  const calls = await commands(page);
  expect(calls.indexOf('prepare_close')).toBeGreaterThan(-1);
  expect(calls.indexOf('prepare_close')).toBeLessThan(calls.indexOf('plugin:window|destroy'));
  await expect(page.getByRole('dialog', { name: 'Close jesses?' })).toHaveCount(0);
});

test('closing during a job asks first and Keep running leaves it alone', async ({ page }) => {
  await installSourceEncodeFixture(page);
  await setJobs(page, [job('running'), job('waiting', { state: 'queued', progressSeconds: 0 })]);
  await requestClose(page);
  const dialog = page.getByRole('dialog', { name: 'Close jesses?' });
  await expect(dialog).toBeVisible();
  await expect(dialog).toContainText('2 jobs are running or queued.');
  await dialog.getByRole('button', { name: 'Keep running' }).click();
  await expect(dialog).toHaveCount(0);
  let calls = await commands(page);
  expect(calls).not.toContain('prepare_close');
  expect(calls).not.toContain('plugin:window|destroy');
  expect(calls).not.toContain('cancel_job');

  await requestClose(page);
  await dialog.getByRole('button', { name: 'Close anyway' }).click();
  await expect.poll(() => commands(page)).toContain('plugin:window|destroy');
  calls = await commands(page);
  expect(calls.indexOf('prepare_close')).toBeLessThan(calls.indexOf('plugin:window|destroy'));
});

test('the taskbar button follows job progress, pause and failure', async ({ page }) => {
  await installSourceEncodeFixture(page);
  const progressStates = () =>
    page.evaluate(() =>
      ((globalThis as any).__sourceCalls as { command: string; payload: any }[])
        .filter((call) => call.command === 'plugin:window|set_progress_bar')
        .map((call) => call.payload.value),
    );
  await setJobs(page, [job('running')]);
  await expect
    .poll(async () => (await progressStates()).at(-1))
    .toMatchObject({ status: 'normal' });
  const running = (await progressStates()).at(-1);
  expect(running.progress).toBeGreaterThanOrEqual(0);
  expect(running.progress).toBeLessThanOrEqual(100);
  await setJobs(page, [job('running', { state: 'paused' })]);
  await expect
    .poll(async () => (await progressStates()).at(-1))
    .toMatchObject({ status: 'paused' });
  await setJobs(page, [
    job('running', {
      state: 'failed',
      error: { code: 'TOOL_FAILED', message: 'The encoder failed.', path: null },
    }),
  ]);
  await expect.poll(async () => (await progressStates()).at(-1)).toMatchObject({ status: 'error' });
  await setJobs(page, [job('running', { state: 'succeeded' })]);
  await expect.poll(async () => (await progressStates()).at(-1)).toMatchObject({ status: 'none' });
});

test('files passed at launch and by a later launch are imported', async ({ page }) => {
  await page.addInitScript(() =>
    sessionStorage.setItem('test-launch-paths', JSON.stringify(['/media/movie.mp4'])),
  );
  await installSourceEncodeFixture(page);
  const probed = () =>
    page.evaluate(() =>
      ((globalThis as any).__sourceCalls as { command: string; payload: any }[])
        .filter((call) => call.command === 'probe_media')
        .map((call) => call.payload.path),
    );
  // The startup handover runs once the window has mounted.
  await expect.poll(() => commands(page)).toContain('take_launch_paths');
  await expect.poll(probed).toContain('/media/movie.mp4');
  await page.evaluate(() => {
    const host = globalThis as any;
    host.__launchPaths = ['/media/second.mp4'];
    void host.__emitEvent('launch-paths');
  });
  // With files already listed, the usual import choice applies.
  await page
    .getByRole('dialog', { name: 'Import 1 File' })
    .getByRole('button', { name: 'Add to file list' })
    .click();
  await expect.poll(probed).toContain('/media/second.mp4');
});
