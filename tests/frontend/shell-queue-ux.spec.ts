import { expect, test, type Page } from '@playwright/test';
import { installSourceEncodeFixture, source } from './helpers/source-encode-fixture';

function job(id: string, state: string, extra: Record<string, unknown> = {}) {
  return {
    id,
    state,
    request: {
      inputPath: `/media/${id}.mkv`,
      outputPath: `/media/${id}-output.mkv`,
      streamIndices: [0, 1],
    },
    encodeSettings: { backend: 'standalone', encoder: 'svtAv1', videoMode: 'encode' },
    progressSeconds: 3,
    durationSeconds: 12,
    logs: [],
    error: null,
    logPath: `/logs/${id}.log`,
    ...extra,
  };
}

async function startWithJobs(page: Page, entries: ReturnType<typeof job>[]) {
  await page.addInitScript(
    (value) => sessionStorage.setItem('test-initial-jobs', JSON.stringify(value)),
    entries,
  );
  await installSourceEncodeFixture(page);
}

async function commands(page: Page, names: string[]) {
  return page.evaluate(
    (wanted) =>
      (globalThis as any).__sourceCalls.filter((call: any) => wanted.includes(call.command)),
    names,
  );
}

for (const window of ['main', 'monitor']) {
  test(`Stop in the ${window} leaves other queued jobs alone`, async ({ page }) => {
    await startWithJobs(page, [job('pending', 'queued'), job('current', 'running')]);
    if (window === 'monitor') await page.goto('/?window=quick-convert');
    await page.getByRole('button', { name: 'Stop', exact: true }).click();
    await expect
      .poll(() =>
        commands(page, ['stop_job', 'cancel_job', 'cancel_all_jobs', 'cancel_queued_jobs']),
      )
      .toEqual([{ command: 'stop_job', payload: { id: 'current' } }]);
    expect(
      await page.evaluate(() => (globalThis as any).__testJobs.map((entry: any) => entry.state)),
    ).toEqual(['queued', 'stopped']);
  });
}

test('queue confirmation counts waiting jobs and excludes one that starts before confirmation', async ({
  page,
}) => {
  await startWithJobs(page, [
    job('waiting', 'queued'),
    job('starting', 'queued'),
    job('current', 'running'),
  ]);
  const cancelQueue = page.getByRole('button', { name: 'Cancel queue (2)…', exact: true });
  await cancelQueue.click();
  const dialog = page.getByRole('dialog', { name: 'Cancel waiting jobs?' });
  await expect(dialog).toContainText('Cancel 2 waiting jobs?');
  await expect(dialog.getByRole('button', { name: 'Keep queue' })).toBeFocused();
  await page.keyboard.press('Escape');
  await expect(cancelQueue).toBeFocused();
  expect(await commands(page, ['cancel_queued_jobs'])).toEqual([]);

  await cancelQueue.click();
  await page.evaluate(() => {
    const host = globalThis as any;
    host.__testJobs.find((entry: any) => entry.id === 'current').state = 'succeeded';
    host.__testJobs.find((entry: any) => entry.id === 'starting').state = 'running';
    host.__emitJobs();
  });
  await expect(dialog).toContainText('Cancel 1 waiting job?');
  await dialog.getByRole('button', { name: 'Cancel waiting jobs' }).click();
  await expect
    .poll(() => commands(page, ['stop_job', 'cancel_job', 'cancel_all_jobs', 'cancel_queued_jobs']))
    .toEqual([{ command: 'cancel_queued_jobs', payload: { ids: ['waiting'] } }]);
  expect(
    await page.evaluate(() => (globalThis as any).__testJobs.map((entry: any) => entry.state)),
  ).toEqual(['canceled', 'running', 'succeeded']);
});

async function importMovies(page: Page, count: number) {
  const files = Array.from({ length: count }, (_, i) => ({
    ...source,
    id: `movie-${i + 1}`,
    name: `movie-${i + 1}.mkv`,
    path: `/media/movie-${i + 1}.mkv`,
    streams: [{ ...source.streams[0], fieldOrder: 'progressive' }, ...source.streams.slice(1)],
  }));
  await page.evaluate((movies) => {
    const host = globalThis as any;
    host.__pickerPaths = movies.map((movie) => movie.path);
    host.__mediaByPath = Object.fromEntries(movies.map((movie) => [movie.path, movie]));
  }, files);
  await page.getByRole('button', { name: 'Add Files…' }).click();
  await expect(page.getByRole('listbox', { name: 'Files' }).getByRole('option')).toHaveCount(count);
}

