import { expect, test, type Page } from '@playwright/test';
import type {
  AppError,
  JobSnapshot,
  MediaFile,
  RemuxRequest,
  ToolInfo,
} from '../../src/lib/ipc/generated';

const inputPath = "C:\\media\\- café's & 東京.mkv";
const outputPath = "C:\\exports\\café's & 東京 remux.mkv";
const fixture: MediaFile = {
  id: 'remux-fixture',
  path: inputPath,
  name: "- café's & 東京.mkv",
  sizeBytes: '123456',
  durationSeconds: 12,
  format: 'matroska,webm',
  streams: [
    {
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
    },
    {
      index: 3,
      kind: 'audio',
      codec: 'aac',
      width: null,
      height: null,
      frameRate: null,
      sampleRate: 48000,
      channels: 2,
      language: 'eng',
      title: 'Original audio',
    },
    {
      index: 7,
      kind: 'subtitle',
      codec: 'subrip',
      width: null,
      height: null,
      frameRate: null,
      sampleRate: null,
      channels: null,
      language: 'eng',
      title: 'English',
    },
  ],
};
const capabilities: ToolInfo[] = ['ffmpeg', 'ffprobe'].map((id) => ({
  id,
  name: id === 'ffmpeg' ? 'FFmpeg' : 'FFprobe',
  available: true,
  path: `C:\\tools\\${id}.exe`,
  version: `${id} test version`,
  detail: null,
}));

type MockCall = { command: string; payload: unknown };
type RemuxMock = {
  calls: MockCall[];
  emit: (jobs: JobSnapshot[]) => void;
};

function snapshot(state: JobSnapshot['state'] = 'running'): JobSnapshot {
  return {
    id: 'remux-job-1',
    state,
    request: { inputPath, outputPath, streamIndices: [0, 3, 7] },
    encodeSettings: null,
    progressSeconds: 3,
    durationSeconds: 12,
    logs: ['Reading source streams.'],
    error: null,
    recovery: null,
    logPath: 'C:\\logs\\remux-job-1.log',
  };
}

async function desktopMock(
  page: Page,
  options: {
    missingTool?: string;
    startError?: AppError;
    jobs?: JobSnapshot[];
    terminalBeforeResponse?: 'start' | 'cancel';
  } = {},
) {
  await page.addInitScript(
    ({ media, tools, destination, startError, initialJobs, terminalBeforeResponse }) => {
      const state = globalThis as unknown as Record<string, unknown>;
      const calls: MockCall[] = [];
      const savedJobs = sessionStorage.getItem('remux-mock-backend-jobs');
      let jobs: JobSnapshot[] = savedJobs ? JSON.parse(savedJobs) : initialJobs;
      let channel: { onmessage: (jobs: JobSnapshot[]) => void } | undefined;
      let callbackId = 0;
      const publish = (next: JobSnapshot[]) => {
        jobs = next;
        sessionStorage.setItem('remux-mock-backend-jobs', JSON.stringify(jobs));
        channel?.onmessage(jobs);
      };
      state.__remuxMock = { calls, emit: publish } satisfies RemuxMock;
      state.isTauri = true;
      state.__TAURI_INTERNALS__ = {
        metadata: { currentWindow: { label: 'main' }, currentWebview: { label: 'main' } },
        transformCallback: () => ++callbackId,
        unregisterCallback: () => {},
        invoke: async (command: string, payload: Record<string, unknown> = {}) => {
          calls.push({ command, payload });
          if (command === 'get_capabilities') return tools;
          if (command === 'plugin:dialog|open') return [media.path];
          if (command === 'plugin:dialog|save') return destination;
          if (command.startsWith('plugin:event|')) return ++callbackId;
          if (command === 'probe_media') return media;
          if (command === 'list_jobs') return jobs;
          if (command === 'subscribe_jobs') {
            channel = payload.channel as typeof channel;
            channel?.onmessage(jobs);
            return jobs;
          }
          if (command === 'start_remux') {
            if (startError) throw startError;
            const job: JobSnapshot = {
              id: 'remux-job-1',
              state: 'running',
              request: payload.request as RemuxRequest,
              encodeSettings: null,
              progressSeconds: 0,
              durationSeconds: media.durationSeconds,
              logs: ['Copying selected streams.'],
              error: null,
              recovery: null,
              logPath: 'C:\\logs\\remux-job-1.log',
            };
            if (terminalBeforeResponse === 'start') {
              publish([{ ...job, state: 'succeeded' }]);
              return { ...job, state: 'queued' };
            }
            publish([job]);
            return job;
          }
          if (command === 'cancel_job') {
            const job = jobs.find((entry) => entry.id === payload.id);
            if (!job) throw { code: 'JOB_NOT_FOUND', message: 'Unknown job.', path: null };
            const canceling: JobSnapshot = { ...job, state: 'canceling' };
            publish([
              terminalBeforeResponse === 'cancel' ? { ...job, state: 'canceled' } : canceling,
            ]);
            return canceling;
          }
          throw new Error(`Unexpected IPC command: ${command}`);
        },
      };
    },
    {
      media: fixture,
      tools: capabilities.map((tool) =>
        tool.id === options.missingTool
          ? { ...tool, available: false, path: null, detail: 'Tool missing from PATH.' }
          : tool,
      ),
      destination: outputPath,
      startError: options.startError,
      initialJobs: options.jobs ?? [],
      terminalBeforeResponse: options.terminalBeforeResponse,
    },
  );
}

