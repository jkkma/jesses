import { expect, test, type Page } from '@playwright/test';
import type { JobSnapshot, MediaFile } from '../../src/lib/ipc/generated';

const savedJob = (
  state: JobSnapshot['state'] = 'interrupted',
  id = 'saved-encode',
): JobSnapshot => ({
  id,
  state,
  request: {
    inputPath: 'C:\\media\\source.mkv',
    outputPath: 'C:\\exports\\saved.mkv',
    streamIndices: [0],
  },
  encodeSettings: {
    backend: 'av1an',
    encoder: 'svtAv1Hdr',
    videoStreamIndex: 0,
    crf: 21,
    preset: 6,
    workers: 3,
    filmGrain: 8,
    hdr10Fallback: false,
    lineartPsyBias: 0,
    texturePsyBias: 0,
    hdrTune: 'filmGrain',
    audio: [],
    framing: {
      crop: { top: 0, right: 0, bottom: 0, left: 0 },
      resizeWidth: null,
      borders: { top: 0, right: 0, bottom: 0, left: 0 },
    },
  },
  recovery: {
    workspace: 'C:\\work\\saved-encode',
    phase: 'encoding',
    completedFrames: 240,
    totalFrames: 480,
  },
  progressSeconds: 10,
  durationSeconds: 20,
  logs: [],
  error: null,
  logPath: null,
});

const editorMedia: MediaFile = {
  id: 'editor-source',
  path: 'C:\\media\\editor.mkv',
  name: 'editor.mkv',
  sizeBytes: '12345',
  durationSeconds: 20,
  format: 'matroska',
  streams: [
    {
      index: 0,
      kind: 'video',
      codec: 'h264',
      width: 1280,
      height: 720,
      frameRate: '24/1',
      sampleRate: null,
      channels: null,
      language: null,
      title: null,
    },
  ],
};

type RecoveryMock = {
  calls: { command: string; payload: Record<string, unknown> }[];
  emit: (jobs: JobSnapshot[]) => void;
  release: (command: string) => void;
};

async function desktopMock(
  page: Page,
  options: {
    jobs?: JobSnapshot[];
    fail?: 'stop_job' | 'resume_job';
    late?: 'stop_job' | 'resume_job';
    held?: string[];
    replyOnly?: boolean;
  } = {},
) {
  await page.addInitScript(
    ({ options, initial, media }) => {
      const host = globalThis as unknown as Record<string, unknown>;
      const stored = sessionStorage.getItem('recovery-test-jobs');
      let jobs: JobSnapshot[] = stored ? JSON.parse(stored) : initial;
      let channel: { onmessage: (jobs: JobSnapshot[]) => void } | undefined;
      const calls: RecoveryMock['calls'] = [];
      const held = new Set(options.held ?? []);
      const waiting = new Map<string, (() => void)[]>();
      let fail = options.fail;
      let callback = 0;
      const publish = (next: JobSnapshot[]) => {
        jobs = next;
        sessionStorage.setItem('recovery-test-jobs', JSON.stringify(jobs));
        channel?.onmessage(jobs);
      };
      host.__recoveryMock = {
        calls,
        emit: publish,
        release: (command) => {
          held.delete(command);
          waiting.get(command)?.forEach((resolve) => resolve());
          waiting.delete(command);
        },
      } satisfies RecoveryMock;
      host.isTauri = true;
      host.__TAURI_INTERNALS__ = {
        metadata: { currentWindow: { label: 'main' }, currentWebview: { label: 'main' } },
        transformCallback: () => ++callback,
        unregisterCallback: () => {},
        invoke: async (command: string, payload: Record<string, unknown> = {}) => {
          calls.push({ command, payload });
          if (command === 'get_capabilities')
            return [
              'ffmpeg',
              'ffprobe',
              'svt-av1',
              'svt-av1-hdr',
              'svt-av1-5fish',
              'x264',
              'av1an',
            ].map((id) => ({
              id,
              name: id,
              available: true,
              path: `C:\\tools\\${id}.exe`,
              version: 'test',
              detail: null,
            }));
          if (command.startsWith('plugin:event|')) return ++callback;
          if (command === 'plugin:dialog|open') return [media.path];
          if (command === 'probe_media') return media;
          if (command === 'subscribe_jobs') {
            channel = payload.channel as typeof channel;
            channel?.onmessage(jobs);
            return;
          }
          if (command === 'stop_job' || command === 'resume_job') {
            if (fail === command) {
              fail = undefined;
              throw {
                code: 'RECOVERY_UNAVAILABLE',
                message: 'Saved progress could not be read. Check the saved files and try again.',
                path: null,
              };
            }
            const current = jobs.find((job) => job.id === payload.id)!;
            const reply: JobSnapshot = {
              ...current,
              state: command === 'stop_job' ? 'stopping' : 'queued',
            };
            const update: JobSnapshot =
              options.late === command
                ? {
                    ...current,
                    state: command === 'stop_job' ? 'stopped' : 'succeeded',
                    recovery: command === 'stop_job' ? current.recovery : null,
                  }
                : reply;
            if (!options.replyOnly)
              publish(jobs.map((job) => (job.id === current.id ? update : job)));
            if (held.has(command))
              await new Promise<void>((resolve) =>
                waiting.set(command, [...(waiting.get(command) ?? []), resolve]),
              );
            return reply;
          }
          if (command === 'cancel_job') {
            const job = jobs.find((entry) => entry.id === payload.id)!;
            const canceled = { ...job, state: 'canceled' as const, recovery: null };
            publish(jobs.map((entry) => (entry.id === job.id ? canceled : entry)));
            return canceled;
          }
          if (command === 'cancel_all_jobs') {
            const next = jobs.map((job) => ({
              ...job,
              state: 'canceled' as const,
              recovery: null,
            }));
            publish(next);
            return next;
          }
          throw new Error(`Unexpected command: ${command}`);
        },
      };
    },
    { options, initial: options.jobs ?? [savedJob()], media: editorMedia },
  );
}