test('stopping one encode in a batch starts the next, and canceling the queue preserves that next encode', async ({
  page,
}) => {
  await installSourceEncodeFixture(page);
  await importMovies(page, 3);
  await page.getByRole('combobox', { name: 'Mode', exact: true }).selectOption('batch');
  await page.getByRole('button', { name: 'Quick Convert', exact: true }).click();
  await page.evaluate(() => ((globalThis as any).__holdTerminal = true));
  await page.getByRole('button', { name: 'Run Batch: Encode', exact: true }).click();
  const status = page.getByLabel('Current action');
  await expect(status).toContainText('File 1/3 (movie-1.mkv)');
  await expect(status).toContainText('Encoding 0%');
  await page.getByRole('button', { name: 'Stop', exact: true }).click();
  await expect(status).toContainText('File 2/3 (movie-2.mkv)');
  await expect(status).toContainText('Encoding 0%');
  await page.getByRole('button', { name: 'Cancel queue (1)…', exact: true }).click();
  await page
    .getByRole('dialog', { name: 'Cancel waiting jobs?' })
    .getByRole('button', { name: 'Cancel waiting jobs' })
    .click();
  await expect(status).toContainText('File 2/3 (movie-2.mkv)');
  await page.evaluate(() => {
    const host = globalThis as any;
    host.__testJobs.find((entry: any) => entry.request.inputPath.endsWith('movie-2.mkv')).state =
      'succeeded';
    host.__emitJobs();
  });
  await expect(status).toHaveText('Batch stopped - 1/3 finished, 1 stopped, 1 not run.');
  expect(
    await commands(page, ['stop_job', 'cancel_job', 'cancel_all_jobs', 'cancel_queued_jobs']),
  ).toEqual([expect.objectContaining({ command: 'stop_job' })]);
  expect(
    await page.evaluate(() =>
      (globalThis as any).__testJobs.map((entry: any) => entry.request.inputPath).sort(),
    ),
  ).toEqual(['/media/movie-1.mkv', '/media/movie-2.mkv']);
});

for (const viewport of [
  { width: 760, height: 600 },
  { width: 1152, height: 720 },
]) {
  test(`an older failure and its log are on screen at ${viewport.width}×${viewport.height}`, async ({
    page,
  }) => {
    await page.setViewportSize(viewport);
    const errorMessage = 'Could not create output because the destination drive ran out of space.';
    await startWithJobs(page, [
      job('latest', 'succeeded'),
      job('older', 'failed', {
        encodeSettings: { backend: 'av1an', encoder: 'svtAv1', videoMode: 'encode' },
        error: { code: 'NO_SPACE', message: errorMessage, path: null },
        logs: [errorMessage],
      }),
    ]);
    await expect(page.getByRole('log', { name: 'Recent activity' })).not.toContainText(
      errorMessage,
    );
    await page.getByRole('button', { name: 'Job history (1 failed)', exact: true }).click();
    const history = page.getByRole('region', { name: 'Job history', exact: true });
    const heading = history.getByText('Saved requests and resume compatibility', { exact: true });
    await expect(heading).toBeFocused();
    await expect(heading).toBeInViewport({ ratio: 1 });
    const jobPicker = history.getByRole('combobox', { name: 'Job from history' });
    await expect(jobPicker).toBeInViewport({ ratio: 1 });
    await expect(jobPicker).toHaveValue('older');
    await expect(history.getByText(errorMessage, { exact: true })).toBeInViewport({ ratio: 1 });
    const viewLog = history.getByRole('button', { name: 'View job log' });
    await expect(viewLog).toBeInViewport({ ratio: 1 });
    await viewLog.click();
    const log = page.getByRole('dialog', { name: 'Job log for older-output.mkv' });
    await expect(log).toContainText('Showing saved diagnostic lines.');
    await expect(log).toContainText(errorMessage);
    await page.keyboard.press('Escape');
    await page.evaluate(
      () => ((globalThis as any).__jobLogs = { older: 'Retained per-job diagnostics.' }),
    );
    await viewLog.click();
    await expect(log).toContainText('Retained per-job diagnostics.');
    expect(await commands(page, ['read_job_log'])).toEqual([
      { command: 'read_job_log', payload: { id: 'older' } },
      { command: 'read_job_log', payload: { id: 'older' } },
    ]);
  });
}

