import { expect, test } from '@playwright/test';
import { installSourceEncodeFixture, source } from './helpers/source-encode-fixture';

// The source has one Burn In selector and a global subtitle codec rather than
// the retired per-track action panels. Preserve source-scoped stream indices,
// retained captions, workflow state and immutable batch request assertions.
function withSubtitles() {
  const subtitle = source.streams.find((stream) => stream.kind === 'subtitle')!;
  return {
    ...source,
    streams: [
      ...source.streams.filter((stream) => stream.kind !== 'subtitle'),
      { ...subtitle, index: 7, title: 'First' },
      { ...subtitle, index: 9, title: 'Second' },
    ],
  };
}

test('one source Burn In selector retains selected stream identity across workflow switches', async ({
  page,
}) => {
  await installSourceEncodeFixture(page);
  const media = withSubtitles();
  await page.evaluate((media) => (globalThis as any).__setSourceMedia(media), media);
  await page.getByRole('button', { name: 'Add Files…' }).click();
  await page.getByRole('button', { name: 'Load Tracks' }).click();
  await page.getByRole('button', { name: 'Quick Convert', exact: true }).click();
  const quick = page.getByRole('region', { name: 'Quick Convert', exact: true });
  await quick.getByRole('tab', { name: 'Subtitles' }).click();
  await quick.getByLabel('Codec', { exact: true }).selectOption('subRip');
  const burn = quick.getByLabel('Burn In');
  await expect(burn.locator('option')).toHaveCount(3);
  await burn.selectOption('7');
  await page.getByRole('button', { name: 'AV1AN', exact: true }).click();
  const av = page.getByRole('region', { name: 'AV1AN', exact: true });
  await av.getByRole('tab', { name: 'Audio & Tracks' }).click();
  await expect(av.getByLabel('Burn In')).toHaveCount(0);
  await page.getByRole('button', { name: 'Quick Convert', exact: true }).click();
  await expect(burn).toHaveValue('7');
  await quick.getByLabel('Output Path', { exact: true }).fill('/media/subtitles');
  await page.getByRole('button', { name: 'Run: Encode', exact: true }).click();
  await expect
    .poll(() =>
      page.evaluate(() =>
        (globalThis as any).__sourceCalls.some((c: any) => c.command === 'mock_start_encode'),
      ),
    )
    .toBe(true);
  const request = await page.evaluate(
    () =>
      (globalThis as any).__sourceCalls.find((c: any) => c.command === 'mock_start_encode').payload
        .request,
  );
  expect(request.source.streamIndices).toEqual([0, 1, 7, 9]);
  expect(request.settings.subtitles).toEqual([
    { streamIndex: 7, mode: 'subRip', burnIn: true },
    { streamIndex: 9, mode: 'subRip' },
  ]);
});

test('batch subtitle choices remain frozen while the current draft changes', async ({ page }) => {
  await installSourceEncodeFixture(page);
  const first = withSubtitles(),
    second = { ...first, id: 'source-2', path: '/other/second.mkv', name: 'second.mkv' };
  await page.evaluate(
    ({ first, second }) => {
      const h = globalThis as any;
      h.__pickerPaths = [first.path, second.path];
      h.__mediaByPath = { [first.path]: first, [second.path]: second };
      h.__holdPrepare = new Promise((resolve) => (h.__releasePrepare = resolve));
    },
    { first, second },
  );
  await page.getByRole('button', { name: 'Add Files…' }).click();
  await page.getByRole('combobox', { name: 'Mode', exact: true }).selectOption('batch');
  await page.getByRole('button', { name: 'Quick Convert', exact: true }).click();
  const quick = page.getByRole('region', { name: 'Quick Convert', exact: true });
  await quick.getByRole('tab', { name: 'Subtitles' }).click();
  await quick.getByLabel('Codec', { exact: true }).selectOption('webVtt');
  await page.getByRole('button', { name: 'Run Batch: Encode' }).click();
  await expect
    .poll(() =>
      page.evaluate(() =>
        (globalThis as any).__sourceCalls.some((c: any) => c.command === 'prepare_command_edit'),
      ),
    )
    .toBe(true);
  await quick.getByLabel('Codec', { exact: true }).selectOption('subRip');
  await page.evaluate(() => (globalThis as any).__releasePrepare());
  await expect
    .poll(() =>
      page.evaluate(
        () =>
          (globalThis as any).__sourceCalls.filter((c: any) => c.command === 'mock_start_encode')
            .length,
      ),
    )
    .toBe(2);
  const calls = await page.evaluate(() => (globalThis as any).__sourceCalls);
  const preview = calls.find((c: any) => c.command === 'preview_encode_batch').payload.request;
  expect(preview.inputs.map((input: any) => input.subtitles)).toEqual([
    [
      { streamIndex: 7, mode: 'webVtt' },
      { streamIndex: 9, mode: 'webVtt' },
    ],
    [
      { streamIndex: 7, mode: 'webVtt' },
      { streamIndex: 9, mode: 'webVtt' },
    ],
  ]);
});
