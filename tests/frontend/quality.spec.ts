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
      let picker = 0;
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
            return [picker++ === 0 ? 'C:\\media\\reference.mkv' : 'C:\\media\\candidate.mkv'];
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
          if (command === 'begin_media_analysis') return `quality-${++callback}`;
          if (command === 'cancel_media_analysis') return;
          if (command === 'analyze_quality') {
            if (hold)
              await new Promise<void>((resolve) => {
                mock.release = resolve;
              });
            const request = payload.request as QualityRequest;
            return {
              metric: request.metric,
              frameCount: 2,
              score: 0.99,
              points: [
                { frame: 0, score: 1 },
                { frame: 1, score: 0.98 },
              ],
              referenceFingerprint: 'reference',
              candidateFingerprint: 'candidate',
              model: null,
              message: 'Review these selected frames.',
            } satisfies QualityResult;
          }
          throw new Error(`Unexpected command ${command}`);
        },
      };
    },
    { hold },
  );
  await page.goto('/');
  await page.getByRole('button', { name: 'Add files', exact: true }).first().click();
  await expect(page.getByRole('heading', { name: 'reference.mkv', exact: true })).toBeVisible();
}
const calls = (page: Page, command: string) =>
  page.evaluate(
    (command) =>
      (globalThis as unknown as { __qualityMock: Mock }).__qualityMock.calls.filter(
        (call) => call.command === command,
      ),
    command,
  );
test('quality comparison uses explicit source indices and intervals with keyboard score inspection', async ({
  page,
}) => {
  await setup(page);
  expect(await calls(page, 'analyze_quality')).toHaveLength(0);
  await page.getByRole('button', { name: /Compare video quality/ }).click();
  const region = page.getByRole('region', { name: 'Quality comparison', exact: true });
  await region.getByRole('button', { name: 'Choose candidate video', exact: true }).click();
  await expect(region).toContainText('candidate.mkv');
  await region.getByLabel('Reference video', { exact: true }).selectOption('4');
  await region.getByLabel('Candidate video', { exact: true }).selectOption('4');
  await region.getByLabel('Reference start frame', { exact: true }).fill('48');
  await region.getByLabel('Candidate start frame', { exact: true }).fill('12');
  await region.getByLabel('Frames to compare', { exact: true }).fill('2');
  await region.getByRole('button', { name: 'Compare selected frames', exact: true }).click();
  await expect(region.getByRole('img', { name: 'SSIM score by frame' })).toBeVisible();
  expect((await calls(page, 'analyze_quality'))[0].payload.request).toEqual({
    referencePath: 'C:\\media\\reference.mkv',
    referenceStreamIndex: 4,
    referenceStartFrame: 48,
    candidatePath: 'C:\\media\\candidate.mkv',
    candidateStreamIndex: 4,
    candidateStartFrame: 12,
    frameCount: 2,
    metric: 'ssim',
  });
  await region.getByRole('slider', { name: 'Inspect comparison frame' }).focus();
  await page.keyboard.press('End');
  await expect(region).toContainText('Frame 1: 0.980000');
  await region.getByRole('button', { name: 'Export CSV', exact: true }).click();
  await expect(region.getByRole('status')).toContainText('Saved: C:\\reports\\quality.csv');
  const exported = (await calls(page, 'export_analysis'))[0].payload.request as {
    format: string;
    report: { request: QualityRequest; result: QualityResult };
  };
  expect(exported.format).toBe('csv');
  expect(exported.report.request).toEqual(
    (await calls(page, 'analyze_quality'))[0].payload.request,
  );
  expect(exported.report.result.points).toEqual([
    { frame: 0, score: 1 },
    { frame: 1, score: 0.98 },
  ]);
  await region.getByLabel('Frames to compare', { exact: true }).fill('60001');
  await expect(
    region.getByRole('button', { name: 'Compare selected frames', exact: true }),
  ).toBeDisabled();
  await expect(region.getByRole('img')).toHaveCount(0);
  await expect(region.getByRole('button', { name: 'Export CSV', exact: true })).toHaveCount(0);
});
test('closing cancels comparison and suppresses late scores', async ({ page }) => {
  await setup(page, true);
  const disclosure = page.getByRole('button', { name: /Compare video quality/ });
  await disclosure.click();
  await page.getByRole('button', { name: 'Choose candidate video', exact: true }).click();
  await page.getByRole('button', { name: 'Compare selected frames', exact: true }).click();
  await expect.poll(async () => (await calls(page, 'analyze_quality')).length).toBe(1);
  await disclosure.click();
  await expect
    .poll(async () => (await calls(page, 'cancel_media_analysis')).length)
    .toBeGreaterThan(0);
  await page.evaluate(() =>
    (globalThis as unknown as { __qualityMock: Mock }).__qualityMock.release?.(),
  );
  await disclosure.click();
  await expect(page.getByRole('img', { name: 'SSIM score by frame' })).toHaveCount(0);
  await expect(page.getByRole('alert')).toHaveCount(0);
});