async function importAndOpenRemux(page: Page) {
  await page.goto('/');
  await page.getByRole('button', { name: 'Add files', exact: true }).first().click();
  await expect(page.getByRole('heading', { name: fixture.name, exact: true })).toBeVisible();
  await page.getByRole('button', { name: 'Remux', exact: true }).click();
  await expect(page.getByRole('heading', { name: 'Remux', exact: true })).toBeVisible();
}

async function callsFor(page: Page, command: string) {
  return page.evaluate(
    (name) =>
      (globalThis as unknown as { __remuxMock: RemuxMock }).__remuxMock.calls.filter(
        (call) => call.command === name,
      ),
    command,
  );
}

async function emitJobs(page: Page, jobs: JobSnapshot[]) {
  await page.evaluate(
    (next) => (globalThis as unknown as { __remuxMock: RemuxMock }).__remuxMock.emit(next),
    jobs,
  );
}

test('remux sends the chosen output and selected source indices in the displayed order', async ({
  page,
}) => {
  await desktopMock(page);
  await page.setViewportSize({ width: 760, height: 600 });
  await importAndOpenRemux(page);
  await page.getByLabel('Include stream #7', { exact: true }).uncheck();
  await page.getByRole('button', { name: 'Move stream #3 up', exact: true }).click();
  await page.getByRole('button', { name: 'Choose destination', exact: true }).click();
  await expect(page.getByLabel('Destination', { exact: true })).toHaveValue(outputPath);
  await page.getByRole('button', { name: 'Start remux', exact: true }).click();
  await expect
    .poll(() => callsFor(page, 'start_remux'))
    .toEqual([
      {
        command: 'start_remux',
        payload: { request: { inputPath, outputPath, streamIndices: [3, 0] } },
      },
    ]);
  await expect(page.getByText('Running', { exact: true })).toBeVisible();
  await expect(page.getByRole('button', { name: 'Start remux', exact: true })).toBeDisabled();
  await page.getByRole('button', { name: 'Quick Convert', exact: true }).click();
  await page.getByRole('button', { name: 'Remux', exact: true }).click();
  await expect(page.getByText('Running', { exact: true })).toBeVisible();
  await expect(page.getByRole('button', { name: 'Cancel job', exact: true })).toBeEnabled();
  expect(await page.evaluate(() => document.documentElement.scrollWidth <= window.innerWidth)).toBe(
    true,
  );
});

test('remux cannot start with an empty stream mapping or a blank destination', async ({ page }) => {
  await desktopMock(page);
  await importAndOpenRemux(page);
  await page.getByLabel('Destination', { exact: true }).fill(outputPath);
  for (const index of [0, 3, 7]) {
    await page.getByLabel(`Include stream #${index}`, { exact: true }).uncheck();
  }
  await expect(page.getByRole('button', { name: 'Start remux', exact: true })).toBeDisabled();
  await page.getByLabel('Include stream #3', { exact: true }).check();
  await expect(page.getByRole('button', { name: 'Start remux', exact: true })).toBeEnabled();
  await page.getByLabel('Destination', { exact: true }).fill('   ');
  await expect(page.getByRole('button', { name: 'Start remux', exact: true })).toBeDisabled();
  expect(await callsFor(page, 'start_remux')).toEqual([]);
});

