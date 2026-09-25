import { expect, test, type Page } from '@playwright/test';
import type {
  AutoCropResult,
  BatchEncodeRequest,
  EncodeRequest,
  FramePreviewRequest,
  MediaFile,
} from '../../src/lib/ipc/generated';

const media: MediaFile = {
  id: 'analysis-source',
  path: 'C:\\media\\source.mkv',
  name: 'source.mkv',
  sizeBytes: '1000',
  durationSeconds: 12,
  format: 'matroska',
  streams: [0, 4].map((index) => ({
    index,
    kind: 'video',
    codec: 'h264',
    width: 320,
    height: 180,
    frameRate: '24/1',
    sampleRate: null,
    channels: null,
    language: null,
    title: `Video ${index}`,
  })),
};
type Call = { command: string; payload: Record<string, unknown> };
type Mock = {
  calls: Call[];
  hold: (command: string) => void;
  release: (command: string, index?: number) => void;
  fingerprint: (value: string) => void;
  failNextPreview: () => void;
};
async function mock(page: Page, source: MediaFile = media) {
  await page.addInitScript(
    ({ source }) => {
      let callback = 0;
      let nextId = 0;
      let cropFingerprint = 'original';
      let failNextPreview = false;
      const calls: Call[] = [];
      const held = new Set<string>();
      const pending = new Map<string, (() => void)[]>();
      const state = globalThis as unknown as Record<string, unknown>;
      state.__analysisMock = {
        calls,
        hold: (command) => held.add(command),
        release: (command, index = 0) => {
          const waits = pending.get(command) ?? [];
          waits.splice(index, 1)[0]?.();
          if (!waits.length) held.delete(command);
        },
        fingerprint: (value) => {
          cropFingerprint = value;
        },
        failNextPreview: () => {
          failNextPreview = true;
        },
      } satisfies Mock;
      state.isTauri = true;
      state.__TAURI_INTERNALS__ = {
        metadata: { currentWindow: { label: 'main' }, currentWebview: { label: 'main' } },
        transformCallback: () => ++callback,
        unregisterCallback: () => {},
        invoke: async (command: string, payload: Record<string, unknown> = {}) => {
          calls.push({ command, payload });
          if (held.has(command))
            await new Promise<void>((resolve) =>
              pending.set(command, [...(pending.get(command) ?? []), resolve]),
            );
          if (command.startsWith('plugin:event|')) return ++callback;
          if (command === 'get_capabilities')
            return [
              'ffmpeg',
              'ffprobe',
              'svt-av1',
              'svt-av1-5fish',
              'svt-av1-hdr',
              'x264',
              'av1an',
            ].map((id) => ({
              id,
              name: id,
              available: true,
              path: `C:\\tools\\${id}.exe`,
              version: 'mock',
              detail: null,
            }));
          if (command === 'get_storage_locations') return [];
          if (command === 'plugin:dialog|open')
            return (payload.options as { directory?: boolean })?.directory
              ? 'C:\\exports'
              : [source.path];
          if (command === 'probe_media') return source;
          if (command === 'list_jobs') return [];
          if (command === 'subscribe_jobs') {
            (payload.channel as { onmessage: (jobs: unknown[]) => void }).onmessage([]);
            return;
          }
          if (command === 'begin_media_analysis') return `analysis-${++nextId}`;
          if (command === 'cancel_media_analysis') return;
          if (command === 'preview_frame') {
            if (failNextPreview) {
              failNextPreview = false;
              throw { code: 'SOURCE_CHANGED', message: 'The source changed during analysis.' };
            }
            const request = payload.request as FramePreviewRequest;
            return {
              imageDataUrl:
                'data:image/png;base64,iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAQAAAC1HAwCAAAAC0lEQVR42mNk+A8AAQUBAScY42YAAAAASUVORK5CYII=',
              width: 320,
              height: 180,
              sourceWidth: 320,
              sourceHeight: 180,
              positionSeconds: request.positionSeconds,
              sourceFingerprint: 'original',
              toneMapped: request.videoStreamIndex === 4,
            };
          }
          if (command === 'detect_crop')
            return {
              crop: { top: 16, right: 8, bottom: 16, left: 8 },
              sourceWidth: 320,
              sourceHeight: 180,
              sampleCount: 10,
              sampledFrames: 60,
              agreementPercent: 100,
              sourceFingerprint: cropFingerprint,
              message: 'Review the detected crop before applying it.',
            } satisfies AutoCropResult;
          if (command === 'preview_encode_batch') {
            const request = payload.request as BatchEncodeRequest;
            return {
              items: request.inputs.map((input) => ({
                inputPath: input.inputPath,
                outputPath: `${request.outputDirectory}\\source.mkv`,
                error: null,
                request: {
                  source: {
                    inputPath: input.inputPath,
                    outputPath: `${request.outputDirectory}\\source.mkv`,
                    streamIndices: input.streamIndices,
                  },
                  settings: {
                    ...request,
                    videoStreamIndex: input.videoStreamIndex,
                    framing: input.framing,
                    audio: input.audio,
                  },
                },
              })),
            };
          }
          if (command === 'enqueue_encode_batch') {
            return (payload.requests as EncodeRequest[]).map((request, index) => ({
              id: `batch-${nextId++}-${index}`,
              state: 'queued',
              request: request.source,
              encodeSettings: request.settings,
              progressSeconds: 0,
              durationSeconds: 12,
              logs: [],
              error: null,
              recovery: null,
              logPath: null,
            }));
          }
          if (command === 'start_encode') {
            const request = payload.request as EncodeRequest;
            return {
              id: 'encode-1',
              state: 'running',
              request: request.source,
              encodeSettings: request.settings,
              progressSeconds: 0,
              durationSeconds: 12,
              logs: [],
              error: null,
              recovery: null,
              logPath: null,
            };
          }
          if (command === 'get_completion_status')
            return {
              options: { notify: false, finishAction: 'none' },
              armedJobs: 0,
              secondsRemaining: null,
              error: null,
            };
          throw new Error(`Unexpected command: ${command}`);
        },
      };
    },
    { source },
  );
  await page.goto('/');
  await page.getByRole('button', { name: 'Add files', exact: true }).first().click();
  await expect(page.getByRole('heading', { name: source.name, exact: true })).toBeVisible();
  await page.getByRole('button', { name: 'Quick Convert', exact: true }).click();
}
const workspace = (page: Page) =>
  page.getByRole('region', { name: 'Quick Convert workspace', exact: true });
