import { expect, test, type Page } from '@playwright/test';
import type {
  BatchEncodeRequest,
  EncodeRequest,
  FolderScanResult,
  JobSnapshot,
  MediaFile,
  MediaStream,
} from '../../src/lib/ipc/generated';

const stream: MediaStream = {
  index: 2,
  kind: 'video',
  codec: 'h264',
  width: 1280,
  height: 720,
  frameRate: '24000/1001',
  sampleRate: null,
  channels: null,
  language: null,
  title: null,
};
const episode = (number: number): MediaFile => ({
  id: `episode-${number}`,
  path: `C:\\media\\Episode ${number}.mkv`,
  name: `Episode ${number}.mkv`,
  sizeBytes: '12345',
  durationSeconds: 1400,
  format: 'matroska',
  streams: [
    stream,
    { ...stream, index: 11, kind: 'attachment', codec: 'ttf' },
    { ...stream, index: 5, kind: 'audio', codec: 'aac', language: 'jpn', channels: 2 },
    { ...stream, index: 8, kind: 'subtitle', codec: 'ass', language: 'eng' },
    { ...stream, index: 9, title: 'Alternate video' },
  ],
});
const episodes = [episode(1), episode(2)];
type Mock = {
  calls: { command: string; payload: Record<string, unknown> }[];
  release: (key: string) => void;
  pickFiles: (files: MediaFile[]) => void;
  hold: (key: string) => void;
};

