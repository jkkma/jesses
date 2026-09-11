import { expect, test, type Page } from '@playwright/test';
import type { EncodeJob, EncodeRequest, MediaFile, ToolInfo } from '../../src/lib/ipc/generated';

const media: MediaFile = {
  id: 'encode-source',
  name: 'café 東京.mkv',
  path: 'C:\\media\\café 東京.mkv',
  sizeBytes: '123456',
  durationSeconds: 20,
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
  ],
};
const tools: ToolInfo[] = ['ffmpeg', 'ffprobe', 'svt-av1'].map((id) => ({
  id,
  name: id,
  available: true,
  path: `C:\\tools\\${id}.exe`,
  version: 'test',
  detail: null,
}));

type Harness = {
  jobs: EncodeJob[];
  requests: EncodeRequest[];
  cancellations: string[];
  nextError: string | null;
  startDelayMs: number;
  listDelayMs: number;
  activeLists: number;
  maxActiveLists: number;
  listCalls: number;
  persist: () => void;
};
declare global {
  interface Window {
    __ENCODE_TEST__: Harness;
  }
}

async function mockDesktop(page: Page, missingTool?: string) {
  await page.addInitScript(
    ({ source, capabilities }) => {
      const state = globalThis as unknown as Record<string, unknown>;
      const harness: Harness = {
        jobs: JSON.parse(sessionStorage.getItem('test.saved-jobs') ?? '[]'),
        requests: [],
        cancellations: [],
        nextError: null,
        startDelayMs: 0,
        listDelayMs: 0,
        activeLists: 0,
        maxActiveLists: 0,
        listCalls: 0,
        persist() {
          sessionStorage.setItem('test.saved-jobs', JSON.stringify(harness.jobs));
        },
      };
      state.__ENCODE_TEST__ = harness;
      state.isTauri = true;
      state.__TAURI_INTERNALS__ = {
        metadata: { currentWindow: { label: 'main' }, currentWebview: { label: 'main' } },
        transformCallback: () => 1,
        invoke: async (
          command: string,
          payload: { request?: EncodeRequest; id?: string; options?: { directory?: boolean } },
        ) => {
          if (command === 'get_capabilities') return capabilities;
          if (command.startsWith('plugin:event|')) return 1;
          if (command === 'plugin:dialog|open')
            return payload.options?.directory ? 'D:\\encodes café\\東京' : [source.path];
          if (command === 'probe_media') return source;
          if (command === 'list_jobs') {
            harness.listCalls += 1;
            harness.activeLists += 1;
            harness.maxActiveLists = Math.max(harness.maxActiveLists, harness.activeLists);
            const snapshot = structuredClone(harness.jobs);
            if (harness.listDelayMs)
              await new Promise((resolve) => setTimeout(resolve, harness.listDelayMs));
            harness.activeLists -= 1;
            return snapshot;
          }
          if (command === 'start_encode') {
            harness.requests.push(structuredClone(payload.request!));
            if (harness.startDelayMs)
              await new Promise((resolve) => setTimeout(resolve, harness.startDelayMs));
            if (harness.nextError) {
              const message = harness.nextError;
              harness.nextError = null;
              throw { code: 'output_exists', message, path: payload.request!.outputPath };
            }
            const job: EncodeJob = {
              id: `job-${harness.requests.length}`,
              request: payload.request!,
              status: 'preparing',
              progress: null,
              message: 'Preparing encoder.',
              createdAtMs: '1000',
              updatedAtMs: '1000',
            };
            harness.jobs.unshift(job);
            harness.persist();
            return structuredClone(job);
          }
          if (command === 'cancel_encode') {
            harness.cancellations.push(payload.id!);
            const job = harness.jobs.find((entry) => entry.id === payload.id)!;
            job.status = 'cancelled';
            job.message = 'Encode cancelled. Temporary files removed.';
            harness.persist();
            return;
          }
          throw new Error(`Unexpected IPC command: ${command}`);
        },
      };
    },
    {
      source: media,
      capabilities: tools.map((tool) => ({ ...tool, available: tool.id !== missingTool })),
    },
  );
  await page.goto('/');
}