const calls = (page: Page, command: string) =>
  page.evaluate(
    (command) =>
      (globalThis as unknown as { __analysisMock: Mock }).__analysisMock.calls.filter(
        (call) => call.command === command,
      ),
    command,
  );
const control = (
  page: Page,
  action: 'hold' | 'release' | 'fingerprint' | 'failNextPreview',
  command: string,
  index?: number,
) =>
  page.evaluate(
    ({ action, command, index }) => {
      const mock = (globalThis as unknown as { __analysisMock: Mock }).__analysisMock;
      if (action === 'release') mock.release(command, index);
      else if (action === 'failNextPreview') mock.failNextPreview();
      else mock[action](command);
    },
    { action, command, index },
  );

test('source analysis stays lazy and detected crop needs explicit application', async ({
  page,
}) => {
  await mock(page);
  expect(await calls(page, 'preview_frame')).toHaveLength(0);
  const quick = workspace(page);
  await quick.getByText('Crop, resize & borders', { exact: true }).click();
  await quick.getByLabel('Crop top (pixels)', { exact: true }).fill('8');
  await quick.getByText('Source preview & automatic crop', { exact: true }).click();
  await expect(quick.getByRole('img')).toBeVisible();
  await quick.getByRole('button', { name: 'Detect black borders', exact: true }).click();
  await expect(quick.getByText(/Proposed crop: top 16/)).toBeVisible();
  await expect(quick.getByLabel('Crop top (pixels)', { exact: true })).toHaveValue('8');
  expect(await calls(page, 'start_encode')).toHaveLength(0);
  await quick.getByRole('button', { name: 'Apply detected crop', exact: true }).click();
  await expect(quick.getByLabel('Crop top (pixels)', { exact: true })).toHaveValue('16');
  await expect(quick.getByLabel('Crop right (pixels)', { exact: true })).toHaveValue('8');
  await quick.getByRole('button', { name: 'Start encode', exact: true }).click();
  const request = (await calls(page, 'start_encode'))[0].payload.request as EncodeRequest;
  expect(request.settings.framing.crop).toEqual({ top: 16, right: 8, bottom: 16, left: 8 });
});