async function desktopMock(
  page: Page,
  options: {
    scan?: FolderScanResult;
    held?: string[];
    invalidPath?: string;
    queueFailure?: boolean;
    lateTerminal?: boolean;
    missing?: string;
    files?: MediaFile[];
    cancelFolder?: boolean;
  } = {},
) {
  await page.addInitScript(
    ({ initialFiles, options }) => {
      const host = globalThis as unknown as Record<string, unknown>;
      let pickedFiles = initialFiles;
      const media = new Map(initialFiles.map((file) => [file.path, file]));
      let jobs: JobSnapshot[] = [];
      let channel: { onmessage: (jobs: JobSnapshot[]) => void } | undefined;
      let callback = 0;
      const held = new Set(options.held ?? []);
      const waits = new Map<string, (() => void)[]>();
      const wait = async (key: string) => {
        if (held.has(key))
          await new Promise<void>((resolve) =>
            waits.set(key, [...(waits.get(key) ?? []), resolve]),
          );
      };
      const calls: Mock['calls'] = [];
      host.__batchMock = {
        calls,
        release: (key) => {
          held.delete(key);
          waits.get(key)?.forEach((resolve) => resolve());
          waits.delete(key);
        },
        hold: (key) => held.add(key),
        pickFiles: (files) => {
          pickedFiles = files;
          files.forEach((file) => media.set(file.path, file));
        },
      } satisfies Mock;
      host.isTauri = true;
      host.__TAURI_INTERNALS__ = {
        metadata: { currentWindow: { label: 'main' }, currentWebview: { label: 'main' } },
        transformCallback: () => ++callback,
        unregisterCallback: () => {},
        invoke: async (command: string, payload: Record<string, unknown> = {}) => {
          calls.push({ command, payload });
          if (command === 'get_capabilities')
            return ['ffmpeg', 'ffprobe', 'svt-av1', 'av1an'].map((id) => ({
              id,
              name: id,
              available: options.missing !== id,
              path: `C:\\tools\\${id}.exe`,
              version: 'test',
              detail: null,
            }));
          if (command === 'plugin:dialog|open') {
            const config = payload.options as { directory?: boolean; title?: string };
            if (config.directory)
              return config.title?.includes('output')
                ? 'C:\\exports'
                : options.cancelFolder
                  ? null
                  : 'C:\\media';
            return pickedFiles.map((file) => file.path);
          }
          if (command.startsWith('plugin:event|')) return ++callback;
          if (command === 'subscribe_jobs') {
            channel = payload.channel as typeof channel;
            channel?.onmessage(jobs);
            return;
          }
          if (command === 'scan_media_folder') {
            await wait('scan');
            return (
              options.scan ?? {
                paths: initialFiles.map((file) => file.path),
                errors: [],
                skippedCount: 0,
                truncated: false,
              }
            );
          }
          if (command === 'probe_media') {
            await wait(`probe:${payload.path}`);
            const file = media.get(payload.path as string);
            if (!file)
              throw {
                code: 'PROBE_FAILED',
                message: 'This media could not be probed.',
                path: payload.path,
              };
            return file;
          }
          if (command === 'preview_encode_batch') {
            const request = payload.request as BatchEncodeRequest;
            await wait('preview');
            return {
              items: request.inputs.map((input) => {
                if (input.inputPath === options.invalidPath)
                  return {
                    inputPath: input.inputPath,
                    outputPath: null,
                    request: null,
                    error: {
                      code: 'UNSUPPORTED_SOURCE',
                      message: 'This source has unsupported HDR metadata.',
                      path: input.inputPath,
                    },
                  };
                const outputPath =
                  request.outputDirectory +
                  '\\' +
                  input.inputPath
                    .split('\\')
                    .at(-1)!
                    .replace(/\.mkv$/, '') +
                  '_av1.mkv';
                return {
                  inputPath: input.inputPath,
                  outputPath,
                  request: {
                    source: {
                      inputPath: input.inputPath,
                      outputPath,
                      streamIndices: input.streamIndices,
                    },
                    settings: {
                      videoStreamIndex: input.videoStreamIndex,
                      crf: request.crf,
                      preset: request.preset,
                      backend: request.backend,
                      workers: request.workers,
                      filmGrain: request.filmGrain,
                      hdr10Fallback: request.hdr10Fallback,
                    },
                  },
                  error: null,
                };
              }),
            };
          }
          if (command === 'enqueue_encode_batch') {
            const requests = payload.requests as EncodeRequest[];
            const added = requests.map((request, index): JobSnapshot => ({
              id: `batch-${jobs.length + index + 1}`,
              state: 'queued',
              request: request.source,
              encodeSettings: request.settings,
              progressSeconds: 0,
              durationSeconds: 1400,
              logs: [],
              error: null,
              logPath: null,
            }));
            if (!options.queueFailure) {
              jobs = [
                ...added
                  .map((job) => ({
                    ...job,
                    state: options.lateTerminal ? ('succeeded' as const) : job.state,
                  }))
                  .reverse(),
                ...jobs,
              ];
              channel?.onmessage(jobs);
            }
            await wait('enqueue');
            if (options.queueFailure)
              throw {
                code: 'DESTINATION_EXISTS',
                message: 'A reviewed destination now exists.',
                path: requests[0].source.outputPath,
              };
            return added;
          }
          if (command === 'cancel_all_jobs') {
            jobs = jobs.map((job) => ({ ...job, state: 'canceled' }));
            channel?.onmessage(jobs);
            return jobs;
          }
          throw new Error(`Unexpected command: ${command}`);
        },
      };
    },
    { initialFiles: options.files ?? episodes, options },
  );
}

async function calls(page: Page, command: string) {
  return page.evaluate(
    (name) =>
      (globalThis as unknown as { __batchMock: Mock }).__batchMock.calls.filter(
        (call) => call.command === name,
      ),
    command,
  );
}
async function release(page: Page, key: string) {
  await page.evaluate(
    (name) => (globalThis as unknown as { __batchMock: Mock }).__batchMock.release(name),
    key,
  );
}
async function openBatch(page: Page) {
  await page.goto('/');
  await page.getByRole('button', { name: 'Add files', exact: true }).first().click();
  await expect(page.getByRole('heading', { name: 'Episode 2.mkv', exact: true })).toBeVisible();
  await page.getByRole('button', { name: 'Batch encode', exact: true }).click();
  await page.getByRole('button', { name: 'Choose output folder', exact: true }).click();
}

