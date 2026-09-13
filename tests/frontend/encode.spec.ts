import { expect, test, type Page } from '@playwright/test';
import type {
  AppError,
  EncodeRequest,
  JobSnapshot,
  MediaFile,
  MediaStream,
  ToolInfo,
} from '../../src/lib/ipc/generated';

const inputPath = 'C:\\media\\café & 東京.mkv';
const outputPath = 'C:\\exports\\café & 東京 AV1.mkv';
const baseStream: MediaStream = {
  index: 0,
  kind: 'video',
  codec: 'h264',
  width: 320,
  height: 180,
  frameRate: '24000/1001',
  sampleRate: null,
  channels: null,
  language: null,
  title: null,
};
const media: MediaFile = {
  id: 'encode-source',
  path: inputPath,
  name: 'café & 東京.mkv',
  sizeBytes: '123456',
  durationSeconds: 12,
  format: 'matroska,webm',
  streams: [
    { ...baseStream, index: 0, title: 'Main video' },
    { ...baseStream, index: 4, title: 'Alternate video' },
    {
      ...baseStream,
      index: 3,
      kind: 'audio',
      codec: 'aac',
      width: null,
      height: null,
      frameRate: null,
      sampleRate: 48000,
      channels: 2,
      title: 'Original audio',
    },
    {
      ...baseStream,
      index: 7,
      kind: 'subtitle',
      codec: 'subrip',
      width: null,
      height: null,
      frameRate: null,
      title: 'English',
    },
    {
      ...baseStream,
      index: 9,
      kind: 'attachment',
      codec: 'ttf',
      width: null,
      height: null,
      frameRate: null,
      title: 'Subtitle font',
    },
  ],
};
const tools: ToolInfo[] = [
  'ffmpeg',
  'ffprobe',
  'svt-av1',
  'svt-av1-5fish',
  'svt-av1-hdr',
  'av1an',
  'x264',
].map((id) => ({
  id,
  name: id,
  available: true,
  path: `C:\\tools\\${id}.exe`,
  version: 'test version',
  detail: null,
}));
type Mock = {
  calls: { command: string; payload: unknown }[];
  emit: (jobs: JobSnapshot[]) => void;
  setMedia: (media: MediaFile) => void;
  hold: (command: string) => void;
  release: (command: string) => void;
};
const snapshot = (state: JobSnapshot['state'] = 'running'): JobSnapshot => ({
  id: 'encode-1',
  state,
  request: { inputPath, outputPath, streamIndices: [0, 3, 7, 9] },
  encodeSettings: {
    videoStreamIndex: 0,
    crf: 30,
    preset: 2,
    backend: 'standalone',
    encoder: 'svtAv1Hdr',
    workers: 2,
    filmGrain: 0,
    hdr10Fallback: false,
    lineartPsyBias: 0,
    texturePsyBias: 0,
    hdrTune: 'filmGrain',
    framing: {
      crop: { top: 0, right: 0, bottom: 0, left: 0 },
      resizeWidth: null,
      borders: { top: 0, right: 0, bottom: 0, left: 0 },
    },
    audio: [{ streamIndex: 3, codec: 'copy', bitrateKbps: 128, channels: 'preserve' }],
  },
  progressSeconds: 3,
  durationSeconds: 12,
  logs: ['Encoding video with standalone SVT-AV1.'],
  error: null,
  recovery: null,
  logPath: 'C:\\logs\\encode-1.log',
});

async function desktopMock(
  page: Page,
  options: {
    missing?: string;
    jobs?: JobSnapshot[];
    failure?: AppError;
    late?: 'start' | 'cancel' | 'enqueue';
    connectionFailures?: number;
    lateStop?: boolean;
    held?: string[];
    pickerFailure?: AppError;
    media?: MediaFile;
  } = {},
) {
  await page.addInitScript(
    ({
      initialMedia,
      capabilities,
      destination,
      initialJobs,
      failure,
      late,
      connectionFailures,
      lateStop,
      heldCommands,
      pickerFailure,
    }) => {
      const state = globalThis as unknown as Record<string, unknown>;
      let selectedMedia = initialMedia;
      let jobs = initialJobs;
      let callbackId = 0;
      let remainingFailures = connectionFailures;
      const calls: Mock['calls'] = [];
      const held = new Set(heldCommands);
      const waiting = new Map<string, (() => void)[]>();
      const wait = async (command: string) => {
        if (held.has(command)) {
          await new Promise<void>((resolve) => {
            waiting.set(command, [...(waiting.get(command) ?? []), resolve]);
          });
        }
      };
      let channel: { onmessage: (jobs: JobSnapshot[]) => void } | undefined;
      const publish = (next: JobSnapshot[]) => {
        jobs = next;
        channel?.onmessage(next);
      };
      state.__encodeMock = {
        calls,
        emit: publish,
        setMedia: (next) => {
          selectedMedia = next;
        },
        hold: (command) => held.add(command),
        release: (command) => {
          held.delete(command);
          for (const resolve of waiting.get(command) ?? []) resolve();
          waiting.delete(command);
        },
      } satisfies Mock;
      state.isTauri = true;
      state.__TAURI_INTERNALS__ = {
        metadata: { currentWindow: { label: 'main' }, currentWebview: { label: 'main' } },
        transformCallback: () => ++callbackId,
        unregisterCallback: () => {},
        invoke: async (command: string, payload: Record<string, unknown> = {}) => {
          calls.push({ command, payload });
          await wait(command);
          if (command === 'get_capabilities') return capabilities;
          if (command === 'plugin:dialog|open') return [selectedMedia.path];
          if (command === 'plugin:dialog|save') {
            if (pickerFailure) throw pickerFailure;
            return destination;
          }
          if (command.startsWith('plugin:event|')) return ++callbackId;
          if (command === 'probe_media') return selectedMedia;
          if (command === 'list_jobs') return jobs;
          if (command === 'subscribe_jobs') {
            if (remainingFailures-- > 0)
              throw {
                code: 'JOB_STORE_UNAVAILABLE',
                message: 'The job history could not be read. Check the storage folder.',
                path: null,
              };
            channel = payload.channel as typeof channel;
            channel?.onmessage(jobs);
            return;
          }
          if (command === 'start_encode' || command === 'enqueue_encode') {
            if (failure) throw failure;
            const request = payload.request as EncodeRequest;
            const job: JobSnapshot = {
              id: `encode-${jobs.length + 1}`,
              state: command === 'enqueue_encode' ? 'queued' : 'running',
              request: request.source,
              encodeSettings: request.settings,
              progressSeconds: 0,
              durationSeconds: selectedMedia.durationSeconds,
              logs: ['Encoding video with standalone SVT-AV1.'],
              error: null,
              recovery: null,
              logPath: null,
            };
            const terminalBeforeReply = late === 'start' || late === 'enqueue';
            publish([{ ...job, state: terminalBeforeReply ? 'succeeded' : job.state }, ...jobs]);
            return { ...job, state: terminalBeforeReply ? 'queued' : job.state };
          }
          if (command === 'cancel_job') {
            const job = jobs.find((entry) => entry.id === payload.id)!;
            const state = job.state === 'queued' || late === 'cancel' ? 'canceled' : 'canceling';
            publish(jobs.map((entry) => (entry.id === job.id ? { ...job, state } : entry)));
            return { ...job, state: job.state === 'queued' ? 'canceled' : 'canceling' };
          }
          if (command === 'cancel_all_jobs') {
            const stopped: JobSnapshot[] = jobs.map((entry) => ({
              ...entry,
              state:
                entry.state === 'queued'
                  ? 'canceled'
                  : entry.state === 'running'
                    ? 'canceling'
                    : entry.state,
            }));
            publish(
              lateStop
                ? stopped.map((entry) => ({
                    ...entry,
                    state: entry.state === 'canceling' ? 'canceled' : entry.state,
                  }))
                : stopped,
            );
            return stopped;
          }
          throw new Error(`Unexpected command ${command}`);
        },
      };
    },
    {
      initialMedia: options.media ?? media,
      capabilities: tools.map((tool) =>
        tool.id === options.missing ? { ...tool, available: false, path: null } : tool,
      ),
      destination: outputPath,
      initialJobs: options.jobs ?? [],
      failure: options.failure,
      late: options.late,
      connectionFailures: options.connectionFailures ?? 0,
      lateStop: options.lateStop ?? false,
      heldCommands: options.held ?? [],
      pickerFailure: options.pickerFailure,
    },
  );
}
const quickWorkspace = (page: Page) =>
  page.getByRole('region', { name: 'Quick Convert workspace', exact: true });
const av1anWorkspace = (page: Page) =>
  page.getByRole('region', { name: 'av1an workspace', exact: true });

for (const tab of ['Quick Convert', 'av1an'] as const) {
  test(`${tab} defaults to HDR with anime and mainline SVT as explicit choices`, async ({
    page,
  }) => {
    await desktopMock(page);
    await openEncode(page, tab);
    const workspace = tab === 'av1an' ? av1anWorkspace(page) : quickWorkspace(page);
    const selector = workspace.getByLabel(tab === 'av1an' ? 'SVT-AV1 build' : 'Video encoder', {
      exact: true,
    });
    await expect(selector).toHaveValue('svtAv1Hdr');
    await expect(selector.locator('option').first()).toHaveAttribute('value', 'svtAv1Hdr');
    await expect(workspace.getByLabel('Quality', { exact: true })).toHaveValue('30');
    await expect(workspace.getByLabel('Encoder preset', { exact: true })).toHaveValue('2');
    await expect(workspace.getByLabel('HDR tune', { exact: true })).toHaveValue('filmGrain');
    await expect(workspace.getByLabel('Allow HDR10 fallback', { exact: true })).not.toBeChecked();
    await expect(selector.locator('option[value="svtAv1FiveFish"]')).toContainText('Anime');
    await selector.selectOption('svtAv1');
    await expect(workspace.getByLabel('Quality', { exact: true })).toHaveValue('30');
    await expect(workspace.getByLabel('Encoder preset', { exact: true })).toHaveValue('4');
    await expect(workspace.getByLabel('HDR tune', { exact: true })).toHaveCount(0);
    await expect(
      workspace.getByRole('button', { name: 'Start encode', exact: true }),
    ).toBeEnabled();
  });
}

for (const encoder of ['svtAv1Hdr', 'svtAv1FiveFish', 'svtAv1', 'x264'] as const) {
  test(`crop resize and borders ${encoder} requests retain immutable framing in job history`, async ({
    page,
  }) => {
    await desktopMock(page);
    await openEncode(page);
    const quick = quickWorkspace(page);
    await quick.getByLabel('Video encoder', { exact: true }).selectOption(encoder);
    await quick.getByLabel('Crop top (pixels)', { exact: true }).fill('2');
    await quick.getByLabel('Crop bottom (pixels)', { exact: true }).fill('2');
    await quick.getByLabel('Crop left (pixels)', { exact: true }).fill('4');
    await quick.getByLabel('Crop right (pixels)', { exact: true }).fill('4');
    await quick.getByLabel('Resize video', { exact: true }).check();
    await quick.getByLabel('Picture width (pixels)', { exact: true }).fill('160');
    await expect(quick.getByLabel('Border top (pixels)', { exact: true })).toHaveCount(0);
    await quick.getByLabel('Add black borders', { exact: true }).check();
    await quick.getByLabel('Border top (pixels)', { exact: true }).fill('6');
    await quick.getByLabel('Border right (pixels)', { exact: true }).fill('8');
    await quick.getByLabel('Border bottom (pixels)', { exact: true }).fill('10');
    await quick.getByLabel('Border left (pixels)', { exact: true }).fill('12');
    await expect(quick.getByLabel('Video dimensions', { exact: true })).toContainText(
      'Source 320 × 180 → Cropped 312 × 176 → Picture 160 × 90 → Output 180 × 106',
    );
    await quick.getByRole('button', { name: 'Start encode', exact: true }).click();
    const started = (await calls(page, 'start_encode'))[0].payload as { request: EncodeRequest };
    expect(started.request.settings.framing).toEqual({
      crop: { top: 2, right: 4, bottom: 2, left: 4 },
      resizeWidth: 160,
      borders: { top: 6, right: 8, bottom: 10, left: 12 },
    });
    await quick.getByRole('button', { name: 'Reset settings', exact: true }).click();
    await expect(quick.getByLabel('Crop top (pixels)', { exact: true })).toHaveValue('0');
    await expect(quick.getByLabel('Resize video', { exact: true })).not.toBeChecked();
    await expect(quick.getByLabel('Add black borders', { exact: true })).not.toBeChecked();
    await quick.getByLabel('Add black borders', { exact: true }).check();
    await expect(quick.getByLabel('Border top (pixels)', { exact: true })).toHaveValue('0');
    await expect(page.getByRole('region', { name: 'Current encode job' })).toContainText(
      'Crop top 2, right 4, bottom 2, left 4 px · Picture width 160 px · Automatic height · Black borders top 6, right 8, bottom 10, left 12 px',
    );
    expect((await calls(page, 'start_encode'))[0].payload).toEqual(started);
  });
}

test('black borders toggle retains its draft and omits disabled borders from queued requests', async ({
  page,
}) => {
  await desktopMock(page);
  await openEncode(page);
  const quick = quickWorkspace(page);
  const toggle = quick.getByLabel('Add black borders', { exact: true });
  await toggle.check();
  await quick.getByLabel('Border top (pixels)', { exact: true }).fill('12');
  await quick.getByLabel('Border bottom (pixels)', { exact: true }).fill('8');
  await expect(quick.getByLabel('Video dimensions', { exact: true })).toContainText(
    'Picture 320 × 180 → Output 320 × 200',
  );
  await toggle.uncheck();
  await expect(quick.getByLabel('Border top (pixels)', { exact: true })).toHaveCount(0);
  await expect(quick.getByLabel('Video dimensions', { exact: true })).toContainText(
    'Output 320 × 180',
  );
  await quick.getByRole('button', { name: 'Add to queue', exact: true }).click();
  const queued = (await calls(page, 'enqueue_encode'))[0].payload as { request: EncodeRequest };
  expect(queued.request.settings.framing.borders).toEqual({ top: 0, right: 0, bottom: 0, left: 0 });
  await toggle.check();
  await expect(quick.getByLabel('Border top (pixels)', { exact: true })).toHaveValue('12');
  await expect(quick.getByLabel('Border bottom (pixels)', { exact: true })).toHaveValue('8');
  await expect(page.getByRole('region', { name: 'Current encode job' })).not.toContainText(
    'Black borders',
  );
});