async function openConvert(page: Page) {
  await page.getByRole('button', { name: 'Add files', exact: true }).first().click();
  await expect(page.getByRole('heading', { name: media.name })).toBeVisible();
  await page.getByRole('button', { name: 'Quick Convert', exact: true }).click();
}

test('passes editable settings and Unicode paths, freezes an in-flight start, and shows progress and completion', async ({
  page,
}) => {
  await mockDesktop(page);
  await openConvert(page);
  await page.getByLabel('Quality', { exact: true }).fill('25');
  await page.getByLabel('Encoder preset').selectOption('7');
  await page.getByLabel('Bitrate', { exact: true }).fill('192');
  await page
    .getByLabel('Channels', { exact: true })
    .selectOption({ label: 'Keep source channels' });
  await page.getByRole('button', { name: 'Choose folder', exact: true }).click();
  await page.getByLabel('Filename suffix').fill('_AV1 日本');
  await page.evaluate(() => {
    window.__ENCODE_TEST__.startDelayMs = 700;
  });
  await page.getByRole('button', { name: 'Start encode', exact: true }).click();
  await expect(page.getByLabel('Quality', { exact: true })).toBeDisabled();
  await expect(page.getByRole('button', { name: 'Change source', exact: true })).toBeDisabled();
  const job = page.getByRole('article', { name: `Encode ${media.name}` });
  await expect(job).toContainText('Preparing encoder.');
  expect(await page.evaluate(() => window.__ENCODE_TEST__.requests)).toEqual([
    {
      inputPath: media.path,
      outputPath: 'D:\\encodes café\\東京\\café 東京_AV1 日本.mkv',
      crf: 25,
      preset: 7,
      audioBitrateKbps: 192,
      audioChannels: null,
    },
  ]);
  await page.evaluate(() => {
    Object.assign(window.__ENCODE_TEST__.jobs[0], {
      status: 'encoding',
      progress: 43,
      message: 'Encoding video.',
    });
    window.__ENCODE_TEST__.persist();
  });
  await expect(job.getByRole('progressbar')).toHaveAttribute('value', '43');
  await expect(job).toContainText('43%');
  await page.evaluate(() => {
    Object.assign(window.__ENCODE_TEST__.jobs[0], {
      status: 'completed',
      progress: 100,
      message: 'Output checked and saved.',
    });
    window.__ENCODE_TEST__.persist();
  });
  await expect(job).toContainText('Completed');
  await expect(job).toContainText('Output checked and saved.');
  await expect(job.getByRole('button', { name: 'Cancel encode' })).toHaveCount(0);
  await expect(page.getByRole('button', { name: 'Start encode' })).toBeEnabled();
});

test('restores an active job after reload and cancels it with no selected source', async ({
  page,
}) => {
  await mockDesktop(page);
  await openConvert(page);
  await page.getByRole('button', { name: 'Start encode', exact: true }).click();
  await expect(page.getByRole('button', { name: 'Cancel encode' })).toBeVisible();
  await page.reload();
  await page.getByRole('button', { name: 'Quick Convert', exact: true }).click();
  await expect(page.getByText('No source selected', { exact: true })).toBeVisible();
  await expect(page.getByRole('button', { name: 'Start encode' })).toBeDisabled();
  await page.getByRole('button', { name: 'Cancel encode' }).click();
  await expect(page.getByRole('article')).toContainText('Cancelled');
  expect(await page.evaluate(() => window.__ENCODE_TEST__.cancellations)).toEqual(['job-1']);
  await page.reload();
  await page.getByRole('button', { name: 'Quick Convert', exact: true }).click();
  await expect(page.getByRole('article')).toContainText('Temporary files removed.');
  await expect(page.getByRole('button', { name: 'Cancel encode' })).toHaveCount(0);
});