test('folder import passes recursion and reports scan limits, skipped entries, and individual probe failures', async ({
  page,
}) => {
  const broken = 'C:\\media\\broken.mkv';
  await desktopMock(page, {
    scan: {
      paths: [episodes[0].path, broken, episodes[0].path, episodes[1].path],
      errors: [
        {
          code: 'SCAN_ERROR',
          message: 'Cannot read protected folder.',
          path: 'C:\\media\\protected',
        },
      ],
      skippedCount: 7,
      truncated: true,
    },
  });
  await page.goto('/');
  await page.getByLabel('Include subfolders').check();
  await page.getByRole('button', { name: 'Add folder', exact: true }).click();
  await expect(page.getByRole('heading', { name: 'Episode 2.mkv', exact: true })).toBeVisible();
  expect((await calls(page, 'scan_media_folder'))[0].payload).toEqual({
    request: { path: 'C:\\media', recursive: true },
  });
  expect((await calls(page, 'probe_media')).map((call) => call.payload.path)).toEqual([
    episodes[0].path,
    broken,
    episodes[1].path,
  ]);
  await expect(page.getByRole('alert')).toContainText('Cannot read protected folder.');
  await expect(page.getByRole('alert')).toContainText('This media could not be probed.');
  await expect(page.getByText(/7 entries skipped.*Scan truncated/)).toBeVisible();
});

test('stopping a scan discards its late result', async ({ page }) => {
  await desktopMock(page, { held: ['scan'] });
  await page.goto('/');
  await page.getByRole('button', { name: 'Add folder', exact: true }).click();
  await expect.poll(() => calls(page, 'scan_media_folder').then((value) => value.length)).toBe(1);
  await page.getByRole('button', { name: 'Stop import', exact: true }).click();
  await release(page, 'scan');
  await expect(page.getByText(/Stopped importing. Completed files are kept/)).toBeVisible();
  expect(await calls(page, 'probe_media')).toHaveLength(0);
  await expect(page.getByRole('button', { name: 'Add folder', exact: true })).toBeEnabled();
});

test('an old stopped probe cannot add files or finish a newer import', async ({ page }) => {
  const third = episode(3);
  await desktopMock(page, { held: [`probe:${episodes[1].path}`, `probe:${third.path}`] });
  await page.goto('/');
  await page.getByRole('button', { name: 'Add folder', exact: true }).click();
  await expect.poll(() => calls(page, 'probe_media').then((value) => value.length)).toBe(2);
  await page.getByRole('button', { name: 'Stop import', exact: true }).click();
  await page.evaluate(
    (file) => (globalThis as unknown as { __batchMock: Mock }).__batchMock.pickFiles([file]),
    third,
  );
  await page.getByRole('button', { name: 'Add files', exact: true }).first().click();
  await expect.poll(() => calls(page, 'probe_media').then((value) => value.length)).toBe(3);
  await release(page, `probe:${episodes[1].path}`);
  await expect(page.getByRole('button', { name: 'Stop import', exact: true })).toBeVisible();
  await expect(page.getByRole('button', { name: /Episode 2.mkv/ })).toHaveCount(0);
  await release(page, `probe:${third.path}`);
  await expect(page.getByRole('heading', { name: third.name, exact: true })).toBeVisible();
  await expect(page.getByRole('button', { name: /^Episode 1.mkv/ })).toBeVisible();
});

