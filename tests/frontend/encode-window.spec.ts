import { expect, test } from '@playwright/test';
import { installSourceEncodeFixture } from './helpers/source-encode-fixture';

const job = {
  id: 'quick-job',
  state: 'running',
  request: { inputPath: '/media/input.mkv', outputPath: '/media/output.mkv', streamIndices: [0] },
  encodeSettings: {
    backend: 'standalone',
    encoder: 'x264',
    videoMode: 'encode',
    quality: 23,
    preset: 'medium',
  },
  progressSeconds: 3,
  durationSeconds: 12,
  logs: ['Encoding a test video.'],
  error: null,
  recovery: null,
  logPath: '/logs/quick-job.log',
};

test('encoding log highlights syntax and diagnostics while preserving literal tool output', async ({
  page,
}, testInfo) => {
  const lines = [
    'Command: ffmpeg -i "C:\\Videos\\sample <one>.mkv" -vf scale=1280:720 | x264 --crf 20 --preset slow',
    '[consumer] x264 [info]: profile High, level 4.0',
    '[consumer] 120 frames: 60.00 fps, 3500.00 kb/s',
    '[producer] Warning: timestamp adjusted.',
    '[consumer] x264 [error]: cannot open output.',
    'Verified output published. The source was preserved.',
    '<img src=x onerror="window.logInjected=true"> & untouched text',
  ];
  await page.addInitScript(
    (entry) => sessionStorage.setItem('test-initial-jobs', JSON.stringify([entry])),
    { ...job, logs: lines },
  );
  await installSourceEncodeFixture(page);
  await page.goto('/?window=quick-convert');
  const log = page.getByRole('log', { name: 'Job log', exact: true });
  await expect(log.getByText('--crf', { exact: true })).toBeVisible();
  const baseColor = await log.evaluate((el) => getComputedStyle(el).color);
  for (const tone of ['warning', 'error', 'success', 'progress']) {
    const line = log.locator(`[data-tone="${tone}"]`);
    await expect(line).toHaveCount(1);
    expect(await line.evaluate((el) => getComputedStyle(el).color)).not.toBe(baseColor);
  }
  expect(await log.textContent()).toMatch(
    new RegExp(lines.map((line) => line.replace(/[.*+?^${}()|[\]\\]/g, '\\$&')).join('\n') + '$'),
  );
  await expect(log.locator('img,script')).toHaveCount(0);
  expect(await page.evaluate(() => (window as any).logInjected)).toBeUndefined();
  await page.evaluate(() => {
    const host = globalThis as any;
    host.__testJobs[0].logs.push('[consumer] x264 [warning]: next live warning.');
    host.__emitJobs();
  });
  await expect(log.locator('[data-tone="warning"]')).toHaveCount(2);
  await page.setViewportSize({ width: 900, height: 700 });
  await page.screenshot({ path: testInfo.outputPath('highlighted-log.png') });
});

test('Quick Convert opens a monitor once when encoding begins, with manual reopening', async ({
  page,
}) => {
  await installSourceEncodeFixture(page);
  await page.evaluate((entry) => {
    const host = globalThis as any;
    host.__testJobs = [{ ...entry, state: 'queued' }];
    host.__emitJobs();
  }, job);
  await expect
    .poll(() =>
      page.evaluate(
        () =>
          (globalThis as any).__sourceCalls.filter((c: any) => c.command === 'open_encode_window')
            .length,
      ),
    )
    .toBe(0);
  await page.evaluate(() => {
    const host = globalThis as any;
    host.__testJobs[0].state = 'preparing';
    host.__emitJobs();
  });
  await expect
    .poll(() =>
      page.evaluate(
        () =>
          (globalThis as any).__sourceCalls.filter((c: any) => c.command === 'open_encode_window')
            .length,
      ),
    )
    .toBe(1);
  await page.evaluate(() => {
    const host = globalThis as any;
    host.__testJobs[0].state = 'running';
    host.__emitJobs();
    host.__emitJobs();
  });
  await page.getByRole('button', { name: 'Show encoding window' }).click();
  await expect
    .poll(() =>
      page.evaluate(
        () =>
          (globalThis as any).__sourceCalls.filter((c: any) => c.command === 'open_encode_window')
            .length,
      ),
    )
    .toBe(2);
});