test('reports existing-output and encoder failures, then restores interrupted history', async ({
  page,
}) => {
  await mockDesktop(page);
  await openConvert(page);
  await page.evaluate(() => {
    window.__ENCODE_TEST__.nextError = 'The output file already exists. Choose another filename.';
  });
  await page.getByRole('button', { name: 'Start encode', exact: true }).click();
  await expect(page.getByRole('alert')).toContainText('The output file already exists.');
  await expect(page.getByRole('button', { name: 'Start encode' })).toBeEnabled();
  expect(await page.evaluate(() => window.__ENCODE_TEST__.jobs)).toEqual([]);
  await page.getByLabel('Filename suffix').fill('_retry');
  await page.getByRole('button', { name: 'Start encode', exact: true }).click();
  await expect(page.getByRole('article')).toBeVisible();
  await page.evaluate(() => {
    Object.assign(window.__ENCODE_TEST__.jobs[0], {
      status: 'failed',
      message: 'SVT-AV1 exited with code 1.',
    });
    window.__ENCODE_TEST__.persist();
  });
  await expect(page.getByRole('article')).toContainText('Failed');
  await expect(page.getByRole('article')).toContainText('SVT-AV1 exited with code 1.');
  await expect(page.getByRole('button', { name: 'Start encode' })).toBeEnabled();
  await page.evaluate(() => {
    Object.assign(window.__ENCODE_TEST__.jobs[0], {
      status: 'interrupted',
      message: 'The app closed before this encode finished.',
    });
    window.__ENCODE_TEST__.persist();
  });
  await page.reload();
  await page.getByRole('button', { name: 'Quick Convert', exact: true }).click();
  await expect(page.getByRole('article')).toContainText('Interrupted');
  await expect(page.getByRole('article')).toContainText(
    'The app closed before this encode finished.',
  );
});

test('validates filenames and settings before sending IPC, and resets destination on source navigation', async ({
  page,
}) => {
  await mockDesktop(page);
  await openConvert(page);
  for (const suffix of ['', '../replace', '_bad:', '_bad.']) {
    await page.getByLabel('Filename suffix').fill(suffix);
    await expect(page.getByRole('button', { name: 'Start encode' })).toBeDisabled();
  }
  await page.getByLabel('Filename suffix').fill('_ok');
  await page.getByLabel('Quality', { exact: true }).fill('64');
  await expect(page.getByRole('button', { name: 'Start encode' })).toBeDisabled();
  await page.getByLabel('Quality', { exact: true }).fill('30');
  await page.getByLabel('Bitrate', { exact: true }).fill('31');
  await expect(page.getByRole('button', { name: 'Start encode' })).toBeDisabled();
  await page.getByLabel('Bitrate', { exact: true }).fill('128');
  await expect(page.getByRole('button', { name: 'Start encode' })).toBeEnabled();
  expect(await page.evaluate(() => window.__ENCODE_TEST__.requests)).toEqual([]);
  await page.getByRole('button', { name: 'Choose folder', exact: true }).click();
  await page.getByRole('button', { name: 'Change source' }).click();
  await page.getByRole('button', { name: `Remove ${media.name}` }).click();
  await page.getByRole('button', { name: 'Quick Convert', exact: true }).click();
  await expect(page.getByLabel('Destination', { exact: true })).toHaveValue(
    'Same folder as source',
  );
  await expect(page.getByLabel('Filename suffix')).toHaveValue('_encoded');
});

test('requires standalone encoder availability and keeps long job paths within minimum window width', async ({
  page,
}) => {
  await mockDesktop(page, 'svt-av1');
  await openConvert(page);
  await expect(page.getByRole('button', { name: 'Start encode' })).toBeDisabled();
  await expect(page.getByText('Required tools: SVT-AV1.', { exact: true })).toBeVisible();
  await page.setViewportSize({ width: 760, height: 600 });
  expect(await page.evaluate(() => document.documentElement.scrollWidth <= window.innerWidth)).toBe(
    true,
  );
});

