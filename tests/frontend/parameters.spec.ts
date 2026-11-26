import { expect, test } from '@playwright/test';
import { installSourceEncodeFixture } from './helpers/source-encode-fixture';

test('AV1AN source argument grid freezes the selected encoder override before asynchronous resolution', async ({
  page,
}) => {
  await installSourceEncodeFixture(page);
  await page.getByRole('button', { name: 'Add Files…' }).click();
  await page.getByRole('button', { name: 'Load Tracks' }).click();
  await page.getByRole('button', { name: 'AV1AN', exact: true }).click();
  const av = page.getByRole('region', { name: 'AV1AN', exact: true });
  await av.getByLabel('Encoder', { exact: true }).selectOption('x264');
  await av.getByRole('tab', { name: 'Advanced' }).click();
  await av.getByLabel('tune value').fill('4');
  await page.evaluate(() => {
    const h = globalThis as any;
    h.__holdResolution = new Promise((resolve) => (h.__releaseResolution = resolve));
  });
  await page.getByRole('button', { name: 'Run: AV1AN encode', exact: true }).click();
  await expect(av.getByLabel('tune value')).toBeDisabled();
  await page.evaluate(() => (globalThis as any).__releaseResolution());
  await expect
    .poll(() =>
      page.evaluate(
        () =>
          (globalThis as any).__sourceCalls.find((c: any) => c.command === 'mock_start_encode')
            ?.payload.request.settings.parameters,
      ),
    )
    .toEqual([{ name: 'tune', value: '4' }]);
  await av.getByLabel('tune value').fill('2');
  await expect(av.getByLabel('tune value')).toHaveValue('2');
});

test('a late argument catalog cannot replace the newly selected encoder grid', async ({ page }) => {
  await installSourceEncodeFixture(page);
  await page.getByRole('button', { name: 'Add Files…' }).click();
  await page.getByRole('button', { name: 'Load Tracks' }).click();
  await page.getByRole('button', { name: 'AV1AN', exact: true }).click();
  const av = page.getByRole('region', { name: 'AV1AN', exact: true });
  await av.getByLabel('Encoder', { exact: true }).selectOption('x264');
  await page.evaluate(() => {
    (globalThis as any).__holdCatalogs = true;
  });
  await av.getByRole('tab', { name: 'Advanced' }).click();
  await expect
    .poll(() =>
      page.evaluate(
        () =>
          (globalThis as any).__pendingCatalogs?.some((p: any) => p.request.encoder === 'x264') ??
          false,
      ),
    )
    .toBe(true);
  await av.getByRole('tab', { name: 'Video', exact: true }).click();
  await av.getByLabel('Encoder', { exact: true }).selectOption('aomAv1');
  await av.getByRole('tab', { name: 'Advanced' }).click();
  await expect
    .poll(() =>
      page.evaluate(
        () =>
          (globalThis as any).__pendingCatalogs?.some((p: any) => p.request.encoder === 'aomAv1') ??
          false,
      ),
    )
    .toBe(true);
  await page.evaluate(() => {
    const pending = (globalThis as any).__pendingCatalogs;
    const result = (encoder: string, name: string) => ({
      encoder,
      backend: 'av1an',
      route: 'test',
      toolPath: '/bin/test',
      toolVersion: 'test',
      notes: [],
      parameters: [
        {
          name,
          label: name,
          argument: `--${name}`,
          minimum: 0,
          maximum: 5,
          valueKind: 'integer',
          minimumValue: '0',
          maximumValue: '5',
          choices: [],
          group: 'Arguments',
          description: name,
          example: '0',
        },
      ],
    });
    pending
      .find((p: any) => p.request.encoder === 'aomAv1')
      .resolve(result('aomAv1', 'new-option'));
    pending.find((p: any) => p.request.encoder === 'x264').resolve(result('x264', 'old-option'));
  });
  await expect(av.getByLabel('new-option value')).toBeVisible();
  await expect(av.getByLabel('old-option value')).toHaveCount(0);
});