for (const window of ['main', 'monitor']) {
  test(`${window} names the checkpoint and confirms before discarding`, async ({ page }) => {
    await startWithJobs(page, [
      job('saved', 'stopped', {
        standaloneRecovery: {
          workspace: '/media/.saved',
          phase: 'videoComplete',
          completedFrames: 3500,
          totalFrames: 3500,
        },
      }),
    ]);
    if (window === 'monitor') await page.goto('/?window=quick-convert');
    else {
      await page.getByRole('button', { name: 'Job history', exact: true }).click();
      await page.getByRole('combobox', { name: 'Job from history' }).selectOption('saved');
    }
    const discard = page.getByRole('button', { name: 'Discard saved progress', exact: true });
    await discard.click();
    const confirmation = page.getByRole('dialog', { name: 'Discard saved progress?' });
    await expect(confirmation).toContainText('saved-output.mkv');
    await expect(confirmation).toContainText('3,500 frames');
    await expect(confirmation.getByRole('button', { name: 'Keep saved progress' })).toBeFocused();
    expect(await commands(page, ['discard_job_progress'])).toEqual([]);
    await page.keyboard.press('Escape');
    await expect(discard).toBeFocused();
    await discard.click();
    await confirmation.getByRole('button', { name: 'Discard saved progress', exact: true }).click();
    await expect
      .poll(() => commands(page, ['discard_job_progress']))
      .toEqual([{ command: 'discard_job_progress', payload: { id: 'saved' } }]);
    await expect(discard).toHaveCount(0);
  });
}

test('file selection extends and shrinks upward with a single tab stop', async ({ page }) => {
  await installSourceEncodeFixture(page);
  await importMovies(page, 4);
  const files = page.getByRole('listbox', { name: 'Files' });
  const rows = files.getByRole('option');
  await rows.nth(3).click();
  await page.keyboard.press('Shift+ArrowUp');
  await expect(rows.nth(2)).toBeFocused();
  await page.keyboard.press('Shift+ArrowUp');
  await expect(rows.nth(1)).toBeFocused();
  await expect(files.getByRole('option', { selected: true })).toHaveCount(3);
  await page.keyboard.press('Shift+ArrowDown');
  await expect(rows.nth(2)).toBeFocused();
  await expect(files.getByRole('option', { selected: true })).toHaveCount(2);
  await expect(files.locator('[role="option"][tabindex="0"]')).toHaveCount(1);
  await page.keyboard.press('Tab');
  await expect(files.locator('[role="option"]:focus')).toHaveCount(0);
});

test('preferences recover after a failed load and failed saves restore the last saved selection', async ({
  page,
}) => {
  await page.addInitScript(
    () => ((globalThis as any).__preferenceLoadError = 'Could not read preferences.'),
  );
  await installSourceEncodeFixture(page);
  await page.getByRole('button', { name: 'Settings', exact: true }).click();
  const setting = page.getByRole('combobox', { name: 'Console window', exact: true });
  await expect(setting).toBeDisabled();
  const settingsAlert = page
    .getByRole('region', { name: 'Settings', exact: true })
    .getByRole('alert');
  await expect(settingsAlert).toContainText('Could not load preferences.');
  expect(await commands(page, ['save_preferences'])).toEqual([]);
  await page.evaluate(() => ((globalThis as any).__preferenceLoadError = null));
  await page.getByRole('button', { name: 'Retry loading preferences' }).click();
  await expect(setting).toBeEnabled();
  await setting.selectOption('visible');
  await expect.poll(() => commands(page, ['save_preferences'])).toHaveLength(1);
  await page.getByRole('button', { name: 'File List', exact: true }).click();
  await page.getByRole('button', { name: 'Settings', exact: true }).click();
  await expect(setting).toHaveValue('visible');

  await page.evaluate(
    () => ((globalThis as any).__preferenceSaveError = 'Could not save preferences.'),
  );
  await setting.selectOption('hidden');
  await expect(settingsAlert).toContainText('Could not save preferences.');
  await expect(setting).toHaveValue('visible');
});
