import { test, expect, type Page } from '@playwright/test';
type Call = { command: string; payload: Record<string, unknown> };
async function setup(page: Page, hold = false) {
  await page.addInitScript(
    ({ hold }) => {
      const state = globalThis as unknown as Record<string, unknown>;
      const mock = {
        calls: [] as Call[],
        release: null as (() => void) | null,
        completion: {
          options: { notify: false, finishAction: 'none' },
          armedJobs: 0,
          secondsRemaining: null as number | null,
          error: null,
        },
      };
      state.__utilityMock = mock;
      state.isTauri = true;
      let sequence = 0;
      state.__TAURI_INTERNALS__ = {
        metadata: { currentWindow: { label: 'main' }, currentWebview: { label: 'main' } },
        transformCallback: () => ++sequence,
        unregisterCallback: () => {},
        invoke: async (command: string, payload: Record<string, unknown> = {}) => {
          mock.calls.push({ command, payload });
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
          if (command === 'plugin:dialog|open') {
            const options = payload.options as { title?: string };
            if (options?.title === 'Choose images in sequence')
              return ['C:\\media\\z.png', 'C:\\media\\a.png'];
            if (options?.title === 'Inspect saved job') return 'C:\\media\\foreign.json';
            return ['C:\\media\\source.mkv'];
          }
          if (command === 'plugin:dialog|save') return 'C:\\output\\new.mkv';
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
                {
                  index: 4,
                  kind: 'subtitle',
                  codec: 'hdmv_pgs_subtitle',
                  width: null,
                  height: null,
                  frameRate: null,
                  sampleRate: null,
                  channels: null,
                  language: 'eng',
                  title: null,
                },
              ],
            };
          if (command === 'begin_media_analysis') return 'utility-' + ++sequence;
          if (command === 'cancel_media_analysis') return;
          if (command === 'run_utility') {
            if (hold) await new Promise<void>((resolve) => (mock.release = resolve));
            return {
              kind: 'artifact',
              result: {
                operation: 'cut',
                outputPath: 'C:\\output\\new.mkv',
                sizeBytes: '200',
                durationSeconds: 2,
                sourceFingerprints: ['verified'],
                message: 'Output verified.',
                diagnostics: ['Packets retained.'],
              },
            };
          }
          if (command === 'run_image_job')
            return {
              outputPath: 'C:\\output\\new.mkv',
              frameCount: 2,
              width: 320,
              height: 180,
              notes: ['Explicit image order retained.'],
            };
          if (command === 'inspect_saved_job')
            return {
              path: payload.path,
              compatible: false,
              request: null,
              message:
                'This saved job cannot be resumed here. Continue it in the application that created it.',
            };
          if (command === 'get_completion_status') return mock.completion;
          if (command === 'set_completion_options') {
            mock.completion = {
              options: { ...(payload.options as { notify: boolean; finishAction: string }) },
              armedJobs: 1,
              secondsRemaining: 60,
              error: null,
            };
            return mock.completion;
          }
          if (command === 'cancel_finish_action') {
            mock.completion.options.finishAction = 'none';
            mock.completion.armedJobs = 0;
            mock.completion.secondsRemaining = null;
            return mock.completion;
          }
          throw new Error('Unexpected command ' + command);
        },
      };
    },
    { hold },
  );
  await page.goto('/');
  await page.getByRole('button', { name: 'Add files', exact: true }).first().click();
  await expect(page.getByRole('heading', { name: 'source.mkv', exact: true })).toBeVisible();
  await page.getByRole('button', { name: 'Utilities', exact: true }).click();
}
async function calls(page: Page, command: string) {
  return page.evaluate(
    (command) =>
      (globalThis as unknown as { __utilityMock: { calls: Call[] } }).__utilityMock.calls.filter(
        (c) => c.command === command,
      ),
    command,
  );
}