async function openJobs(page: Page) {
  await page.goto('/');
  await page.getByRole('button', { name: 'av1an', exact: true }).click();
}
const currentJob = (page: Page) =>
  page.getByRole('region', { name: 'Current encode job', exact: true });
const historyJob = (page: Page, id: string) =>
  page.getByRole('article', { name: `Job ${id}`, exact: true });
async function calls(page: Page, command: string) {
  return page.evaluate(
    (name) =>
      (globalThis as unknown as { __recoveryMock: RecoveryMock }).__recoveryMock.calls.filter(
        (call) => call.command === name,
      ),
    command,
  );
}
async function emit(page: Page, jobs: JobSnapshot[]) {
  await page.evaluate(
    (next) => (globalThis as unknown as { __recoveryMock: RecoveryMock }).__recoveryMock.emit(next),
    jobs,
  );
}
async function release(page: Page, command: string) {
  await page.evaluate(
    (name) =>
      (globalThis as unknown as { __recoveryMock: RecoveryMock }).__recoveryMock.release(name),
    command,
  );
}

for (const phase of ['preparing', 'running', 'finalizing'] as const) {
  test(`av1an ${phase} can stop and keep progress, then explicitly resume`, async ({ page }) => {
    const active = savedJob(phase);
    await desktopMock(page, { jobs: [active] });
    await openJobs(page);
    await currentJob(page)
      .getByRole('button', { name: 'Stop and keep progress', exact: true })
      .click();
    expect(await calls(page, 'stop_job')).toEqual([
      { command: 'stop_job', payload: { id: active.id } },
    ]);
    await expect(currentJob(page)).toContainText('Stopping and saving progress');
    await expect(
      currentJob(page).getByRole('button', { name: 'Cancel job', exact: true }),
    ).toBeDisabled();
    await expect(currentJob(page).getByRole('button', { name: 'Resume', exact: true })).toHaveCount(
      0,
    );
    await emit(page, [{ ...active, state: 'stopped' }]);
    await expect(currentJob(page)).toContainText('Progress saved · Ready to resume');
    await expect(currentJob(page)).toContainText('240 of 480 frames kept');
    await expect(page.getByRole('button', { name: 'Stop queue', exact: true })).toHaveCount(0);
    await currentJob(page).getByRole('button', { name: 'Resume', exact: true }).click();
    expect(await calls(page, 'resume_job')).toEqual([
      { command: 'resume_job', payload: { id: active.id } },
    ]);
    await expect(currentJob(page).getByRole('status')).toHaveText('Queued');
    await expect(currentJob(page)).toContainText('CRF 21 · Preset 6');
  });
}

