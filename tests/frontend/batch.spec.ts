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
          if (command === 'get_completion_status')
            return {
              options: { notify: false, finishAction: 'none' },
              armedJobs: 0,
              secondsRemaining: null,
              error: null,
            };
          if (command === 'get_capabilities')
            return [
              'ffmpeg',
              'ffprobe',
              'svt-av1',
              'svt-av1-5fish',
              'svt-av1-hdr',
              'av1an',
              'x264',
              'aomenc',
              'vpxenc',
              'x265',
              'mkvmerge',
            ].map((id) => ({
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
                  {
                    svtAv1: '_av1.mkv',
                    svtAv1FiveFish: '_av1_5fish.mkv',
                    svtAv1Hdr: '_av1_hdr.mkv',
                    x264: '_x264.mkv',
                    x265: '_x265.mkv',
                    vp9: '_vp9.mkv',
                    aomAv1: '_aom_av1.mkv',
                    x265Standalone: '_x265_standalone.mkv',
                    vpxStandalone: '_vpx.mkv',
                    h264Nvenc: '_h264_nvenc.mkv',
                    hevcNvenc: '_hevc_nvenc.mkv',
                  }[request.encoder];
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
                      ...(request.av1anOptions ? { av1anOptions: request.av1anOptions } : {}),
                      ...(request.rateControl ? { rateControl: request.rateControl } : {}),
                      preset: request.preset,
                      lossless: request.lossless,
                      ...(request.svtCrfQuarterSteps === undefined
                        ? {}
                        : { svtCrfQuarterSteps: request.svtCrfQuarterSteps }),
                      ...(request.svtPreset === undefined ? {} : { svtPreset: request.svtPreset }),
                      backend: request.backend,
                      encoder: request.encoder,
                      workers: request.workers,
                      filmGrain: request.filmGrain,
                      hdr10Fallback: request.hdr10Fallback,
                      lineartPsyBias: request.lineartPsyBias,
                      texturePsyBias: request.texturePsyBias,
                      hdrTune: request.hdrTune,
                      framing: input.framing,
                      audio: input.audio,
                      ...(input.trim ? { trim: input.trim } : {}),
                      ...(input.toneMap ? { toneMap: input.toneMap } : {}),
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
              recovery: null,
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

test('crop resize and borders batch preview retains each source framing and immutable queue history', async ({
  page,
}) => {
  await desktopMock(page);
  await openBatch(page);
  const workspace = page.getByRole('region', { name: 'Batch encode workspace' });
  const first = workspace.locator('article.episode').filter({ hasText: 'Episode 1.mkv' });
  const second = workspace.locator('article.episode').filter({ hasText: 'Episode 2.mkv' });
  await first.locator('.episode-tracks > summary').click();
  await second.locator('.episode-tracks > summary').click();
  await first.getByText('Crop, resize & borders', { exact: true }).click();
  await second.getByText('Crop, resize & borders', { exact: true }).click();
  await first.getByLabel('Crop top (pixels)', { exact: true }).fill('40');
  await first.getByLabel('Crop bottom (pixels)', { exact: true }).fill('40');
  await first.getByLabel('Resize video', { exact: true }).check();
  await first.getByLabel('Picture width (pixels)', { exact: true }).fill('640');
  await first.getByLabel('Add black borders', { exact: true }).check();
  await first.getByLabel('Border top (pixels)', { exact: true }).fill('20');
  await first.getByLabel('Border bottom (pixels)', { exact: true }).fill('20');
  await expect(first.getByLabel('Video dimensions', { exact: true })).toContainText(
    'Picture 640 × 320 → Output 640 × 360',
  );
  await second.getByLabel('Crop left (pixels)', { exact: true }).fill('20');
  await second.getByLabel('Add black borders', { exact: true }).check();
  await second.getByLabel('Border left (pixels)', { exact: true }).fill('20');
  await expect(second.getByLabel('Video dimensions', { exact: true })).toContainText(
    'Picture 1260 × 720 → Output 1280 × 720',
  );
  await workspace.getByRole('button', { name: 'Preview batch', exact: true }).click();
  await expect(workspace.getByRole('region', { name: 'Batch output preview' })).toContainText(
    'Picture width 640 px',
  );
  const preview = (await calls(page, 'preview_encode_batch'))[0].payload
    .request as BatchEncodeRequest;
  expect(preview.inputs.map((input) => input.framing)).toEqual([
    {
      crop: { top: 40, right: 0, bottom: 40, left: 0 },
      resizeWidth: 640,
      borders: { top: 20, right: 0, bottom: 20, left: 0 },
    },
    {
      crop: { top: 0, right: 0, bottom: 0, left: 20 },
      resizeWidth: null,
      borders: { top: 0, right: 0, bottom: 0, left: 20 },
    },
  ]);
  await workspace.getByRole('button', { name: 'Queue ready files', exact: true }).click();
  const queued = (await calls(page, 'enqueue_encode_batch'))[0].payload.requests as EncodeRequest[];
  expect(queued.map((request) => request.settings.framing)).toEqual(
    preview.inputs.map((input) => input.framing),
  );
  await workspace.getByRole('button', { name: 'Select up to 100', exact: true }).click();
  await first.locator('.episode-tracks > summary').click();
  await first.getByText('Crop, resize & borders', { exact: true }).click();
  await first.getByLabel('Crop top (pixels)', { exact: true }).fill('0');
  await first.getByLabel('Border top (pixels)', { exact: true }).fill('0');
  await page.getByRole('button', { name: 'Quick Convert', exact: true }).click();
  await expect(page.getByRole('region', { name: 'Current encode job' })).toContainText(
    'Crop top 40, right 0, bottom 40, left 0 px · Picture width 640 px · Automatic height · Black borders top 20, right 0, bottom 20, left 0 px',
  );
  expect((await calls(page, 'enqueue_encode_batch'))[0].payload.requests).toEqual(queued);
});

for (const edit of ['crop', 'width', 'resize', 'border', 'bordersEnabled']) {
  test(`crop resize and borders batch ${edit} edit invalidates an outstanding preview`, async ({
    page,
  }) => {
    await desktopMock(page, { held: ['preview'] });
    await openBatch(page);
    const workspace = page.getByRole('region', { name: 'Batch encode workspace' });
    const first = workspace.locator('article.episode').filter({ hasText: 'Episode 1.mkv' });
    await first.locator('.episode-tracks > summary').click();
    await first.getByText('Crop, resize & borders', { exact: true }).click();
    await first.getByLabel('Resize video', { exact: true }).check();
    await first.getByLabel('Picture width (pixels)', { exact: true }).fill('640');
    await first.getByLabel('Add black borders', { exact: true }).check();
    await first.getByLabel('Border top (pixels)', { exact: true }).fill('20');
    await workspace.getByRole('button', { name: 'Preview batch', exact: true }).click();
    await expect.poll(() => calls(page, 'preview_encode_batch')).toHaveLength(1);
    if (edit === 'crop') await first.getByLabel('Crop bottom (pixels)', { exact: true }).fill('40');
    if (edit === 'width')
      await first.getByLabel('Picture width (pixels)', { exact: true }).fill('960');
    if (edit === 'resize') await first.getByLabel('Resize video', { exact: true }).uncheck();
    if (edit === 'border')
      await first.getByLabel('Border bottom (pixels)', { exact: true }).fill('20');
    if (edit === 'bordersEnabled')
      await first.getByLabel('Add black borders', { exact: true }).uncheck();
    await release(page, 'preview');
    await expect(
      workspace.getByRole('button', { name: 'Queue ready files', exact: true }),
    ).toBeDisabled();
    await expect(page.getByRole('region', { name: 'Batch output preview' })).toContainText(
      'Review required before queueing',
    );
    await workspace.getByRole('button', { name: 'Preview batch', exact: true }).click();
    await expect(
      workspace.getByRole('button', { name: 'Queue ready files', exact: true }),
    ).toBeEnabled();
    expect(await calls(page, 'enqueue_encode_batch')).toHaveLength(0);
  });
}

test('crop and resize default framing permits mixed source compatibility results in batch preview', async ({
  page,
}) => {
  const odd = {
    ...episodes[1],
    streams: episodes[1].streams.map((stream) =>
      stream.kind === 'video' ? { ...stream, width: 1279 } : stream,
    ),
  };
  await desktopMock(page, { files: [episodes[0], odd], invalidPath: odd.path });
  await openBatch(page);
  const workspace = page.getByRole('region', { name: 'Batch encode workspace' });
  await expect(workspace.getByRole('button', { name: 'Preview batch', exact: true })).toBeEnabled();
  await workspace.getByRole('button', { name: 'Preview batch', exact: true }).click();
  await expect(page.getByRole('region', { name: 'Batch output preview' })).toContainText(
    '1 ready / 2 reviewed',
  );
  await workspace.getByRole('button', { name: 'Queue ready files', exact: true }).click();
  const queued = (await calls(page, 'enqueue_encode_batch'))[0].payload.requests as EncodeRequest[];
  expect(queued.map((request) => request.source.inputPath)).toEqual([episodes[0].path]);
});

test('crop resize and borders batch rejects invalid per-file values before requesting preview', async ({
  page,
}) => {
  await desktopMock(page);
  await openBatch(page);
  const workspace = page.getByRole('region', { name: 'Batch encode workspace' });
  const first = workspace.locator('article.episode').filter({ hasText: 'Episode 1.mkv' });
  await first.locator('.episode-tracks > summary').click();
  await first.getByText('Crop, resize & borders', { exact: true }).click();
  const preview = workspace.getByRole('button', { name: 'Preview batch', exact: true });
  for (const edge of ['top', 'right', 'bottom', 'left']) {
    for (const invalid of ['', '-2', '3', '2.5', '8192']) {
      await first.getByLabel(`Crop ${edge} (pixels)`, { exact: true }).fill(invalid);
      await expect(preview).toBeDisabled();
    }
    await first.getByLabel(`Crop ${edge} (pixels)`, { exact: true }).fill('0');
  }
  await first.getByLabel('Resize video', { exact: true }).check();
  for (const invalid of ['', '63', '65', '640.5', '8194', '64']) {
    await first.getByLabel('Picture width (pixels)', { exact: true }).fill(invalid);
    await expect(preview).toBeDisabled();
  }
  await first.getByLabel('Picture width (pixels)', { exact: true }).fill('640');
  await first.getByLabel('Add black borders', { exact: true }).check();
  for (const edge of ['top', 'right', 'bottom', 'left']) {
    for (const invalid of ['', '-2', '3', '2.5', '8192']) {
      await first.getByLabel(`Border ${edge} (pixels)`, { exact: true }).fill(invalid);
      await expect(preview).toBeDisabled();
    }
    await first.getByLabel(`Border ${edge} (pixels)`, { exact: true }).fill('0');
  }
  await first.getByLabel('Border right (pixels)', { exact: true }).fill('7552');
  await expect(first.getByLabel('Video dimensions', { exact: true })).toContainText(
    'Output 8192 × 360',
  );
  await expect(preview).toBeEnabled();
  await first.getByLabel('Border left (pixels)', { exact: true }).fill('2');
  await expect(preview).toBeDisabled();
  await first.getByLabel('Add black borders', { exact: true }).uncheck();
  await expect(preview).toBeEnabled();
  expect(await calls(page, 'preview_encode_batch')).toHaveLength(0);
});

test('crop resize and border batch drafts restore per encoder and workflow with independent av1an framing', async ({
  page,
}) => {
  await desktopMock(page);
  await openBatch(page);
  const workspace = page.getByRole('region', { name: 'Batch encode workspace' });
  const first = workspace.locator('article.episode').filter({ hasText: 'Episode 1.mkv' });
  await first.locator('.episode-tracks > summary').click();
  await first.getByText('Crop, resize & borders', { exact: true }).click();
  await first.getByLabel('Crop top (pixels)', { exact: true }).fill('40');
  await first.getByLabel('Resize video', { exact: true }).check();
  await first.getByLabel('Picture width (pixels)', { exact: true }).fill('640');
  await first.getByLabel('Add black borders', { exact: true }).check();
  await first.getByLabel('Border top (pixels)', { exact: true }).fill('20');
  await workspace.getByLabel('Video encoder', { exact: true }).selectOption('x264');
  await expect(first.getByLabel('Crop top (pixels)', { exact: true })).toHaveValue('0');
  await expect(first.getByLabel('Add black borders', { exact: true })).not.toBeChecked();
  await first.getByLabel('Crop left (pixels)', { exact: true }).fill('20');
  await first.getByLabel('Add black borders', { exact: true }).check();
  await first.getByLabel('Border left (pixels)', { exact: true }).fill('40');
  await workspace.getByLabel('Encode backend', { exact: true }).selectOption('av1an');
  await expect(first.getByLabel('Crop top (pixels)', { exact: true })).toHaveValue('0');
  await expect(first.getByLabel('Add black borders', { exact: true })).not.toBeChecked();
  await workspace.getByRole('button', { name: 'Preview batch', exact: true }).click();
  const chunked = (await calls(page, 'preview_encode_batch'))[0].payload
    .request as BatchEncodeRequest;
  expect(
    chunked.inputs.every(
      (input) =>
        input.framing.resizeWidth === null &&
        Object.values(input.framing.crop).every((edge) => edge === 0) &&
        Object.values(input.framing.borders).every((edge) => edge === 0),
    ),
  ).toBe(true);
  await workspace.getByLabel('Encode backend', { exact: true }).selectOption('standalone');
  await expect(first.getByLabel('Crop left (pixels)', { exact: true })).toHaveValue('20');
  await expect(first.getByLabel('Border left (pixels)', { exact: true })).toHaveValue('40');
  await expect(first.getByLabel('Border top (pixels)', { exact: true })).toHaveValue('0');
  await workspace.getByLabel('Video encoder', { exact: true }).selectOption('svtAv1Hdr');
  await expect(first.getByLabel('Crop top (pixels)', { exact: true })).toHaveValue('40');
  await expect(first.getByLabel('Picture width (pixels)', { exact: true })).toHaveValue('640');
  await expect(first.getByLabel('Border top (pixels)', { exact: true })).toHaveValue('20');
  await expect(first.getByLabel('Border left (pixels)', { exact: true })).toHaveValue('0');
  await expect(
    workspace.getByRole('button', { name: 'Queue ready files', exact: true }),
  ).toBeDisabled();
});

for (const variant of ['svtAv1FiveFish', 'svtAv1Hdr'] as const) {
  test(`SVT fork ${variant} batch snapshots retain reviewed controls and output names`, async ({
    page,
  }) => {
    await desktopMock(page);
    await openBatch(page);
    const workspace = page.getByRole('region', { name: 'Batch encode workspace' });
    const fiveFish = variant === 'svtAv1FiveFish';
    if (fiveFish)
      await workspace.getByLabel('Encode backend', { exact: true }).selectOption('av1an');
    const selector = workspace.getByLabel(fiveFish ? 'SVT-AV1 build' : 'Video encoder', {
      exact: true,
    });
    await selector.selectOption(variant);
    await expect(workspace.getByLabel('CRF', { exact: true })).toHaveValue(fiveFish ? '18' : '30');
    await expect(workspace.getByLabel('Preset', { exact: true })).toHaveValue('2');
    if (fiveFish) {
      await expect(workspace.getByLabel('Lineart psy bias', { exact: true })).toHaveValue('5');
      await expect(workspace.getByLabel('Texture psy bias', { exact: true })).toHaveValue('4');
      await workspace.getByLabel('Texture psy bias', { exact: true }).fill('6');
    } else {
      await expect(workspace.getByLabel('HDR tune', { exact: true })).toHaveValue('filmGrain');
      await workspace.getByLabel('HDR tune', { exact: true }).selectOption('visualQuality');
    }
    await expect(workspace.getByLabel('Allow HDR10 fallback', { exact: true })).not.toBeChecked();
    await workspace.getByRole('button', { name: 'Preview batch', exact: true }).click();
    await expect(
      workspace.getByRole('button', { name: 'Queue ready files', exact: true }),
    ).toBeEnabled();
    const preview = (await calls(page, 'preview_encode_batch'))[0].payload
      .request as BatchEncodeRequest;
    expect(preview).toMatchObject({
      backend: fiveFish ? 'av1an' : 'standalone',
      encoder: variant,
      crf: fiveFish ? 18 : 30,
      preset: 2,
      lineartPsyBias: fiveFish ? 5 : 0,
      texturePsyBias: fiveFish ? 6 : 0,
      hdrTune: 'visualQuality',
      filmGrain: 0,
      hdr10Fallback: false,
    });
    await expect(page.getByRole('region', { name: 'Batch output preview' })).toContainText(
      `Episode 1_av1_${fiveFish ? '5fish' : 'hdr'}.mkv`,
    );
    await workspace.getByRole('button', { name: 'Queue ready files', exact: true }).click();
    const requests = (await calls(page, 'enqueue_encode_batch'))[0].payload
      .requests as EncodeRequest[];
    expect(requests).toHaveLength(2);
    for (const request of requests)
      expect(request.settings).toMatchObject({
        encoder: preview.encoder,
        crf: preview.crf,
        preset: preview.preset,
        lineartPsyBias: preview.lineartPsyBias,
        texturePsyBias: preview.texturePsyBias,
        hdrTune: preview.hdrTune,
      });
    await workspace.getByRole('button', { name: 'Reset batch settings', exact: true }).click();
    await selector.selectOption('svtAv1');
    await page.getByRole('button', { name: 'Quick Convert', exact: true }).click();
    const job = page.getByRole('article', { name: 'Job batch-2', exact: true });
    await job.getByText('Saved settings and log', { exact: true }).click();
    await expect(job).toContainText(fiveFish ? 'av1an / SVT-AV1 5fish' : 'Standalone SVT-AV1-HDR');
    await expect(job).toContainText(fiveFish ? 'Lineart 5 · Texture 6' : 'HDR tune visual quality');
    expect((await calls(page, 'enqueue_encode_batch'))[0].payload.requests).toEqual(requests);
  });

  test(`SVT fork ${variant} batch requires its exact tool`, async ({ page }) => {
    await desktopMock(page, {
      missing: variant === 'svtAv1FiveFish' ? 'svt-av1-5fish' : 'svt-av1-hdr',
    });
    await openBatch(page);
    const workspace = page.getByRole('region', { name: 'Batch encode workspace' });
    await workspace.getByLabel('Video encoder', { exact: true }).selectOption('svtAv1');
    await expect(
      workspace.getByRole('button', { name: 'Preview batch', exact: true }),
    ).toBeEnabled();
    await workspace.getByLabel('Video encoder', { exact: true }).selectOption(variant);
    await expect(
      workspace.getByRole('button', { name: 'Preview batch', exact: true }),
    ).toBeDisabled();
    await workspace.getByLabel('Encode backend', { exact: true }).selectOption('av1an');
    await workspace.getByLabel('SVT-AV1 build', { exact: true }).selectOption(variant);
    await expect(
      workspace.getByRole('button', { name: 'Preview batch', exact: true }),
    ).toBeDisabled();
  });
}

for (const control of ['Lineart psy bias', 'Texture psy bias', 'HDR tune'] as const) {
  test(`SVT fork batch invalidates a deferred preview after editing ${control}`, async ({
    page,
  }) => {
    await desktopMock(page, { held: ['preview'] });
    await openBatch(page);
    const workspace = page.getByRole('region', { name: 'Batch encode workspace' });
    await workspace
      .getByLabel('Video encoder', { exact: true })
      .selectOption(control === 'HDR tune' ? 'svtAv1Hdr' : 'svtAv1FiveFish');
    await workspace.getByRole('button', { name: 'Preview batch', exact: true }).click();
    await expect.poll(() => calls(page, 'preview_encode_batch')).toHaveLength(1);
    if (control === 'HDR tune')
      await workspace.getByLabel(control, { exact: true }).selectOption('visualQuality');
    else await workspace.getByLabel(control, { exact: true }).fill('7');
    await release(page, 'preview');
    await expect(
      workspace.getByRole('button', { name: 'Queue ready files', exact: true }),
    ).toBeDisabled();
    await expect(page.getByRole('region', { name: 'Batch output preview' })).toContainText(
      'Review required before queueing',
    );
    await workspace.getByRole('button', { name: 'Preview batch', exact: true }).click();
    await expect(
      workspace.getByRole('button', { name: 'Queue ready files', exact: true }),
    ).toBeEnabled();
    const latest = (await calls(page, 'preview_encode_batch'))[1].payload
      .request as BatchEncodeRequest;
    if (control === 'HDR tune') expect(latest.hdrTune).toBe('visualQuality');
    else
      expect(control === 'Lineart psy bias' ? latest.lineartPsyBias : latest.texturePsyBias).toBe(
        7,
      );
  });
}

test('SVT fork batch preserves independent standalone and av1an build choices and per-build source drafts', async ({
  page,
}) => {
  await desktopMock(page);
  await openBatch(page);
  const workspace = page.getByRole('region', { name: 'Batch encode workspace' });
  await workspace.getByLabel('Video encoder', { exact: true }).selectOption('svtAv1FiveFish');
  await workspace.getByLabel('Lineart psy bias', { exact: true }).fill('7');
  await workspace.getByLabel('Select Episode 2.mkv', { exact: true }).uncheck();
  await workspace.getByLabel('Encode backend', { exact: true }).selectOption('av1an');
  await workspace.getByLabel('SVT-AV1 build', { exact: true }).selectOption('svtAv1Hdr');
  await expect(
    workspace.getByLabel('SVT-AV1 build', { exact: true }).locator('option[value="x264"]'),
  ).toHaveCount(0);
  await expect(workspace.getByLabel('Select Episode 2.mkv', { exact: true })).toBeChecked();
  await workspace.getByLabel('HDR tune', { exact: true }).selectOption('visualQuality');
  await workspace.getByLabel('Parallel chunks', { exact: true }).fill('3');
  await workspace.getByLabel('Encode backend', { exact: true }).selectOption('standalone');
  await expect(workspace.getByLabel('Video encoder', { exact: true })).toHaveValue(
    'svtAv1FiveFish',
  );
  await expect(workspace.getByLabel('Lineart psy bias', { exact: true })).toHaveValue('7');
  await expect(workspace.getByLabel('Select Episode 2.mkv', { exact: true })).not.toBeChecked();
  await workspace.getByLabel('Video encoder', { exact: true }).selectOption('x264');
  await expect(workspace.getByLabel('Lineart psy bias', { exact: true })).toHaveCount(0);
  await expect(workspace.getByLabel('CRF', { exact: true })).toHaveValue('23');
  await workspace.getByLabel('Encode backend', { exact: true }).selectOption('av1an');
  await expect(workspace.getByLabel('SVT-AV1 build', { exact: true })).toHaveValue('svtAv1Hdr');
  await expect(workspace.getByLabel('HDR tune', { exact: true })).toHaveValue('visualQuality');
  await expect(workspace.getByLabel('Parallel chunks', { exact: true })).toHaveValue('3');
});

test('x264 batch defaults and copied tracks become immutable reviewed H.264 queue requests', async ({
  page,
}) => {
  await desktopMock(page);
  await openBatch(page);
  const workspace = page.getByRole('region', { name: 'Batch encode workspace' });
  await workspace.getByLabel('Film grain synthesis', { exact: true }).fill('10');
  await workspace.getByLabel('Allow HDR10 fallback', { exact: true }).check();
  await workspace.getByLabel('Video encoder', { exact: true }).selectOption('x264');
  await expect(workspace.getByLabel('CRF', { exact: true })).toHaveValue('23');
  await expect(workspace.getByLabel('Preset', { exact: true })).toHaveValue('5');
  await expect(
    workspace.getByLabel('Preset', { exact: true }).locator('option:checked'),
  ).toHaveText('medium');
  await expect(workspace.getByLabel('Film grain synthesis', { exact: true })).toHaveCount(0);
  await expect(workspace.getByLabel('Allow HDR10 fallback', { exact: true })).toHaveCount(0);
  for (const invalid of ['-1', '52', '23.5', '']) {
    await workspace.getByLabel('CRF', { exact: true }).fill(invalid);
    await expect(
      workspace.getByRole('button', { name: 'Preview batch', exact: true }),
    ).toBeDisabled();
  }
  for (const valid of ['0', '51', '23']) {
    await workspace.getByLabel('CRF', { exact: true }).fill(valid);
    await expect(
      workspace.getByRole('button', { name: 'Preview batch', exact: true }),
    ).toBeEnabled();
  }
  await workspace
    .getByText('Video, audio & source tracks · 3 selected', { exact: true })
    .first()
    .click();
  await workspace.getByLabel('Video stream for Episode 1.mkv', { exact: true }).selectOption('9');
  await workspace
    .getByLabel('Include audio stream #5 from Episode 1.mkv', { exact: true })
    .uncheck();
  await workspace.getByRole('button', { name: 'Preview batch', exact: true }).click();
  const preview = page.getByRole('region', { name: 'Batch output preview' });
  await expect(preview).toContainText('2 ready / 2 reviewed');
  await expect(preview).toContainText('Episode 1_x264.mkv');
  await expect(preview).toContainText('Episode 2_x264.mkv');
  const settings = {
    crf: 23,
    preset: 5,
    lossless: false,
    backend: 'standalone',
    encoder: 'x264',
    workers: 2,
    filmGrain: 0,
    hdr10Fallback: false,
    lineartPsyBias: 0,
    texturePsyBias: 0,
    hdrTune: 'visualQuality',
  };
  expect((await calls(page, 'preview_encode_batch'))[0].payload.request).toEqual({
    outputContainer: 'matroska',
    outputDirectory: 'C:\\exports',
    ...settings,
    inputs: [
      {
        inputPath: episodes[0].path,
        videoStreamIndex: 9,
        streamIndices: [9, 8, 11],
        framing: {
          crop: { top: 0, right: 0, bottom: 0, left: 0 },
          resizeWidth: null,
          borders: { top: 0, right: 0, bottom: 0, left: 0 },
        },
        audio: [],
      },
      {
        inputPath: episodes[1].path,
        videoStreamIndex: 2,
        streamIndices: [2, 5, 8, 11],
        framing: {
          crop: { top: 0, right: 0, bottom: 0, left: 0 },
          resizeWidth: null,
          borders: { top: 0, right: 0, bottom: 0, left: 0 },
        },
        audio: [{ streamIndex: 5, codec: 'copy', bitrateKbps: 128, channels: 'preserve' }],
      },
    ],
  });
  await workspace.getByRole('button', { name: 'Queue ready files', exact: true }).click();
  const queued = (await calls(page, 'enqueue_encode_batch'))[0].payload.requests as EncodeRequest[];
  expect(queued).toEqual([
    {
      source: {
        inputPath: episodes[0].path,
        outputPath: 'C:\\exports\\Episode 1_x264.mkv',
        streamIndices: [9, 8, 11],
      },
      settings: {
        ...settings,
        videoStreamIndex: 9,
        framing: {
          crop: { top: 0, right: 0, bottom: 0, left: 0 },
          resizeWidth: null,
          borders: { top: 0, right: 0, bottom: 0, left: 0 },
        },
        audio: [],
      },
    },
    {
      source: {
        inputPath: episodes[1].path,
        outputPath: 'C:\\exports\\Episode 2_x264.mkv',
        streamIndices: [2, 5, 8, 11],
      },
      settings: {
        ...settings,
        videoStreamIndex: 2,
        framing: {
          crop: { top: 0, right: 0, bottom: 0, left: 0 },
          resizeWidth: null,
          borders: { top: 0, right: 0, bottom: 0, left: 0 },
        },
        audio: [{ streamIndex: 5, codec: 'copy', bitrateKbps: 128, channels: 'preserve' }],
      },
    },
  ]);
  await workspace.getByLabel('Video encoder', { exact: true }).selectOption('svtAv1Hdr');
  await workspace.getByRole('button', { name: 'Reset batch settings', exact: true }).click();
  await page.getByRole('button', { name: 'Quick Convert', exact: true }).click();
  const job = page.getByRole('article', { name: 'Job batch-2', exact: true });
  await job.getByText('Saved settings and log', { exact: true }).click();
  await expect(job).toContainText('H.264 encode');
  await expect(job).toContainText(
    'Standalone x264 · H.264 · Source bit depth · CRF 23 · Preset medium',
  );
  expect((await calls(page, 'enqueue_encode_batch'))[0].payload.requests).toEqual(queued);
});

test('x264 batch encoder switches restore source choices and common settings while av1an stays SVT', async ({
  page,
}) => {
  await desktopMock(page);
  await openBatch(page);
  const workspace = page.getByRole('region', { name: 'Batch encode workspace' });
  const encoder = workspace.getByLabel('Video encoder', { exact: true });
  await workspace.getByLabel('CRF', { exact: true }).fill('29');
  await workspace.getByLabel('Film grain synthesis', { exact: true }).fill('8');
  await workspace.getByLabel('Select Episode 2.mkv', { exact: true }).uncheck();
  await workspace
    .getByText('Video, audio & source tracks · 3 selected', { exact: true })
    .first()
    .click();
  await workspace
    .getByLabel('Include audio stream #5 from Episode 1.mkv', { exact: true })
    .uncheck();
  await encoder.selectOption('x264');
  await expect(workspace.getByLabel('CRF', { exact: true })).toHaveValue('23');
  await expect(workspace.getByLabel('Select Episode 2.mkv', { exact: true })).toBeChecked();
  await expect(
    workspace.getByLabel('Include audio stream #5 from Episode 1.mkv', { exact: true }),
  ).toBeChecked();
  await workspace.getByLabel('CRF', { exact: true }).fill('19');
  await workspace.getByLabel('Preset', { exact: true }).selectOption('8');
  await workspace.getByLabel('Select Episode 1.mkv', { exact: true }).uncheck();
  await encoder.selectOption('svtAv1Hdr');
  await expect(workspace.getByLabel('CRF', { exact: true })).toHaveValue('29');
  await expect(workspace.getByLabel('Film grain synthesis', { exact: true })).toHaveValue('8');
  await expect(workspace.getByLabel('Select Episode 1.mkv', { exact: true })).toBeChecked();
  await expect(workspace.getByLabel('Select Episode 2.mkv', { exact: true })).not.toBeChecked();
  // Restoring a previously deselected source keeps its controls lazy; open it
  // before inspecting the saved per-workflow stream choices.
  await workspace
    .locator('article.episode')
    .filter({ hasText: 'Episode 1.mkv' })
    .locator('.episode-tracks > summary')
    .click();
  await expect(
    workspace.getByLabel('Include audio stream #5 from Episode 1.mkv', { exact: true }),
  ).not.toBeChecked();
  await encoder.selectOption('x264');
  await workspace.getByLabel('Encode backend', { exact: true }).selectOption('av1an');
  await expect(encoder).toHaveCount(0);
  await expect(workspace.getByLabel('CRF', { exact: true })).toHaveValue('30');
  await workspace.getByRole('button', { name: 'Preview batch', exact: true }).click();
  await expect(
    workspace.getByRole('button', { name: 'Queue ready files', exact: true }),
  ).toBeEnabled();
  expect((await calls(page, 'preview_encode_batch'))[0].payload.request).toMatchObject({
    backend: 'av1an',
    encoder: 'svtAv1Hdr',
    crf: 30,
    filmGrain: 0,
  });
  await workspace.getByLabel('Encode backend', { exact: true }).selectOption('standalone');
  await expect(encoder).toHaveValue('x264');
  await expect(workspace.getByLabel('CRF', { exact: true })).toHaveValue('19');
  await expect(workspace.getByLabel('Preset', { exact: true })).toHaveValue('8');
  await expect(workspace.getByLabel('Select Episode 1.mkv', { exact: true })).not.toBeChecked();
  await expect(workspace.getByLabel('Select Episode 2.mkv', { exact: true })).toBeChecked();
  await workspace.getByRole('button', { name: 'Reset batch settings', exact: true }).click();
  await expect(encoder).toHaveValue('x264');
  await expect(workspace.getByLabel('CRF', { exact: true })).toHaveValue('23');
  await expect(workspace.getByLabel('Preset', { exact: true })).toHaveValue('5');
  await encoder.selectOption('svtAv1Hdr');
  await expect(workspace.getByLabel('CRF', { exact: true })).toHaveValue('29');
});

test('x264 batch keeps HDR failures visible while queueing only reviewed SDR requests', async ({
  page,
}) => {
  await desktopMock(page, {
    invalidPath: episodes[1].path,
    files: [
      episodes[0],
      { ...episodes[1], streams: [{ ...stream, bitDepth: 10, colorTransfer: 'smpte2084' }] },
    ],
  });
  await openBatch(page);
  const workspace = page.getByRole('region', { name: 'Batch encode workspace' });
  await workspace.getByLabel('Video encoder', { exact: true }).selectOption('x264');
  await expect(
    workspace.getByText(
      "x264 needs SDR video. Open this episode's settings and enable explicit HDR-to-SDR tone mapping, or choose an SVT build for compatible HDR10 output.",
      {
        exact: true,
      },
    ),
  ).toBeVisible();
  await expect(workspace).not.toContainText('lossless');
  await workspace.getByRole('button', { name: 'Preview batch', exact: true }).click();
  await expect(page.getByRole('region', { name: 'Batch output preview' })).toContainText(
    '1 ready / 2 reviewed',
  );
  await expect(page.getByRole('region', { name: 'Batch output preview' })).toContainText(
    'unsupported HDR metadata',
  );
  await workspace.getByRole('button', { name: 'Queue ready files', exact: true }).click();
  const requests = (await calls(page, 'enqueue_encode_batch'))[0].payload
    .requests as EncodeRequest[];
  expect(requests).toHaveLength(1);
  expect(requests[0]).toMatchObject({
    source: { inputPath: episodes[0].path },
    settings: { encoder: 'x264', backend: 'standalone', filmGrain: 0, hdr10Fallback: false },
  });
  await expect(workspace.getByLabel('Select Episode 2.mkv', { exact: true })).toBeChecked();
});

test('x264 switching away and back invalidates an outstanding batch preview', async ({ page }) => {
  await desktopMock(page, { held: ['preview'] });
  await openBatch(page);
  const workspace = page.getByRole('region', { name: 'Batch encode workspace' });
  await workspace.getByRole('button', { name: 'Preview batch', exact: true }).click();
  await expect.poll(() => calls(page, 'preview_encode_batch')).toHaveLength(1);
  await workspace.getByLabel('Video encoder', { exact: true }).selectOption('x264');
  await workspace.getByLabel('Video encoder', { exact: true }).selectOption('svtAv1Hdr');
  await release(page, 'preview');
  await expect(
    workspace.getByRole('button', { name: 'Queue ready files', exact: true }),
  ).toBeDisabled();
  await expect(page.getByRole('region', { name: 'Batch output preview' })).not.toContainText(
    '_av1_hdr.mkv',
  );
  await workspace.getByLabel('Video encoder', { exact: true }).selectOption('x264');
  await workspace.getByRole('button', { name: 'Preview batch', exact: true }).click();
  await expect(
    workspace.getByRole('button', { name: 'Queue ready files', exact: true }),
  ).toBeEnabled();
  expect(
    (await calls(page, 'preview_encode_batch')).map(
      ({ payload }) => (payload.request as BatchEncodeRequest).encoder,
    ),
  ).toEqual(['svtAv1Hdr', 'x264']);
  await expect(page.getByRole('region', { name: 'Batch output preview' })).toContainText(
    '_x264.mkv',
  );
});

for (const missing of ['x264', 'svt-av1-hdr'] as const) {
  test(`x264 batch uses independent discovery gates when ${missing} is missing`, async ({
    page,
  }) => {
    await desktopMock(page, { missing });
    await openBatch(page);
    const workspace = page.getByRole('region', { name: 'Batch encode workspace' });
    const preview = workspace.getByRole('button', { name: 'Preview batch', exact: true });
    await expect(preview).toBeEnabled({ enabled: missing === 'x264' });
    await workspace.getByLabel('Video encoder', { exact: true }).selectOption('x264');
    await expect(preview).toBeEnabled({ enabled: missing === 'svt-av1-hdr' });
  });
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
  await expect(page.getByLabel('Preset', { exact: true })).toHaveValue('2');
  await page
    .getByText('Video, audio & source tracks · 3 selected', { exact: true })
    .first()
    .click();
  await page.getByLabel('Video stream for Episode 1.mkv', { exact: true }).selectOption('9');
  await page.getByLabel('Include audio stream #5 from Episode 1.mkv', { exact: true }).uncheck();
  await page.getByLabel('CRF', { exact: true }).fill('27');
  await page.getByRole('button', { name: 'Preview batch', exact: true }).click();
  await expect(page.getByRole('region', { name: 'Batch output preview' })).toContainText(
    '2 ready / 2 reviewed',
  );
  expect((await calls(page, 'preview_encode_batch'))[0].payload).toEqual({
    request: {
      outputContainer: 'matroska',
      outputDirectory: 'C:\\exports',
      crf: 27,
      preset: 2,
      lossless: false,
      svtCrfQuarterSteps: 108,
      svtPreset: 2,
      backend: 'standalone',
      encoder: 'svtAv1Hdr',
      workers: 2,
      filmGrain: 0,
      hdr10Fallback: false,
      lineartPsyBias: 0,
      texturePsyBias: 0,
      hdrTune: 'filmGrain',
      inputs: [
        {
          inputPath: episodes[0].path,
          videoStreamIndex: 9,
          streamIndices: [9, 8, 11],
          framing: {
            crop: { top: 0, right: 0, bottom: 0, left: 0 },
            resizeWidth: null,
            borders: { top: 0, right: 0, bottom: 0, left: 0 },
          },
          audio: [],
        },
        {
          inputPath: episodes[1].path,
          videoStreamIndex: 2,
          streamIndices: [2, 5, 8, 11],
          framing: {
            crop: { top: 0, right: 0, bottom: 0, left: 0 },
            resizeWidth: null,
            borders: { top: 0, right: 0, bottom: 0, left: 0 },
          },
          audio: [{ streamIndex: 5, codec: 'copy', bitrateKbps: 128, channels: 'preserve' }],
        },
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
    '_av1_hdr.mkv',
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
  await desktopMock(page, { missing: 'svt-av1-hdr' });
  await openBatch(page);
  await expect(page.getByRole('button', { name: 'Preview batch', exact: true })).toBeDisabled();
  await expect(
    page
      .getByText(
        'Install FFmpeg, FFprobe, standalone SVT-AV1-HDR, then refresh Tools & settings.',
        {
          exact: true,
        },
      )
      .last(),
  ).toBeVisible();
  await page.getByLabel('CRF', { exact: true }).fill('64');
  await page.getByLabel('Preset', { exact: true }).selectOption('8');
  await page.getByRole('button', { name: 'Reset batch settings', exact: true }).click();
  await expect(page.getByLabel('CRF', { exact: true })).toHaveValue('30');
  await expect(page.getByLabel('Preset', { exact: true })).toHaveValue('2');
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
  // Hidden Batch does not mount hundreds of closed episode control trees.
  await expect(page.locator('.episode-tracks .episode-framing')).toHaveCount(0);
  await page.getByRole('button', { name: 'Batch encode', exact: true }).click();
  await expect(page.getByRole('checkbox', { name: /^Select /, checked: true })).toHaveCount(100);
  await expect(page.getByLabel('Select Episode 101.mkv', { exact: true })).toBeDisabled();
  const first = page.locator('article.episode').filter({ hasText: 'Episode 1.mkv' });
  const toggle = first.locator('.episode-tracks > summary');
  await toggle.click();
  await first.getByText('Crop, resize & borders', { exact: true }).click();
  await expect(page.locator('.episode-tracks .episode-framing')).toHaveCount(1);
  await first.getByLabel('Crop left (pixels)', { exact: true }).fill('20');
  await toggle.click();
  await expect(page.locator('.episode-tracks .episode-framing')).toHaveCount(0);
  await page.getByRole('button', { name: 'Choose output folder', exact: true }).click();
  await page.getByRole('button', { name: 'Preview batch', exact: true }).click();
  const request = (await calls(page, 'preview_encode_batch'))[0].payload
    .request as BatchEncodeRequest;
  expect(request.inputs).toHaveLength(100);
  expect(request.inputs[0].framing.crop.left).toBe(20);
  await expect(page.getByRole('button', { name: 'Queue ready files', exact: true })).toBeEnabled();
  await toggle.click();
  await first.getByText('Crop, resize & borders', { exact: true }).click();
  await expect(first.getByLabel('Crop left (pixels)', { exact: true })).toHaveValue('20');
  // Expanding display-only controls does not invalidate the reviewed request.
  await expect(page.getByRole('button', { name: 'Queue ready files', exact: true })).toBeEnabled();
  await toggle.click();

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
  await page.getByLabel('CRF', { exact: true }).fill('25.2');
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
        backend: setting === 'backend' || setting === 'workers' ? 'av1an' : 'standalone',
        encoder: 'svtAv1Hdr',
        workers: setting === 'workers' ? 4 : 2,
        filmGrain: setting === 'grain' ? 10 : 0,
        hdr10Fallback: setting === 'fallback',
        lineartPsyBias: 0,
        texturePsyBias: 0,
        hdrTune: 'filmGrain',
      });
    }
    await page.getByRole('button', { name: 'Reset batch settings', exact: true }).click();
    expect((await calls(page, 'enqueue_encode_batch'))[0].payload.requests).toEqual(submitted);
    await expect(workspace.getByLabel('Film grain synthesis', { exact: true })).toHaveValue('0');
    await expect(workspace.getByLabel('Allow HDR10 fallback', { exact: true })).not.toBeChecked();
    await expect(workspace.getByLabel('Encode backend', { exact: true })).toHaveValue(
      setting === 'backend' || setting === 'workers' ? 'av1an' : 'standalone',
    );
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
  await workspace.getByLabel('Encode backend', { exact: true }).selectOption('standalone');
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

test('batch audio converts independently per source and queues reviewed immutable settings', async ({
  page,
}) => {
  await desktopMock(page);
  await openBatch(page);
  const workspace = page.getByRole('region', { name: 'Batch encode workspace' });
  const first = workspace.locator('article.episode').filter({ hasText: 'Episode 1.mkv' });
  const second = workspace.locator('article.episode').filter({ hasText: 'Episode 2.mkv' });
  await first.locator('.episode-tracks > summary').click();
  await second.locator('.episode-tracks > summary').click();
  await first.getByLabel('Audio codec', { exact: true }).selectOption('opus');
  await first.getByLabel('Audio bitrate', { exact: true }).fill('192');
  await first.getByLabel('Audio channels', { exact: true }).selectOption('stereo');
  await expect(second.getByLabel('Audio codec', { exact: true })).toHaveValue('copy');
  await workspace.getByRole('button', { name: 'Preview batch', exact: true }).click();
  const preview = (await calls(page, 'preview_encode_batch'))[0].payload
    .request as BatchEncodeRequest;
  expect(preview.inputs.map((input) => input.audio)).toEqual([
    [{ streamIndex: 5, codec: 'opus', bitrateKbps: 192, channels: 'stereo' }],
    [{ streamIndex: 5, codec: 'copy', bitrateKbps: 128, channels: 'preserve' }],
  ]);
  await workspace.getByRole('button', { name: 'Queue ready files', exact: true }).click();
  const queued = (await calls(page, 'enqueue_encode_batch'))[0].payload.requests as EncodeRequest[];
  expect(queued.map((request) => request.settings.audio)).toEqual(
    preview.inputs.map((input) => input.audio),
  );
  await workspace.getByRole('button', { name: 'Select up to 100', exact: true }).click();
  await first.locator('.episode-tracks > summary').click();
  await first.getByLabel('Audio codec', { exact: true }).selectOption('aac');
  await page.getByRole('button', { name: 'Quick Convert', exact: true }).click();
  await expect(page.getByRole('region', { name: 'Current encode job' })).toContainText(
    'Audio #5 → Opus 192 kb/s · stereo',
  );
  expect((await calls(page, 'enqueue_encode_batch'))[0].payload.requests).toEqual(queued);
});

for (const edit of ['codec', 'bitrate', 'channels', 'selection'] as const) {
  test(`batch audio ${edit} changes discard an outstanding preview`, async ({ page }) => {
    await desktopMock(page, { held: ['preview'] });
    await openBatch(page);
    const workspace = page.getByRole('region', { name: 'Batch encode workspace' });
    const first = workspace.locator('article.episode').filter({ hasText: 'Episode 1.mkv' });
    await first.locator('.episode-tracks > summary').click();
    await first.getByLabel('Audio codec', { exact: true }).selectOption('opus');
    await workspace.getByRole('button', { name: 'Preview batch', exact: true }).click();
    await expect.poll(() => calls(page, 'preview_encode_batch')).toHaveLength(1);
    if (edit === 'codec')
      await first.getByLabel('Audio codec', { exact: true }).selectOption('aac');
    if (edit === 'bitrate') await first.getByLabel('Audio bitrate', { exact: true }).fill('192');
    if (edit === 'channels')
      await first.getByLabel('Audio channels', { exact: true }).selectOption('mono');
    if (edit === 'selection')
      await first
        .getByLabel('Include audio stream #5 from Episode 1.mkv', { exact: true })
        .uncheck();
    await release(page, 'preview');
    await expect(
      workspace.getByRole('button', { name: 'Queue ready files', exact: true }),
    ).toBeDisabled();
    await expect(page.getByRole('region', { name: 'Batch output preview' })).toContainText(
      'Review required before queueing',
    );
    await workspace.getByRole('button', { name: 'Preview batch', exact: true }).click();
    await expect(
      workspace.getByRole('button', { name: 'Queue ready files', exact: true }),
    ).toBeEnabled();
  });
}

test('batch audio drafts are isolated by encoder and workflow, with av1an conversion', async ({
  page,
}) => {
  await desktopMock(page);
  await openBatch(page);
  const workspace = page.getByRole('region', { name: 'Batch encode workspace' });
  const first = workspace.locator('article.episode').filter({ hasText: 'Episode 1.mkv' });
  await first.locator('.episode-tracks > summary').click();
  await first.getByLabel('Audio codec', { exact: true }).selectOption('opus');
  await first.getByLabel('Audio bitrate', { exact: true }).fill('192');
  await workspace.getByLabel('Video encoder', { exact: true }).selectOption('x264');
  await expect(first.getByLabel('Audio codec', { exact: true })).toHaveValue('copy');
  await first.getByLabel('Audio codec', { exact: true }).selectOption('aac');
  await workspace.getByLabel('Video encoder', { exact: true }).selectOption('svtAv1Hdr');
  await expect(first.getByLabel('Audio codec', { exact: true })).toHaveValue('opus');
  await expect(first.getByLabel('Audio bitrate', { exact: true })).toHaveValue('192');
  await workspace.getByLabel('Encode backend', { exact: true }).selectOption('av1an');
  await expect(first.getByLabel('Audio codec', { exact: true })).toHaveValue('copy');
  await first.getByLabel('Audio codec', { exact: true }).selectOption('aac');
  await workspace.getByRole('button', { name: 'Preview batch', exact: true }).click();
  const preview = (await calls(page, 'preview_encode_batch'))[0].payload
    .request as BatchEncodeRequest;
  expect(preview.inputs.map((input) => input.audio[0].codec)).toEqual(['aac', 'copy']);
  await workspace.getByLabel('Encode backend', { exact: true }).selectOption('standalone');
  await expect(first.getByLabel('Audio codec', { exact: true })).toHaveValue('opus');
  await expect(first.getByLabel('Audio bitrate', { exact: true })).toHaveValue('192');
});

test('batch invalid audio blocks preview only for selected source tracks', async ({ page }) => {
  await desktopMock(page);
  await openBatch(page);
  const workspace = page.getByRole('region', { name: 'Batch encode workspace' });
  const first = workspace.locator('article.episode').filter({ hasText: 'Episode 1.mkv' });
  await first.locator('.episode-tracks > summary').click();
  await first.getByLabel('Audio codec', { exact: true }).selectOption('aac');
  await first.getByLabel('Audio bitrate', { exact: true }).fill('');
  const preview = workspace.getByRole('button', { name: 'Preview batch', exact: true });
  await expect(preview).toBeDisabled();
  await workspace.getByLabel('Select Episode 1.mkv', { exact: true }).uncheck();
  await expect(preview).toBeEnabled();
  await workspace.getByLabel('Select Episode 1.mkv', { exact: true }).check();
  await first.locator('.episode-tracks > summary').click();
  await expect(preview).toBeDisabled();
  await first.getByLabel('Include audio stream #5 from Episode 1.mkv', { exact: true }).uncheck();
  await expect(preview).toBeEnabled();
  await preview.click();
  const request = (await calls(page, 'preview_encode_batch'))[0].payload
    .request as BatchEncodeRequest;
  expect(request.inputs[0].audio).toEqual([]);
  expect(request.inputs[0].streamIndices).toEqual([2, 8, 11]);
});

test('batch queues reviewed FLAC and MP3 settings independently per file', async ({ page }) => {
  await desktopMock(page);
  await openBatch(page);
  const workspace = page.getByRole('region', { name: 'Batch encode workspace' });
  const first = workspace.locator('article.episode').filter({ hasText: 'Episode 1.mkv' });
  const second = workspace.locator('article.episode').filter({ hasText: 'Episode 2.mkv' });
  await first.locator('.episode-tracks > summary').click();
  await second.locator('.episode-tracks > summary').click();
  await first.getByLabel('Audio codec', { exact: true }).selectOption('flac');
  await expect(first.getByLabel('Audio bitrate', { exact: true })).toHaveCount(0);
  await second.getByLabel('Audio codec', { exact: true }).selectOption('mp3');
  await second.getByLabel('Audio bitrate', { exact: true }).selectOption('192');
  await workspace.getByRole('button', { name: 'Preview batch', exact: true }).click();
  const preview = (await calls(page, 'preview_encode_batch'))[0].payload
    .request as BatchEncodeRequest;
  expect(preview.inputs.map((input) => input.audio[0].codec)).toEqual(['flac', 'mp3']);
  expect(preview.inputs.map((input) => input.audio[0].bitrateKbps)).toEqual([128, 192]);
  await workspace.getByRole('button', { name: 'Queue ready files', exact: true }).click();
  const queued = (await calls(page, 'enqueue_encode_batch'))[0].payload.requests as EncodeRequest[];
  expect(queued.map((request) => request.settings.audio)).toEqual(
    preview.inputs.map((input) => input.audio),
  );
});

for (const encoder of ['x265', 'vp9'] as const) {
  test(`${encoder} batch uses encoder quality limits and snapshots reviewed codec requests`, async ({
    page,
  }) => {
    await desktopMock(page);
    await openBatch(page);
    const workspace = page.getByRole('region', { name: 'Batch encode workspace' });
    await workspace.getByLabel('Video encoder', { exact: true }).selectOption(encoder);
    const quality = workspace.getByLabel('CRF', { exact: true });
    await expect(quality).toHaveValue(encoder === 'x265' ? '28' : '32');
    await expect(workspace.getByLabel('Preset', { exact: true }).locator('option')).toHaveCount(
      encoder === 'x265' ? 10 : 6,
    );
    await expect(workspace.getByLabel('Film grain synthesis', { exact: true })).toHaveCount(0);
    const preview = workspace.getByRole('button', { name: 'Preview batch', exact: true });
    await quality.fill(encoder === 'x265' ? '52' : '64');
    await expect(preview).toBeDisabled();
    await quality.fill('30');
    await preview.click();
    const reviewed = page.getByRole('region', { name: 'Batch output preview' });
    await expect(reviewed).toContainText(`Episode 1_${encoder}.mkv`);
    await reviewed.getByRole('button', { name: 'Queue ready files', exact: true }).click();
    const queued = (await calls(page, 'enqueue_encode_batch'))[0].payload as {
      requests: EncodeRequest[];
    };
    expect(queued.requests).toHaveLength(2);
    for (const request of queued.requests)
      expect(request.settings).toMatchObject({
        encoder,
        crf: 30,
        backend: 'standalone',
        filmGrain: 0,
        hdr10Fallback: false,
      });
  });
}

test('batch exposes direct encoders and blocks unsupported NVENC two-pass and target-size requests', async ({
  page,
}) => {
  await desktopMock(page);
  await openBatch(page);
  const workspace = page.getByRole('region', { name: 'Batch encode workspace' });
  const encoder = workspace.getByLabel('Video encoder', { exact: true });
  await expect(encoder.locator('option[value="aomAv1"]')).toHaveText('AOM · AV1 (standalone)');
  await expect(encoder.locator('option[value="vpxStandalone"]')).toHaveText(
    'VP9 · vpxenc (standalone)',
  );
  await expect(encoder.locator('option[value="x265Standalone"]')).toHaveText(
    'x265 · HEVC (standalone)',
  );
  await encoder.selectOption('x265Standalone');
  await workspace.getByRole('button', { name: 'Preview batch', exact: true }).click();
  const request = (await calls(page, 'preview_encode_batch'))[0].payload
    .request as BatchEncodeRequest;
  expect(request).toMatchObject({
    encoder: 'x265Standalone',
    backend: 'standalone',
    lossless: false,
  });
  await expect(page.getByRole('region', { name: 'Batch output preview' })).toContainText(
    'Episode 1_x265_standalone.mkv',
  );

  await encoder.selectOption('hevcNvenc');
  await workspace.getByLabel('Rate control', { exact: true }).selectOption('bitrate');
  await expect(workspace).toContainText('NVENC bitrate mode is one pass');
  await expect(
    workspace.getByRole('button', { name: 'Preview batch', exact: true }),
  ).toBeDisabled();
  await workspace.getByLabel('Two passes', { exact: true }).uncheck();
  await expect(workspace.getByRole('button', { name: 'Preview batch', exact: true })).toBeEnabled();
  await workspace.getByLabel('Rate control', { exact: true }).selectOption('targetSize');
  await expect(workspace).toContainText('NVENC does not support target-size mode');
  await expect(
    workspace.getByRole('button', { name: 'Preview batch', exact: true }),
  ).toBeDisabled();
});

test('frame trim batch keeps per-source intervals and invalidates reviewed output after edits', async ({
  page,
}) => {
  await desktopMock(page);
  await openBatch(page);
  const workspace = page.getByRole('region', { name: 'Batch encode workspace' });
  await workspace
    .getByText('Video, audio & source tracks · 3 selected', { exact: true })
    .first()
    .click();
  await workspace.getByLabel('Trim video interval', { exact: true }).first().check();
  await workspace.getByLabel('Start frame', { exact: true }).fill('12');
  await workspace.getByLabel('End frame (excluded)', { exact: true }).fill('36');
  const preview = workspace.getByRole('button', { name: 'Preview batch', exact: true });
  await expect(preview).toBeDisabled();
  await workspace.getByLabel('Audio codec', { exact: true }).first().selectOption('flac');
  await preview.click();
  const request = (await calls(page, 'preview_encode_batch'))[0].payload as {
    request: BatchEncodeRequest;
  };
  expect(request.request.inputs[0].trim).toEqual({ startFrame: 12, endFrameExclusive: 36 });
  expect(request.request.inputs[1].trim).toBeUndefined();
  const reviewed = page.getByRole('region', { name: 'Batch output preview' });
  await expect(reviewed).toContainText('2 ready / 2 reviewed');
  await workspace.getByLabel('End frame (excluded)', { exact: true }).fill('48');
  await expect(
    reviewed.getByRole('button', { name: 'Queue ready files', exact: true }),
  ).toBeDisabled();
  await preview.click();
  await page.getByRole('button', { name: 'Queue ready files', exact: true }).click();
  const queued = (await calls(page, 'enqueue_encode_batch'))[0].payload as {
    requests: EncodeRequest[];
  };
  expect(queued.requests[0].settings.trim).toEqual({ startFrame: 12, endFrameExclusive: 48 });
  expect(queued.requests[1].settings.trim).toBeUndefined();
});

test('rate control batch propagates file-size targets and invalidates the reviewed request', async ({
  page,
}) => {
  await desktopMock(page);
  await openBatch(page);
  const workspace = page.getByRole('region', { name: 'Batch encode workspace' });
  await workspace.getByLabel('Rate control', { exact: true }).selectOption('targetSize');
  await workspace.getByLabel('Target file size (MiB)', { exact: true }).fill('700');
  const preview = workspace.getByRole('button', { name: 'Preview batch', exact: true });
  await preview.click();
  const submitted = (await calls(page, 'preview_encode_batch'))[0].payload
    .request as BatchEncodeRequest;
  expect(submitted.rateControl).toEqual({ mode: 'targetSize', targetSizeMib: 700 });
  const reviewed = workspace.getByRole('region', { name: 'Batch output preview' });
  await expect(reviewed).toContainText('Target 700 MiB');
  await workspace.getByLabel('Target file size (MiB)', { exact: true }).fill('800');
  await expect(
    reviewed.getByRole('button', { name: 'Queue ready files', exact: true }),
  ).toBeDisabled();
  await preview.click();
  await reviewed.getByRole('button', { name: 'Queue ready files', exact: true }).click();
  const queued = (await calls(page, 'enqueue_encode_batch'))[0].payload.requests as EncodeRequest[];
  expect(queued).toHaveLength(2);
  expect(queued.map((request) => request.settings.rateControl)).toEqual(
    Array(2).fill({ mode: 'targetSize', targetSizeMib: 800 }),
  );
});

test('tone mapping batch keeps per-file HDR choices and invalidates its reviewed request', async ({
  page,
}) => {
  const files = [
    {
      ...episodes[0],
      streams: episodes[0].streams.map((stream) =>
        stream.kind === 'video'
          ? {
              ...stream,
              pixelFormat: 'yuv420p10le',
              bitDepth: 10,
              colorPrimaries: 'bt2020',
              colorTransfer: 'arib-std-b67',
              colorSpace: 'bt2020nc',
              colorRange: 'tv',
              hdrFormat: 'HLG',
            }
          : stream,
      ),
    },
    episodes[1],
  ];
  await desktopMock(page, { files });
  await openBatch(page);
  const workspace = page.getByRole('region', { name: 'Batch encode workspace' });
  await workspace
    .getByText('Video, audio & source tracks · 3 selected', { exact: true })
    .first()
    .click();
  await workspace.getByLabel('HDR / HLG to SDR', { exact: true }).first().check();
  await workspace.getByLabel('Signal peak (nits)', { exact: true }).fill('1200');
  const preview = workspace.getByRole('button', { name: 'Preview batch', exact: true });
  await preview.click();
  const submitted = (
    (await calls(page, 'preview_encode_batch'))[0].payload as { request: BatchEncodeRequest }
  ).request;
  expect(submitted.inputs[0].toneMap).toEqual({ sourcePeakNits: 1200, hdr10BaseLayer: false });
  expect(submitted.inputs[1].toneMap).toBeUndefined();
  const reviewed = page.getByRole('region', { name: 'Batch output preview' });
  await expect(reviewed).toContainText('Hable 1200 → 100 nits');
  await workspace.getByLabel('Signal peak (nits)', { exact: true }).fill('1000');
  await expect(
    reviewed.getByRole('button', { name: 'Queue ready files', exact: true }),
  ).toBeDisabled();
  await preview.click();
  await reviewed.getByRole('button', { name: 'Queue ready files', exact: true }).click();
  const queued = (await calls(page, 'enqueue_encode_batch'))[0].payload.requests as EncodeRequest[];
  expect(queued[0].settings.toneMap).toEqual({ sourcePeakNits: 1000, hdr10BaseLayer: false });
  expect(queued[1].settings.toneMap).toBeUndefined();
});

test('av1an scene batch settings invalidate review and preserve queued target parameters', async ({
  page,
}) => {
  await desktopMock(page);
  await openBatch(page);
  const workspace = page.getByRole('region', { name: 'Batch encode workspace' });
  await workspace.getByLabel('Encode backend', { exact: true }).selectOption('av1an');
  await workspace.getByText('Scenes and quality targeting', { exact: true }).click();
  await workspace.getByLabel('Source reader', { exact: true }).selectOption('bestsource');
  await workspace.getByLabel('Target perceptual quality', { exact: true }).check();
  const preview = workspace.getByRole('button', { name: 'Preview batch', exact: true });
  await preview.click();
  const first = (await calls(page, 'preview_encode_batch'))[0].payload
    .request as BatchEncodeRequest;
  expect(first.av1anOptions).toMatchObject({
    chunkMethod: 'bestsource',
    targetQuality: { minimumScoreTenths: 940 },
  });
  const reviewed = workspace.getByRole('region', { name: 'Batch output preview' });
  await workspace.getByLabel('Chunk order', { exact: true }).selectOption('sequential');
  await expect(
    reviewed.getByRole('button', { name: 'Queue ready files', exact: true }),
  ).toBeDisabled();
  await preview.click();
  await reviewed.getByRole('button', { name: 'Queue ready files', exact: true }).click();
  const requests = (await calls(page, 'enqueue_encode_batch'))[0].payload
    .requests as EncodeRequest[];
  expect(requests).toHaveLength(2);
  for (const request of requests)
    expect(request.settings.av1anOptions).toMatchObject({
      chunkMethod: 'bestsource',
      chunkOrder: 'sequential',
      targetQuality: { minimumScoreTenths: 940 },
    });
});