test('keyframe cut submits the reviewed source interval and shows validation', async ({ page }) => {
  await setup(page);
  const region = page.getByRole('region', { name: 'Media utilities', exact: true });
  await region.getByLabel('Start (seconds)', { exact: true }).fill('2.5');
  await region.getByLabel('End (seconds)', { exact: true }).fill('6');
  await region.getByRole('button', { name: 'Run utility', exact: true }).click();
  await expect(region.getByText('Output verified.', { exact: true })).toBeVisible();
  expect((await calls(page, 'run_utility'))[0].payload.request).toEqual({
    kind: 'keyframeCut',
    request: {
      inputPath: 'C:\\media\\source.mkv',
      outputPath: 'C:\\output\\new.mkv',
      startSeconds: 2.5,
      endSeconds: 6,
    },
  });
  if (process.env.JESSES_CAPTURE_UI) {
    await page.screenshot({ path: 'target/utilities-desktop.png', fullPage: true });
    await page.setViewportSize({ width: 760, height: 600 });
    await page.screenshot({ path: 'target/utilities-minimum.png', fullPage: true });
  }
});
test('image sequence uses reviewed order instead of sorting source names', async ({ page }) => {
  await setup(page);
  await page.getByText('Images and sequences', { exact: true }).click();
  const region = page.getByRole('region', { name: 'Images and sequences', exact: true });
  await region.getByRole('button', { name: 'Choose images', exact: true }).click();
  await region.getByRole('button', { name: 'Move image 2 up', exact: true }).click();
  await region.getByLabel('Frame-rate numerator', { exact: true }).fill('24000');
  await region.getByLabel('Frame-rate denominator', { exact: true }).fill('1001');
  await region.getByRole('button', { name: 'Save and import sequence', exact: true }).click();
  await expect.poll(async () => (await calls(page, 'probe_media')).length).toBe(2);
  await page.getByRole('button', { name: 'Utilities', exact: true }).click();
  await expect(region.getByText('Explicit image order retained.', { exact: true })).toBeVisible();
  expect((await calls(page, 'run_image_job'))[0].payload.request).toEqual({
    operation: 'importSequence',
    paths: ['C:\\media\\a.png', 'C:\\media\\z.png'],
    frameRate: { numerator: 24000, denominator: 1001 },
    outputPath: 'C:\\output\\new.mkv',
  });
});
test('canceled utility ignores a late success and sends the cancellation ticket', async ({
  page,
}) => {
  await setup(page, true);
  const region = page.getByRole('region', { name: 'Media utilities', exact: true });
  await region.getByRole('button', { name: 'Run utility', exact: true }).click();
  await expect.poll(async () => (await calls(page, 'run_utility')).length).toBe(1);
  await region.getByRole('button', { name: 'Cancel utility', exact: true }).click();
  await expect
    .poll(async () => (await calls(page, 'cancel_media_analysis')).length)
    .toBeGreaterThan(0);
  await page.evaluate(() =>
    (
      globalThis as unknown as { __utilityMock: { release: (() => void) | null } }
    ).__utilityMock.release?.(),
  );
  await expect(region.getByRole('alert')).toHaveText('Utility canceled.');
  await expect(region.getByText('Output verified.', { exact: true })).toHaveCount(0);
});
test('foreign saved jobs explain compatibility without queueing saved commands', async ({
  page,
}) => {
  await setup(page);
  await page.getByText('Saved requests and resume compatibility', { exact: true }).click();
  await page.getByRole('button', { name: 'Inspect saved job', exact: true }).click();
  await expect(page.getByText(/Continue it in the application that created it/)).toBeVisible();
  await expect(
    page.getByRole('button', { name: 'Choose destination and queue a new encode', exact: true }),
  ).toHaveCount(0);
  expect(await calls(page, 'enqueue_encode')).toHaveLength(0);
});
test('finish action is explicit, session-only and can be canceled from its countdown', async ({
  page,
}) => {
  await setup(page);
  await page.getByRole('button', { name: 'Quick Convert', exact: true }).click();
  await page.getByText('When the queue finishes', { exact: true }).click();
  await page.getByLabel('Finish action', { exact: true }).selectOption('shutdown');
  await page.getByRole('button', { name: 'Apply to this queue', exact: true }).click();
  const banner = page.getByRole('complementary', { name: 'Armed finish action', exact: true });
  await expect(banner).toBeVisible();
  await expect(banner).toContainText('Computer shutdown');
  await page.getByText('When the queue finishes', { exact: true }).click();
  await page.getByRole('button', { name: 'Utilities', exact: true }).click();
  await expect(banner).toBeVisible();
  await expect(banner).toContainText('60 seconds remaining');
  await page.getByRole('button', { name: 'Cancel finish action', exact: true }).click();
  await expect(page.getByRole('button', { name: 'Cancel finish action', exact: true })).toHaveCount(
    0,
  );
  expect((await calls(page, 'set_completion_options'))[0].payload.options).toEqual({
    notify: false,
    finishAction: 'shutdown',
  });
  expect(await calls(page, 'cancel_finish_action')).toHaveLength(1);
});

test('finish-action failures remain visible outside the queue settings', async ({ page }) => {
  await setup(page);
  await page.evaluate(() => {
    const mock = (
      globalThis as unknown as {
        __utilityMock: { completion: { error: string | null } };
      }
    ).__utilityMock;
    mock.completion.error = 'The system declined shutdown: permission denied';
  });
  await expect(page.getByRole('alert')).toHaveText(
    'The system declined shutdown: permission denied',
  );
  await expect(page.getByText('When the queue finishes', { exact: true })).not.toBeVisible();
});

test('server disarm clears the selected finish action while ordinary polling preserves edits', async ({
  page,
}) => {
  await setup(page);
  await page.getByRole('button', { name: 'Quick Convert', exact: true }).click();
  await page.getByText('When the queue finishes', { exact: true }).click();
  const select = page.getByLabel('Finish action', { exact: true });
  await select.selectOption('shutdown');
  await expect
    .poll(async () => (await calls(page, 'get_completion_status')).length)
    .toBeGreaterThan(1);
  await expect(select).toHaveValue('shutdown');
  await page.getByRole('button', { name: 'Apply to this queue', exact: true }).click();
  await expect(
    page.getByRole('complementary', { name: 'Armed finish action', exact: true }),
  ).toBeVisible();
  await select.selectOption('closeApp');
  await page.evaluate(() => {
    const mock = (
      globalThis as unknown as {
        __utilityMock: {
          completion: {
            options: { finishAction: string };
            armedJobs: number;
            secondsRemaining: number | null;
            error: string | null;
          };
        };
      }
    ).__utilityMock;
    mock.completion.options.finishAction = 'none';
    mock.completion.armedJobs = 0;
    mock.completion.secondsRemaining = null;
    mock.completion.error = 'Finish action disarmed because a job failed.';
  });
  await expect(select).toHaveValue('none');
  await expect(page.getByRole('button', { name: 'Cancel finish action', exact: true })).toHaveCount(
    0,
  );
});