test('stream selection, order, and destination survive visiting other workspace tabs', async ({
  page,
}) => {
  await desktopMock(page);
  await importAndOpenRemux(page);
  await page.getByLabel('Include stream #7', { exact: true }).uncheck();
  await page.getByRole('button', { name: 'Move stream #3 up', exact: true }).click();
  await page.getByLabel('Destination', { exact: true }).fill(outputPath);
  await page.getByRole('button', { name: 'Tools & settings', exact: true }).click();
  await page
    .getByRole('navigation', { name: 'Workspace' })
    .getByRole('button', { name: /^Files/ })
    .click();
  await page.getByRole('button', { name: 'Remux', exact: true }).click();
  await expect(page.getByLabel('Include stream #7', { exact: true })).not.toBeChecked();
  await expect(page.getByLabel('Destination', { exact: true })).toHaveValue(outputPath);
  await expect(page.getByRole('button', { name: 'Move stream #3 up', exact: true })).toBeDisabled();
  await page.getByRole('button', { name: 'Start remux', exact: true }).click();
  await expect
    .poll(() => callsFor(page, 'start_remux'))
    .toEqual([
      {
        command: 'start_remux',
        payload: { request: { inputPath, outputPath, streamIndices: [3, 0] } },
      },
    ]);
});

test('choosing another source rebuilds the draft from its original stream indices', async ({
  page,
}) => {
  const pageErrors: string[] = [];
  page.on('pageerror', (error) => pageErrors.push(error.message));
  await desktopMock(page);
  await importAndOpenRemux(page);
  await page.getByLabel('Destination', { exact: true }).fill(outputPath);
  await page.getByLabel('Include stream #7', { exact: true }).uncheck();
  const nextMedia: MediaFile = {
    ...fixture,
    id: 'another-source',
    name: 'audio-only.mkv',
    path: 'C:\\media\\audio-only.mkv',
    streams: [{ ...fixture.streams[1], index: 2 }],
  };
  await page.evaluate((media) => {
    const state = globalThis as unknown as {
      __TAURI_INTERNALS__: {
        invoke: (command: string, payload?: unknown) => Promise<unknown>;
      };
    };
    const originalInvoke = state.__TAURI_INTERNALS__.invoke;
    state.__TAURI_INTERNALS__.invoke = async (command, payload) => {
      if (command === 'plugin:dialog|open') return [media.path];
      if (command === 'probe_media') return media;
      return originalInvoke(command, payload);
    };
  }, nextMedia);
  await page
    .getByRole('navigation', { name: 'Workspace' })
    .getByRole('button', { name: /^Files/ })
    .click();
  await page.getByRole('button', { name: 'Add files', exact: true }).first().click();
  await expect(page.getByRole('heading', { name: nextMedia.name, exact: true })).toBeVisible();
  await page.getByRole('button', { name: 'Remux', exact: true }).click();
  await expect(page.getByLabel('Include stream #2', { exact: true })).toBeChecked();
  await expect(page.getByRole('checkbox')).toHaveCount(1);
  await expect(page.getByLabel('Destination', { exact: true })).toHaveValue(
    'C:\\media\\audio-only_remux.mkv',
  );
  expect(pageErrors).toEqual([]);
});

for (const missingTool of ['ffmpeg', 'ffprobe']) {
  test(`remux cannot start without ${missingTool}`, async ({ page }) => {
    await desktopMock(page, { missingTool });
    await importAndOpenRemux(page);
    await page.getByLabel('Destination', { exact: true }).fill(outputPath);
    await expect(page.getByRole('button', { name: 'Start remux', exact: true })).toBeDisabled();
    expect(await callsFor(page, 'start_remux')).toEqual([]);
  });
}

test('browser sample remains unavailable for remux and fits the minimum window width', async ({
  page,
}) => {
  await page.setViewportSize({ width: 760, height: 600 });
  await page.goto('/');
  await page.getByRole('button', { name: 'Remux', exact: true }).click();
  await expect(page.getByRole('button', { name: 'Start remux', exact: true })).toBeDisabled();
  await page
    .getByRole('navigation', { name: 'Workspace' })
    .getByRole('button', { name: /^Files/ })
    .click();
  await page.getByRole('button', { name: 'Load sample', exact: true }).click();
  await page.getByRole('button', { name: 'Remux', exact: true }).click();
  await expect(page.getByRole('heading', { name: 'Remux', exact: true })).toBeVisible();
  await expect(page.getByRole('button', { name: 'Start remux', exact: true })).toBeDisabled();
  await expect(
    page.getByRole('button', { name: 'Choose destination', exact: true }),
  ).toBeDisabled();
  expect(await page.evaluate(() => document.documentElement.scrollWidth <= window.innerWidth)).toBe(
    true,
  );
});