test('black borders reject invalid edges and final dimensions before start or queue', async ({
  page,
}) => {
  await desktopMock(page);
  await openEncode(page);
  const quick = quickWorkspace(page);
  const start = quick.getByRole('button', { name: 'Start encode', exact: true });
  const queue = quick.getByRole('button', { name: 'Add to queue', exact: true });
  await quick.getByLabel('Add black borders', { exact: true }).check();
  for (const edge of ['top', 'right', 'bottom', 'left']) {
    const input = quick.getByLabel(`Border ${edge} (pixels)`, { exact: true });
    for (const invalid of ['', '-2', '3', '2.5', '8192']) {
      await input.fill(invalid);
      await expect(start).toBeDisabled();
      await expect(queue).toBeDisabled();
    }
    await input.fill('0');
  }
  await quick.getByLabel('Border right (pixels)', { exact: true }).fill('7872');
  await quick.getByLabel('Border bottom (pixels)', { exact: true }).fill('8012');
  await expect(quick.getByLabel('Video dimensions', { exact: true })).toContainText(
    'Output 8192 × 8192',
  );
  await expect(start).toBeEnabled();
  await quick.getByLabel('Border left (pixels)', { exact: true }).fill('2');
  await expect(start).toBeDisabled();
  await expect(queue).toBeDisabled();
  await expect(quick.getByLabel('Video dimensions', { exact: true })).toContainText(
    'including borders must not exceed 8192',
  );
  await quick.getByLabel('Add black borders', { exact: true }).uncheck();
  await expect(start).toBeEnabled();
  await expect(queue).toBeEnabled();
  expect(await calls(page, 'start_encode')).toHaveLength(0);
  expect(await calls(page, 'enqueue_encode')).toHaveLength(0);
});

test('legacy framing history remains readable when black borders are absent', async ({ page }) => {
  const legacy = snapshot('succeeded');
  const framing = legacy.encodeSettings!.framing;
  framing.crop.top = 4;
  framing.resizeWidth = 160;
  delete (framing as Partial<typeof framing>).borders;
  await desktopMock(page, { jobs: [legacy] });
  await openEncode(page);
  await expect(page.getByRole('region', { name: 'Current encode job' })).toContainText(
    'Crop top 4, right 0, bottom 0, left 0 px · Picture width 160 px · Automatic height',
  );
  await expect(page.getByRole('region', { name: 'Current encode job' })).not.toContainText(
    'Black borders',
  );
});

test('crop and resize invalid edits block both commands and selected video changes recompute dimensions', async ({
  page,
}) => {
  await desktopMock(page, {
    media: {
      ...media,
      streams: media.streams.map((stream) =>
        stream.index === 4 ? { ...stream, width: 640, height: 480 } : stream,
      ),
    },
  });
  await openEncode(page);
  const quick = quickWorkspace(page);
  const start = quick.getByRole('button', { name: 'Start encode', exact: true });
  const queue = quick.getByRole('button', { name: 'Add to queue', exact: true });
  for (const edge of ['top', 'right', 'bottom', 'left']) {
    const input = quick.getByLabel(`Crop ${edge} (pixels)`, { exact: true });
    for (const invalid of ['', '-2', '1', '2.5', '8192']) {
      await input.fill(invalid);
      await expect(start).toBeDisabled();
      await expect(queue).toBeDisabled();
    }
    await input.fill('0');
  }
  await quick.getByLabel('Crop top (pixels)', { exact: true }).fill('118');
  await expect(start).toBeDisabled();
  await quick.getByLabel('Crop top (pixels)', { exact: true }).fill('0');
  await quick.getByLabel('Resize video', { exact: true }).check();
  const width = quick.getByLabel('Picture width (pixels)', { exact: true });
  for (const invalid of ['', '-2', '63', '161', '160.5', '8194', '64']) {
    await width.fill(invalid);
    await expect(start).toBeDisabled();
    await expect(queue).toBeDisabled();
  }
  await width.fill('160');
  await expect(start).toBeEnabled();
  await expect(quick.getByLabel('Video dimensions', { exact: true })).toContainText(
    'Output 160 × 90',
  );
  await quick.getByLabel('Video stream', { exact: true }).selectOption('4');
  await expect(quick.getByLabel('Video dimensions', { exact: true })).toContainText(
    'Output 160 × 120',
  );
  expect(await calls(page, 'start_encode')).toHaveLength(0);
  expect(await calls(page, 'enqueue_encode')).toHaveLength(0);
});

test('crop resize and border drafts survive source, encoder and workflow switches while av1an sends defaults', async ({
  page,
}) => {
  await desktopMock(page);
  await page.setViewportSize({ width: 760, height: 650 });
  await openEncode(page);
  const quick = quickWorkspace(page);
  await quick.getByLabel('Crop top (pixels)', { exact: true }).fill('4');
  await quick.getByLabel('Resize video', { exact: true }).check();
  await quick.getByLabel('Picture width (pixels)', { exact: true }).fill('160');
  await quick.getByLabel('Add black borders', { exact: true }).check();
  await quick.getByLabel('Border top (pixels)', { exact: true }).fill('12');
  await quick.getByLabel('Video encoder', { exact: true }).selectOption('x264');
  await expect(quick.getByLabel('Crop top (pixels)', { exact: true })).toHaveValue('0');
  await expect(quick.getByLabel('Add black borders', { exact: true })).not.toBeChecked();
  await quick.getByLabel('Crop left (pixels)', { exact: true }).fill('8');
  await quick.getByLabel('Add black borders', { exact: true }).check();
  await quick.getByLabel('Border left (pixels)', { exact: true }).fill('20');
  await quick.getByLabel('Video encoder', { exact: true }).selectOption('svtAv1Hdr');
  await expect(quick.getByLabel('Crop top (pixels)', { exact: true })).toHaveValue('4');
  await expect(quick.getByLabel('Border top (pixels)', { exact: true })).toHaveValue('12');
  const next = {
    ...media,
    id: 'framing-second',
    name: 'second.mkv',
    path: 'C:\\media\\second.mkv',
  };
  await importAnotherSource(page, next);
  await page.getByRole('button', { name: 'Quick Convert', exact: true }).click();
  await expect(quick.getByLabel('Crop top (pixels)', { exact: true })).toHaveValue('0');
  await expect(quick.getByLabel('Add black borders', { exact: true })).not.toBeChecked();
  await quick.getByLabel('Crop bottom (pixels)', { exact: true }).fill('8');
  await quick.getByLabel('Add black borders', { exact: true }).check();
  await quick.getByLabel('Border top (pixels)', { exact: true }).fill('40');
  await page
    .getByRole('navigation', { name: 'Workspace' })
    .getByRole('button', { name: /^Files/ })
    .click();
  await page.locator('button.file-select').filter({ hasText: media.name }).click();
  await page.getByRole('button', { name: 'av1an', exact: true }).click();
  const av1an = av1anWorkspace(page);
  await expect(av1an.getByLabel('Crop top (pixels)', { exact: true })).toHaveCount(0);
  await expect(av1an.getByLabel('Add black borders', { exact: true })).toHaveCount(0);
  await av1an.getByRole('button', { name: 'Add to queue', exact: true }).click();
  const queued = (await calls(page, 'enqueue_encode'))[0].payload as { request: EncodeRequest };
  expect(queued.request.settings.framing).toEqual({
    crop: { top: 0, right: 0, bottom: 0, left: 0 },
    resizeWidth: null,
    borders: { top: 0, right: 0, bottom: 0, left: 0 },
  });
  await page.getByRole('button', { name: 'Quick Convert', exact: true }).click();
  await expect(quick.getByLabel('Crop top (pixels)', { exact: true })).toHaveValue('4');
  await expect(quick.getByLabel('Picture width (pixels)', { exact: true })).toHaveValue('160');
  await expect(quick.getByLabel('Border top (pixels)', { exact: true })).toHaveValue('12');
  await expect(quick.getByLabel('Border left (pixels)', { exact: true })).toHaveValue('0');
  await quick.getByLabel('Video encoder', { exact: true }).selectOption('x264');
  await expect(quick.getByLabel('Crop left (pixels)', { exact: true })).toHaveValue('8');
  await expect(quick.getByLabel('Crop top (pixels)', { exact: true })).toHaveValue('0');
  await expect(quick.getByLabel('Border top (pixels)', { exact: true })).toHaveValue('0');
  await expect(quick.getByLabel('Border left (pixels)', { exact: true })).toHaveValue('20');
  expect(await page.evaluate(() => document.documentElement.scrollWidth <= innerWidth)).toBe(true);
});

const x264PresetNames = [
  'ultrafast',
  'superfast',
  'veryfast',
  'faster',
  'fast',
  'medium',
  'slow',
  'slower',
  'veryslow',
  'placebo',
];

test('tool discovery shows all seven tools as pending before the native reply', async ({
  page,
}) => {
  await desktopMock(page, { held: ['get_capabilities'], missing: 'svt-av1-hdr' });
  await page.goto('/');
  await page.getByRole('button', { name: 'Tools & settings', exact: true }).click();
  const workspace = page.getByRole('region', { name: 'Tools and settings', exact: true });
  await expect(workspace.getByRole('row')).toHaveCount(8);
  await expect(workspace.getByText('Checking…', { exact: true })).toHaveCount(7);
  await expect(workspace.getByText('Not found', { exact: true })).toHaveCount(0);
  await expect(workspace.getByRole('row').filter({ hasText: 'svt-av1-5fish' })).toContainText(
    'SVT-AV1 5fish',
  );
  await expect(workspace.getByRole('row').filter({ hasText: 'svt-av1-hdr' })).toContainText(
    'SVT-AV1-HDR',
  );
  await release(page, 'get_capabilities');
  await expect(workspace).toContainText('6 / 7 available');
  await expect(workspace.getByText('Checking…', { exact: true })).toHaveCount(0);
  await expect(workspace.getByRole('row').filter({ hasText: 'svt-av1-hdr' })).toContainText(
    'Not found',
  );
  await expect(workspace.getByRole('row').filter({ hasText: 'svt-av1-5fish' })).toContainText(
    'Available',
  );
});

