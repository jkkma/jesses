import { test, expect, type Page } from '@playwright/test';
import type { QualityRequest, QualityResult } from '../../src/lib/ipc/generated';
type Call = { command: string; payload: Record<string, unknown> };
type Mock = { calls: Call[]; release: (() => void) | null };
async function setup(page: Page, hold = false) {
  await page.addInitScript(
    ({ hold }) => {
      const state = globalThis as unknown as Record<string, unknown>;
      const mock: Mock = { calls: [], release: null };
      state.__qualityMock = mock;
      state.isTauri = true;
      let callback = 0;
      state.__TAURI_INTERNALS__ = {
        metadata: { currentWindow: { label: 'main' }, currentWebview: { label: 'main' } },
        transformCallback: () => ++callback,
        unregisterCallback: () => {},
        invoke: async (command: string, payload: Record<string, unknown> = {}) => {
          mock.calls.push({ command, payload });
          if (command.startsWith('plugin:event|')) return ++callback;
          if (command === 'get_capabilities' || command === 'list_jobs') return [];
          if (command === 'subscribe_jobs') {
            (payload.channel as { onmessage: (jobs: unknown[]) => void }).onmessage([]);
            return;
          }
          if (command === 'plugin:dialog|open')
            return ['C:\\media\\reference.mkv', 'C:\\media\\candidate.mkv'];
          if (command === 'list_av1an_recoveries') return [];
          if (command === 'resolve_output_path') {
            const request = payload.request as any,
              stem = request.outputStem ?? String(request.inputPath).replace(/\.[^.]+$/, '');
            return {
              outputStem: stem,
              outputPath: `${stem}.mkv`,
              isDirectory: false,
              numbered: false,
            };
          }
          if (command === 'plugin:dialog|save') return 'C:\\reports\\quality.csv';
          if (command === 'export_analysis')
            return (payload.request as { outputPath: string }).outputPath;
          if (command === 'probe_media') {
            const path = payload.path as string;
            return {
              id: path,
              path,
              name: path.split('\\').at(-1),
              sizeBytes: '10000',
              durationSeconds: 12,
              format: 'matroska',
              streams: [0, 4].map((index) => ({
                index,
                kind: 'video',
                codec: 'h264',
                width: 320,
                height: 180,
                frameRate: '24/1',
                channels: null,
                sampleRate: null,
                language: null,
                title: null,
              })),
            };
          }
          if (command === 'begin_media_analysis')
            return payload.forImport ? 'import-reservation' : `quality-${++callback}`;
          if (command === 'claim_media_import' || command === 'cancel_media_analysis') return;
          if (command === 'analyze_quality') {
            if (hold)
              await new Promise<void>((resolve) => {
                mock.release = resolve;
              });
            const request = payload.request as QualityRequest;
            const step = request.options?.subsample ?? 1;
            const frameCount = Math.ceil(
              (request.fullSource ? 48 : (request.frameCount ?? 48)) / step,
            );
            return {
              metric: request.metric,
              frameCount,
              score: 0.99,
              points: Array.from({ length: frameCount }, (_, i) => ({
                frame: i * step,
                score: i === 0 ? 1 : 0.98,
              })),
              referenceFingerprint: 'reference',
              candidateFingerprint: 'candidate',
              model:
                request.metric === 'vmaf'
                  ? {
                      standard: 'vmaf_v0.6.1',
                      negative: 'vmaf_v0.6.1neg',
                      fourK: 'vmaf_4k_v0.6.1',
                    }[request.options?.vmafModel ?? 'standard']
                  : null,
              message: 'Review these selected frames.',
            } satisfies QualityResult;
          }
          if (command === 'get_completion_status')
            return {
              options: { notify: false, finishAction: 'none' },
              armedJobs: 0,
              secondsRemaining: null,
              error: null,
            };
          throw new Error(`Unexpected command ${command}`);
        },
      };
    },
    { hold },
  );
  await page.goto('/');
  await page.getByRole('button', { name: 'Add Files…', exact: true }).click();
  await page.getByRole('option', { name: /reference.mkv/ }).click();
  await page.getByRole('option', { name: /candidate.mkv/ }).click({ modifiers: ['Control'] });
  await page.getByRole('button', { name: 'Load Tracks', exact: true }).click();
  await page.getByRole('button', { name: 'Utilities', exact: true }).click();
}
const calls = (page: Page, command: string) =>
  page.evaluate(
    (command) =>
      (globalThis as unknown as { __qualityMock: Mock }).__qualityMock.calls.filter(
        (call) => call.command === command,
      ),
    command,
  );
// The fork's Get Metrics compares complete loaded sources and reports in the
// log. The retired frame-range inspector, score graph and CSV export are not
// source controls; preserve request, option, cancellation and partial-result checks.
async function configure(page: Page) {
  await page
    .getByRole('region', { name: 'Utilities' })
    .getByRole('button', { name: /Get Metrics/ })
    .click();
  return page.getByRole('dialog', { name: 'Quality Metrics' });
}