test('a rejected remux start reports the backend error and permits correction', async ({
  page,
}) => {
  await desktopMock(page, {
    startError: {
      code: 'OUTPUT_EXISTS',
      message: 'Choose another destination. The output file already exists.',
      path: outputPath,
    },
  });
  await importAndOpenRemux(page);
  await page.getByLabel('Destination', { exact: true }).fill(outputPath);
  await page.getByRole('button', { name: 'Start remux', exact: true }).click();
  await expect(page.getByRole('alert')).toContainText('The output file already exists.');
  await expect(page.getByRole('button', { name: 'Start remux', exact: true })).toBeEnabled();
  expect(await callsFor(page, 'cancel_job')).toEqual([]);
});

test('cancel targets the running job and waits for a terminal backend snapshot', async ({
  page,
}) => {
  await desktopMock(page);
  await importAndOpenRemux(page);
  await page.getByLabel('Destination', { exact: true }).fill(outputPath);
  await page.getByRole('button', { name: 'Start remux', exact: true }).click();
  await page.getByRole('button', { name: 'Cancel job', exact: true }).click();
  await expect
    .poll(() => callsFor(page, 'cancel_job'))
    .toEqual([{ command: 'cancel_job', payload: { id: 'remux-job-1' } }]);
  await expect(page.getByRole('button', { name: 'Start remux', exact: true })).toBeDisabled();
  await expect(page.getByText('Canceled', { exact: true })).toHaveCount(0);
  await emitJobs(page, [snapshot('canceled')]);
  await expect(page.getByText('Canceled', { exact: true })).toBeVisible();
  await expect(page.getByRole('button', { name: 'Start remux', exact: true })).toBeEnabled();
});

test('a webview reload recovers the running job and follows finalization to success', async ({
  page,
}) => {
  await desktopMock(page, { jobs: [snapshot()] });
  await page.goto('/');
  await page.getByRole('button', { name: 'Remux', exact: true }).click();
  await expect(page.getByText('Running', { exact: true })).toBeVisible();
  await page.reload();
  await page.getByRole('button', { name: 'Remux', exact: true }).click();
  await expect(page.getByText('Running', { exact: true })).toBeVisible();
  await expect(page.getByRole('button', { name: 'Cancel job', exact: true })).toBeEnabled();
  await emitJobs(page, [snapshot('finalizing')]);
  await expect(page.getByText('Finalizing', { exact: true })).toBeVisible();
  await expect(
    page.getByText('Checking the output before saving it.', { exact: true }),
  ).toBeVisible();
  await expect(page.getByRole('progressbar', { name: 'Output validation progress' })).toHaveCount(
    0,
  );
  await emitJobs(page, [snapshot('succeeded')]);
  await expect(page.getByText('Succeeded', { exact: true })).toBeVisible();
  await expect(page.getByText('Running', { exact: true })).toHaveCount(0);
});

test('a runtime failure is shown without reporting a successful output', async ({ page }) => {
  await desktopMock(page, { jobs: [snapshot()] });
  await page.goto('/');
  await page.getByRole('button', { name: 'Remux', exact: true }).click();
  await expect(page.getByText('Running', { exact: true })).toBeVisible();
  await emitJobs(page, [
    {
      ...snapshot('failed'),
      error: {
        code: 'OUTPUT_INVALID',
        message: 'The remux output failed verification. No final output was published.',
        path: outputPath,
      },
    },
  ]);
  await expect(page.getByText('Failed', { exact: true })).toBeVisible();
  await expect(page.getByRole('alert')).toContainText('The remux output failed verification.');
  await expect(page.getByText('Succeeded', { exact: true })).toHaveCount(0);
});

for (const operation of ['start', 'cancel'] as const) {
  test(`a late ${operation} response cannot regress a terminal channel snapshot`, async ({
    page,
  }) => {
    await desktopMock(page, { terminalBeforeResponse: operation });
    await importAndOpenRemux(page);
    await page.getByLabel('Destination', { exact: true }).fill(outputPath);
    await page.getByRole('button', { name: 'Start remux', exact: true }).click();
    if (operation === 'cancel') {
      await page.getByRole('button', { name: 'Cancel job', exact: true }).click();
    }
    await expect(
      page.getByText(operation === 'start' ? 'Succeeded' : 'Canceled', { exact: true }),
    ).toBeVisible();
    await expect(page.getByRole('button', { name: 'Start remux', exact: true })).toBeEnabled();
  });
}