test('switching video stream cancels and discards an older preview completion', async ({
  page,
}) => {
  await mock(page);
  await control(page, 'hold', 'preview_frame');
  const quick = workspace(page);
  await quick.getByText('Source preview & automatic crop', { exact: true }).click();
  await expect.poll(async () => (await calls(page, 'preview_frame')).length).toBe(1);
  await quick.getByLabel('Video stream', { exact: true }).selectOption('4');
  await expect.poll(async () => (await calls(page, 'preview_frame')).length).toBe(2);
  await control(page, 'release', 'preview_frame', 1);
  await expect(
    quick.getByText('HDR is shown as SDR for this preview only.', { exact: false }),
  ).toBeVisible();
  await control(page, 'release', 'preview_frame', 0);
  await expect(
    quick.getByText('HDR is shown as SDR for this preview only.', { exact: false }),
  ).toBeVisible();
  const cancellations = await calls(page, 'cancel_media_analysis');
  expect(cancellations.some((call) => call.payload.id === 'analysis-1')).toBe(true);
});

test('closing while analysis registration is pending cancels before starting the media tool', async ({
  page,
}) => {
  await mock(page);
  await control(page, 'hold', 'begin_media_analysis');
  const disclosure = workspace(page).getByText('Source preview & automatic crop', { exact: true });
  await disclosure.click();
  await expect.poll(async () => (await calls(page, 'begin_media_analysis')).length).toBe(1);
  await disclosure.click();
  await control(page, 'release', 'begin_media_analysis');
  await expect
    .poll(async () => (await calls(page, 'cancel_media_analysis')).length)
    .toBeGreaterThan(0);
  expect(await calls(page, 'preview_frame')).toHaveLength(0);
});

test('source fingerprint mismatch blocks applying an obsolete detection', async ({ page }) => {
  await mock(page);
  const quick = workspace(page);
  await quick.getByText('Source preview & automatic crop', { exact: true }).click();
  await expect(quick.getByRole('img')).toBeVisible();
  await control(page, 'fingerprint', 'changed');
  await quick.getByRole('button', { name: 'Detect black borders', exact: true }).click();
  await expect(quick.getByRole('alert')).toContainText('The source changed since the preview');
  await expect(quick.getByRole('button', { name: 'Apply detected crop', exact: true })).toHaveCount(
    0,
  );
  await expect(quick.getByLabel('Crop top (pixels)', { exact: true })).toHaveValue('0');
});

test('changing streams discards a late crop proposal', async ({ page }) => {
  await mock(page);
  const quick = workspace(page);
  await quick.getByText('Source preview & automatic crop', { exact: true }).click();
  await expect(quick.getByRole('img')).toBeVisible();
  await control(page, 'hold', 'detect_crop');
  await quick.getByRole('button', { name: 'Detect black borders', exact: true }).click();
  await expect.poll(async () => (await calls(page, 'detect_crop')).length).toBe(1);
  await quick.getByLabel('Video stream', { exact: true }).selectOption('4');
  await control(page, 'release', 'detect_crop');
  await expect(
    quick.getByText('HDR is shown as SDR for this preview only.', { exact: false }),
  ).toBeVisible();
  await expect(quick.getByRole('button', { name: 'Apply detected crop', exact: true })).toHaveCount(
    0,
  );
  await expect(quick.getByLabel('Crop top (pixels)', { exact: true })).toHaveValue('0');
});