test('Get Metrics uses the original source video indices for full-source comparison', async ({
  page,
}) => {
  await setup(page);
  expect(await calls(page, 'analyze_quality')).toHaveLength(0);
  const d = await configure(page);
  await d.getByLabel('Encoded (Low Quality)').selectOption('C:\\media\\candidate.mkv');
  await d.getByLabel('Reference (Source)').selectOption('C:\\media\\reference.mkv');
  await d.getByLabel('VMAF', { exact: true }).uncheck();
  await d.getByLabel('SSIM', { exact: true }).check();
  await d.getByRole('button', { name: 'OK' }).click();
  await page.getByRole('button', { name: 'Run: Metrics calculation' }).click();
  await expect.poll(async () => (await calls(page, 'analyze_quality')).length).toBe(1);
  expect((await calls(page, 'analyze_quality'))[0].payload.request).toEqual({
    referencePath: 'C:\\media\\reference.mkv',
    referenceStreamIndex: 0,
    candidatePath: 'C:\\media\\candidate.mkv',
    candidateStreamIndex: 0,
    referenceStartFrame: 0,
    candidateStartFrame: 0,
    frameCount: 0,
    fullSource: true,
    metric: 'ssim',
    options: { alignment: 'none', vmafModel: 'standard', subsample: 1, fixFrameRate: false },
  });
  await expect(page.getByRole('log', { name: 'Recent activity' })).toContainText('SSIM: 0.9900');
});

test('Stop cancels a running metric and suppresses its late result', async ({ page }) => {
  await setup(page, true);
  const d = await configure(page);
  await d.getByRole('button', { name: 'OK' }).click();
  await page.getByRole('button', { name: 'Run: Metrics calculation' }).click();
  await expect.poll(async () => (await calls(page, 'analyze_quality')).length).toBe(1);
  await page.getByRole('button', { name: 'Stop', exact: true }).click();
  await expect
    .poll(async () =>
      (await calls(page, 'cancel_media_analysis')).some(
        (call) => call.payload.id !== 'import-reservation',
      ),
    )
    .toBe(true);
  await page.evaluate(() => (globalThis as any).__qualityMock.release?.());
  await expect(page.getByRole('button', { name: 'Run: Metrics calculation' })).toBeEnabled();
  await expect(page.getByRole('log', { name: 'Recent activity' })).not.toContainText(
    'VMAF: 0.9900',
  );
});

test('source alignment, VMAF model and subsampling reach both immutable metric requests', async ({
  page,
}) => {
  await setup(page);
  const d = await configure(page);
  await d.getByLabel('SSIM', { exact: true }).check();
  await d.getByLabel('Alignment', { exact: true }).selectOption('cropAndResizeReference');
  await d.getByLabel('VMAF Model', { exact: true }).selectOption('fourK');
  await d.getByLabel('Subsampling', { exact: true }).selectOption('5');
  await d.getByRole('button', { name: 'OK' }).click();
  await page.getByRole('button', { name: 'Run: Metrics calculation' }).click();
  await expect.poll(async () => (await calls(page, 'analyze_quality')).length).toBe(2);
  const requests = (await calls(page, 'analyze_quality')).map(
    (c) => c.payload.request as QualityRequest,
  );
  expect(requests.map((r) => r.metric)).toEqual(['vmaf', 'ssim']);
  for (const request of requests)
    expect(request.options).toEqual({
      alignment: 'cropAndResizeReference',
      vmafModel: 'fourK',
      subsample: 5,
      fixFrameRate: false,
    });
});

test('stopping a later metric keeps completed log output without adding a late score', async ({
  page,
}) => {
  await setup(page, true);
  const d = await configure(page);
  await d.getByLabel('SSIM', { exact: true }).check();
  await d.getByRole('button', { name: 'OK' }).click();
  await page.getByRole('button', { name: 'Run: Metrics calculation' }).click();
  await expect.poll(async () => (await calls(page, 'analyze_quality')).length).toBe(1);
  await page.evaluate(() => (globalThis as any).__qualityMock.release?.());
  await expect.poll(async () => (await calls(page, 'analyze_quality')).length).toBe(2);
  await page.getByRole('button', { name: 'Stop', exact: true }).click();
  await page.evaluate(() => (globalThis as any).__qualityMock.release?.());
  await expect(page.getByRole('button', { name: 'Run: Metrics calculation' })).toBeEnabled();
  await expect(page.getByRole('log', { name: 'Recent activity' })).toContainText('VMAF: 0.9900');
  await expect(page.getByRole('log', { name: 'Recent activity' })).not.toContainText(
    'SSIM: 0.9900',
  );
});