test('batch defaults preserve original per-file stream indices and keep attachments last', async ({
  page,
}) => {
  await desktopMock(page);
  await openBatch(page);
  await expect(page.getByLabel('CRF', { exact: true })).toHaveValue('30');
  await expect(page.getByLabel('Preset', { exact: true })).toHaveValue('4');
  await page.getByText('Video and copied tracks · 3 copies', { exact: true }).first().click();
  await page.getByLabel('Video stream for Episode 1.mkv', { exact: true }).selectOption('9');
  await page.getByLabel('Copy stream #5 from Episode 1.mkv', { exact: true }).uncheck();
  await page.getByLabel('CRF', { exact: true }).fill('27');
  await page.getByRole('button', { name: 'Preview batch', exact: true }).click();
  await expect(page.getByRole('region', { name: 'Batch output preview' })).toContainText(
    '2 ready / 2 reviewed',
  );
  expect((await calls(page, 'preview_encode_batch'))[0].payload).toEqual({
    request: {
      outputDirectory: 'C:\\exports',
      crf: 27,
      preset: 4,
      backend: 'svtAv1',
      workers: 2,
      filmGrain: 0,
      hdr10Fallback: false,
      inputs: [
        { inputPath: episodes[0].path, videoStreamIndex: 9, streamIndices: [9, 8, 11] },
        { inputPath: episodes[1].path, videoStreamIndex: 2, streamIndices: [2, 5, 8, 11] },
      ],
    },
  });
  await page.getByRole('button', { name: 'Queue ready files', exact: true }).click();
  const requests = (await calls(page, 'enqueue_encode_batch'))[0].payload
    .requests as EncodeRequest[];
  expect(requests.map((request) => request.source.streamIndices)).toEqual([
    [9, 8, 11],
    [2, 5, 8, 11],
  ]);
  await expect(page.getByLabel('Select Episode 1.mkv', { exact: true })).not.toBeChecked();
  await expect(page.getByLabel('Select Episode 2.mkv', { exact: true })).not.toBeChecked();
  await expect(page.getByRole('button', { name: 'Queue ready files', exact: true })).toBeDisabled();
  await page.getByRole('button', { name: 'Quick Convert', exact: true }).click();
  await expect(page.getByRole('button', { name: 'Stop queue', exact: true })).toBeVisible();
});

test('invalid preview rows remain visible and only ready requests enter one batch command', async ({
  page,
}) => {
  await desktopMock(page, { invalidPath: episodes[1].path });
  await openBatch(page);
  await page.getByRole('button', { name: 'Preview batch', exact: true }).click();
  await expect(page.getByRole('region', { name: 'Batch output preview' })).toContainText(
    'unsupported HDR metadata',
  );
  await expect(page.getByRole('region', { name: 'Batch output preview' })).toContainText(
    '1 ready / 2 reviewed',
  );
  await page.getByRole('button', { name: 'Queue ready files', exact: true }).click();
  expect((await calls(page, 'enqueue_encode_batch'))[0].payload.requests).toHaveLength(1);
  await expect(page.getByLabel('Select Episode 2.mkv', { exact: true })).toBeChecked();
});

test('draft edits discard late preview replies and require review again', async ({ page }) => {
  await desktopMock(page, { held: ['preview'] });
  await openBatch(page);
  await page.getByRole('button', { name: 'Preview batch', exact: true }).click();
  await expect
    .poll(() => calls(page, 'preview_encode_batch').then((value) => value.length))
    .toBe(1);
  await page.getByLabel('CRF', { exact: true }).fill('23');
  await release(page, 'preview');
  await expect(page.getByRole('button', { name: 'Queue ready files', exact: true })).toBeDisabled();
  await expect(page.getByRole('region', { name: 'Batch output preview' })).not.toContainText(
    '_av1.mkv',
  );
  await page.getByRole('button', { name: 'Preview batch', exact: true }).click();
  await expect(page.getByRole('button', { name: 'Queue ready files', exact: true })).toBeEnabled();
  await page.getByLabel('Output folder', { exact: true }).fill('C:\\other');
  await expect(page.getByRole('button', { name: 'Queue ready files', exact: true })).toBeDisabled();
  await page.getByLabel('Output folder', { exact: true }).fill('C:\\exports');
  await expect(page.getByRole('button', { name: 'Queue ready files', exact: true })).toBeDisabled();
});