test('applying detected batch crop invalidates a ready preview and preserves earlier queued settings', async ({
  page,
}) => {
  await mock(page);
  await page.getByRole('button', { name: 'Batch encode', exact: true }).click();
  const batch = page.getByRole('region', { name: 'Batch encode workspace', exact: true });
  await batch.getByRole('button', { name: 'Choose output folder', exact: true }).click();
  const previewBatch = batch.getByRole('button', { name: 'Preview batch', exact: true });
  const queue = batch.getByRole('button', { name: 'Queue ready files', exact: true });
  await previewBatch.click();
  await expect(queue).toBeEnabled();
  await queue.click();
  const original = (await calls(page, 'enqueue_encode_batch'))[0].payload
    .requests as EncodeRequest[];
  expect(original[0].settings.framing.crop.top).toBe(0);
  await batch.getByLabel('Select source.mkv', { exact: true }).check();
  await previewBatch.click();
  await expect(queue).toBeEnabled();
  const episode = batch.locator('article.episode').first();
  await episode.locator('.episode-tracks > summary').click();
  await episode.getByText('Source preview & automatic crop', { exact: true }).click();
  await expect(episode.getByRole('img')).toBeVisible();
  await episode.getByRole('button', { name: 'Detect black borders', exact: true }).click();
  await expect(
    episode.getByRole('button', { name: 'Apply detected crop', exact: true }),
  ).toBeEnabled();
  await expect(queue).toBeEnabled();
  await episode.getByRole('button', { name: 'Apply detected crop', exact: true }).click();
  await expect(queue).toBeDisabled();
  await expect(episode.getByLabel('Crop top (pixels)', { exact: true })).toHaveValue('16');
  expect((await calls(page, 'enqueue_encode_batch'))[0].payload.requests).toEqual(original);
  await previewBatch.click();
  await expect(queue).toBeEnabled();
  await queue.click();
  const submitted = await calls(page, 'enqueue_encode_batch');
  expect(submitted[0].payload.requests).toEqual(original);
  expect((submitted[1].payload.requests as EncodeRequest[])[0].settings.framing.crop.top).toBe(16);
});

test('inspector thumbnails request display orientation and discard stale scrub results', async ({
  page,
}) => {
  await mock(page);
  await page
    .getByRole('navigation', { name: 'Workspace' })
    .getByRole('button', { name: /^Files/ })
    .click();
  const thumbnail = page.getByRole('region', { name: 'Video thumbnail', exact: true });
  await control(page, 'hold', 'preview_frame');
  await thumbnail.getByRole('button', { name: 'Video thumbnail & scrubbing', exact: true }).click();
  await expect.poll(async () => (await calls(page, 'preview_frame')).length).toBe(1);
  await thumbnail.getByLabel('Thumbnail video stream', { exact: true }).selectOption('4');
  await expect.poll(async () => (await calls(page, 'preview_frame')).length).toBe(2);
  await control(page, 'release', 'preview_frame', 1);
  await expect(thumbnail).toContainText('HDR shown as SDR.');
  await control(page, 'release', 'preview_frame', 0);
  await expect(thumbnail).toContainText('HDR shown as SDR.');
  expect((await calls(page, 'preview_frame'))[1].payload.request).toMatchObject({
    videoStreamIndex: 4,
    displayOrientation: true,
  });
  expect((await calls(page, 'cancel_media_analysis')).length).toBeGreaterThan(0);
  await thumbnail.getByLabel('Thumbnail position (seconds)', { exact: true }).fill('5');
  await thumbnail.getByLabel('Thumbnail position (seconds)', { exact: true }).press('Tab');
  await expect.poll(async () => (await calls(page, 'preview_frame')).length).toBe(3);
  expect((await calls(page, 'preview_frame'))[2].payload.request).toMatchObject({
    positionSeconds: 5,
  });
});