for (const tab of ['Quick Convert', 'av1an'] as const) {
  test(`SVT forks in ${tab} use explicit defaults and preserve both variants in queue history`, async ({
    page,
  }) => {
    await desktopMock(page);
    await openEncode(page, tab);
    const workspace = tab === 'av1an' ? av1anWorkspace(page) : quickWorkspace(page);
    const selector = workspace.getByLabel(tab === 'av1an' ? 'SVT-AV1 build' : 'Video encoder', {
      exact: true,
    });
    await expect(selector).toHaveValue('svtAv1Hdr');
    if (tab === 'av1an') await expect(selector.locator('option[value="x264"]')).toHaveCount(0);
    await selector.selectOption('svtAv1FiveFish');
    await expect(workspace.getByLabel('Quality', { exact: true })).toHaveValue('18');
    await expect(workspace.getByLabel('Encoder preset', { exact: true })).toHaveValue('2');
    await expect(workspace.getByLabel('Lineart psy bias', { exact: true })).toHaveValue('5');
    await expect(workspace.getByLabel('Texture psy bias', { exact: true })).toHaveValue('4');
    await expect(workspace.getByLabel('HDR tune', { exact: true })).toHaveCount(0);
    await expect(workspace.getByLabel('Encode destination', { exact: true })).toHaveValue(
      inputPath.replace(/\.mkv$/, `_${tab === 'av1an' ? 'av1an' : 'av1'}_5fish.mkv`),
    );
    await workspace.getByLabel('Lineart psy bias', { exact: true }).fill('6');
    await workspace.getByLabel('Texture psy bias', { exact: true }).fill('3');
    await workspace.getByLabel('Film grain synthesis', { exact: true }).fill('8');
    await workspace.getByRole('button', { name: 'Add to queue', exact: true }).click();
    const first = (await calls(page, 'enqueue_encode'))[0].payload as { request: EncodeRequest };
    expect(first.request.settings).toEqual({
      backend: tab === 'av1an' ? 'av1an' : 'standalone',
      encoder: 'svtAv1FiveFish',
      videoStreamIndex: 0,
      workers: 2,
      crf: 18,
      preset: 2,
      lineartPsyBias: 6,
      texturePsyBias: 3,
      hdrTune: 'visualQuality',
      framing: {
        crop: { top: 0, right: 0, bottom: 0, left: 0 },
        resizeWidth: null,
        borders: { top: 0, right: 0, bottom: 0, left: 0 },
      },
      audio:
        tab === 'av1an'
          ? []
          : [{ streamIndex: 3, codec: 'copy', bitrateKbps: 128, channels: 'preserve' }],
      filmGrain: 8,
      hdr10Fallback: false,
    });
    await selector.selectOption('svtAv1Hdr');
    await expect(workspace.getByLabel('Quality', { exact: true })).toHaveValue('30');
    await expect(workspace.getByLabel('Encoder preset', { exact: true })).toHaveValue('2');
    await expect(workspace.getByLabel('HDR tune', { exact: true })).toHaveValue('filmGrain');
    await expect(workspace.getByLabel('Lineart psy bias', { exact: true })).toHaveCount(0);
    await expect(workspace.getByLabel('Film grain synthesis', { exact: true })).toHaveValue('0');
    await expect(workspace.getByLabel('Allow HDR10 fallback', { exact: true })).not.toBeChecked();
    await expect(workspace.getByLabel('Encode destination', { exact: true })).toHaveValue(
      inputPath.replace(/\.mkv$/, `_${tab === 'av1an' ? 'av1an' : 'av1'}_hdr.mkv`),
    );
    await workspace.getByLabel('HDR tune', { exact: true }).selectOption('visualQuality');
    await workspace.getByLabel('Allow HDR10 fallback', { exact: true }).check();
    await workspace.getByRole('button', { name: 'Add to queue', exact: true }).click();
    const second = (await calls(page, 'enqueue_encode'))[1].payload as { request: EncodeRequest };
    expect(second.request.settings).toEqual({
      backend: tab === 'av1an' ? 'av1an' : 'standalone',
      encoder: 'svtAv1Hdr',
      videoStreamIndex: 0,
      workers: 2,
      crf: 30,
      preset: 2,
      lineartPsyBias: 0,
      texturePsyBias: 0,
      hdrTune: 'visualQuality',
      framing: {
        crop: { top: 0, right: 0, bottom: 0, left: 0 },
        resizeWidth: null,
        borders: { top: 0, right: 0, bottom: 0, left: 0 },
      },
      audio:
        tab === 'av1an'
          ? []
          : [{ streamIndex: 3, codec: 'copy', bitrateKbps: 128, channels: 'preserve' }],
      filmGrain: 0,
      hdr10Fallback: true,
    });
    await workspace.getByRole('button', { name: 'Reset settings', exact: true }).click();
    await expect(workspace.getByLabel('HDR tune', { exact: true })).toHaveValue('filmGrain');
    await selector.selectOption('svtAv1FiveFish');
    await expect(workspace.getByLabel('Lineart psy bias', { exact: true })).toHaveValue('6');
    await expect(workspace.getByLabel('Texture psy bias', { exact: true })).toHaveValue('3');
    await expect(page.getByRole('region', { name: 'Current encode job' })).toContainText(
      'SVT-AV1 5fish',
    );
    await expect(page.getByRole('region', { name: 'Current encode job' })).toContainText(
      'Lineart 6 · Texture 3',
    );
    const history = page.getByRole('article', { name: 'Job encode-2', exact: true });
    await history.getByText('Saved settings and log', { exact: true }).click();
    await expect(history).toContainText('SVT-AV1-HDR');
    await expect(history).toContainText('HDR tune visual quality');
    expect((await calls(page, 'enqueue_encode')).map((call) => call.payload)).toEqual([
      first,
      second,
    ]);
  });

  test(`SVT 5fish in ${tab} validates both biases without applying them to other builds`, async ({
    page,
  }) => {
    await desktopMock(page);
    await openEncode(page, tab);
    const workspace = tab === 'av1an' ? av1anWorkspace(page) : quickWorkspace(page);
    const selector = workspace.getByLabel(tab === 'av1an' ? 'SVT-AV1 build' : 'Video encoder', {
      exact: true,
    });
    await selector.selectOption('svtAv1FiveFish');
    for (const label of ['Lineart psy bias', 'Texture psy bias']) {
      for (const invalid of ['-1', '8', '2.5', '']) {
        await workspace.getByLabel(label, { exact: true }).fill(invalid);
        await expect(
          workspace.getByRole('button', { name: 'Start encode', exact: true }),
        ).toBeDisabled();
      }
      for (const valid of ['0', '7']) {
        await workspace.getByLabel(label, { exact: true }).fill(valid);
        await expect(
          workspace.getByRole('button', { name: 'Start encode', exact: true }),
        ).toBeEnabled();
      }
    }
    await selector.selectOption('svtAv1');
    await expect(workspace.getByLabel('Lineart psy bias', { exact: true })).toHaveCount(0);
    await expect(workspace.getByLabel('Quality', { exact: true })).toHaveValue('30');
    await expect(workspace.getByLabel('Encoder preset', { exact: true })).toHaveValue('4');
    await workspace.getByRole('button', { name: 'Add to queue', exact: true }).click();
    expect(
      ((await calls(page, 'enqueue_encode'))[0].payload as { request: EncodeRequest }).request
        .settings,
    ).toMatchObject({
      encoder: 'svtAv1',
      lineartPsyBias: 0,
      texturePsyBias: 0,
      hdrTune: 'visualQuality',
    });
  });

  for (const [variant, tool, name] of [
    ['svtAv1FiveFish', 'svt-av1-5fish', 'SVT-AV1 5fish'],
    ['svtAv1Hdr', 'svt-av1-hdr', 'SVT-AV1-HDR'],
  ] as const) {
    test(`SVT fork ${variant} in ${tab} requires its own executable`, async ({ page }) => {
      await desktopMock(page, { missing: tool });
      await openEncode(page, tab);
      const workspace = tab === 'av1an' ? av1anWorkspace(page) : quickWorkspace(page);
      const selector = workspace.getByLabel(tab === 'av1an' ? 'SVT-AV1 build' : 'Video encoder', {
        exact: true,
      });
      await selector.selectOption('svtAv1');
      await expect(
        workspace.getByRole('button', { name: 'Start encode', exact: true }),
      ).toBeEnabled();
      await selector.selectOption(variant);
      await expect(
        workspace.getByRole('button', { name: 'Start encode', exact: true }),
      ).toBeDisabled();
      await expect(
        workspace.getByRole('button', { name: 'Add to queue', exact: true }),
      ).toBeDisabled();
      await expect(workspace.locator('.disabled-reason')).toContainText(`standalone ${name}`);
      await selector.selectOption(variant === 'svtAv1Hdr' ? 'svtAv1FiveFish' : 'svtAv1Hdr');
      await expect(
        workspace.getByRole('button', { name: 'Start encode', exact: true }),
      ).toBeEnabled();
    });
  }
}

test('SVT fork drafts stay separate between sources and workflows without guessing content', async ({
  page,
}) => {
  await desktopMock(page);
  await openEncode(page);
  const quick = quickWorkspace(page);
  await quick.getByLabel('Video encoder', { exact: true }).selectOption('svtAv1FiveFish');
  await quick.getByLabel('Lineart psy bias', { exact: true }).fill('7');
  await quick
    .getByLabel('Encode destination', { exact: true })
    .fill('C:\\exports\\anime-custom.mkv');
  await page.getByRole('button', { name: 'av1an', exact: true }).click();
  const av1an = av1anWorkspace(page);
  await expect(av1an.getByLabel('SVT-AV1 build', { exact: true })).toHaveValue('svtAv1Hdr');
  await av1an.getByLabel('SVT-AV1 build', { exact: true }).selectOption('svtAv1FiveFish');
  await expect(av1an.getByLabel('Lineart psy bias', { exact: true })).toHaveValue('5');
  await av1an.getByLabel('Texture psy bias', { exact: true }).fill('2');
  const next = {
    ...media,
    id: 'fork-second-source',
    name: 'HDR movie.mkv',
    path: 'C:\\media\\HDR movie.mkv',
    streams: [{ ...baseStream, colorTransfer: 'smpte2084', bitDepth: 10 }],
  };
  await importAnotherSource(page, next);
  await page.getByRole('button', { name: 'Quick Convert', exact: true }).click();
  await expect(quick.getByLabel('Video encoder', { exact: true })).toHaveValue('svtAv1FiveFish');
  await expect(quick.getByLabel('Lineart psy bias', { exact: true })).toHaveValue('5');
  await expect(quick.getByLabel('Allow HDR10 fallback', { exact: true })).not.toBeChecked();
  await quick.getByLabel('Video encoder', { exact: true }).selectOption('svtAv1Hdr');
  await quick.getByLabel('HDR tune', { exact: true }).selectOption('visualQuality');
  await page
    .getByRole('navigation', { name: 'Workspace' })
    .getByRole('button', { name: /^Files/ })
    .click();
  await page.locator('button.file-select').filter({ hasText: media.name }).click();
  await page.getByRole('button', { name: 'Quick Convert', exact: true }).click();
  await expect(quick.getByLabel('HDR tune', { exact: true })).toHaveValue('filmGrain');
  await quick.getByLabel('Video encoder', { exact: true }).selectOption('svtAv1FiveFish');
  await expect(quick.getByLabel('Lineart psy bias', { exact: true })).toHaveValue('7');
  await expect(quick.getByLabel('Encode destination', { exact: true })).toHaveValue(
    'C:\\exports\\anime-custom.mkv',
  );
  await quick.getByRole('button', { name: 'Reset settings', exact: true }).click();
  await page.getByRole('button', { name: 'av1an', exact: true }).click();
  await expect(av1an.getByLabel('Texture psy bias', { exact: true })).toHaveValue('2');
});

for (const outcome of ['destination', 'error'] as const) {
  test(`SVT fork switch discards a deferred picker ${outcome} even after returning to the same build`, async ({
    page,
  }) => {
    await desktopMock(page, {
      held: ['plugin:dialog|save'],
      pickerFailure:
        outcome === 'error'
          ? { code: 'PICKER_FAILED', message: 'Old fork picker failed', path: null }
          : undefined,
    });
    await openEncode(page, 'av1an');
    const workspace = av1anWorkspace(page);
    await workspace.getByLabel('SVT-AV1 build', { exact: true }).selectOption('svtAv1FiveFish');
    await workspace.getByRole('button', { name: 'Choose encode destination', exact: true }).click();
    await expect.poll(() => calls(page, 'plugin:dialog|save')).toHaveLength(1);
    await workspace.getByLabel('SVT-AV1 build', { exact: true }).selectOption('svtAv1Hdr');
    await workspace.getByLabel('SVT-AV1 build', { exact: true }).selectOption('svtAv1FiveFish');
    await release(page, 'plugin:dialog|save');
    await expect(workspace.getByLabel('Encode destination', { exact: true })).toHaveValue(
      inputPath.replace(/\.mkv$/, '_av1an_5fish.mkv'),
    );
    await expect(workspace.getByRole('alert')).toHaveCount(0);
  });
}

test('x264 uses its own defaults, preset names, validation, copied tracks, and immutable job settings', async ({
  page,
}) => {
  await desktopMock(page, {
    media: {
      ...media,
      streams: media.streams.map((stream) => ({ ...stream, pixelFormat: 'yuv420p' })),
    },
  });
  await page.setViewportSize({ width: 760, height: 600 });
  await openEncode(page);
  const quick = quickWorkspace(page);
  await quick.getByLabel('Film grain synthesis', { exact: true }).fill('12');
  await quick.getByLabel('Allow HDR10 fallback', { exact: true }).check();
  await quick.getByLabel('Video encoder', { exact: true }).selectOption('x264');
  await expect(quick.getByLabel('Quality', { exact: true })).toHaveValue('23');
  await expect(quick.getByLabel('Encoder preset', { exact: true })).toHaveValue('5');
  await expect(quick.getByLabel('Encoder preset', { exact: true }).locator('option')).toHaveText(
    x264PresetNames,
  );
  await expect(quick.getByText('Standalone x264 · 8-bit source', { exact: true })).toBeVisible();
  await expect(quick.getByLabel('Encode destination', { exact: true })).toHaveValue(
    inputPath.replace(/\.mkv$/, '_x264.mkv'),
  );
  await expect(quick.getByLabel('Film grain synthesis', { exact: true })).toHaveCount(0);
  await expect(quick.getByLabel('Allow HDR10 fallback', { exact: true })).toHaveCount(0);
  await expect(quick.getByLabel('Parallel chunks', { exact: true })).toHaveCount(0);
  const start = quick.getByRole('button', { name: 'Start encode', exact: true });
  for (const invalid of ['-1', '52', '23.5', '']) {
    await quick.getByLabel('Quality', { exact: true }).fill(invalid);
    await expect(start).toBeDisabled();
  }
  for (const valid of ['0', '51', '23']) {
    await quick.getByLabel('Quality', { exact: true }).fill(valid);
    await expect(start).toBeEnabled();
  }
  await quick.getByLabel('Video stream', { exact: true }).selectOption('4');
  await quick.getByLabel('Copy stream #7', { exact: true }).uncheck();
  await start.click();
  const started = (await calls(page, 'start_encode'))[0].payload as { request: EncodeRequest };
  expect(started.request).toEqual({
    source: {
      inputPath,
      outputPath: inputPath.replace(/\.mkv$/, '_x264.mkv'),
      streamIndices: [4, 3, 9],
    },
    settings: {
      videoStreamIndex: 4,
      crf: 23,
      preset: 5,
      backend: 'standalone',
      encoder: 'x264',
      workers: 2,
      filmGrain: 0,
      hdr10Fallback: false,
      lineartPsyBias: 0,
      texturePsyBias: 0,
      hdrTune: 'visualQuality',
      framing: {
        crop: { top: 0, right: 0, bottom: 0, left: 0 },
        resizeWidth: null,
        borders: { top: 0, right: 0, bottom: 0, left: 0 },
      },
      audio: [{ streamIndex: 3, codec: 'copy', bitrateKbps: 128, channels: 'preserve' }],
    },
  });
  const current = page.getByRole('region', { name: 'Current encode job' });
  await expect(current).toContainText('H.264 encode');
  await expect(current).toContainText(
    'Standalone x264 · H.264 · Source bit depth · CRF 23 · Preset medium',
  );
  await expect(current).not.toContainText('10-bit');
  await quick.getByLabel('Video encoder', { exact: true }).selectOption('svtAv1Hdr');
  await quick.getByRole('button', { name: 'Reset settings', exact: true }).click();
  await expect(current).toContainText('Preset medium');
  expect((await calls(page, 'start_encode'))[0].payload).toEqual(started);
  expect(await page.evaluate(() => document.documentElement.scrollWidth <= window.innerWidth)).toBe(
    true,
  );
});

