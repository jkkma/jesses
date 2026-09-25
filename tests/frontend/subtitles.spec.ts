import { expect, test, type Page } from '@playwright/test';
import { showAllEncodeSettings } from './helpers/encode-settings';
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
  await showAllEncodeSettings(page);
}
const calls = (page: Page, command: string) =>
  page.evaluate(
    (command) =>
      (globalThis as unknown as { __subtitleCalls: Call[] }).__subtitleCalls.filter(
        (call) => call.command === command,
      ),
    command,
  );

test('subtitle actions use source indices, retain separate workflow drafts and prevent two burn tracks', async ({
  page,
}) => {
  await mock(page);
  const quick = page.getByRole('region', { name: 'Quick Convert workspace', exact: true });
  const first = quick
    .getByRole('group', { name: 'Subtitle settings for stream #7', exact: true })
    .getByLabel('Subtitle action', { exact: true });
  const second = quick
    .getByRole('group', { name: 'Subtitle settings for stream #9', exact: true })
    .getByLabel('Subtitle action', { exact: true });
  await first.selectOption('burnIn');
  await second.selectOption('burnIn');
  await expect(quick.getByRole('alert')).toHaveText(
    'Choose at most one subtitle track to burn into the video.',
  );
  await expect(quick.getByRole('button', { name: 'Start encode', exact: true })).toBeDisabled();
  await second.selectOption('subRip');
  await expect(
    quick.getByText('Changing text format can change fonts, styling and positions.', {
      exact: false,
    }),
  ).toBeVisible();
  await page.getByRole('button', { name: 'av1an', exact: true }).click();
  await showAllEncodeSettings(page);
  const chunked = page.getByRole('region', { name: 'av1an workspace', exact: true });
  await expect(
    chunked
      .getByRole('group', { name: 'Subtitle settings for stream #7', exact: true })
      .getByLabel('Subtitle action', { exact: true }),
  ).toHaveValue('copy');
  await expect(
    chunked
      .getByRole('group', { name: 'Subtitle settings for stream #7', exact: true })
      .locator('option[value="burnIn"]'),
  ).toBeDisabled();
  await page.getByRole('button', { name: 'Quick Convert', exact: true }).click();
  await showAllEncodeSettings(page);
  await expect(first).toHaveValue('burnIn');
  await second.selectOption('copy');
  await quick.getByRole('button', { name: 'Start encode', exact: true }).click();
  const request = (await calls(page, 'start_encode'))[0].payload.request as EncodeRequest;
  expect(request.source.streamIndices).toEqual([2, 7, 9]);
  expect(request.settings.subtitles).toEqual([{ streamIndex: 7, mode: 'burnIn' }]);
});

test('batch subtitle edits invalidate a reviewed preview and preserve already queued settings', async ({
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
  await episode
    .getByRole('group', { name: 'Subtitle settings for stream #9', exact: true })
    .getByLabel('Subtitle action', { exact: true })
    .selectOption('webVtt');
  await expect(queue).toBeDisabled();
  await preview.click();
  await expect(batch.getByText('Subtitle #9: WebVTT', { exact: true })).toBeVisible();
  await queue.click();
  const submitted = await calls(page, 'enqueue_encode_batch');
  expect(submitted[0].payload.requests).toEqual(original);
  expect((submitted[1].payload.requests as EncodeRequest[])[0].settings.subtitles).toEqual([
    { streamIndex: 9, mode: 'webVtt' },
  ]);
});