test('a failed source refresh removes the old frame and crop proposal', async ({ page }) => {
  await mock(page);
  const quick = workspace(page);
  await quick.getByText('Source preview & automatic crop', { exact: true }).click();
  await expect(quick.getByRole('img')).toBeVisible();
  await quick.getByRole('button', { name: 'Detect black borders', exact: true }).click();
  await expect(
    quick.getByRole('button', { name: 'Apply detected crop', exact: true }),
  ).toBeEnabled();

  await control(page, 'hold', 'preview_frame');
  await control(page, 'failNextPreview', '');
  await quick.getByRole('button', { name: 'Refresh preview', exact: true }).click();
  await expect.poll(async () => (await calls(page, 'preview_frame')).length).toBe(2);
  await expect(quick.getByRole('img')).toHaveCount(0);
  await expect(
    quick.getByRole('button', { name: 'Apply detected crop', exact: true }),
  ).toBeDisabled();

  await control(page, 'release', 'preview_frame');
  await expect(quick.getByRole('alert')).toContainText('The source changed during analysis.');
  await expect(quick.getByRole('button', { name: 'Apply detected crop', exact: true })).toHaveCount(
    0,
  );
});

test('typing a thumbnail position cancels pending work before blur and rejects an invalid seek', async ({
  page,
}) => {
  await mock(page);
  await page
    .getByRole('navigation', { name: 'Workspace' })
    .getByRole('button', { name: /^Files/ })
    .click();
  const thumbnail = page.getByRole('region', { name: 'Video thumbnail', exact: true });
  await thumbnail.getByRole('button', { name: 'Video thumbnail & scrubbing', exact: true }).click();
  await expect(thumbnail.getByRole('img')).toBeVisible();

  await control(page, 'hold', 'preview_frame');
  await thumbnail.getByRole('button', { name: 'Refresh thumbnail', exact: true }).click();
  await expect.poll(async () => (await calls(page, 'preview_frame')).length).toBe(2);
  await expect(thumbnail.getByRole('img')).toHaveCount(0);
  await thumbnail.getByLabel('Thumbnail position (seconds)', { exact: true }).fill('5');
  await expect
    .poll(async () =>
      (await calls(page, 'cancel_media_analysis')).some((call) => call.payload.id === 'analysis-2'),
    )
    .toBe(true);
  await expect.poll(async () => (await calls(page, 'preview_frame')).length).toBe(3);
  await control(page, 'release', 'preview_frame');
  await expect(thumbnail.getByRole('img')).toHaveCount(0);
  await control(page, 'release', 'preview_frame');
  await expect(thumbnail).toContainText('5.00 s');

  await thumbnail.getByLabel('Thumbnail position (seconds)', { exact: true }).fill('-1');
  await expect(thumbnail.getByRole('alert')).toContainText(
    'Choose a position within this video stream.',
  );
  await expect(thumbnail.getByRole('img')).toHaveCount(0);
  expect(await calls(page, 'preview_frame')).toHaveLength(3);
});

test('both preview position limits follow the selected video stream duration', async ({ page }) => {
  const source = {
    ...media,
    streams: media.streams.map((stream) => ({
      ...stream,
      durationSeconds: stream.index === 4 ? 2 : 12,
    })),
  };
  await mock(page, source);
  const quick = workspace(page);
  await quick.getByText('Source preview & automatic crop', { exact: true }).click();
  await quick.getByLabel('Video stream', { exact: true }).selectOption('4');
  const quickMaximum = Number(
    await quick.getByLabel('Position (seconds)', { exact: true }).getAttribute('max'),
  );
  expect(quickMaximum).toBeCloseTo(2 - 1 / 24);

  await page
    .getByRole('navigation', { name: 'Workspace' })
    .getByRole('button', { name: /^Files/ })
    .click();
  const thumbnail = page.getByRole('region', { name: 'Video thumbnail', exact: true });
  await thumbnail.getByRole('button', { name: 'Video thumbnail & scrubbing', exact: true }).click();
  await thumbnail.getByLabel('Thumbnail video stream', { exact: true }).selectOption('4');
  const thumbnailMaximum = Number(
    await thumbnail.getByLabel('Thumbnail position (seconds)', { exact: true }).getAttribute('max'),
  );
  expect(thumbnailMaximum).toBeCloseTo(2 - 1 / 24);
  const before = (await calls(page, 'preview_frame')).length;
  await thumbnail.getByLabel('Thumbnail position (seconds)', { exact: true }).fill('3');
  await expect(thumbnail.getByRole('alert')).toContainText(
    'Choose a position within this video stream.',
  );
  expect(await calls(page, 'preview_frame')).toHaveLength(before);
});
