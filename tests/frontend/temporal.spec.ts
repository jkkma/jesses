import { expect, test, type Page } from '@playwright/test';
import type { BatchEncodeRequest, EncodeRequest, MediaFile } from '../../src/lib/ipc/generated';

const source: MediaFile = {
  id: 'subtitle-source',
  path: 'C:\\media\\captions.mkv',
  name: 'captions.mkv',
  sizeBytes: '1000',
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
      fieldOrder: 'bb',
      sampleRate: null,
      channels: null,
      language: null,
      title: null,
    },
    ...[7, 9].map((index) => ({
      index,
      kind: 'subtitle' as const,
      codec: 'ass',
      width: null,
      height: null,
      frameRate: null,
      sampleRate: null,
      channels: null,
      language: 'eng',
      title: `Captions ${index}`,
    })),
  ],
};
type Call = { command: string; payload: Record<string, unknown> };
async function mock(page: Page) {
  await page.addInitScript(
    ({ source }) => {
      const state = globalThis as unknown as Record<string, unknown>;
      const calls: Call[] = [];
      state.__subtitleCalls = calls;
      let nextId = 0;
      state.isTauri = true;
      state.__TAURI_INTERNALS__ = {
        metadata: { currentWindow: { label: 'main' }, currentWebview: { label: 'main' } },
        transformCallback: () => ++nextId,
        unregisterCallback: () => {},
        invoke: async (command: string, payload: Record<string, unknown> = {}) => {
          calls.push({ command, payload });
          if (command.startsWith('plugin:event|')) return ++nextId;
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
              ? 'C:\\output'
              : [source.path];
          if (command === 'import_paths') return { files: [source], errors: [] };
          if (command === 'probe_media') return source;
          if (command === 'list_jobs') return [];
          if (command === 'subscribe_jobs') {
            (payload.channel as { onmessage: (jobs: unknown[]) => void }).onmessage([]);
            return;
          }
          if (command === 'preview_encode_batch') {
            const request = payload.request as BatchEncodeRequest;
            return {
              items: request.inputs.map((input) => ({
                inputPath: input.inputPath,
                outputPath: 'C:\\output\\captions.mkv',
                error: null,
                request: {
                  source: {
                    inputPath: input.inputPath,
                    outputPath: 'C:\\output\\captions.mkv',
                    streamIndices: input.streamIndices,
                  },
                  settings: {
                    ...request,
                    videoStreamIndex: input.videoStreamIndex,
                    audio: input.audio,
                    framing: input.framing,
                    subtitles: input.subtitles,
                    temporal: input.temporal,
                  },
                },
              })),
            };
          }
          if (
            command === 'start_encode' ||
            command === 'enqueue_encode' ||
            command === 'enqueue_encode_batch'
          ) {
            const jobs = (
              (payload.requests as EncodeRequest[]) ?? [payload.request as EncodeRequest]
            ).map((request) => ({
              id: `job-${++nextId}`,
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
            return command === 'enqueue_encode_batch' ? jobs : jobs[0];
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
const calls = (page: Page, command: string) =>
  page.evaluate(
    (command) =>
      (globalThis as unknown as { __subtitleCalls: Call[] }).__subtitleCalls.filter(
        (call) => call.command === command,
      ),
    command,
  );

test('frame controls retain workflow drafts and submit explicit rational processing', async ({
  page,
}) => {
  await mock(page);
  const quick = page.getByRole('region', { name: 'Quick Convert workspace', exact: true });
  await quick.getByText('Frame processing', { exact: true }).click();
  await expect(quick.getByText(/ · Source frame rate$/)).toBeVisible();
  await quick.getByLabel('Source reconstruction', { exact: true }).selectOption('bob');
  await expect(quick.getByText(/ · Double source frame rate \(BWDIF bob\)$/)).toBeVisible();
  await expect(quick.getByLabel('Source field order', { exact: true })).toHaveValue('bottomFirst');
  await quick.getByLabel('Set output frame rate', { exact: true }).check();
  await quick.getByLabel('FPS numerator', { exact: true }).fill('30000');
  await quick.getByLabel('FPS denominator', { exact: true }).fill('0');
  await expect(quick.getByRole('button', { name: 'Start encode', exact: true })).toBeDisabled();
  await expect(quick.getByText(/ · Check output frame rate$/)).toBeVisible();
  await quick.getByLabel('FPS denominator', { exact: true }).fill('1001');
  await expect(quick.getByText(/ · 30000\/1001 fps output$/)).toBeVisible();
  await quick.getByLabel('Resize filter', { exact: true }).selectOption('bicubic');
  await page.getByRole('button', { name: 'av1an', exact: true }).click();
  const chunked = page.getByRole('region', { name: 'av1an workspace', exact: true });
  await chunked.getByText('Frame processing', { exact: true }).click();
  await expect(chunked.getByLabel('Source reconstruction', { exact: true })).toBeEnabled();
  await expect(chunked.getByLabel('Source reconstruction', { exact: true })).toHaveValue('off');
  await expect(chunked.getByLabel('Set output frame rate', { exact: true })).toBeEnabled();
  await expect(chunked.getByLabel('Resize filter', { exact: true })).toHaveValue('lanczos');
  await page.getByRole('button', { name: 'Quick Convert', exact: true }).click();
  await expect(quick.getByLabel('Source reconstruction', { exact: true })).toHaveValue('bob');
  await expect(quick.getByText(/ · 30000\/1001 fps output$/)).toBeVisible();
  await quick.getByRole('button', { name: 'Start encode', exact: true }).click();
  const request = (await calls(page, 'start_encode'))[0].payload.request as EncodeRequest;
  expect(request.settings.temporal).toEqual({
    deinterlace: { mode: 'bob', fieldOrder: 'bottomFirst' },
    frameRate: { numerator: 30000, denominator: 1001 },
    resizeFilter: 'bicubic',
  });
  await quick.getByLabel('Resize filter', { exact: true }).selectOption('nearest');
  expect(request.settings.temporal!.resizeFilter).toBe('bicubic');
});

test('batch frame processing invalidates review and preserves previously queued inputs', async ({
  page,
}) => {
  await mock(page);
  await page.getByRole('button', { name: 'Batch encode', exact: true }).click();
  const batch = page.getByRole('region', { name: 'Batch encode workspace', exact: true });
  await batch.getByRole('button', { name: 'Choose output folder', exact: true }).click();
  const preview = batch.getByRole('button', { name: 'Preview batch', exact: true });
  const queue = batch.getByRole('button', { name: 'Queue ready files', exact: true });
  await preview.click();
  await queue.click();
  const original = (await calls(page, 'enqueue_encode_batch'))[0].payload.requests;
  await batch.getByLabel('Select captions.mkv', { exact: true }).check();
  await preview.click();
  const episode = batch.locator('article.episode').first();
  await episode.locator('.episode-tracks > summary').click();
  await episode.getByText('Frame processing', { exact: true }).click();
  await episode.getByLabel('Source reconstruction', { exact: true }).selectOption('frame');
  await expect(queue).toBeDisabled();
  await episode.getByLabel('Source field order', { exact: true }).selectOption('topFirst');
  await episode.getByLabel('Resize filter', { exact: true }).selectOption('bilinear');
  await preview.click();
  await queue.click();
  const submitted = await calls(page, 'enqueue_encode_batch');
  expect(submitted[0].payload.requests).toEqual(original);
  expect((submitted[1].payload.requests as EncodeRequest[])[0].settings.temporal).toEqual({
    deinterlace: { mode: 'frame', fieldOrder: 'topFirst' },
    resizeFilter: 'bilinear',
  });
});

test('padded capture requires an intended rate and serializes guarded cadence repair', async ({
  page,
}) => {
  await mock(page);
  const quick = page.getByRole('region', { name: 'Quick Convert workspace', exact: true });
  await quick.getByText('Frame processing', { exact: true }).click();
  await quick.getByLabel('Source reconstruction', { exact: true }).selectOption('exactDuplicates');
  await expect(quick.getByRole('alert')).toContainText(
    'requires the intended constant output frame rate',
  );
  await expect(quick.getByRole('button', { name: 'Start encode', exact: true })).toBeDisabled();
  await quick.getByLabel('Set output frame rate', { exact: true }).check();
  await quick.getByLabel('FPS numerator', { exact: true }).fill('12');
  await quick.getByLabel('FPS denominator', { exact: true }).fill('1');
  await expect(quick.getByRole('button', { name: 'Start encode', exact: true })).toBeEnabled();
  await quick.getByRole('button', { name: 'Start encode', exact: true }).click();
  const request = (await calls(page, 'start_encode'))[0].payload.request as EncodeRequest;
  expect(request.settings.temporal).toEqual({
    cadenceRepair: {
      kind: 'exactDuplicates',
      fieldOrder: 'bottomFirst',
      combedFallback: false,
    },
    frameRate: { numerator: 12, denominator: 1 },
    resizeFilter: 'lanczos',
  });
});

test('av1an accepts managed QTGMC and submits its explicit preparation settings', async ({
  page,
}) => {
  await mock(page);
  await page.getByRole('button', { name: 'av1an', exact: true }).click();
  const chunked = page.getByRole('region', { name: 'av1an workspace', exact: true });
  await chunked.getByText('Frame processing', { exact: true }).click();
  await chunked.getByLabel('Source reconstruction', { exact: true }).selectOption('qtgmcBob');
  await chunked.getByLabel('QTGMC preset', { exact: true }).selectOption('medium');
  await expect(chunked.getByRole('button', { name: 'Start encode', exact: true })).toBeEnabled();
  await chunked.getByRole('button', { name: 'Start encode', exact: true }).click();
  const request = (await calls(page, 'start_encode'))[0].payload.request as EncodeRequest;
  expect(request.settings.backend).toBe('av1an');
  expect(request.settings.temporal).toEqual({
    qtgmc: { mode: 'bob', fieldOrder: 'bottomFirst', preset: 'medium' },
    resizeFilter: 'lanczos',
  });
});
