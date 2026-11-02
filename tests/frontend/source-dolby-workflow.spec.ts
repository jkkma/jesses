import { expect, test, type Page } from '@playwright/test';
import { installSourceEncodeFixture, source } from './helpers/source-encode-fixture';

const hdr = {
  ...source,
  streams: source.streams.map((stream) =>
    stream.kind === 'video'
      ? {
          ...stream,
          codec: 'hevc',
          pixelFormat: 'yuv420p10le',
          fieldOrder: 'progressive',
          colorRange: 'tv',
          colorPrimaries: 'bt2020',
          colorTransfer: 'smpte2084',
          colorSpace: 'bt2020nc',
          dolbyVisionProfile: 8,
        }
      : stream,
  ),
};
async function open(page: Page, batch = false) {
  await installSourceEncodeFixture(page);
  await page.evaluate(
    ({ hdr, batch }) => {
      const h = globalThis as any;
      h.__setSourceMedia(hdr);
      if (batch) {
        const second = { ...hdr, id: 'second', path: '/media/second.mkv', name: 'second.mkv' };
        h.__pickerPaths = [hdr.path, second.path];
        h.__mediaByPath = { [hdr.path]: hdr, [second.path]: second };
      }
    },
    { hdr, batch },
  );
  await page.getByRole('button', { name: 'Add Files…' }).click();
  if (batch) await page.getByRole('combobox', { name: 'Mode', exact: true }).selectOption('batch');
  else await page.getByRole('button', { name: 'Load Tracks' }).click();
  await page.getByRole('button', { name: 'Quick Convert', exact: true }).click();
  return page.getByRole('region', { name: 'Quick Convert', exact: true });
}
async function calls(page: Page, command: string, count = 1) {
  await expect
    .poll(() =>
      page.evaluate(
        (command) =>
          (globalThis as any).__sourceCalls.filter((call: any) => call.command === command).length,
        command,
      ),
    )
    .toBe(count);
  return page.evaluate(
    (command) =>
      (globalThis as any).__sourceCalls
        .filter((call: any) => call.command === command)
        .map((call: any) => call.payload.request),
    command,
  );
}
test('ordinary and held source CPU requests freeze the implicit tone-map workflow', async ({
  page,
}) => {
  const region = await open(page);
  await region.getByLabel('Tone Mapping', { exact: true }).selectOption('hable');
  await page.getByRole('button', { name: 'Run: Encode', exact: true }).click();
  expect((await calls(page, 'mock_start_encode'))[0].settings.toneMap).toMatchObject({
    sourceWorkflow: true,
    hdr10BaseLayer: false,
  });
  await page.evaluate(() => {
    const h = globalThis as any;
    h.__holdResolution = new Promise((resolve) => (h.__releaseResolution = resolve));
  });
  await page.keyboard.down('Shift');
  await page.getByRole('button', { name: 'Run: Encode', exact: true }).click();
  await region.getByLabel('Tone Mapping', { exact: true }).selectOption('off');
  await page.evaluate(() => {
    const h = globalThis as any;
    h.__releaseResolution();
    h.__holdResolution = null;
  });
  const dialog = page.getByRole('dialog', { name: 'Edit ffmpeg Arguments' });
  await expect(dialog).toBeVisible();
  await page.keyboard.up('Shift');
  await dialog.getByRole('button', { name: 'Run', exact: true }).click();
  expect((await calls(page, 'mock_start_encode', 2))[1].settings.toneMap).toMatchObject({
    sourceWorkflow: true,
    hdr10BaseLayer: false,
  });
});
for (const batch of [false, true])
  test(`${batch ? 'batch' : 'ordinary'} image source request keeps compatible tone-map admission without a new control`, async ({
    page,
  }) => {
    const region = await open(page, batch);
    await region.getByLabel('Tone Mapping', { exact: true }).selectOption('hable');
    await region.getByLabel('Codec', { exact: true }).selectOption('png');
    await page
      .getByRole('button', { name: batch ? 'Run Batch: Encode' : 'Run: Encode', exact: true })
      .click();
    const requests = await calls(page, 'mock_image_execution', batch ? 2 : 1);
    expect(
      requests.every(
        (request: any) =>
          request.processing.toneMap.sourceWorkflow === true &&
          request.processing.toneMap.hdr10BaseLayer === false,
      ),
    ).toBe(true);
  });
test('source CPU batch freezes tone workflow before asynchronous per-file resolution', async ({
  page,
}) => {
  const region = await open(page, true);
  await region.getByLabel('Tone Mapping', { exact: true }).selectOption('hable');
  await page.evaluate(() => {
    const h = globalThis as any;
    h.__holdBatchPreview = new Promise((resolve) => (h.__releaseBatchTone = resolve));
  });
  await page.getByRole('button', { name: 'Run Batch: Encode', exact: true }).click();
  await region.getByLabel('Tone Mapping', { exact: true }).selectOption('off');
  await page.evaluate(() => {
    const h = globalThis as any;
    h.__releaseBatchTone();
    h.__holdBatchPreview = null;
  });
  expect(
    (await calls(page, 'mock_start_encode', 2)).every(
      (request: any) => request.settings.toneMap.sourceWorkflow === true,
    ),
  ).toBe(true);
});
test('source AV1AN CPU tone request freezes both source workflow authorities', async ({ page }) => {
  await open(page);
  await page.getByRole('button', { name: 'AV1AN', exact: true }).click();
  const region = page.getByRole('region', { name: 'AV1AN', exact: true });
  await region.getByLabel('Tone Mapping', { exact: true }).selectOption('hable');
  await page.getByRole('button', { name: 'Run: AV1AN encode', exact: true }).click();
  expect((await calls(page, 'mock_start_encode'))[0].settings).toMatchObject({
    toneMap: { sourceWorkflow: true, hdr10BaseLayer: false },
    av1anOptions: { sourceWorkflow: true },
  });
});