test('recovery controls stay hidden for unsupported and unrecoverable snapshots', async ({
  page,
}) => {
  const legacy = savedJob('interrupted', 'legacy');
  delete (legacy as Partial<JobSnapshot>).recovery;
  const standalone = savedJob('stopped', 'standalone');
  standalone.encodeSettings!.backend = 'standalone';
  const snapshots: JobSnapshot[] = [
    { ...savedJob('running', 'remux'), encodeSettings: null },
    {
      ...savedJob('running', 'standalone-active'),
      encodeSettings: { ...savedJob().encodeSettings!, backend: 'standalone' },
    },
    savedJob('succeeded', 'succeeded'),
    savedJob('queued', 'queued'),
    savedJob('canceling', 'canceling'),
    { ...savedJob('stopped', 'empty-stop'), recovery: null },
    standalone,
    legacy,
  ];
  await desktopMock(page, { jobs: snapshots });
  await openJobs(page);
  await expect(page.getByRole('button', { name: 'Resume', exact: true })).toHaveCount(0);
  await expect(
    page.getByRole('button', { name: 'Stop and keep progress', exact: true }),
  ).toHaveCount(0);
  await expect(historyJob(page, 'empty-stop')).toContainText('Stopped before progress was saved');
  await expect(historyJob(page, 'legacy')).toContainText('This job will not resume');
});

test('interrupted, failed, canceled and stopped history jobs resume only from saved IDs', async ({
  page,
}) => {
  const active = {
    ...savedJob('running', 'active-other'),
    encodeSettings: { ...savedJob().encodeSettings!, backend: 'standalone' as const },
  };
  const saved = ['interrupted', 'failed', 'canceled', 'stopped'].map((state) =>
    savedJob(state as JobSnapshot['state'], state),
  );
  await desktopMock(page, { jobs: [active, ...saved] });
  await openJobs(page);
  for (const job of saved) {
    const row = historyJob(page, job.id);
    await expect(row).toContainText('240 of 480 frames kept');
    await row.getByRole('button', { name: 'Resume', exact: true }).click();
    await expect(row).toContainText('Queued');
    await expect(row.getByRole('button', { name: 'Resume', exact: true })).toHaveCount(0);
  }
  expect((await calls(page, 'resume_job')).map((call) => call.payload)).toEqual(
    saved.map((job) => ({ id: job.id })),
  );
  await expect(currentJob(page)).toContainText('Running');
});

test('resume uses immutable saved settings despite a different editor draft and survives reload', async ({
  page,
}) => {
  const original = savedJob('interrupted');
  original.recovery!.phase = 'finalizing';
  await desktopMock(page, { jobs: [original] });
  await page.goto('/');
  await page.getByRole('button', { name: 'Add files', exact: true }).first().click();
  await page.getByRole('button', { name: 'av1an', exact: true }).click();
  const editor = page.getByRole('region', { name: 'av1an workspace', exact: true });
  await editor.getByLabel('SVT-AV1 build', { exact: true }).selectOption('svtAv1FiveFish');
  await editor.getByLabel('Quality', { exact: true }).fill('45');
  await editor.getByLabel('Encode destination', { exact: true }).fill('C:\\exports\\different.mkv');
  await expect(currentJob(page)).toContainText(
    'Encoded video is saved. Resume combines tracks and checks the output.',
  );
  await currentJob(page).getByRole('button', { name: 'Resume', exact: true }).click();
  expect((await calls(page, 'resume_job'))[0].payload).toEqual({ id: original.id });
  expect(await calls(page, 'start_encode')).toHaveLength(0);
  await expect(currentJob(page)).toContainText(original.request.outputPath);
  await expect(currentJob(page)).toContainText('av1an / SVT-AV1-HDR · 3 parallel chunks');
  await expect(currentJob(page)).toContainText('CRF 21 · Preset 6');
  await emit(page, [{ ...original, state: 'stopped' }]);
  await page.reload();
  await page.getByRole('button', { name: 'av1an', exact: true }).click();
  await expect(currentJob(page)).toContainText('Progress saved · Ready to resume');
  expect(await calls(page, 'resume_job')).toHaveLength(0);
});