test('removing a source invalidates a pending preview while common settings survive tab navigation', async ({
  page,
}) => {
  await desktopMock(page, { held: ['preview'] });
  await openBatch(page);
  await page.getByLabel('CRF', { exact: true }).fill('25');
  await page.getByRole('button', { name: 'Preview batch', exact: true }).click();
  await page.getByRole('button', { name: /^Files/ }).click();
  await page.getByRole('button', { name: 'Remove Episode 2.mkv', exact: true }).click();
  await page.getByRole('button', { name: 'Batch encode', exact: true }).click();
  await release(page, 'preview');
  await expect(page.getByLabel('CRF', { exact: true })).toHaveValue('25');
  await expect(page.getByLabel('Output folder', { exact: true })).toHaveValue('C:\\exports');
  await expect(page.getByRole('button', { name: 'Queue ready files', exact: true })).toBeDisabled();
  await expect(page.getByLabel('Select Episode 2.mkv', { exact: true })).toHaveCount(0);
});

test('duplicate clicks submit once and late queued replies do not regress terminal snapshots', async ({
  page,
}) => {
  await desktopMock(page, { held: ['enqueue'], lateTerminal: true });
  await openBatch(page);
  await page.getByRole('button', { name: 'Preview batch', exact: true }).click();
  const queue = page.getByRole('button', { name: 'Queue ready files', exact: true });
  await expect(queue).toBeEnabled();
  await queue.evaluate((button: HTMLButtonElement) => {
    button.click();
    button.click();
  });
  await expect(page.getByRole('button', { name: 'Queueing…', exact: true })).toBeDisabled();
  expect(await calls(page, 'enqueue_encode_batch')).toHaveLength(1);
  await release(page, 'enqueue');
  await expect(page.getByText('Succeeded', { exact: true }).first()).toBeVisible();
  await expect(page.getByRole('button', { name: 'Stop queue', exact: true })).toHaveCount(0);
  await expect(page.getByLabel('Select Episode 1.mkv', { exact: true })).not.toBeChecked();
});

test('a destination admission failure preserves selections and requires a fresh preview', async ({
  page,
}) => {
  await desktopMock(page, { queueFailure: true });
  await openBatch(page);
  await page.getByRole('button', { name: 'Preview batch', exact: true }).click();
  await page.getByRole('button', { name: 'Queue ready files', exact: true }).click();
  await expect(page.getByRole('alert')).toContainText('Preview the batch again before retrying.');
  await expect(page.getByLabel('Select Episode 1.mkv', { exact: true })).toBeChecked();
  await expect(page.getByRole('button', { name: 'Queue ready files', exact: true })).toBeDisabled();
});

test('missing tools and invalid settings block batch preparation; defaults can be restored', async ({
  page,
}) => {
  await desktopMock(page, { missing: 'svt-av1' });
  await openBatch(page);
  await expect(page.getByRole('button', { name: 'Preview batch', exact: true })).toBeDisabled();
  await expect(
    page
      .getByText('Install FFmpeg, FFprobe, standalone SVT-AV1, then refresh Tools & settings.', {
        exact: true,
      })
      .last(),
  ).toBeVisible();
  await page.getByLabel('CRF', { exact: true }).fill('64');
  await page.getByLabel('Preset', { exact: true }).selectOption('8');
  await page.getByRole('button', { name: 'Reset batch settings', exact: true }).click();
  await expect(page.getByLabel('CRF', { exact: true })).toHaveValue('30');
  await expect(page.getByLabel('Preset', { exact: true })).toHaveValue('4');
});

test('a batch selects at most 100 imported episodes and the minimum-width layout stays within the window', async ({
  page,
}) => {
  const many = Array.from({ length: 101 }, (_, index) => episode(index + 1));
  await desktopMock(page, { files: many });
  await page.setViewportSize({ width: 760, height: 600 });
  await page.goto('/');
  await page.getByRole('button', { name: 'Add folder', exact: true }).click();
  await expect(page.getByRole('heading', { name: 'Episode 101.mkv', exact: true })).toBeVisible();
  await page.getByRole('button', { name: 'Batch encode', exact: true }).click();
  await expect(page.getByRole('checkbox', { name: /^Select /, checked: true })).toHaveCount(100);
  await expect(page.getByLabel('Select Episode 101.mkv', { exact: true })).toBeDisabled();
  expect(await page.evaluate(() => document.documentElement.scrollWidth <= innerWidth)).toBe(true);
  await page.screenshot({ path: test.info().outputPath('minimum-width.png'), fullPage: true });
});

