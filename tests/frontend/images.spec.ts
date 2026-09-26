import { test, expect, type Page } from '@playwright/test';

type Call = { command: string; payload: Record<string, unknown> };

async function setup(page: Page) {
  await page.addInitScript(() => {
    const state = globalThis as unknown as Record<string, unknown>;
    const calls: Call[] = [];
    state.__imageCalls = calls;
    state.isTauri = true;
    let sequence = 0;
    state.__TAURI_INTERNALS__ = {
      metadata: { currentWindow: { label: 'main' }, currentWebview: { label: 'main' } },
      transformCallback: () => ++sequence,
      unregisterCallback: () => {},
      invoke: async (command: string, payload: Record<string, unknown> = {}) => {
        calls.push({ command, payload });
        if (command.startsWith('plugin:event|')) return ++sequence;
        if (command === 'get_capabilities' || command === 'list_jobs') return [];
        if (command === 'subscribe_jobs') {
          (payload.channel as { onmessage: (jobs: unknown[]) => void }).onmessage([]);
          return;
        }
        if (command === 'get_preferences' || command === 'remember_recent_media')
          return {
            version: 1,
            general: { recursiveImport: false, defaultOutputDirectory: null },
            recentPaths: [],
          };
        if (command === 'plugin:dialog|open') return ['C:\\media\\source.mkv'];
        if (command === 'plugin:dialog|save') return 'C:\\output\\frame.png';
        if (command === 'begin_media_analysis') return 'image-' + ++sequence;
        if (command === 'cancel_media_analysis') return;
        if (command === 'probe_media')
          return {
            id: payload.path,
            path: payload.path,
            name: 'source.mkv',
            sizeBytes: '10000',
            durationSeconds: 12,
            format: 'matroska',
            streams: [
              {
                index: 2,
                kind: 'video',
                codec: 'h264',
                width: 320,
                height: 180,
                frameRate: '24/1',
                sampleRate: null,
                channels: null,
                language: null,
                title: null,
              },
            ],
          };
        if (command === 'run_image_job')
          return {
            outputPath: 'C:\\output\\frame.png',
            frameCount: 1,
            width: 320,
            height: 180,
            notes: [],
          };
        if (command === 'get_completion_status')
          return {
            options: { notify: false, finishAction: 'none' },
            armedJobs: 0,
            secondsRemaining: null,
            error: null,
          };
        throw new Error('Unexpected command ' + command);
      },
    };
  });
  await page.goto('/');
  await page.getByRole('button', { name: 'Add files', exact: true }).first().click();
  await expect(page.getByRole('heading', { name: 'source.mkv', exact: true })).toBeVisible();
  await page.getByRole('button', { name: 'Utilities', exact: true }).click();
  await page.getByText('Images and sequences', { exact: true }).click();
  const region = page.getByRole('region', { name: 'Images and sequences', exact: true });
  await region.getByRole('combobox', { name: 'Operation', exact: true }).selectOption('export');
  return region;
}

async function submittedImages(page: Page) {
  return page.evaluate(() =>
    (globalThis as unknown as { __imageCalls: Call[] }).__imageCalls.filter(
      (call) => call.command === 'run_image_job',
    ),
  );
}

test('explicit 16-bit RGBA PNG is submitted and JPEG ignores PNG-only depth', async ({ page }) => {
  const region = await setup(page);
  await region
    .getByRole('combobox', { name: 'PNG color depth', exact: true })
    .selectOption('rgba64');
  await region.getByRole('button', { name: 'Export', exact: true }).click();
  await expect.poll(async () => (await submittedImages(page)).length).toBe(1);
  expect((await submittedImages(page))[0].payload.request).toMatchObject({
    operation: 'export',
    inputPath: 'C:\\media\\source.mkv',
    streamIndex: 2,
    format: 'png',
    pixelFormat: 'rgba64',
  });

  await region.getByRole('combobox', { name: 'Output', exact: true }).selectOption('jpeg');
  await expect(region.getByRole('combobox', { name: 'PNG color depth', exact: true })).toHaveCount(
    0,
  );
  await region.getByRole('button', { name: 'Export', exact: true }).click();
  await expect.poll(async () => (await submittedImages(page)).length).toBe(2);
  expect((await submittedImages(page))[1].payload.request).toMatchObject({
    operation: 'export',
    format: 'jpeg',
  });
  expect((await submittedImages(page))[1].payload.request).not.toHaveProperty('pixelFormat');
});