test('does not overlap slow polling and ignores a stale list returned after starting', async ({
  page,
}) => {
  await mockDesktop(page);
  await openConvert(page);
  await expect(page.getByRole('button', { name: 'Start encode' })).toBeEnabled();
  await page.evaluate(() => {
    window.__ENCODE_TEST__.listDelayMs = 1500;
  });
  await expect.poll(() => page.evaluate(() => window.__ENCODE_TEST__.activeLists)).toBe(1);
  await page.getByRole('button', { name: 'Start encode' }).click();
  await expect(page.getByRole('button', { name: 'Cancel encode' })).toBeVisible();
  await expect
    .poll(() => page.evaluate(() => window.__ENCODE_TEST__.listCalls))
    .toBeGreaterThanOrEqual(3);
  await expect(page.getByRole('button', { name: 'Cancel encode' })).toBeVisible();
  expect(await page.evaluate(() => window.__ENCODE_TEST__.maxActiveLists)).toBe(1);
  await page.getByRole('button', { name: 'Files', exact: false }).first().click();
  await expect(page.getByRole('heading', { name: media.name })).toBeVisible();
  await page.getByRole('button', { name: 'Quick Convert', exact: true }).click();
  await expect(page.getByRole('button', { name: 'Cancel encode' })).toBeVisible();
});

test('running jobs and long failure diagnostics remain readable at desktop and minimum size', async ({
  page,
}) => {
  await mockDesktop(page);
  await openConvert(page);
  await page.evaluate(() => {
    const request: EncodeRequest = {
      inputPath:
        'C:\\Users\\Editor\\Videos\\日本語と英語の字幕\\[Studio] とても長い番組タイトルの東京で過ごした特別な一日 - 01 [1080p BluRay FLAC] [ABC12345].mkv',
      outputPath:
        'D:\\Encoded media\\Completed projects\\日本語と英語の字幕\\[Studio] とても長い番組タイトルの東京で過ごした特別な一日 - 01 [1080p BluRay FLAC] [ABC12345]_encoded.mkv',
      crf: 30,
      preset: 4,
      audioBitrateKbps: 128,
      audioChannels: 2,
    };
    window.__ENCODE_TEST__.jobs = [
      {
        id: 'visual-active',
        request,
        status: 'encoding',
        progress: 47.3,
        message: 'Encoding video. Audio and subtitles will be muxed when video encoding finishes.',
        createdAtMs: '1000',
        updatedAtMs: '2000',
      },
      {
        id: 'visual-failed',
        request: {
          ...request,
          inputPath: request.inputPath.replace(' - 01 ', ' - 02 '),
          outputPath: request.outputPath.replace(' - 01 ', ' - 02 '),
        },
        status: 'failed',
        progress: 23,
        message:
          'SVT-AV1 exited with code 1. Encoder diagnostic: Failed to open temporary output D:\\Encoded media\\Completed projects\\日本語と英語の字幕\\.jesses-5ef68c23b2be4a018df1102c53bb19a8\\intermediate-video-with-a-very-long-name-and-an-additional-description-for-checking-diagnostic-wrapping.ivf. The device reported insufficient free space.',
        createdAtMs: '500',
        updatedAtMs: '900',
      },
    ];
    window.__ENCODE_TEST__.persist();
  });
  await expect(page.getByRole('article')).toHaveCount(2);
  await expect(page.getByRole('button', { name: 'Cancel encode' })).toBeVisible();
  await expect(
    page.getByText('For standard SDR video with a constant frame rate.', { exact: false }),
  ).toBeVisible();
  for (const viewport of [
    { width: 1280, height: 850, name: 'quick-convert-desktop' },
    { width: 760, height: 600, name: 'minimum' },
  ]) {
    await page.setViewportSize(viewport);
    expect(
      await page.evaluate(() => document.documentElement.scrollWidth <= window.innerWidth),
    ).toBe(true);
    await page.screenshot({ path: `test-results/${viewport.name}.png`, fullPage: true });
  }
});