test('x264 and SVT restore independent drafts for each source and reset only the selected encoder', async ({
  page,
}) => {
  await desktopMock(page);
  await openEncode(page);
  const quick = quickWorkspace(page);
  const encoder = quick.getByLabel('Video encoder', { exact: true });
  await quick.getByLabel('Quality', { exact: true }).fill('21');
  await quick.getByLabel('Film grain synthesis', { exact: true }).fill('8');
  await quick.getByLabel('Allow HDR10 fallback', { exact: true }).check();
  await quick.getByLabel('Encode destination', { exact: true }).fill('C:\\exports\\source-av1.mkv');
  await encoder.selectOption('x264');
  await quick.getByLabel('Quality', { exact: true }).fill('18');
  await quick.getByLabel('Encoder preset', { exact: true }).selectOption('7');
  await quick.getByLabel('Include audio stream #3', { exact: true }).uncheck();
  await quick
    .getByLabel('Encode destination', { exact: true })
    .fill('C:\\exports\\source-h264.mkv');
  const next = {
    ...media,
    id: 'x264-second-source',
    path: 'C:\\media\\second.mkv',
    name: 'second.mkv',
  };
  await importAnotherSource(page, next);
  await page.getByRole('button', { name: 'Quick Convert', exact: true }).click();
  await expect(encoder).toHaveValue('x264');
  await expect(quick.getByLabel('Quality', { exact: true })).toHaveValue('23');
  await expect(quick.getByLabel('Encode destination', { exact: true })).toHaveValue(
    'C:\\media\\second_x264.mkv',
  );
  await quick.getByLabel('Quality', { exact: true }).fill('26');
  await encoder.selectOption('svtAv1Hdr');
  await expect(quick.getByLabel('Quality', { exact: true })).toHaveValue('30');
  await page
    .getByRole('navigation', { name: 'Workspace' })
    .getByRole('button', { name: /^Files/ })
    .click();
  await page.locator('button.file-select').filter({ hasText: media.name }).click();
  await page.getByRole('button', { name: 'Quick Convert', exact: true }).click();
  await expect(quick.getByLabel('Quality', { exact: true })).toHaveValue('21');
  await expect(quick.getByLabel('Film grain synthesis', { exact: true })).toHaveValue('8');
  await expect(quick.getByLabel('Allow HDR10 fallback', { exact: true })).toBeChecked();
  await expect(quick.getByLabel('Encode destination', { exact: true })).toHaveValue(
    'C:\\exports\\source-av1.mkv',
  );
  await encoder.selectOption('x264');
  await expect(quick.getByLabel('Quality', { exact: true })).toHaveValue('18');
  await expect(quick.getByLabel('Encoder preset', { exact: true })).toHaveValue('7');
  await expect(quick.getByLabel('Include audio stream #3', { exact: true })).not.toBeChecked();
  await expect(quick.getByLabel('Encode destination', { exact: true })).toHaveValue(
    'C:\\exports\\source-h264.mkv',
  );
  await quick.getByRole('button', { name: 'Reset settings', exact: true }).click();
  await expect(encoder).toHaveValue('x264');
  await expect(quick.getByLabel('Quality', { exact: true })).toHaveValue('23');
  await expect(quick.getByLabel('Encoder preset', { exact: true })).toHaveValue('5');
  await expect(quick.getByLabel('Include audio stream #3', { exact: true })).toBeChecked();
  await encoder.selectOption('svtAv1Hdr');
  await expect(quick.getByLabel('Quality', { exact: true })).toHaveValue('21');
  await page.getByRole('button', { name: 'av1an', exact: true }).click();
  await expect(av1anWorkspace(page).getByLabel('Video encoder', { exact: true })).toHaveCount(0);
  await expect(av1anWorkspace(page).getByLabel('Quality', { exact: true })).toHaveValue('30');
  await page
    .getByRole('navigation', { name: 'Workspace' })
    .getByRole('button', { name: /^Files/ })
    .click();
  await page.locator('button.file-select').filter({ hasText: next.name }).click();
  await page.getByRole('button', { name: 'Quick Convert', exact: true }).click();
  await encoder.selectOption('x264');
  await expect(quick.getByLabel('Quality', { exact: true })).toHaveValue('26');
});

for (const missing of ['x264', 'svt-av1-hdr'] as const) {
  test(`x264 tool discovery is independent when ${missing} is missing`, async ({ page }) => {
    await desktopMock(page, { missing });
    await openEncode(page);
    const quick = quickWorkspace(page);
    const start = quick.getByRole('button', { name: 'Start encode', exact: true });
    await expect(start).toBeEnabled({ enabled: missing === 'x264' });
    await quick.getByLabel('Video encoder', { exact: true }).selectOption('x264');
    await expect(start).toBeEnabled({ enabled: missing === 'svt-av1-hdr' });
    await expect(quick.getByRole('button', { name: 'Add to queue', exact: true })).toBeEnabled({
      enabled: missing === 'svt-av1-hdr',
    });
    await page.getByRole('button', { name: 'av1an', exact: true }).click();
    await expect(
      av1anWorkspace(page).getByRole('button', { name: 'Start encode', exact: true }),
    ).toBeEnabled({ enabled: missing === 'x264' });
  });
}

for (const depth of [8, 10]) {
  test(`x264 displays ${depth}-bit SDR source depth without offering HDR controls`, async ({
    page,
  }) => {
    await desktopMock(page, { media: { ...media, streams: [{ ...baseStream, bitDepth: depth }] } });
    await openEncode(page);
    const quick = quickWorkspace(page);
    await quick.getByLabel('Video encoder', { exact: true }).selectOption('x264');
    await expect(
      quick.getByText(`Standalone x264 · ${depth}-bit source`, { exact: true }),
    ).toBeVisible();
    await expect(quick).not.toContainText('lossless');
    await expect(quick.getByRole('button', { name: 'Start encode', exact: true })).toBeEnabled();
  });
}

test('x264 ignores an enqueue error after source replacement and keeps submission locked until settlement', async ({
  page,
}) => {
  await desktopMock(page, {
    held: ['enqueue_encode'],
    failure: {
      code: 'ENCODE_UNSUPPORTED_SOURCE',
      message: 'The original x264 source could not be queued.',
      path: inputPath,
    },
  });
  await openEncode(page);
  const quick = quickWorkspace(page);
  await quick.getByLabel('Video encoder', { exact: true }).selectOption('x264');
  await quick.getByLabel('Quality', { exact: true }).fill('18');
  await quick.getByRole('button', { name: 'Add to queue', exact: true }).click();
  await expect.poll(() => calls(page, 'enqueue_encode')).toHaveLength(1);
  await expect(quick.getByLabel('Video encoder', { exact: true })).toBeDisabled();
  await importAnotherSource(page, {
    ...media,
    id: 'x264-pending-next',
    name: 'next.mkv',
    path: 'C:\\media\\next.mkv',
  });
  await page.getByRole('button', { name: 'Quick Convert', exact: true }).click();
  await expect(quick.getByLabel('Quality', { exact: true })).toHaveValue('23');
  await expect(quick.getByLabel('Video encoder', { exact: true })).toBeDisabled();
  await expect(quick.getByRole('button', { name: 'Add to queue', exact: true })).toBeDisabled();
  await release(page, 'enqueue_encode');
  await expect(quick.getByRole('button', { name: 'Add to queue', exact: true })).toBeEnabled();
  await expect(quick.getByRole('alert')).toHaveCount(0);
  await expect(quick.getByLabel('Encode destination', { exact: true })).toHaveValue(
    'C:\\media\\next_x264.mkv',
  );
  expect(
    ((await calls(page, 'enqueue_encode'))[0].payload as { request: EncodeRequest }).request,
  ).toMatchObject({
    source: { inputPath },
    settings: {
      encoder: 'x264',
      backend: 'standalone',
      crf: 18,
      filmGrain: 0,
      hdr10Fallback: false,
      lineartPsyBias: 0,
      texturePsyBias: 0,
      hdrTune: 'visualQuality',
      framing: {
        crop: { top: 0, right: 0, bottom: 0, left: 0 },
        resizeWidth: null,
        borders: { top: 0, right: 0, bottom: 0, left: 0 },
      },
      audio: [{ streamIndex: 3, codec: 'copy', bitrateKbps: 128, channels: 'preserve' }],
    },
  });
});

test('x264 blocks known HDR while SVT remains selectable', async ({ page }) => {
  await desktopMock(page, {
    media: { ...media, streams: [{ ...baseStream, bitDepth: 10, colorTransfer: 'smpte2084' }] },
  });
  await openEncode(page);
  const quick = quickWorkspace(page);
  await quick.getByLabel('Video encoder', { exact: true }).selectOption('x264');
  await expect(
    quick.getByText(
      'x264 supports SDR sources only. Choose SVT-AV1-HDR for compatible HDR10 video.',
      {
        exact: true,
      },
    ),
  ).toBeVisible();
  await expect(quick.getByRole('button', { name: 'Start encode', exact: true })).toBeDisabled();
  await expect(quick.getByRole('button', { name: 'Add to queue', exact: true })).toBeDisabled();
  await quick.getByLabel('Video encoder', { exact: true }).selectOption('svtAv1Hdr');
  await expect(quick.getByRole('button', { name: 'Start encode', exact: true })).toBeEnabled();
});

for (const outcome of ['destination', 'error'] as const) {
  test(`x264 encoder switches and resets invalidate a deferred picker ${outcome}`, async ({
    page,
  }) => {
    await desktopMock(page, {
      held: ['plugin:dialog|save'],
      pickerFailure:
        outcome === 'error'
          ? { code: 'PICKER_FAILED', message: 'Stale picker error', path: null }
          : undefined,
    });
    await openEncode(page);
    const quick = quickWorkspace(page);
    const encoder = quick.getByLabel('Video encoder', { exact: true });
    const destination = quick.getByLabel('Encode destination', { exact: true });
    await quick.getByRole('button', { name: 'Choose encode destination', exact: true }).click();
    await expect.poll(() => calls(page, 'plugin:dialog|save')).toHaveLength(1);
    await encoder.selectOption('x264');
    await encoder.selectOption('svtAv1Hdr');
    await release(page, 'plugin:dialog|save');
    await expect(destination).toHaveValue(inputPath.replace(/\.mkv$/, '_av1_hdr.mkv'));
    await expect(quick.getByRole('alert')).toHaveCount(0);
    await encoder.selectOption('x264');
    await page.evaluate(() =>
      (globalThis as unknown as { __encodeMock: Mock }).__encodeMock.hold('plugin:dialog|save'),
    );
    await destination.fill('C:\\exports\\x264-draft.mkv');
    await quick.getByRole('button', { name: 'Choose encode destination', exact: true }).click();
    await expect.poll(() => calls(page, 'plugin:dialog|save')).toHaveLength(2);
    await quick.getByRole('button', { name: 'Reset settings', exact: true }).click();
    await release(page, 'plugin:dialog|save');
    await expect(destination).toHaveValue(inputPath.replace(/\.mkv$/, '_x264.mkv'));
    await expect(quick.getByRole('alert')).toHaveCount(0);
  });
}

async function openEncode(page: Page, tab: 'Quick Convert' | 'av1an' = 'Quick Convert') {
  await page.goto('/');
  await page.getByRole('button', { name: 'Add files', exact: true }).first().click();
  await expect(page.getByRole('heading', { name: media.name, exact: true })).toBeVisible();
  await page.getByRole('button', { name: tab, exact: true }).click();
}
async function calls(page: Page, command: string) {
  return page.evaluate(
    (name) =>
      (globalThis as unknown as { __encodeMock: Mock }).__encodeMock.calls.filter(
        (entry) => entry.command === name,
      ),
    command,
  );
}
async function emit(page: Page, jobs: JobSnapshot[]) {
  await page.evaluate(
    (next) => (globalThis as unknown as { __encodeMock: Mock }).__encodeMock.emit(next),
    jobs,
  );
}

async function release(page: Page, command: string) {
  await page.evaluate(async (name) => {
    (globalThis as unknown as { __encodeMock: Mock }).__encodeMock.release(name);
    // Let the released IPC reply and the Svelte update settle before asserting
    // that no stale error or destination has appeared in the current draft.
    await new Promise<void>((resolve) => requestAnimationFrame(() => resolve()));
  }, command);
}

async function importAnotherSource(page: Page, next: MediaFile) {
  await page.evaluate(
    (value) => (globalThis as unknown as { __encodeMock: Mock }).__encodeMock.setMedia(value),
    next,
  );
  await page
    .getByRole('navigation', { name: 'Workspace' })
    .getByRole('button', { name: /^Files/ })
    .click();
  await page.getByRole('button', { name: 'Add files', exact: true }).first().click();
  await expect(page.getByRole('heading', { name: next.name, exact: true })).toBeVisible();
}

test('encode submits the selected video, quality, preset, copied tracks, and native destination', async ({
  page,
}) => {
  await desktopMock(page);
  await page.setViewportSize({ width: 760, height: 600 });
  await openEncode(page);
  await expect(quickWorkspace(page).getByLabel('Quality', { exact: true })).toHaveValue('30');
  await expect(quickWorkspace(page).getByLabel('Encoder preset', { exact: true })).toHaveValue('2');
  await quickWorkspace(page).getByLabel('Video stream', { exact: true }).selectOption('4');
  await quickWorkspace(page).getByLabel('Quality', { exact: true }).fill('28');
  await quickWorkspace(page).getByLabel('Encoder preset', { exact: true }).selectOption('6');
  await quickWorkspace(page).getByLabel('Copy stream #7', { exact: true }).uncheck();
  await page.getByRole('button', { name: 'Choose encode destination', exact: true }).click();
  await expect(quickWorkspace(page).getByLabel('Encode destination', { exact: true })).toHaveValue(
    outputPath,
  );
  await expect(
    quickWorkspace(page).getByText(
      'Choose Copy source, Opus, or AAC for each selected audio track. Subtitles and attachments are copied.',
    ),
  ).toBeVisible();
  await page.getByRole('button', { name: 'Start encode', exact: true }).click();
  await expect
    .poll(() => calls(page, 'start_encode'))
    .toEqual([
      {
        command: 'start_encode',
        payload: {
          request: {
            source: { inputPath, outputPath, streamIndices: [4, 3, 9] },
            settings: {
              videoStreamIndex: 4,
              crf: 28,
              preset: 6,
              backend: 'standalone',
              encoder: 'svtAv1Hdr',
              workers: 2,
              filmGrain: 0,
              hdr10Fallback: false,
              lineartPsyBias: 0,
              texturePsyBias: 0,
              hdrTune: 'filmGrain',
              framing: {
                crop: { top: 0, right: 0, bottom: 0, left: 0 },
                resizeWidth: null,
                borders: { top: 0, right: 0, bottom: 0, left: 0 },
              },
              audio: [{ streamIndex: 3, codec: 'copy', bitrateKbps: 128, channels: 'preserve' }],
            },
          },
        },
      },
    ]);
  await expect(page.getByText('Running', { exact: true })).toBeVisible();
  await expect(page.getByRole('button', { name: 'Start encode', exact: true })).toBeDisabled();
  await page.getByRole('button', { name: 'Remux', exact: true }).click();
  await expect(page.getByRole('button', { name: 'Start remux', exact: true })).toBeDisabled();
  await expect(page.getByText('Running', { exact: true })).toBeVisible();
  expect(await page.evaluate(() => document.documentElement.scrollWidth <= window.innerWidth)).toBe(
    true,
  );
});