for (const command of ['stop_job', 'resume_job'] as const) {
  test(`late ${command} replies cannot regress authoritative terminal states or duplicate requests`, async ({
    page,
  }) => {
    const original = savedJob(command === 'stop_job' ? 'running' : 'stopped');
    await desktopMock(page, { jobs: [original], late: command, held: [command] });
    await openJobs(page);
    await currentJob(page)
      .getByRole('button', {
        name: command === 'stop_job' ? 'Stop and keep progress' : 'Resume',
        exact: true,
      })
      .evaluate((button: HTMLButtonElement) => {
        button.click();
        button.click();
      });
    await expect.poll(() => calls(page, command)).toHaveLength(1);
    await expect(currentJob(page).getByRole('status')).toHaveText(
      command === 'stop_job' ? 'Stopped' : 'Succeeded',
    );
    await release(page, command);
    await expect(currentJob(page).getByRole('status')).toHaveText(
      command === 'stop_job' ? 'Stopped' : 'Succeeded',
    );
    if (command === 'stop_job')
      await expect(
        currentJob(page).getByRole('button', { name: 'Resume', exact: true }),
      ).toBeEnabled();
    else
      await expect(
        currentJob(page).getByRole('button', { name: 'Resume', exact: true }),
      ).toHaveCount(0);
  });
}

test('resume command replies update the existing job when no channel snapshot arrives', async ({
  page,
}) => {
  await desktopMock(page, { replyOnly: true });
  await openJobs(page);
  await currentJob(page).getByRole('button', { name: 'Resume', exact: true }).click();
  await expect(currentJob(page).getByRole('status')).toHaveText('Queued');
  await expect(currentJob(page)).toContainText('CRF 21 · Preset 6');
});

test('resume reply places saved work behind jobs already queued', async ({ page }) => {
  const waiting = savedJob('queued', 'already-waiting');
  await desktopMock(page, { jobs: [waiting, savedJob('stopped')], replyOnly: true });
  await openJobs(page);
  await historyJob(page, 'saved-encode')
    .getByRole('button', { name: 'Resume', exact: true })
    .click();
  await expect(historyJob(page, 'saved-encode')).toContainText('Queued');
  await expect(historyJob(page, 'already-waiting')).toHaveCount(0);
  await expect(currentJob(page).getByRole('status')).toHaveText('Queued');
});

for (const command of ['stop_job', 'resume_job'] as const) {
  test(`${command} errors keep the job visible and permit retry in current and history views`, async ({
    page,
  }) => {
    const original = savedJob(command === 'stop_job' ? 'running' : 'stopped');
    await desktopMock(page, { jobs: [original], fail: command });
    await openJobs(page);
    const action = command === 'stop_job' ? 'Stop and keep progress' : 'Resume';
    await currentJob(page).getByRole('button', { name: action, exact: true }).click();
    await expect(currentJob(page).getByRole('alert')).toContainText(
      'Saved progress could not be read',
    );
    await expect(currentJob(page).getByRole('button', { name: action, exact: true })).toBeEnabled();
    await page.getByRole('button', { name: 'Tools & settings', exact: true }).click();
    await page.getByRole('button', { name: 'av1an', exact: true }).click();
    await expect(currentJob(page).getByRole('alert')).toContainText(
      'Saved progress could not be read',
    );
    const other = {
      ...savedJob('running', 'other-active'),
      encodeSettings: { ...original.encodeSettings!, backend: 'standalone' as const },
    };
    await emit(page, [other, original]);
    const history = historyJob(page, original.id);
    await expect(history.getByRole('alert')).toContainText('Saved progress could not be read');
    await history.getByRole('button', { name: action, exact: true }).click();
    await expect(history.getByRole('alert')).toHaveCount(0);
    expect(await calls(page, command)).toHaveLength(2);
  });
}

test('ordinary cancel and queue stop keep using their existing commands', async ({ page }) => {
  await desktopMock(page, { jobs: [savedJob('running')] });
  await openJobs(page);
  await currentJob(page).getByRole('button', { name: 'Cancel job', exact: true }).click();
  expect(await calls(page, 'cancel_job')).toHaveLength(1);
  expect(await calls(page, 'stop_job')).toHaveLength(0);
  await emit(page, [savedJob('running')]);
  await page.getByRole('button', { name: 'Stop queue', exact: true }).click();
  expect(await calls(page, 'cancel_all_jobs')).toHaveLength(1);
  expect(await calls(page, 'stop_job')).toHaveLength(0);
});