test('AV1AN VP9 omits unsupported alpha format and retains supported pixel formats in WEBM', async ({
  page,
}) => {
  await installSourceEncodeFixture(page);
  await page.getByRole('button', { name: 'Add Files…' }).click();
  await page.getByRole('button', { name: 'Load Tracks' }).click();
  await page.getByRole('button', { name: 'AV1AN', exact: true }).click();
  const av = page.getByRole('region', { name: 'AV1AN', exact: true });
  await av.getByLabel('Encoder', { exact: true }).selectOption('vpxStandalone');
  await expect(av.getByLabel('Color Format').locator('option[value="yuva420p"]')).toHaveCount(0);
  await av.getByLabel('Color Format').selectOption('yuv444p10le');
  await av.getByLabel('Container').selectOption('webm');
  await page.getByRole('button', { name: 'Run: AV1AN encode', exact: true }).click();
  await expect
    .poll(() =>
      page.evaluate(
        () =>
          (globalThis as any).__sourceCalls.find((c: any) => c.command === 'mock_start_encode')
            ?.payload.request.settings,
      ),
    )
    .toMatchObject({ encoder: 'vpxStandalone', av1anOptions: { pixelFormat: 'yuv444p10le' } });
});

test('batch AV1AN freezes the source argument grid into each per-file request', async ({
  page,
}) => {
  await installSourceEncodeFixture(page);
  const first = {
    id: 'first',
    path: '/media/first.mkv',
    name: 'first.mkv',
    sizeBytes: '1000',
    durationSeconds: 12,
    format: 'matroska',
    streams: [
      {
        index: 0,
        kind: 'video',
        codec: 'h264',
        width: 640,
        height: 360,
        frameRate: '24/1',
        fieldOrder: 'progressive',
        sampleRate: null,
        channels: null,
        language: null,
        title: null,
      },
    ],
  };
  const second = { ...first, id: 'second', path: '/media/second.mkv', name: 'second.mkv' };
  await page.evaluate(
    ({ first, second }) => {
      const h = globalThis as any;
      h.__pickerPaths = [first.path, second.path];
      h.__mediaByPath = { [first.path]: first, [second.path]: second };
    },
    { first, second },
  );
  await page.getByRole('button', { name: 'Add Files…' }).click();
  await page.getByRole('combobox', { name: 'Mode', exact: true }).selectOption('batch');
  await page.getByRole('button', { name: 'AV1AN', exact: true }).click();
  const av = page.getByRole('region', { name: 'AV1AN', exact: true });
  await av.getByLabel('Encoder', { exact: true }).selectOption('x264');
  await av.getByRole('tab', { name: 'Advanced' }).click();
  await av.getByLabel('tune value').fill('4');
  await page.getByRole('button', { name: 'Run Batch: AV1AN encode', exact: true }).click();
  await expect
    .poll(() =>
      page.evaluate(
        () =>
          (globalThis as any).__sourceCalls.filter((c: any) => c.command === 'mock_start_encode')
            .length,
      ),
    )
    .toBe(2);
  const requests = await page.evaluate(() =>
    (globalThis as any).__sourceCalls
      .filter((c: any) => c.command === 'mock_start_encode')
      .map((c: any) => c.payload.request),
  );
  expect(requests.map((request: any) => request.source.inputPath)).toEqual([
    '/media/first.mkv',
    '/media/second.mkv',
  ]);
  for (const request of requests)
    expect(request.settings.parameters).toEqual([{ name: 'tune', value: '4' }]);
});