test('encode draft survives navigation and Reset settings restores defaults', async ({ page }) => {
  await desktopMock(page);
  await openEncode(page);
  await quickWorkspace(page).getByLabel('Video stream', { exact: true }).selectOption('4');
  await quickWorkspace(page).getByLabel('Quality', { exact: true }).fill('20');
  await quickWorkspace(page).getByLabel('Encoder preset', { exact: true }).selectOption('8');
  await quickWorkspace(page).getByLabel('Include audio stream #3', { exact: true }).uncheck();
  await quickWorkspace(page).getByLabel('Encode destination', { exact: true }).fill(outputPath);
  await page.getByRole('button', { name: 'Tools & settings', exact: true }).click();
  await page.getByRole('button', { name: 'Quick Convert', exact: true }).click();
  await expect(quickWorkspace(page).getByLabel('Video stream', { exact: true })).toHaveValue('4');
  await expect(quickWorkspace(page).getByLabel('Quality', { exact: true })).toHaveValue('20');
  await expect(quickWorkspace(page).getByLabel('Encoder preset', { exact: true })).toHaveValue('8');
  await expect(
    quickWorkspace(page).getByLabel('Include audio stream #3', { exact: true }),
  ).not.toBeChecked();
  await expect(quickWorkspace(page).getByLabel('Encode destination', { exact: true })).toHaveValue(
    outputPath,
  );
  await page.getByRole('button', { name: 'Reset settings', exact: true }).click();
  await expect(quickWorkspace(page).getByLabel('Quality', { exact: true })).toHaveValue('30');
  await expect(quickWorkspace(page).getByLabel('Encoder preset', { exact: true })).toHaveValue('2');
  await expect(quickWorkspace(page).getByLabel('Video stream', { exact: true })).toHaveValue('0');
  await expect(
    quickWorkspace(page).getByLabel('Include audio stream #3', { exact: true }),
  ).toBeChecked();
});

for (const missing of ['ffmpeg', 'ffprobe', 'svt-av1-hdr']) {
  test(`encoding requires ${missing}`, async ({ page }) => {
    await desktopMock(page, { missing });
    await openEncode(page);
    await expect(page.getByRole('button', { name: 'Start encode', exact: true })).toBeDisabled();
    expect(await calls(page, 'start_encode')).toEqual([]);
  });
}

test('invalid quality, blank output, and audio-only sources cannot start an encode', async ({
  page,
}) => {
  await desktopMock(page);
  await openEncode(page);
  for (const invalid of ['0', '64', '2.5', '']) {
    await quickWorkspace(page).getByLabel('Quality', { exact: true }).fill(invalid);
    await expect(page.getByRole('button', { name: 'Start encode', exact: true })).toBeDisabled();
  }
  await quickWorkspace(page).getByLabel('Quality', { exact: true }).fill('30');
  await quickWorkspace(page).getByLabel('Encode destination', { exact: true }).fill('  ');
  await expect(page.getByRole('button', { name: 'Start encode', exact: true })).toBeDisabled();
  const next: MediaFile = {
    ...media,
    id: 'audio-only',
    name: 'audio.mkv',
    path: 'C:\\media\\audio.mkv',
    streams: [{ ...media.streams[2], index: 12 }],
  };
  await page.evaluate(
    (value) => (globalThis as unknown as { __encodeMock: Mock }).__encodeMock.setMedia(value),
    next,
  );
  await page
    .getByRole('navigation', { name: 'Workspace' })
    .getByRole('button', { name: /^Files/ })
    .click();
  await page.getByRole('button', { name: 'Add files', exact: true }).first().click();
  await page.getByRole('button', { name: 'Quick Convert', exact: true }).click();
  await expect(quickWorkspace(page).getByLabel('Quality', { exact: true })).toHaveValue('30');
  await expect(
    quickWorkspace(page).getByLabel('Include audio stream #12', { exact: true }),
  ).toBeChecked();
  await expect(
    quickWorkspace(page).getByLabel('Include audio stream #3', { exact: true }),
  ).toHaveCount(0);
  await expect(page.getByRole('button', { name: 'Start encode', exact: true })).toBeDisabled();
  expect(await calls(page, 'start_encode')).toEqual([]);
});

test('backend compatibility errors remain visible and allow correcting the draft', async ({
  page,
}) => {
  await desktopMock(page, {
    failure: {
      code: 'ENCODE_UNSUPPORTED_SOURCE',
      message: 'This HDR source has no compatible HDR10 base layer.',
      path: inputPath,
    },
  });
  await openEncode(page);
  await page.getByRole('button', { name: 'Start encode', exact: true }).click();
  await expect(page.getByRole('alert')).toContainText('no compatible HDR10 base layer');
  await expect(page.getByRole('button', { name: 'Start encode', exact: true })).toBeEnabled();
});

test('encode cancellation and runtime errors follow authoritative job snapshots', async ({
  page,
}) => {
  await desktopMock(page);
  await openEncode(page);
  await page.getByRole('button', { name: 'Start encode', exact: true }).click();
  await page.getByText('Job log', { exact: true }).click();
  await expect(page.getByLabel('Job log', { exact: true })).toContainText('standalone SVT-AV1');
  await page.getByRole('button', { name: 'Cancel job', exact: true }).click();
  await expect
    .poll(() => calls(page, 'cancel_job'))
    .toEqual([{ command: 'cancel_job', payload: { id: 'encode-1' } }]);
  await expect(page.getByText('Canceling', { exact: true })).toBeVisible();
  await emit(page, [snapshot('canceled')]);
  await expect(page.getByText('Canceled', { exact: true })).toBeVisible();
  await expect(page.getByRole('button', { name: 'Start encode', exact: true })).toBeEnabled();
  await emit(page, [
    {
      ...snapshot('failed'),
      error: { code: 'ENCODE_FAILED', message: 'SVT-AV1 exited unsuccessfully.', path: inputPath },
    },
  ]);
  await expect(page.getByRole('alert')).toContainText('SVT-AV1 exited unsuccessfully.');
  await expect(page.getByText('Failed', { exact: true })).toBeVisible();
});

for (const late of ['start', 'cancel', 'enqueue'] as const) {
  test(`late encode ${late} replies cannot replace a completed channel state`, async ({ page }) => {
    await desktopMock(page, { late });
    await openEncode(page);
    await page
      .getByRole('button', {
        name: late === 'enqueue' ? 'Add to queue' : 'Start encode',
        exact: true,
      })
      .click();
    if (late === 'cancel')
      await page.getByRole('button', { name: 'Cancel job', exact: true }).click();
    await expect(
      page.getByText(late === 'cancel' ? 'Canceled' : 'Succeeded', { exact: true }),
    ).toBeVisible();
    await expect(page.getByRole('button', { name: 'Start encode', exact: true })).toBeEnabled();
  });
}

test('reconnected encode jobs distinguish source scans, encoding, and output validation', async ({
  page,
}) => {
  const durationSeconds = 7200;
  await desktopMock(page, {
    jobs: [{ ...snapshot('preparing'), progressSeconds: null, durationSeconds: null }],
  });
  await page.goto('/');
  await page.getByRole('button', { name: 'Quick Convert', exact: true }).click();
  const job = page.getByRole('region', { name: 'Current encode job' });
  const source = job.getByRole('progressbar', { name: 'Source validation progress', exact: true });
  const encode = job.getByRole('progressbar', { name: 'Encode progress', exact: true });
  const output = job.getByRole('progressbar', { name: 'Output validation progress', exact: true });
  await expect(source).toBeVisible();
  await expect(source).not.toHaveAttribute('value');
  await expect(job).toContainText('Checking source metadata and frames before encoding.');
  await expect(job).toContainText('Source validation · Waiting for scan progress…');
  await expect(encode).toHaveCount(0);
  await expect(job.getByRole('button', { name: 'Cancel job', exact: true })).toBeEnabled();

  await emit(page, [{ ...snapshot('preparing'), progressSeconds: 1800, durationSeconds }]);
  await expect(source).toHaveAttribute('value', '1800');
  await expect(source).toHaveAttribute('max', '7200');
  await expect(job).toContainText('Source validation · 00:30:00 / 02:00:00 of video scanned');
  await page.screenshot({
    path: test.info().outputPath('source-validation-progress.png'),
    fullPage: true,
  });

  await emit(page, [{ ...snapshot('running'), progressSeconds: 0, durationSeconds }]);
  await expect(source).toHaveCount(0);
  await expect(encode).toHaveAttribute('value', '0');
  await expect(job).toContainText('Encoding · 00:00:00 / 02:00:00 of video processed');
  await emit(page, [{ ...snapshot('running'), progressSeconds: durationSeconds, durationSeconds }]);
  await expect(encode).toHaveAttribute('value', '7200');

  await emit(page, [{ ...snapshot('finalizing'), progressSeconds: null, durationSeconds }]);
  await expect(page.getByText('Finalizing', { exact: true })).toBeVisible();
  await expect(encode).toHaveCount(0);
  await expect(output).toBeVisible();
  await expect(output).not.toHaveAttribute('value');
  await expect(job).toContainText('Combining tracks and checking the output before saving it.');
  await expect(job).toContainText('Output validation · Waiting for scan progress…');
  await expect(job.getByRole('button', { name: 'Cancel job', exact: true })).toBeEnabled();
  await emit(page, [{ ...snapshot('finalizing'), progressSeconds: 0, durationSeconds }]);
  await expect(output).toHaveAttribute('value', '0');
  await emit(page, [{ ...snapshot('finalizing'), progressSeconds: 1800, durationSeconds }]);
  await expect(output).toHaveAttribute('value', '1800');
  await expect(job).toContainText('Output validation · 00:30:00 / 02:00:00 of video scanned');
  await expect(job.getByText('Output verified and saved.', { exact: true })).toHaveCount(0);

  await emit(page, [snapshot('succeeded')]);
  await expect(page.getByText('Output verified and saved.', { exact: true })).toBeVisible();
  await expect(job.getByRole('progressbar')).toHaveCount(0);
});

test('validation stays indeterminate when source duration is unknown and supports cancellation', async ({
  page,
}) => {
  await desktopMock(page, {
    jobs: [{ ...snapshot('preparing'), progressSeconds: 3600, durationSeconds: null }],
  });
  await page.goto('/');
  await page.getByRole('button', { name: 'Quick Convert', exact: true }).click();
  const job = page.getByRole('region', { name: 'Current encode job' });
  const progress = job.getByRole('progressbar', {
    name: 'Source validation progress',
    exact: true,
  });
  await expect(progress).toBeVisible();
  await expect(progress).not.toHaveAttribute('value');
  await expect(job).toContainText('01:00:00 of video scanned · Duration unavailable');
  await job.getByRole('button', { name: 'Cancel job', exact: true }).click();
  await expect
    .poll(() => calls(page, 'cancel_job'))
    .toEqual([{ command: 'cancel_job', payload: { id: 'encode-1' } }]);
  await expect(job.getByRole('progressbar')).toHaveCount(0);
  await expect(job.getByRole('button', { name: 'Cancel job', exact: true })).toBeDisabled();
  await expect(job).toContainText('Canceling');
});

test('phase estimates use observed time, age during a stall, and reset before output validation', async ({
  page,
}) => {
  const now = new Date('2026-09-11T12:00:00Z');
  await page.clock.install({ time: now });
  await page.clock.pauseAt(now);
  await desktopMock(page, {
    jobs: [{ ...snapshot('preparing'), progressSeconds: 600, durationSeconds: 7_200 }],
  });
  await page.goto('/');
  await page.getByRole('button', { name: 'Quick Convert', exact: true }).click();
  const job = page.getByRole('region', { name: 'Current encode job' });
  const estimate = job.getByLabel('Current phase estimate', { exact: true });
  await expect(estimate).toHaveCount(0);
  await page.clock.runFor(5_000);
  await emit(page, [{ ...snapshot('preparing'), progressSeconds: 610, durationSeconds: 7_200 }]);
  await expect(estimate).toHaveText(
    'Estimated speed ~2.0× realtime · ~00:54:55 remaining in source validation',
  );
  await page.clock.runFor(5_000);
  await expect(estimate).toHaveText(
    'Estimated speed ~1.0× realtime · ~01:49:50 remaining in source validation',
  );
  await page.clock.runFor(10_000);
  await expect(estimate).toHaveText('Waiting for new progress; estimate unavailable.');

  await emit(page, [{ ...snapshot('running'), progressSeconds: 0, durationSeconds: 100 }]);
  await expect(estimate).toHaveCount(0);
  await page.clock.runFor(5_000);
  await emit(page, [{ ...snapshot('running'), progressSeconds: 5, durationSeconds: 100 }]);
  await expect(estimate).toHaveText(
    'Estimated speed ~1.0× realtime · ~00:01:35 remaining in encoding',
  );
  await emit(page, [{ ...snapshot('running'), progressSeconds: 2, durationSeconds: 100 }]);
  await expect(estimate).toHaveCount(0);

  await emit(page, [{ ...snapshot('finalizing'), progressSeconds: null, durationSeconds: 100 }]);
  await page.clock.runFor(20_000);
  await expect(estimate).toHaveCount(0);
  await emit(page, [{ ...snapshot('finalizing'), progressSeconds: 0, durationSeconds: 100 }]);
  await page.clock.runFor(5_000);
  await emit(page, [{ ...snapshot('finalizing'), progressSeconds: 10, durationSeconds: 100 }]);
  await expect(estimate).toHaveText(
    'Estimated speed ~2.0× realtime · ~00:00:45 remaining in output validation',
  );
  await emit(page, [snapshot('succeeded')]);
  await expect(estimate).toHaveCount(0);
});