test('browser sample remains available for inspection but cannot become a batch encode', async ({
  page,
}) => {
  await page.goto('/');
  await page.getByRole('button', { name: 'Load sample', exact: true }).click();
  await page.getByRole('button', { name: 'Batch encode', exact: true }).click();
  await expect(page.getByLabel('Select Coastal walk.mkv', { exact: true })).toBeDisabled();
  await expect(page.getByRole('button', { name: 'Preview batch', exact: true })).toBeDisabled();
});

test('canceling a native folder picker restores import controls without scanning', async ({
  page,
}) => {
  await desktopMock(page, { cancelFolder: true });
  await page.goto('/');
  await page.getByRole('button', { name: 'Add folder', exact: true }).click();
  await expect(page.getByRole('button', { name: 'Add folder', exact: true })).toBeEnabled();
  await expect(page.getByRole('button', { name: 'Stop import', exact: true })).toHaveCount(0);
  expect(await calls(page, 'scan_media_folder')).toHaveLength(0);
});

test('invalid CRF, empty destination, and empty selection block otherwise available preview', async ({
  page,
}) => {
  await desktopMock(page);
  await openBatch(page);
  const preview = page.getByRole('button', { name: 'Preview batch', exact: true });
  await expect(preview).toBeEnabled();
  await page.getByLabel('CRF', { exact: true }).fill('0');
  await expect(preview).toBeDisabled();
  await page.getByLabel('CRF', { exact: true }).fill('25.5');
  await expect(preview).toBeDisabled();
  await page.getByRole('button', { name: 'Reset batch settings', exact: true }).click();
  await expect(preview).toBeEnabled();
  await page.getByLabel('Output folder', { exact: true }).fill('   ');
  await expect(preview).toBeDisabled();
  await page.getByLabel('Output folder', { exact: true }).fill('C:\\exports');
  await page.getByRole('button', { name: 'Clear selection', exact: true }).click();
  await expect(preview).toBeDisabled();
  expect(await calls(page, 'preview_encode_batch')).toHaveLength(0);
});

test('a late batch reply clears submitted rows while preserving a newly imported selection', async ({
  page,
}) => {
  await desktopMock(page, { held: ['enqueue'] });
  await openBatch(page);
  await page.getByRole('button', { name: 'Preview batch', exact: true }).click();
  await page.getByRole('button', { name: 'Queue ready files', exact: true }).click();
  await page.getByRole('button', { name: /^Files/ }).click();
  const third = episode(3);
  await page.evaluate(
    (file) => (globalThis as unknown as { __batchMock: Mock }).__batchMock.pickFiles([file]),
    third,
  );
  await page.getByRole('button', { name: 'Add files', exact: true }).first().click();
  await expect(page.getByRole('heading', { name: third.name, exact: true })).toBeVisible();
  await page.getByRole('button', { name: 'Batch encode', exact: true }).click();
  await release(page, 'enqueue');
  await expect(page.getByLabel('Select Episode 1.mkv', { exact: true })).not.toBeChecked();
  await expect(page.getByLabel('Select Episode 3.mkv', { exact: true })).toBeChecked();
  await expect(page.getByRole('button', { name: 'Queue ready files', exact: true })).toBeDisabled();
  expect(await calls(page, 'enqueue_encode_batch')).toHaveLength(1);
});

