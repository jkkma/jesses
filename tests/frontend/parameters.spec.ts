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

test('AV1AN VP9 alpha retains the selected pixel format in its WEBM request', async ({ page }) => {
  await installSourceEncodeFixture(page);
  await page.getByRole('button', { name: 'Add Files…' }).click();
  await page.getByRole('button', { name: 'Load Tracks' }).click();
  await page.getByRole('button', { name: 'AV1AN', exact: true }).click();
  const av = page.getByRole('region', { name: 'AV1AN', exact: true });
  await av.getByLabel('Encoder', { exact: true }).selectOption('vpxStandalone');
  await av.getByLabel('Color Format').selectOption('yuva420p');
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
    .toMatchObject({ encoder: 'vpxStandalone', av1anOptions: { pixelFormat: 'yuva420p' } });
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