test('command preparation and AV1AN jobs do not open the Quick Convert window', async ({
  page,
}) => {
  await installSourceEncodeFixture(page);
  await page.evaluate((entry) => {
    const host = globalThis as any;
    host.__testJobs = [
      { ...entry, state: 'preparing' },
      { ...entry, id: 'av1an', encodeSettings: { ...entry.encodeSettings, backend: 'av1an' } },
    ];
    host.__emitJobs();
  }, job);
  await expect(page.getByRole('button', { name: 'Show encoding window' })).toBeVisible();
  expect(
    await page.evaluate(() =>
      (globalThis as any).__sourceCalls.filter((c: any) => c.command === 'open_encode_window'),
    ),
  ).toHaveLength(0);
});

test('dedicated monitor follows job progress and keeps the final result and log', async ({
  page,
}) => {
  await page.addInitScript(
    (entry) => sessionStorage.setItem('test-initial-jobs', JSON.stringify([entry])),
    job,
  );
  await installSourceEncodeFixture(page);
  await page.goto('/?window=quick-convert');
  await expect(page.getByRole('heading', { name: 'Quick Convert' })).toBeVisible();
  await expect(
    page.getByRole('progressbar', { name: 'Encode progress', exact: true }),
  ).toHaveAttribute('value', '3');
  await expect(page.getByLabel('Job log', { exact: true })).toBeVisible();
  await expect(page.getByLabel('Job log', { exact: true })).toContainText('Encoding a test video.');
  await expect(page.getByRole('button', { name: 'Pause encoding', exact: true })).toBeVisible();
  await expect(page.getByRole('button', { name: 'Cancel job', exact: true })).toBeVisible();
  await expect(page.getByRole('region', { name: 'Job queue and history' })).toHaveCount(0);
  await page.getByRole('button', { name: 'Pause encoding', exact: true }).click();
  await expect(page.getByRole('button', { name: 'Continue encoding', exact: true })).toBeVisible();
  await page.getByRole('button', { name: 'Continue encoding', exact: true }).click();
  await expect(page.getByRole('button', { name: 'Pause encoding', exact: true })).toBeVisible();
  await page.evaluate(() => {
    const host = globalThis as any;
    host.__testJobs[0].state = 'succeeded';
    host.__testJobs[0].logs.push('Finished and saved.');
    host.__emitJobs();
  });
  await expect(page.getByText('Output verified and saved.')).toBeVisible();
  await expect(page.getByLabel('Job log', { exact: true })).toContainText('Finished and saved.');
  expect(
    await page.evaluate(() =>
      (globalThis as any).__sourceCalls.some((c: any) => c.command === 'plugin:window|close'),
    ),
  ).toBe(false);
});

test('a confirmed Quick Convert opens the monitor even if the job finishes immediately', async ({
  page,
}) => {
  await installSourceEncodeFixture(page);
  await page.getByRole('button', { name: 'Add Files…' }).click();
  await page.getByRole('button', { name: 'Load Tracks' }).click();
  await page.getByRole('button', { name: 'Quick Convert', exact: true }).click();
  await page
    .getByRole('region', { name: 'Quick Convert', exact: true })
    .getByLabel('Output Path', { exact: true })
    .fill('/media/quick-window');
  await page
    .getByRole('button', { name: 'Run: Encode', exact: true })
    .click({ modifiers: ['Shift'] });
  const editor = page.getByRole('dialog', { name: 'Edit ffmpeg Arguments' });
  await expect(editor).toBeVisible();
  expect(
    await page.evaluate(() =>
      (globalThis as any).__sourceCalls.filter((c: any) => c.command === 'open_encode_window'),
    ),
  ).toHaveLength(0);
  await editor.getByRole('button', { name: 'Run', exact: true }).click();
  await expect
    .poll(() =>
      page.evaluate(
        () =>
          (globalThis as any).__sourceCalls.filter((c: any) => c.command === 'open_encode_window')
            .length,
      ),
    )
    .toBe(1);
  await expect(page.getByLabel('Current action', { exact: true })).toContainText('Done');
});

test('window creation failure reports an error without failing or resubmitting the encode', async ({
  page,
}) => {
  await installSourceEncodeFixture(page);
  await page.getByRole('button', { name: 'Add Files…' }).click();
  await page.getByRole('button', { name: 'Load Tracks' }).click();
  await page.getByRole('button', { name: 'Quick Convert', exact: true }).click();
  await page.evaluate(() => {
    (globalThis as any).__encodeWindowFailure = 'Window unavailable';
  });
  await page.getByRole('button', { name: 'Run: Encode', exact: true }).click();
  await expect(page.getByRole('log', { name: 'Recent activity' })).toContainText(
    'Could not open the encoding window: Window unavailable',
  );
  await expect(page.getByLabel('Current action', { exact: true })).toContainText('Done');
  expect(
    await page.evaluate(() =>
      (globalThis as any).__sourceCalls.filter((c: any) => c.command === 'confirm_command_edit'),
    ),
  ).toHaveLength(1);
});

