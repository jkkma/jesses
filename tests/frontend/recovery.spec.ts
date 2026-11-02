import { expect, test } from '@playwright/test';
import { installSourceEncodeFixture } from './helpers/source-encode-fixture';

test('saved AV1AN resume survives reload and ignores the current editor draft', async ({
  page,
}) => {
  await installSourceEncodeFixture(page);
  const entry = {
    job: {
      id: 'saved',
      state: 'stopped',
      request: {
        inputPath: '/media/saved.mkv',
        outputPath: '/media/saved.out.mkv',
        streamIndices: [0],
      },
      encodeSettings: { backend: 'av1an', encoder: 'svtAv1', crf: 37 },
      recovery: {
        workspace: '/tmp/saved',
        phase: 'encoding',
        completedFrames: 10,
        totalFrames: 100,
      },
      progressSeconds: 0,
      durationSeconds: 12,
      logs: [],
      error: null,
      logPath: null,
    },
    encodedBytes: '1024',
    chunkCount: 2,
    createdAt: null,
    lastRunAt: null,
  };
  await page.evaluate(
    (entry) => sessionStorage.setItem('test-recoveries', JSON.stringify([entry])),
    entry,
  );
  await page.reload();
  await page.getByRole('button', { name: 'AV1AN', exact: true }).click();
  const av = page.getByRole('region', { name: 'AV1AN', exact: true });
  await av.getByLabel('Quality', { exact: true }).fill('21');
  await av.getByRole('button', { name: 'Resume…' }).click();
  const dialog = page.getByRole('dialog', { name: 'Resume AV1AN Encode' });
  await expect(dialog.getByRole('option')).toHaveCount(1);
  await dialog.getByRole('button', { name: 'Resume (Saved Settings)' }).click();
  await expect
    .poll(() =>
      page.evaluate(
        () =>
          (globalThis as any).__sourceCalls.find((c: any) => c.command === 'prepare_command_edit')
            ?.payload.request,
      ),
    )
    .toEqual({ kind: 'resumeSaved', id: 'saved' });
  await expect
    .poll(() =>
      page.evaluate(
        () =>
          (globalThis as any).__testJobs.find((job: any) => job.id === 'saved')?.encodeSettings
            ?.crf,
      ),
    )
    .toBe(37);
  expect(
    await page.evaluate(() =>
      (globalThis as any).__sourceCalls.some((c: any) =>
        ['start_encode', 'enqueue_encode'].includes(c.command),
      ),
    ),
  ).toBe(false);
  await expect(page.getByRole('button', { name: 'Pause', exact: true })).toBeEnabled();
  await page.getByRole('button', { name: 'Pause', exact: true }).click();
  await expect(page.getByRole('button', { name: 'Resume', exact: true })).toBeVisible();
  await page.getByRole('button', { name: 'Resume', exact: true }).click();
  await expect(page.getByRole('button', { name: 'Pause', exact: true })).toBeEnabled();
  await page.getByRole('button', { name: 'Stop', exact: true }).click();
  await expect(page.getByLabel('Current action')).toHaveText('Canceled.');
  expect(
    await page.evaluate(() =>
      (globalThis as any).__sourceCalls
        .filter((c: any) => c.command === 'set_job_paused')
        .map((c: any) => c.payload),
    ),
  ).toEqual([
    { id: 'saved', paused: true },
    { id: 'saved', paused: false },
  ]);
});