test('named presets save, apply and delete per encoder and route', async ({ page }) => {
  await installSourceEncodeFixture(page);
  await page.getByRole('button', { name: 'Add Files…' }).click();
  await page.getByRole('button', { name: 'Load Tracks' }).click();
  await page.getByRole('button', { name: 'AV1AN', exact: true }).click();
  const av = page.getByRole('region', { name: 'AV1AN', exact: true });
  await av.getByLabel('Encoder', { exact: true }).selectOption('x264');
  await av.getByRole('tab', { name: 'Advanced' }).click();
  const group = av.getByRole('group', { name: 'Saved presets' });
  const saved = group.getByLabel('Saved preset', { exact: true });
  await expect(saved).toBeDisabled();
  await av.getByLabel('tune value').fill('4');
  await group.getByRole('button', { name: 'Save as…' }).click();
  const dialog = page.getByRole('dialog', { name: 'Save preset' });
  await dialog.getByLabel('Preset name').fill('Anime');
  await dialog.getByRole('button', { name: 'Save', exact: true }).click();
  await expect(dialog).toHaveCount(0);
  await expect(saved).toHaveValue('Anime');
  expect(await page.evaluate(() => (globalThis as any).__parameterPresets)).toEqual([
    {
      name: 'Anime',
      encoder: 'x264',
      backend: 'av1an',
      parameters: [{ name: 'tune', value: '4' }],
    },
  ]);

  await av.getByLabel('tune value').fill('');
  await group.getByRole('button', { name: 'Apply', exact: true }).click();
  await expect(av.getByLabel('tune value')).toHaveValue('4');

  // A preset belongs to its encoder: another encoder does not offer it.
  await av.getByRole('tab', { name: 'Video', exact: true }).click();
  await av.getByLabel('Encoder', { exact: true }).selectOption('aomAv1');
  await av.getByRole('tab', { name: 'Advanced' }).click();
  await expect(saved.getByRole('option', { name: 'Anime' })).toHaveCount(0);

  await av.getByRole('tab', { name: 'Video', exact: true }).click();
  await av.getByLabel('Encoder', { exact: true }).selectOption('x264');
  await av.getByRole('tab', { name: 'Advanced' }).click();
  await saved.selectOption('Anime');
  await group.getByRole('button', { name: 'Delete', exact: true }).click();
  const confirmDelete = page.getByRole('dialog', { name: 'Delete saved preset?' });
  await expect(confirmDelete.getByRole('button', { name: 'Keep preset' })).toBeFocused();
  await confirmDelete.getByRole('button', { name: 'Keep preset' }).click();
  await expect(saved.getByRole('option', { name: 'Anime' })).toHaveCount(1);
  await group.getByRole('button', { name: 'Delete', exact: true }).click();
  await confirmDelete.getByRole('button', { name: 'Delete preset' }).click();
  await expect(saved.getByRole('option', { name: 'Anime' })).toHaveCount(0);
  expect(await page.evaluate(() => (globalThis as any).__parameterPresets)).toEqual([]);
});

test('AV1AN exposes HDR10 fallback on HDR sources and unblocks MKV when Copy Subtitles is off', async ({
  page,
}) => {
  await installSourceEncodeFixture(page);
  await page.evaluate(() => {
    (globalThis as any).__setSourceMedia({
      id: 'hdr-movtext',
      path: '/media/hdr.mp4',
      name: 'hdr.mp4',
      sizeBytes: '4000',
      durationSeconds: 30,
      format: 'mov,mp4,m4a,3gp,3g2,mj2',
      streams: [
        {
          index: 0,
          kind: 'video',
          codec: 'hevc',
          width: 3840,
          height: 2160,
          frameRate: '24000/1001',
          pixelFormat: 'yuv420p10le',
          colorPrimaries: 'bt2020',
          colorTransfer: 'smpte2084',
          colorSpace: 'bt2020nc',
          colorRange: 'tv',
          hdrFormat: 'Dolby Vision',
          dynamicHdrFormats: ['Dolby Vision', 'HDR10+'],
          dolbyVisionProfile: 8,
        },
        {
          index: 1,
          kind: 'audio',
          codec: 'aac',
          sampleRate: 48000,
          channels: 2,
          language: 'eng',
          title: 'English',
        },
        {
          index: 2,
          kind: 'subtitle',
          codec: 'mov_text',
          language: 'eng',
          title: 'Subtitles',
        },
      ],
    });
  });
  await page.getByRole('button', { name: 'Add Files…' }).click();
  await page.getByRole('button', { name: 'Load Tracks' }).click();
  await page.getByRole('button', { name: 'AV1AN', exact: true }).click();
  const av = page.getByRole('region', { name: 'AV1AN', exact: true });
  await av.getByLabel('Container').selectOption('matroska');
  const runButton = page.getByRole('button', { name: 'Run: AV1AN encode', exact: true });
  // MKV cannot store mov_text while Copy Subtitles is on.
  await expect(runButton).toBeDisabled();
  await av.getByRole('tab', { name: 'Audio & Tracks' }).click();
  await av.getByLabel('Subtitles', { exact: true }).uncheck();
  await expect(runButton).toBeEnabled();

  await av.getByRole('tab', { name: 'Video', exact: true }).click();
  await av.getByLabel('Tone Mapping').selectOption('off');
  const fallback = av.getByLabel('HDR10 fallback (discard DV / HDR10+)');
  await expect(fallback).toBeVisible();
  await fallback.check();
  await runButton.click();
  await expect
    .poll(() =>
      page.evaluate(
        () =>
          (globalThis as any).__sourceCalls.find((c: any) => c.command === 'mock_start_encode')
            ?.payload.request.settings,
      ),
    )
    .toMatchObject({ hdr10Fallback: true });
});