for (const setting of ['grain', 'fallback', 'backend', 'workers'] as const) {
  test(`changing ${setting} invalidates a late batch preview and preserves the submitted settings`, async ({
    page,
  }) => {
    await desktopMock(page, { held: ['preview'] });
    await openBatch(page);
    const workspace = page.getByRole('region', { name: 'Batch encode workspace' });
    if (setting === 'workers')
      await workspace.getByLabel('Encode backend', { exact: true }).selectOption('av1an');
    await page.getByRole('button', { name: 'Preview batch', exact: true }).click();
    await expect
      .poll(() => calls(page, 'preview_encode_batch').then((items) => items.length))
      .toBe(1);
    const original = (await calls(page, 'preview_encode_batch'))[0].payload;
    if (setting === 'grain')
      await workspace.getByLabel('Film grain synthesis', { exact: true }).fill('10');
    if (setting === 'fallback')
      await workspace.getByLabel('Allow HDR10 fallback', { exact: true }).check();
    if (setting === 'backend')
      await workspace.getByLabel('Encode backend', { exact: true }).selectOption('av1an');
    if (setting === 'workers')
      await workspace.getByLabel('Parallel chunks', { exact: true }).fill('4');
    await release(page, 'preview');
    await expect(
      page.getByRole('button', { name: 'Queue ready files', exact: true }),
    ).toBeDisabled();
    expect((await calls(page, 'preview_encode_batch'))[0].payload).toEqual(original);
    await page.getByRole('button', { name: 'Preview batch', exact: true }).click();
    await page.getByRole('button', { name: 'Queue ready files', exact: true }).click();
    const submitted = (await calls(page, 'enqueue_encode_batch'))[0].payload
      .requests as EncodeRequest[];
    expect(submitted).toHaveLength(2);
    for (const request of submitted) {
      expect(request.settings).toMatchObject({
        backend: setting === 'backend' || setting === 'workers' ? 'av1an' : 'svtAv1',
        workers: setting === 'workers' ? 4 : 2,
        filmGrain: setting === 'grain' ? 10 : 0,
        hdr10Fallback: setting === 'fallback',
      });
    }
    await page.getByRole('button', { name: 'Reset batch settings', exact: true }).click();
    expect((await calls(page, 'enqueue_encode_batch'))[0].payload.requests).toEqual(submitted);
    await expect(workspace.getByLabel('Film grain synthesis', { exact: true })).toHaveValue('0');
    await expect(workspace.getByLabel('Allow HDR10 fallback', { exact: true })).not.toBeChecked();
    await expect(workspace.getByLabel('Encode backend', { exact: true })).toHaveValue('svtAv1');
  });
}

test('batch validates grain and requires av1an only when selected', async ({ page }) => {
  await desktopMock(page, { missing: 'av1an' });
  await openBatch(page);
  const workspace = page.getByRole('region', { name: 'Batch encode workspace' });
  const preview = page.getByRole('button', { name: 'Preview batch', exact: true });
  for (const invalid of ['-1', '51', '1.5', '']) {
    await workspace.getByLabel('Film grain synthesis', { exact: true }).fill(invalid);
    await expect(preview).toBeDisabled();
  }
  await workspace.getByLabel('Film grain synthesis', { exact: true }).fill('0');
  await expect(preview).toBeEnabled();
  await workspace.getByLabel('Encode backend', { exact: true }).selectOption('av1an');
  await expect(preview).toBeDisabled();
  await workspace.getByLabel('Encode backend', { exact: true }).selectOption('svtAv1');
  await expect(preview).toBeEnabled();
});

test('batch rejects invalid parallel chunk counts before native preview', async ({ page }) => {
  await desktopMock(page);
  await openBatch(page);
  const workspace = page.getByRole('region', { name: 'Batch encode workspace' });
  const preview = page.getByRole('button', { name: 'Preview batch', exact: true });
  await workspace.getByLabel('Encode backend', { exact: true }).selectOption('av1an');
  for (const invalid of ['0', '33', '2.5', '']) {
    await workspace.getByLabel('Parallel chunks', { exact: true }).fill(invalid);
    await expect(preview).toBeDisabled();
  }
  await workspace.getByLabel('Parallel chunks', { exact: true }).fill('2');
  await expect(preview).toBeEnabled();
  expect(await calls(page, 'preview_encode_batch')).toHaveLength(0);
});
