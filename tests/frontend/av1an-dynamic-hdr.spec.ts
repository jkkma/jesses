import { expect, test, type Page } from '@playwright/test';
import { installSourceEncodeFixture, source } from './helpers/source-encode-fixture';

// Stream properties from the Resident Evil source in Videos.
const hdrVideo = {
  ...source.streams[0],
  codec: 'hevc',
  fieldOrder: 'progressive',
  width: 3832,
  height: 1600,
  frameRate: '24000/1001',
  pixelFormat: 'yuv420p10le',
  bitDepth: 10,
  colorPrimaries: 'bt2020',
  colorTransfer: 'smpte2084',
  colorSpace: 'bt2020nc',
  colorRange: 'tv',
  hdrFormat: 'HDR / PQ',
  dynamicHdrFormats: ['Dolby Vision'],
  dolbyVisionProfile: 8,
};
const hdrSource = {
  ...source,
  id: 'resident-evil',
  path: '/media/resident-evil.mp4',
  name: 'resident-evil.mp4',
  format: 'mov,mp4,m4a,3gp,3g2,mj2',
  streams: [hdrVideo, ...source.streams.slice(1)],
};

async function openAv1an(page: Page, batch = false, mixed = false) {
  await installSourceEncodeFixture(page);
  await page.evaluate(
    ({ media, batch, mixed }) => {
      const state = globalThis as any;
      state.__setSourceMedia(media);
      if (batch) {
        const second = {
          ...media,
          id: 'second',
          path: '/media/second.mp4',
          name: 'second.mp4',
          streams: mixed
            ? [
                { ...media.streams[0], dolbyVisionProfile: undefined, dynamicHdrFormats: [] },
                ...media.streams.slice(1),
              ]
            : media.streams,
        };
        state.__pickerPaths = [media.path, second.path];
        state.__mediaByPath = { [media.path]: media, [second.path]: second };
      }
    },
    { media: hdrSource, batch, mixed },
  );
  await page.getByRole('button', { name: 'Add Files…' }).click();
  if (batch) await page.getByRole('combobox', { name: 'Mode', exact: true }).selectOption('batch');
  else await page.getByRole('button', { name: 'Load Tracks' }).click();
  await page.getByRole('button', { name: 'AV1AN', exact: true }).click();
  const region = page.getByRole('region', { name: 'AV1AN', exact: true });
  await region.getByRole('tab', { name: 'Video', exact: true }).click();
  return region;
}

for (const batch of [false, true]) {
  test(`${batch ? 'batch' : 'single'} AV1AN requires an explicit HDR10 fallback`, async ({
    page,
  }) => {
    const region = await openAv1an(page, batch);
    const tone = region.getByLabel('Tone Mapping');
    const fallback = region.getByLabel('HDR10 fallback (discard DV / HDR10+)');
    const run = page.getByRole('button', {
      name: batch ? 'Run Batch: AV1AN encode' : 'Run: AV1AN encode',
      exact: true,
    });
    await expect(tone).toHaveValue('mobius');
    await expect(run).toBeEnabled();
    await tone.selectOption('off');
    await expect(fallback).not.toBeChecked();
    await expect(run).toBeDisabled();
    await expect(
      page
        .getByText(
          'Enable HDR10 fallback to discard Dolby Vision / HDR10+, or enable Tone Mapping.',
        )
        .first(),
    ).toBeVisible();
    expect(
      await page.evaluate(
        () =>
          (globalThis as any).__sourceCalls.filter(
            (call: any) => call.command === 'mock_start_encode',
          ).length,
      ),
    ).toBe(0);
    await fallback.check();
    await expect(run).toBeEnabled();
    await run.click();
    await expect
      .poll(() =>
        page.evaluate(
          () =>
            (globalThis as any).__sourceCalls.filter(
              (call: any) => call.command === 'mock_start_encode',
            ).length,
        ),
      )
      .toBe(batch ? 2 : 1);
    const settings = await page.evaluate(() =>
      (globalThis as any).__sourceCalls
        .filter((call: any) => call.command === 'mock_start_encode')
        .map((call: any) => call.payload.request.settings),
    );
    expect(
      settings.every(
        (setting: any) =>
          setting.encoder === 'svtAv1Hdr' && setting.hdr10Fallback === true && !setting.toneMap,
      ),
    ).toBe(true);
  });
}

test('AV1AN offers only supported fallback with known dynamic HDR', async ({ page }) => {
  const region = await openAv1an(page);
  await region.getByLabel('Tone Mapping').selectOption('off');
  await region.getByLabel('HDR10 fallback (discard DV / HDR10+)').check();
  await region.getByLabel('Encoder', { exact: true }).selectOption('aomAv1');
  await expect(region.getByLabel('HDR10 fallback (discard DV / HDR10+)')).toHaveCount(0);
  await expect(page.getByRole('button', { name: 'Run: AV1AN encode', exact: true })).toBeDisabled();
  await expect(
    page
      .getByText(
        'Choose an SVT-AV1 encoder and enable HDR10 fallback to discard Dolby Vision / HDR10+, or enable Tone Mapping.',
      )
      .first(),
  ).toBeVisible();
  await region.getByLabel('Tone Mapping').selectOption('mobius');
  const run = page.getByRole('button', { name: 'Run: AV1AN encode', exact: true });
  await expect(run).toBeEnabled();
  await run.click();
  await expect
    .poll(() =>
      page.evaluate(
        () =>
          (globalThis as any).__sourceCalls.find(
            (call: any) => call.command === 'mock_start_encode',
          )?.payload.request.settings,
      ),
    )
    .toMatchObject({
      encoder: 'aomAv1',
      hdr10Fallback: false,
      toneMap: { algorithm: 'mobius' },
    });
});

test('mixed AV1AN batch only starts the compatible file without fallback', async ({ page }) => {
  const region = await openAv1an(page, true, true);
  await region.getByLabel('Tone Mapping').selectOption('off');
  const run = page.getByRole('button', { name: 'Run Batch: AV1AN encode', exact: true });
  await expect(run).toBeEnabled();
  await run.click();
  await expect
    .poll(() =>
      page.evaluate(() =>
        (globalThis as any).__sourceCalls
          .filter((call: any) => call.command === 'mock_start_encode')
          .map((call: any) => call.payload.request.source.inputPath),
      ),
    )
    .toEqual(['/media/second.mp4']);
  await expect(
    page
      .getByText('Enable HDR10 fallback to discard Dolby Vision / HDR10+, or enable Tone Mapping.')
      .first(),
  ).toBeVisible();
});

test('Quick Convert retains its HDR10 fallback control', async ({ page }) => {
  await installSourceEncodeFixture(page);
  await page.evaluate((media) => (globalThis as any).__setSourceMedia(media), hdrSource);
  await page.getByRole('button', { name: 'Add Files…' }).click();
  await page.getByRole('button', { name: 'Load Tracks' }).click();
  await page.getByRole('button', { name: 'Quick Convert', exact: true }).click();
  const quick = page.getByRole('region', { name: 'Quick Convert', exact: true });
  await quick.getByLabel('Tone Mapping').selectOption('off');
  await expect(quick.getByLabel('HDR10 fallback (discard DV / HDR10+)')).toBeVisible();
});