test('monitor cancellation targets only the displayed Quick Convert job', async ({ page }) => {
  await page.addInitScript(
    (entry) =>
      sessionStorage.setItem(
        'test-initial-jobs',
        JSON.stringify([
          { ...entry, id: 'other', encodeSettings: { ...entry.encodeSettings, backend: 'av1an' } },
          entry,
        ]),
      ),
    job,
  );
  await installSourceEncodeFixture(page);
  await page.goto('/?window=quick-convert');
  await page.getByRole('button', { name: 'Cancel job', exact: true }).click();
  await expect(page.getByRole('status', { exact: true })).toHaveText('Canceled');
  expect(
    await page.evaluate(
      () => (globalThis as any).__testJobs.find((entry: any) => entry.id === 'other').state,
    ),
  ).toBe('running');
});

test('monitor follows fresh log output until the user scrolls back', async ({ page }) => {
  await page.addInitScript(
    (entry) => sessionStorage.setItem('test-initial-jobs', JSON.stringify([entry])),
    {
      ...job,
      logs: Array.from({ length: 80 }, (_, i) => `Encoder output line ${i}`),
    },
  );
  await installSourceEncodeFixture(page);
  await page.goto('/?window=quick-convert');
  const log = page.getByLabel('Job log', { exact: true });
  await expect
    .poll(() => log.evaluate((el) => el.scrollHeight - el.clientHeight - el.scrollTop))
    .toBeLessThan(2);
  await page.evaluate(() => {
    const host = globalThis as any;
    host.__testJobs[0].logs.push('Latest encoder output');
    host.__emitJobs();
  });
  await expect(log).toContainText('Latest encoder output');
  await expect
    .poll(() => log.evaluate((el) => el.scrollHeight - el.clientHeight - el.scrollTop))
    .toBeLessThan(2);
  await log.evaluate((el) => {
    el.scrollTop = 0;
    el.dispatchEvent(new Event('scroll'));
  });
  await page.evaluate(() => {
    const host = globalThis as any;
    host.__testJobs[0].logs.push('Output while reading older lines');
    host.__emitJobs();
  });
  await expect(log).toContainText('Output while reading older lines');
  expect(await log.evaluate((el) => el.scrollTop)).toBe(0);
});

for (const viewport of [
  { width: 360, height: 640 },
  { width: 600, height: 400 },
  { width: 900, height: 760 },
  { width: 1440, height: 900 },
]) {
  test(`monitor fits ${viewport.width}x${viewport.height} with long paths and output`, async ({
    page,
  }, testInfo) => {
    await page.setViewportSize(viewport);
    await page.addInitScript(
      (entry) => sessionStorage.setItem('test-initial-jobs', JSON.stringify([entry])),
      {
        ...job,
        request: {
          ...job.request,
          inputPath: `C:\\Videos\\${'long-source-name'.repeat(12)}.mkv`,
          outputPath: `C:\\Videos\\${'long-output-name'.repeat(12)}.mkv`,
        },
        logs: ['Long command: ' + 'parameter='.repeat(100)],
      },
    );
    await installSourceEncodeFixture(page);
    await page.goto('/?window=quick-convert');
    await expect(page.getByRole('button', { name: 'Pause encoding', exact: true })).toBeVisible();
    expect(await page.evaluate(() => document.documentElement.scrollWidth)).toBeLessThanOrEqual(
      viewport.width + 1,
    );
    for (const name of ['Pause encoding', 'Stop and keep progress', 'Cancel job']) {
      const control = page.getByRole('button', { name, exact: true });
      await control.scrollIntoViewIfNeeded();
      const bounds = await control.boundingBox();
      expect(bounds!.x).toBeGreaterThanOrEqual(0);
      expect(bounds!.x + bounds!.width).toBeLessThanOrEqual(viewport.width + 1);
    }
    await page.getByLabel('Job log', { exact: true }).scrollIntoViewIfNeeded();
    await page.screenshot({ path: testInfo.outputPath('responsive-monitor.png'), fullPage: true });
  });
}