test('unknown duration shows estimated speed alone and canceling clears it', async ({ page }) => {
  const now = new Date('2026-09-11T12:00:00Z');
  await page.clock.install({ time: now });
  await page.clock.pauseAt(now);
  await desktopMock(page, {
    jobs: [{ ...snapshot('preparing'), progressSeconds: 0, durationSeconds: null }],
  });
  await page.goto('/');
  await page.getByRole('button', { name: 'Quick Convert', exact: true }).click();
  const job = page.getByRole('region', { name: 'Current encode job' });
  const estimate = job.getByLabel('Current phase estimate', { exact: true });
  await page.clock.runFor(5_000);
  await emit(page, [{ ...snapshot('preparing'), progressSeconds: 0.025, durationSeconds: null }]);
  await expect(estimate).toHaveText('Estimated speed ~0.0050× realtime');
  await emit(page, [snapshot('canceling')]);
  await expect(estimate).toHaveCount(0);
  await emit(page, [snapshot('queued')]);
  await page.clock.runFor(5_000);
  await emit(page, [{ ...snapshot('queued'), progressSeconds: 10 }]);
  await expect(estimate).toHaveCount(0);
});

test('encodes can be queued for different sources while another job is running', async ({
  page,
}) => {
  await desktopMock(page, { jobs: [snapshot()] });
  await openEncode(page);
  await quickWorkspace(page)
    .getByLabel('Encode destination', { exact: true })
    .fill('C:\\exports\\second.mkv');
  await expect(page.getByRole('button', { name: 'Start encode', exact: true })).toBeDisabled();
  await page.getByRole('button', { name: 'Add to queue', exact: true }).click();
  const next: MediaFile = {
    ...media,
    id: 'next-source',
    name: 'next.mkv',
    path: 'C:\\media\\next.mkv',
    streams: [{ ...baseStream, index: 12 }],
  };
  await page.evaluate(
    (value) => (globalThis as unknown as { __encodeMock: Mock }).__encodeMock.setMedia(value),
    next,
  );
  await page
    .getByRole('navigation', { name: 'Workspace' })
    .getByRole('button', { name: /^Files/ })
    .click();
  await page.getByRole('button', { name: 'Add files', exact: true }).first().click();
  await expect(page.getByRole('heading', { name: next.name, exact: true })).toBeVisible();
  await page.getByRole('button', { name: 'Quick Convert', exact: true }).click();
  await page.getByRole('button', { name: 'Add to queue', exact: true }).click();
  await expect
    .poll(() => calls(page, 'enqueue_encode'))
    .toEqual([
      {
        command: 'enqueue_encode',
        payload: {
          request: {
            source: {
              inputPath,
              outputPath: 'C:\\exports\\second.mkv',
              streamIndices: [0, 3, 7, 9],
            },
            settings: {
              videoStreamIndex: 0,
              crf: 30,
              preset: 2,
              backend: 'standalone',
              encoder: 'svtAv1Hdr',
              workers: 2,
              filmGrain: 0,
              hdr10Fallback: false,
              lineartPsyBias: 0,
              texturePsyBias: 0,
              hdrTune: 'filmGrain',
              framing: {
                crop: { top: 0, right: 0, bottom: 0, left: 0 },
                resizeWidth: null,
                borders: { top: 0, right: 0, bottom: 0, left: 0 },
              },
              audio: [{ streamIndex: 3, codec: 'copy', bitrateKbps: 128, channels: 'preserve' }],
            },
          },
        },
      },
      {
        command: 'enqueue_encode',
        payload: {
          request: {
            source: {
              inputPath: next.path,
              outputPath: 'C:\\media\\next_av1_hdr.mkv',
              streamIndices: [12],
            },
            settings: {
              videoStreamIndex: 12,
              crf: 30,
              preset: 2,
              backend: 'standalone',
              encoder: 'svtAv1Hdr',
              workers: 2,
              filmGrain: 0,
              hdr10Fallback: false,
              lineartPsyBias: 0,
              texturePsyBias: 0,
              hdrTune: 'filmGrain',
              framing: {
                crop: { top: 0, right: 0, bottom: 0, left: 0 },
                resizeWidth: null,
                borders: { top: 0, right: 0, bottom: 0, left: 0 },
              },
              audio: [],
            },
          },
        },
      },
    ]);
  await expect(page.getByRole('region', { name: 'Current encode job' })).toContainText(outputPath);
  await expect(page.getByText('Running', { exact: true })).toBeVisible();
  const queued = page.getByRole('region', { name: 'Job queue and history' }).getByRole('article');
  await expect(queued).toHaveCount(2);
  await expect(queued.nth(0)).toContainText('second.mkv');
  await expect(queued.nth(1)).toContainText('next_av1_hdr.mkv');
});

test('canceling a queued entry preserves the active job and Stop queue ends all pending work', async ({
  page,
}) => {
  const queued = {
    ...snapshot('queued'),
    id: 'encode-2',
    request: { ...snapshot().request, outputPath: 'C:\\exports\\queued.mkv' },
  };
  await desktopMock(page, { jobs: [queued, snapshot()], lateStop: true });
  await openEncode(page);
  await expect(page.getByRole('region', { name: 'Current encode job' })).toContainText(outputPath);
  await page.getByRole('button', { name: 'Cancel queued job encode-2', exact: true }).click();
  await expect(page.getByRole('article', { name: 'Job encode-2', exact: true })).toContainText(
    'Canceled',
  );
  await expect(page.getByText('Running', { exact: true })).toBeVisible();
  await page.getByRole('button', { name: 'Stop queue', exact: true }).click();
  await expect.poll(() => calls(page, 'cancel_all_jobs')).toHaveLength(1);
  await expect(page.getByRole('button', { name: 'Stop queue', exact: true })).toHaveCount(0);
  await expect(page.getByRole('button', { name: 'Start encode', exact: true })).toBeEnabled();
  await expect(page.getByText('Canceling', { exact: true })).toHaveCount(0);
});

test('interrupted history is terminal and never resumes automatically', async ({ page }) => {
  await desktopMock(page, { jobs: [snapshot('interrupted')] });
  await openEncode(page);
  await expect(page.getByText('Interrupted', { exact: true })).toBeVisible();
  await expect(page.getByText(/Review the destination and any temporary files/)).toBeVisible();
  await expect(page.getByRole('button', { name: 'Cancel job', exact: true })).toHaveCount(0);
  await expect(page.getByRole('button', { name: 'Start encode', exact: true })).toBeEnabled();
  expect(await calls(page, 'start_encode')).toEqual([]);
  expect(await calls(page, 'enqueue_encode')).toEqual([]);
});

for (const tab of ['Quick Convert', 'av1an'] as const) {
  test(`${tab} exposes job storage errors and reconnect preserves the source`, async ({ page }) => {
    await desktopMock(page, { connectionFailures: 1 });
    await openEncode(page, tab);
    await expect(page.getByRole('alert')).toContainText('The job history could not be read.');
    await expect(page.getByRole('button', { name: 'Start encode', exact: true })).toBeDisabled();
    await expect(page.getByRole('button', { name: 'Add to queue', exact: true })).toBeDisabled();
    await page.getByRole('button', { name: 'Reconnect jobs', exact: true }).click();
    await expect(page.getByRole('alert')).toHaveCount(0);
    await expect(page.getByRole('button', { name: 'Start encode', exact: true })).toBeEnabled();
    const workspace = tab === 'av1an' ? av1anWorkspace(page) : quickWorkspace(page);
    await expect(workspace.getByLabel('Video stream', { exact: true })).toHaveValue('0');
    await expect.poll(() => calls(page, 'subscribe_jobs')).toHaveLength(2);
  });
}

