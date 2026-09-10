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
const tools: ToolInfo[] = ['ffmpeg', 'ffprobe', 'svt-av1'].map((id) => ({
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
};
const snapshot = (state: JobSnapshot['state'] = 'running'): JobSnapshot => ({
  id: 'encode-1',
  state,
  request: { inputPath, outputPath, streamIndices: [0, 3, 7, 9] },
  encodeSettings: { videoStreamIndex: 0, crf: 30, preset: 4 },
  progressSeconds: 3,
  durationSeconds: 12,
  logs: ['Encoding video with standalone SVT-AV1.'],
  error: null,
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
    }) => {
      const state = globalThis as unknown as Record<string, unknown>;
      let selectedMedia = initialMedia;
      let jobs = initialJobs;
      let callbackId = 0;
      let remainingFailures = connectionFailures;
      const calls: Mock['calls'] = [];
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
      } satisfies Mock;
      state.isTauri = true;
      state.__TAURI_INTERNALS__ = {
        metadata: { currentWindow: { label: 'main' }, currentWebview: { label: 'main' } },
        transformCallback: () => ++callbackId,
        unregisterCallback: () => {},
        invoke: async (command: string, payload: Record<string, unknown> = {}) => {
          calls.push({ command, payload });
          if (command === 'get_capabilities') return capabilities;
          if (command === 'plugin:dialog|open') return [selectedMedia.path];
          if (command === 'plugin:dialog|save') return destination;
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
      initialMedia: media,
      capabilities: tools.map((tool) =>
        tool.id === options.missing ? { ...tool, available: false, path: null } : tool,
      ),
      destination: outputPath,
      initialJobs: options.jobs ?? [],
      failure: options.failure,
      late: options.late,
      connectionFailures: options.connectionFailures ?? 0,
      lateStop: options.lateStop ?? false,
    },
  );
}
async function openEncode(page: Page) {
  await page.goto('/');
  await page.getByRole('button', { name: 'Add files', exact: true }).first().click();
  await expect(page.getByRole('heading', { name: media.name, exact: true })).toBeVisible();
  await page.getByRole('button', { name: 'Quick Convert', exact: true }).click();
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

test('encode submits the selected video, quality, preset, copied tracks, and native destination', async ({
  page,
}) => {
  await desktopMock(page);
  await page.setViewportSize({ width: 760, height: 600 });
  await openEncode(page);
  await expect(page.getByLabel('Quality', { exact: true })).toHaveValue('30');
  await expect(page.getByLabel('Encoder preset', { exact: true })).toHaveValue('4');
  await page.getByLabel('Video stream', { exact: true }).selectOption('4');
  await page.getByLabel('Quality', { exact: true }).fill('28');
  await page.getByLabel('Encoder preset', { exact: true }).selectOption('6');
  await page.getByLabel('Copy stream #7', { exact: true }).uncheck();
  await page.getByRole('button', { name: 'Choose encode destination', exact: true }).click();
  await expect(page.getByLabel('Encode destination', { exact: true })).toHaveValue(outputPath);
  await expect(
    page.getByText(
      'Selected audio, subtitles, and attachments are copied without encoding. Audio keeps its source codec and channels.',
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
            settings: { videoStreamIndex: 4, crf: 28, preset: 6 },
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
  await page.getByLabel('Video stream', { exact: true }).selectOption('4');
  await page.getByLabel('Quality', { exact: true }).fill('20');
  await page.getByLabel('Encoder preset', { exact: true }).selectOption('8');
  await page.getByLabel('Copy stream #3', { exact: true }).uncheck();
  await page.getByLabel('Encode destination', { exact: true }).fill(outputPath);
  await page.getByRole('button', { name: 'Tools & settings', exact: true }).click();
  await page.getByRole('button', { name: 'Quick Convert', exact: true }).click();
  await expect(page.getByLabel('Video stream', { exact: true })).toHaveValue('4');
  await expect(page.getByLabel('Quality', { exact: true })).toHaveValue('20');
  await expect(page.getByLabel('Encoder preset', { exact: true })).toHaveValue('8');
  await expect(page.getByLabel('Copy stream #3', { exact: true })).not.toBeChecked();
  await expect(page.getByLabel('Encode destination', { exact: true })).toHaveValue(outputPath);
  await page.getByRole('button', { name: 'Reset settings', exact: true }).click();
  await expect(page.getByLabel('Quality', { exact: true })).toHaveValue('30');
  await expect(page.getByLabel('Encoder preset', { exact: true })).toHaveValue('4');
  await expect(page.getByLabel('Video stream', { exact: true })).toHaveValue('0');
  await expect(page.getByLabel('Copy stream #3', { exact: true })).toBeChecked();
});

for (const missing of ['ffmpeg', 'ffprobe', 'svt-av1']) {
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
    await page.getByLabel('Quality', { exact: true }).fill(invalid);
    await expect(page.getByRole('button', { name: 'Start encode', exact: true })).toBeDisabled();
  }
  await page.getByLabel('Quality', { exact: true }).fill('30');
  await page.getByLabel('Encode destination', { exact: true }).fill('  ');
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
  await expect(page.getByLabel('Quality', { exact: true })).toHaveValue('30');
  await expect(page.getByLabel('Copy stream #12', { exact: true })).toBeChecked();
  await expect(page.getByLabel('Copy stream #3', { exact: true })).toHaveCount(0);
  await expect(page.getByRole('button', { name: 'Start encode', exact: true })).toBeDisabled();
  expect(await calls(page, 'start_encode')).toEqual([]);
});

test('backend compatibility errors remain visible and allow correcting the draft', async ({
  page,
}) => {
  await desktopMock(page, {
    failure: {
      code: 'ENCODE_UNSUPPORTED_SOURCE',
      message: 'HDR video is not supported by this workflow.',
      path: inputPath,
    },
  });
  await openEncode(page);
  await page.getByRole('button', { name: 'Start encode', exact: true }).click();
  await expect(page.getByRole('alert')).toContainText('HDR video is not supported');
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

test('reconnected encode jobs show progress, finalization, and a verified output', async ({
  page,
}) => {
  await desktopMock(page, { jobs: [snapshot()] });
  await page.goto('/');
  await page.getByRole('button', { name: 'Quick Convert', exact: true }).click();
  await expect(page.getByRole('progressbar', { name: 'Encode progress' })).toHaveAttribute(
    'value',
    '3',
  );
  await emit(page, [snapshot('finalizing')]);
  await expect(page.getByText('Finalizing', { exact: true })).toBeVisible();
  await emit(page, [snapshot('succeeded')]);
  await expect(page.getByText('Output verified and saved.', { exact: true })).toBeVisible();
});

test('encodes can be queued for different sources while another job is running', async ({
  page,
}) => {
  await desktopMock(page, { jobs: [snapshot()] });
  await openEncode(page);
  await page.getByLabel('Encode destination', { exact: true }).fill('C:\\exports\\second.mkv');
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
            settings: { videoStreamIndex: 0, crf: 30, preset: 4 },
          },
        },
      },
      {
        command: 'enqueue_encode',
        payload: {
          request: {
            source: {
              inputPath: next.path,
              outputPath: 'C:\\media\\next_av1.mkv',
              streamIndices: [12],
            },
            settings: { videoStreamIndex: 12, crf: 30, preset: 4 },
          },
        },
      },
    ]);
  await expect(page.getByRole('region', { name: 'Current encode job' })).toContainText(outputPath);
  await expect(page.getByText('Running', { exact: true })).toBeVisible();
  const queued = page.getByRole('region', { name: 'Job queue and history' }).getByRole('article');
  await expect(queued).toHaveCount(2);
  await expect(queued.nth(0)).toContainText('second.mkv');
  await expect(queued.nth(1)).toContainText('next_av1.mkv');
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

test('job storage errors are visible and reconnect preserves the imported source', async ({
  page,
}) => {
  await desktopMock(page, { connectionFailures: 1 });
  await openEncode(page);
  await expect(page.getByRole('alert')).toContainText('The job history could not be read.');
  await expect(page.getByRole('button', { name: 'Start encode', exact: true })).toBeDisabled();
  await expect(page.getByRole('button', { name: 'Add to queue', exact: true })).toBeDisabled();
  await page.getByRole('button', { name: 'Reconnect jobs', exact: true }).click();
  await expect(page.getByRole('alert')).toHaveCount(0);
  await expect(page.getByRole('button', { name: 'Start encode', exact: true })).toBeEnabled();
  await expect(page.getByLabel('Video stream', { exact: true })).toHaveValue('0');
  await expect.poll(() => calls(page, 'subscribe_jobs')).toHaveLength(2);
});