test('av1an tab settings are explicit, immutable in queued jobs, and reset safely', async ({
  page,
}) => {
  await desktopMock(page);
  await openEncode(page, 'av1an');
  const workspace = av1anWorkspace(page);
  await expect(workspace.getByLabel('Film grain synthesis', { exact: true })).toHaveValue('0');
  await expect(workspace.getByLabel('Allow HDR10 fallback', { exact: true })).not.toBeChecked();
  await expect(workspace).toContainText('does not exactly restore the original grain');
  await expect(workspace).toContainText('dynamic metadata to be discarded');
  await expect(workspace.getByLabel('Encode backend', { exact: true })).toHaveCount(0);
  await expect(workspace).toContainText('first video track only');
  await expect(
    workspace.getByLabel('Video stream', { exact: true }).getByRole('option', { name: /#4/ }),
  ).toBeDisabled();
  await expect(workspace).toContainText('at most 240 frames');
  await expect(workspace).toContainText('VapourSynth with the L-SMASH Works source plugin');
  await expect(workspace).toContainText('inside the output folder');
  await expect(workspace).toContainText('remaining work files are retained');
  await expect(workspace).toContainText('Jobs never resume automatically');
  await workspace.getByLabel('Parallel chunks', { exact: true }).fill('3');
  await workspace.getByLabel('Film grain synthesis', { exact: true }).fill('12');
  await workspace.getByLabel('Allow HDR10 fallback', { exact: true }).check();
  await page.screenshot({
    path: test.info().outputPath('av1an-workspace-options.png'),
    fullPage: true,
  });
  await page.getByRole('button', { name: 'Add to queue', exact: true }).click();
  const submitted = (await calls(page, 'enqueue_encode'))[0].payload as { request: EncodeRequest };
  expect(submitted.request.settings).toEqual({
    videoStreamIndex: 0,
    crf: 30,
    preset: 2,
    backend: 'av1an',
    encoder: 'svtAv1Hdr',
    workers: 3,
    filmGrain: 12,
    hdr10Fallback: true,
    lineartPsyBias: 0,
    texturePsyBias: 0,
    hdrTune: 'filmGrain',
    framing: {
      crop: { top: 0, right: 0, bottom: 0, left: 0 },
      resizeWidth: null,
      borders: { top: 0, right: 0, bottom: 0, left: 0 },
    },
    audio: [],
  });
  await page.getByRole('button', { name: 'Reset settings', exact: true }).click();
  await expect(workspace.getByLabel('Encode backend', { exact: true })).toHaveCount(0);
  await expect(workspace.getByLabel('Parallel chunks', { exact: true })).toHaveValue('2');
  await expect(workspace.getByLabel('Film grain synthesis', { exact: true })).toHaveValue('0');
  await expect(workspace.getByLabel('Allow HDR10 fallback', { exact: true })).not.toBeChecked();
  await expect(page.getByRole('region', { name: 'Current encode job' })).toContainText(
    'av1an / SVT-AV1-HDR · 3 parallel chunks',
  );
  await expect(page.getByRole('region', { name: 'Current encode job' })).toContainText(
    'Grain 12 · HDR10 fallback allowed',
  );
  expect((await calls(page, 'enqueue_encode'))[0].payload).toEqual(submitted);
});

test('Quick Convert validates grain and stays available without av1an installed', async ({
  page,
}) => {
  await desktopMock(page, { missing: 'av1an' });
  await openEncode(page);
  const workspace = quickWorkspace(page);
  await expect(workspace.getByLabel('Encode backend', { exact: true })).toHaveCount(0);
  await expect(workspace.getByLabel('Parallel chunks', { exact: true })).toHaveCount(0);
  const start = page.getByRole('button', { name: 'Start encode', exact: true });
  for (const invalid of ['-1', '51', '1.5', '']) {
    await workspace.getByLabel('Film grain synthesis', { exact: true }).fill(invalid);
    await expect(start).toBeDisabled();
  }
  await workspace.getByLabel('Film grain synthesis', { exact: true }).fill('50');
  await expect(start).toBeEnabled();
  await page.getByRole('button', { name: 'av1an', exact: true }).click();
  await expect(start).toBeDisabled();
  await expect(av1anWorkspace(page)).toContainText('and av1an');
  await page.getByRole('button', { name: 'Quick Convert', exact: true }).click();
  await expect(start).toBeEnabled();
  await expect(workspace.getByLabel('Film grain synthesis', { exact: true })).toHaveValue('50');
  expect(await calls(page, 'start_encode')).toHaveLength(0);
});

test('av1an accepts only whole worker counts in range', async ({ page }) => {
  await desktopMock(page);
  await openEncode(page, 'av1an');
  const workspace = av1anWorkspace(page);
  for (const invalid of ['0', '33', '2.5', '']) {
    await workspace.getByLabel('Parallel chunks', { exact: true }).fill(invalid);
    await expect(page.getByRole('button', { name: 'Start encode', exact: true })).toBeDisabled();
  }
  await workspace.getByLabel('Parallel chunks', { exact: true }).fill('32');
  await expect(page.getByRole('button', { name: 'Start encode', exact: true })).toBeEnabled();
});

for (const missing of ['ffmpeg', 'ffprobe', 'svt-av1-hdr', 'av1an']) {
  test(`the dedicated av1an tab requires ${missing}`, async ({ page }) => {
    await desktopMock(page, { missing });
    await openEncode(page, 'av1an');
    const workspace = av1anWorkspace(page);
    await expect(workspace).toBeVisible();
    await expect(
      workspace.getByRole('button', { name: 'Start encode', exact: true }),
    ).toBeDisabled();
    await expect(
      workspace.getByRole('button', { name: 'Add to queue', exact: true }),
    ).toBeDisabled();
    expect(await calls(page, 'start_encode')).toEqual([]);
    expect(await calls(page, 'enqueue_encode')).toEqual([]);
  });
}

test('standalone and av1an drafts stay independent across navigation and reset', async ({
  page,
}) => {
  await desktopMock(page);
  await openEncode(page);
  const quick = quickWorkspace(page);
  const av1an = av1anWorkspace(page);
  await expect(quick.getByLabel('Encode backend', { exact: true })).toHaveCount(0);
  await expect(quick.getByLabel('Parallel chunks', { exact: true })).toHaveCount(0);
  await quick.getByLabel('Video stream', { exact: true }).selectOption('4');
  await quick.getByLabel('Quality', { exact: true }).fill('20');
  await quick.getByLabel('Encoder preset', { exact: true }).selectOption('8');
  await quick.getByLabel('Film grain synthesis', { exact: true }).fill('6');
  await quick.getByLabel('Allow HDR10 fallback', { exact: true }).check();
  await quick.getByLabel('Include audio stream #3', { exact: true }).uncheck();
  await quick.getByLabel('Encode destination', { exact: true }).fill('C:\\exports\\standalone.mkv');

  await page.getByRole('button', { name: 'av1an', exact: true }).click();
  await expect(av1an).toBeVisible();
  await expect(av1an.getByLabel('Quality', { exact: true })).toHaveValue('30');
  await expect(av1an.getByLabel('Encoder preset', { exact: true })).toHaveValue('2');
  await expect(av1an.getByLabel('Film grain synthesis', { exact: true })).toHaveValue('0');
  await expect(av1an.getByLabel('Allow HDR10 fallback', { exact: true })).not.toBeChecked();
  await expect(av1an.getByLabel('Include audio stream #3', { exact: true })).toBeChecked();
  await expect(av1an.getByLabel('Video stream', { exact: true })).toHaveValue('0');
  await av1an.getByLabel('Quality', { exact: true }).fill('27');
  await av1an.getByLabel('Encoder preset', { exact: true }).selectOption('5');
  await av1an.getByLabel('Film grain synthesis', { exact: true }).fill('12');
  await av1an.getByLabel('Parallel chunks', { exact: true }).fill('3');
  await av1an.getByLabel('Copy stream #7', { exact: true }).uncheck();
  await av1an.getByLabel('Encode destination', { exact: true }).fill('C:\\exports\\chunks.mkv');
  await page.getByRole('button', { name: 'Tools & settings', exact: true }).click();
  await page.getByRole('button', { name: 'Quick Convert', exact: true }).click();
  await expect(quick.getByLabel('Video stream', { exact: true })).toHaveValue('4');
  await expect(quick.getByLabel('Quality', { exact: true })).toHaveValue('20');
  await expect(quick.getByLabel('Encoder preset', { exact: true })).toHaveValue('8');
  await expect(quick.getByLabel('Film grain synthesis', { exact: true })).toHaveValue('6');
  await expect(quick.getByLabel('Allow HDR10 fallback', { exact: true })).toBeChecked();
  await expect(quick.getByLabel('Include audio stream #3', { exact: true })).not.toBeChecked();
  await expect(quick.getByLabel('Encode destination', { exact: true })).toHaveValue(
    'C:\\exports\\standalone.mkv',
  );
  await quick.getByRole('button', { name: 'Reset settings', exact: true }).click();
  await page.getByRole('button', { name: 'av1an', exact: true }).click();
  await expect(av1an.getByLabel('Quality', { exact: true })).toHaveValue('27');
  await expect(av1an.getByLabel('Encoder preset', { exact: true })).toHaveValue('5');
  await expect(av1an.getByLabel('Film grain synthesis', { exact: true })).toHaveValue('12');
  await expect(av1an.getByLabel('Parallel chunks', { exact: true })).toHaveValue('3');
  await expect(av1an.getByLabel('Copy stream #7', { exact: true })).not.toBeChecked();
  await expect(av1an.getByLabel('Encode destination', { exact: true })).toHaveValue(
    'C:\\exports\\chunks.mkv',
  );
  await av1an.getByRole('button', { name: 'Reset settings', exact: true }).click();
  await expect(av1an.getByLabel('Quality', { exact: true })).toHaveValue('30');
  await expect(av1an.getByLabel('Parallel chunks', { exact: true })).toHaveValue('2');
  await expect(av1an.getByLabel('Encode backend', { exact: true })).toHaveCount(0);
  expect(await calls(page, 'start_encode')).toEqual([]);
  expect(await calls(page, 'enqueue_encode')).toEqual([]);
});

test('each encoder restores its own source draft and resets only that source', async ({ page }) => {
  await desktopMock(page);
  await openEncode(page);
  const quick = quickWorkspace(page);
  const av1an = av1anWorkspace(page);
  await quick.getByLabel('Quality', { exact: true }).fill('21');
  await page.getByRole('button', { name: 'av1an', exact: true }).click();
  await av1an.getByLabel('Quality', { exact: true }).fill('27');
  await av1an.getByLabel('Parallel chunks', { exact: true }).fill('3');

  const next: MediaFile = {
    ...media,
    id: 'second-draft-source',
    name: 'second.mkv',
    path: 'C:\\media\\second.mkv',
    streams: [{ ...baseStream, index: 12 }],
  };
  await page.evaluate(
    (value) => (globalThis as unknown as { __encodeMock: Mock }).__encodeMock.setMedia(value),
    next,
  );
  const files = page
    .getByRole('navigation', { name: 'Workspace' })
    .getByRole('button', { name: /^Files/ });
  await files.click();
  await page.getByRole('button', { name: 'Add files', exact: true }).first().click();
  await expect(page.getByRole('heading', { name: next.name, exact: true })).toBeVisible();
  await page.getByRole('button', { name: 'Quick Convert', exact: true }).click();
  await expect(quick.getByLabel('Quality', { exact: true })).toHaveValue('30');
  await expect(quick.getByLabel('Video stream', { exact: true })).toHaveValue('12');
  await quick.getByLabel('Quality', { exact: true }).fill('35');
  await page.getByRole('button', { name: 'av1an', exact: true }).click();
  await expect(av1an.getByLabel('Quality', { exact: true })).toHaveValue('30');
  await expect(av1an.getByLabel('Parallel chunks', { exact: true })).toHaveValue('2');
  await expect(av1an.getByLabel('Encode destination', { exact: true })).toHaveValue(
    'C:\\media\\second_av1an_hdr.mkv',
  );
  await av1an.getByLabel('Quality', { exact: true }).fill('33');
  await av1an.getByLabel('Parallel chunks', { exact: true }).fill('6');

  await files.click();
  await page.locator('button.file-select').filter({ hasText: media.name }).click();
  await page.getByRole('button', { name: 'Quick Convert', exact: true }).click();
  await expect(quick.getByLabel('Quality', { exact: true })).toHaveValue('21');
  await page.getByRole('button', { name: 'av1an', exact: true }).click();
  await expect(av1an.getByLabel('Quality', { exact: true })).toHaveValue('27');
  await expect(av1an.getByLabel('Parallel chunks', { exact: true })).toHaveValue('3');
  await av1an.getByRole('button', { name: 'Reset settings', exact: true }).click();
  await expect(av1an.getByLabel('Quality', { exact: true })).toHaveValue('30');
  await files.click();
  await page.locator('button.file-select').filter({ hasText: next.name }).click();
  await page.getByRole('button', { name: 'av1an', exact: true }).click();
  await expect(av1an.getByLabel('Quality', { exact: true })).toHaveValue('33');
  await expect(av1an.getByLabel('Parallel chunks', { exact: true })).toHaveValue('6');
  await page.getByRole('button', { name: 'Quick Convert', exact: true }).click();
  await expect(quick.getByLabel('Quality', { exact: true })).toHaveValue('35');
  await files.click();
  await page.locator('button.file-select').filter({ hasText: media.name }).click();
  await page.getByRole('button', { name: 'Quick Convert', exact: true }).click();
  await expect(quick.getByLabel('Quality', { exact: true })).toHaveValue('21');
  await page.getByRole('button', { name: 'av1an', exact: true }).click();
  await expect(av1an.getByLabel('Quality', { exact: true })).toHaveValue('30');
  await expect(av1an.getByLabel('Parallel chunks', { exact: true })).toHaveValue('2');
});

test('standalone and av1an submit fixed backends into one shared queue and history', async ({
  page,
}) => {
  await desktopMock(page);
  await openEncode(page);
  const quick = quickWorkspace(page);
  const av1an = av1anWorkspace(page);
  const standaloneOutput = 'C:\\exports\\standalone.mkv';
  const av1anOutput = 'C:\\exports\\chunked.mkv';
  await quick.getByLabel('Quality', { exact: true }).fill('25');
  await quick.getByLabel('Encode destination', { exact: true }).fill(standaloneOutput);
  await quick.getByRole('button', { name: 'Start encode', exact: true }).click();
  const started = (await calls(page, 'start_encode'))[0].payload as { request: EncodeRequest };
  expect(started.request.settings.backend).toBe('standalone');
  expect(started.request.settings.crf).toBe(25);
  await page.getByRole('button', { name: 'av1an', exact: true }).click();
  await expect(av1an.getByRole('button', { name: 'Start encode', exact: true })).toBeDisabled();
  await av1an.getByLabel('Parallel chunks', { exact: true }).fill('3');
  await av1an.getByLabel('Quality', { exact: true }).fill('22');
  await av1an.getByLabel('Copy stream #7', { exact: true }).uncheck();
  await av1an.getByLabel('Encode destination', { exact: true }).fill(av1anOutput);
  await av1an.getByRole('button', { name: 'Add to queue', exact: true }).click();
  const queued = (await calls(page, 'enqueue_encode'))[0].payload as { request: EncodeRequest };
  expect(queued.request).toEqual({
    source: { inputPath, outputPath: av1anOutput, streamIndices: [0, 3, 9] },
    settings: {
      videoStreamIndex: 0,
      crf: 22,
      preset: 2,
      backend: 'av1an',
      encoder: 'svtAv1Hdr',
      workers: 3,
      filmGrain: 0,
      hdr10Fallback: false,
      lineartPsyBias: 0,
      texturePsyBias: 0,
      hdrTune: 'filmGrain',
      framing: {
        crop: { top: 0, right: 0, bottom: 0, left: 0 },
        resizeWidth: null,
        borders: { top: 0, right: 0, bottom: 0, left: 0 },
      },
      audio: [],
    },
  });
  const current = page.getByRole('region', { name: 'Current encode job' });
  const pending = page.getByRole('article', { name: 'Job encode-2', exact: true });
  await expect(current).toContainText(standaloneOutput);
  await expect(current).toContainText('Standalone SVT-AV1');
  await expect(pending).toContainText(av1anOutput);
  await pending.getByText('Saved settings and log', { exact: true }).click();
  await expect(pending).toContainText('av1an / SVT-AV1-HDR · 3 parallel chunks');
  await av1an.getByRole('button', { name: 'Reset settings', exact: true }).click();
  await page.getByRole('button', { name: 'Quick Convert', exact: true }).click();
  await quick.getByRole('button', { name: 'Reset settings', exact: true }).click();
  await expect(current).toContainText(standaloneOutput);
  await expect(pending).toContainText(av1anOutput);
  await expect(pending).toContainText('av1an / SVT-AV1-HDR · 3 parallel chunks');
  await pending.getByRole('button', { name: 'Cancel queued job encode-2', exact: true }).click();
  await expect(pending).toContainText('Canceled');
  await expect(current).toContainText('Running');
  await page.getByRole('button', { name: 'av1an', exact: true }).click();
  await expect(pending).toContainText('Canceled');
  await expect(current).toContainText(standaloneOutput);
  expect((await calls(page, 'start_encode'))[0].payload).toEqual(started);
  expect((await calls(page, 'enqueue_encode'))[0].payload).toEqual(queued);
});

for (const tab of ['Quick Convert', 'av1an'] as const) {
  test(`${tab} discards a deferred enqueue error after changing source without unlocking duplicate submission`, async ({
    page,
  }) => {
    await desktopMock(page, {
      held: ['enqueue_encode'],
      failure: {
        code: 'ENCODE_UNSUPPORTED_SOURCE',
        message: 'The original source could not be queued.',
        path: inputPath,
      },
    });
    await openEncode(page, tab);
    const workspace = tab === 'av1an' ? av1anWorkspace(page) : quickWorkspace(page);
    await workspace.getByLabel('Quality', { exact: true }).fill('24');
    await workspace.getByRole('button', { name: 'Add to queue', exact: true }).click();
    await expect.poll(() => calls(page, 'enqueue_encode')).toHaveLength(1);
    const next: MediaFile = {
      ...media,
      id: 'pending-enqueue-next',
      name: 'later.mkv',
      path: 'C:\\media\\later.mkv',
    };
    await importAnotherSource(page, next);
    await page.getByRole('button', { name: tab, exact: true }).click();
    await expect(workspace.getByLabel('Quality', { exact: true })).toHaveValue('30');
    await expect(workspace.getByLabel('Quality', { exact: true })).toBeDisabled();
    await expect(workspace.getByRole('button', { name: 'Starting…', exact: true })).toBeDisabled();
    await expect(
      workspace.getByRole('button', { name: 'Add to queue', exact: true }),
    ).toBeDisabled();
    await release(page, 'enqueue_encode');
    await expect(
      workspace.getByRole('button', { name: 'Add to queue', exact: true }),
    ).toBeEnabled();
    await expect(
      workspace.getByRole('button', { name: 'Start encode', exact: true }),
    ).toBeEnabled();
    await expect(workspace.getByRole('alert')).toHaveCount(0);
    await expect(workspace.getByLabel('Encode destination', { exact: true })).toHaveValue(
      `C:\\media\\later_${tab === 'av1an' ? 'av1an' : 'av1'}_hdr.mkv`,
    );
    await workspace.getByLabel('Quality', { exact: true }).fill('32');
    const requests = await calls(page, 'enqueue_encode');
    expect(requests).toHaveLength(1);
    const original = requests[0].payload as { request: EncodeRequest };
    expect(original.request.source.inputPath).toBe(inputPath);
    expect(original.request.settings.crf).toBe(24);
    expect(original.request.settings.backend).toBe(tab === 'av1an' ? 'av1an' : 'standalone');
    expect(await calls(page, 'start_encode')).toEqual([]);
  });

  for (const outcome of ['destination', 'error'] as const) {
    test(`${tab} ignores a deferred picker ${outcome} after source changes or reset`, async ({
      page,
    }) => {
      const pickerMessage = 'The original destination picker failed.';
      await desktopMock(page, {
        held: ['plugin:dialog|save'],
        pickerFailure:
          outcome === 'error'
            ? { code: 'PICKER_FAILED', message: pickerMessage, path: null }
            : undefined,
      });
      await openEncode(page, tab);
      const workspace = tab === 'av1an' ? av1anWorkspace(page) : quickWorkspace(page);
      await workspace
        .getByRole('button', { name: 'Choose encode destination', exact: true })
        .click();
      await expect.poll(() => calls(page, 'plugin:dialog|save')).toHaveLength(1);
      const next: MediaFile = {
        ...media,
        id: 'pending-picker-next',
        name: 'later.mkv',
        path: 'C:\\media\\later.mkv',
      };
      await importAnotherSource(page, next);
      await page.getByRole('button', { name: tab, exact: true }).click();
      const expectedDestination = `C:\\media\\later_${tab === 'av1an' ? 'av1an' : 'av1'}_hdr.mkv`;
      await expect(workspace.getByLabel('Encode destination', { exact: true })).toHaveValue(
        expectedDestination,
      );
      await release(page, 'plugin:dialog|save');
      await expect(workspace.getByLabel('Encode destination', { exact: true })).toHaveValue(
        expectedDestination,
      );
      await expect(workspace.getByRole('alert')).toHaveCount(0);

      await page.evaluate(() => {
        (globalThis as unknown as { __encodeMock: Mock }).__encodeMock.hold('plugin:dialog|save');
      });
      await workspace
        .getByLabel('Encode destination', { exact: true })
        .fill('C:\\exports\\draft.mkv');
      await workspace
        .getByRole('button', { name: 'Choose encode destination', exact: true })
        .click();
      await expect.poll(() => calls(page, 'plugin:dialog|save')).toHaveLength(2);
      await workspace.getByRole('button', { name: 'Reset settings', exact: true }).click();
      await release(page, 'plugin:dialog|save');
      await expect(workspace.getByLabel('Encode destination', { exact: true })).toHaveValue(
        expectedDestination,
      );
      await expect(workspace.getByRole('alert')).toHaveCount(0);

      // A new picker on the current draft must still be able to update its
      // destination or surface its own error after the stale replies were ignored.
      await workspace
        .getByRole('button', { name: 'Choose encode destination', exact: true })
        .click();
      if (outcome === 'error') {
        await expect(workspace.getByRole('alert')).toContainText(pickerMessage);
      } else {
        await expect(workspace.getByLabel('Encode destination', { exact: true })).toHaveValue(
          outputPath,
        );
      }
      expect(await calls(page, 'plugin:dialog|save')).toHaveLength(3);
      expect(await calls(page, 'start_encode')).toEqual([]);
      expect(await calls(page, 'enqueue_encode')).toEqual([]);
    });
  }
}

test('per-track audio settings retain source details and queue independent Opus, AAC, and copied tracks', async ({
  page,
}) => {
  const multiAudio = {
    ...media,
    streams: [
      ...media.streams,
      { ...media.streams[2], index: 10, codec: 'flac', channels: 6, title: 'Surround' },
      { ...media.streams[2], index: 12, codec: 'ac3', title: 'Commentary' },
    ],
  };
  await desktopMock(page, { media: multiAudio });
  await openEncode(page);
  const quick = quickWorkspace(page);
  const first = quick.getByRole('group', { name: 'Audio settings for stream #3', exact: true });
  const second = quick.getByRole('group', { name: 'Audio settings for stream #10', exact: true });
  await expect(first.getByLabel('Audio codec', { exact: true })).toHaveValue('copy');
  await expect(first.getByLabel('Audio bitrate', { exact: true })).toHaveCount(0);
  await first.getByLabel('Audio codec', { exact: true }).selectOption('opus');
  await first.getByLabel('Audio bitrate', { exact: true }).fill('160');
  await first.getByLabel('Audio channels', { exact: true }).selectOption('stereo');
  await second.getByLabel('Audio codec', { exact: true }).selectOption('aac');
  await second.getByLabel('Audio bitrate', { exact: true }).fill('256');
  await second.getByLabel('Audio channels', { exact: true }).selectOption('mono');
  await expect(second).toContainText('Source: flac · 6 channels · 48 kHz');
  await quick.getByRole('button', { name: 'Add to queue', exact: true }).click();
  const queued = (await calls(page, 'enqueue_encode'))[0].payload as { request: EncodeRequest };
  expect(queued.request.settings.audio).toEqual([
    { streamIndex: 3, codec: 'opus', bitrateKbps: 160, channels: 'stereo' },
    { streamIndex: 10, codec: 'aac', bitrateKbps: 256, channels: 'mono' },
    { streamIndex: 12, codec: 'copy', bitrateKbps: 128, channels: 'preserve' },
  ]);
  expect(queued.request.source.streamIndices).toEqual([0, 3, 7, 10, 12, 9]);
  await quick.getByRole('button', { name: 'Reset settings', exact: true }).click();
  await expect(first.getByLabel('Audio codec', { exact: true })).toHaveValue('copy');
  const current = page.getByRole('region', { name: 'Current encode job' });
  await expect(current).toContainText('Audio #3 → Opus 160 kb/s · stereo');
  await expect(current).toContainText('Audio #10 → AAC 256 kb/s · mono');
  await expect(current).toContainText('Audio #12 copied');
  expect((await calls(page, 'enqueue_encode'))[0].payload).toEqual(queued);
});

test('audio bitrate validation applies only to included converted tracks and reselect restores the draft', async ({
  page,
}) => {
  await desktopMock(page);
  await openEncode(page);
  const quick = quickWorkspace(page);
  const codec = quick.getByLabel('Audio codec', { exact: true });
  await codec.selectOption('opus');
  const start = quick.getByRole('button', { name: 'Start encode', exact: true });
  for (const value of ['', '31', '513', '160.5']) {
    await quick.getByLabel('Audio bitrate', { exact: true }).fill(value);
    await expect(start).toBeDisabled();
  }
  await quick.getByLabel('Include audio stream #3', { exact: true }).uncheck();
  await expect(codec).toHaveCount(0);
  await expect(start).toBeEnabled();
  await quick.getByRole('button', { name: 'Add to queue', exact: true }).click();
  const queued = (await calls(page, 'enqueue_encode'))[0].payload as { request: EncodeRequest };
  expect(queued.request.settings.audio).toEqual([]);
  expect(queued.request.source.streamIndices).toEqual([0, 7, 9]);
  await quick.getByLabel('Include audio stream #3', { exact: true }).check();
  await expect(codec).toHaveValue('opus');
  await expect(quick.getByLabel('Audio bitrate', { exact: true })).toHaveValue('160.5');
  await codec.selectOption('copy');
  await expect(quick.getByRole('button', { name: 'Add to queue', exact: true })).toBeEnabled();
  await expect(quick.getByLabel('Audio channels', { exact: true })).toHaveCount(0);
});

test('audio drafts are isolated by source and video encoder while av1an stays copy-only', async ({
  page,
}) => {
  await desktopMock(page);
  await openEncode(page);
  const quick = quickWorkspace(page);
  await quick.getByLabel('Audio codec', { exact: true }).selectOption('opus');
  await quick.getByLabel('Audio bitrate', { exact: true }).fill('192');
  await quick.getByLabel('Video encoder', { exact: true }).selectOption('x264');
  await expect(quick.getByLabel('Audio codec', { exact: true })).toHaveValue('copy');
  await quick.getByLabel('Audio codec', { exact: true }).selectOption('aac');
  await quick.getByLabel('Audio channels', { exact: true }).selectOption('mono');
  await page.getByRole('button', { name: 'av1an', exact: true }).click();
  const chunked = av1anWorkspace(page);
  await expect(chunked.getByLabel('Audio codec', { exact: true })).toHaveCount(0);
  await chunked.getByRole('button', { name: 'Add to queue', exact: true }).click();
  const queued = (await calls(page, 'enqueue_encode'))[0].payload as { request: EncodeRequest };
  expect(queued.request.settings.audio).toEqual([]);
  await page.getByRole('button', { name: 'Quick Convert', exact: true }).click();
  await expect(quick.getByLabel('Audio codec', { exact: true })).toHaveValue('aac');
  await expect(quick.getByLabel('Audio channels', { exact: true })).toHaveValue('mono');
  const next = {
    ...media,
    id: 'another-audio-source',
    path: 'C:\\media\\another.mkv',
    name: 'another.mkv',
    streams: [media.streams[0], { ...media.streams[2], index: 13 }],
  };
  await importAnotherSource(page, next);
  await page.getByRole('button', { name: 'Quick Convert', exact: true }).click();
  await expect(
    quick.getByRole('group', { name: 'Audio settings for stream #3', exact: true }),
  ).toHaveCount(0);
  await expect(quick.getByLabel('Audio codec', { exact: true })).toHaveValue('copy');
  await quick.getByRole('button', { name: 'Add to queue', exact: true }).click();
  const nextQueued = (await calls(page, 'enqueue_encode'))[1].payload as { request: EncodeRequest };
  expect(nextQueued.request.settings.audio).toEqual([
    { streamIndex: 13, codec: 'copy', bitrateKbps: 128, channels: 'preserve' },
  ]);
  expect(nextQueued.request.source.streamIndices).toEqual([0, 13]);
  await page
    .getByRole('navigation', { name: 'Workspace' })
    .getByRole('button', { name: /^Files/ })
    .click();
  await page.locator('button.file-select').filter({ hasText: media.name }).click();
  await page.getByRole('button', { name: 'Quick Convert', exact: true }).click();
  await expect(quick.getByLabel('Audio codec', { exact: true })).toHaveValue('aac');
  await quick.getByLabel('Video encoder', { exact: true }).selectOption('svtAv1Hdr');
  await expect(quick.getByLabel('Audio codec', { exact: true })).toHaveValue('opus');
  await expect(quick.getByLabel('Audio bitrate', { exact: true })).toHaveValue('192');
});

test('legacy job history remains readable without audio settings', async ({ page }) => {
  const legacy = snapshot('succeeded');
  delete (legacy.encodeSettings as Partial<EncodeRequest['settings']>).audio;
  await desktopMock(page, { jobs: [legacy] });
  await openEncode(page);
  await expect(page.getByRole('region', { name: 'Current encode job' })).toContainText(
    'Audio copied when selected',
  );
  await expect(
    quickWorkspace(page).getByRole('button', { name: 'Start encode', exact: true }),
  ).toBeEnabled();
});

for (const sampleRate of [8000, 44100]) {
  test(`AAC bitrate limits follow ${sampleRate} Hz source audio and selected channels`, async ({
    page,
  }) => {
    const source = {
      ...media,
      streams: media.streams.map((stream) =>
        stream.kind === 'audio' ? { ...stream, sampleRate, channels: 1 } : stream,
      ),
    };
    await desktopMock(page, { media: source });
    await openEncode(page);
    const quick = quickWorkspace(page);
    await quick.getByLabel('Audio codec', { exact: true }).selectOption('aac');
    const bitrate = quick.getByLabel('Audio bitrate', { exact: true });
    const max = Math.floor((sampleRate * 6) / 1000);
    await expect(bitrate).toHaveAttribute('max', String(max));
    await expect(bitrate).toHaveValue(String(Math.min(128, max)));
    await bitrate.fill(String(max + 1));
    await expect(quick.getByRole('button', { name: 'Start encode', exact: true })).toBeDisabled();
    await quick.getByLabel('Audio channels', { exact: true }).selectOption('stereo');
    await expect(bitrate).toHaveAttribute('max', String(Math.min(512, max * 2)));
    await expect(quick.getByRole('button', { name: 'Start encode', exact: true })).toBeEnabled();
    await quick.getByLabel('Audio channels', { exact: true }).selectOption('mono');
    await expect(bitrate).toHaveValue(String(max));
    await quick.getByRole('button', { name: 'Add to queue', exact: true }).click();
    const queued = (await calls(page, 'enqueue_encode'))[0].payload as { request: EncodeRequest };
    expect(queued.request.settings.audio).toEqual([
      { streamIndex: 3, codec: 'aac', bitrateKbps: max, channels: 'mono' },
    ]);
  });
}

test('Opus mono bitrate respects the encoder limit while stereo allows higher rates', async ({
  page,
}) => {
  const source = {
    ...media,
    streams: media.streams.map((stream) =>
      stream.kind === 'audio' ? { ...stream, channels: 1 } : stream,
    ),
  };
  await desktopMock(page, { media: source });
  await openEncode(page);
  const quick = quickWorkspace(page);
  await quick.getByLabel('Audio codec', { exact: true }).selectOption('opus');
  const bitrate = quick.getByLabel('Audio bitrate', { exact: true });
  await expect(bitrate).toHaveAttribute('max', '256');
  await bitrate.fill('257');
  await expect(quick.getByRole('button', { name: 'Start encode', exact: true })).toBeDisabled();
  await quick.getByLabel('Audio channels', { exact: true }).selectOption('stereo');
  await expect(bitrate).toHaveAttribute('max', '512');
  await bitrate.fill('512');
  await expect(quick.getByRole('button', { name: 'Start encode', exact: true })).toBeEnabled();
  await quick.getByLabel('Audio channels', { exact: true }).selectOption('mono');
  await expect(bitrate).toHaveValue('256');
  await expect(quick.getByRole('button', { name: 'Start encode', exact: true })).toBeEnabled();
});
