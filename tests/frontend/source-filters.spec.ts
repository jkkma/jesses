import { expect, test, type Locator, type Page } from '@playwright/test';
import { installSourceEncodeFixture, source } from './helpers/source-encode-fixture';

const preferencesKey = 'jesses.av1an.preferences.v1';
const progressive = {
  ...source,
  streams: source.streams.map((stream) =>
    stream.kind === 'video' ? { ...stream, fieldOrder: 'progressive' } : stream,
  ),
};
async function open(
  page: Page,
  workflow: 'Quick Convert' | 'AV1AN' = 'Quick Convert',
  batch = false,
) {
  await installSourceEncodeFixture(page);
  await page.evaluate(
    ({ first, batch }) => {
      const h = globalThis as any;
      h.__setSourceMedia(first);
      if (batch) {
        const second = { ...first, id: 'second', path: '/other/second.mkv', name: 'second.mkv' };
        h.__pickerPaths = [first.path, second.path];
        h.__mediaByPath = { [first.path]: first, [second.path]: second };
      }
    },
    { first: progressive, batch },
  );
  await page.getByRole('button', { name: 'Add Files…' }).click();
  if (batch) await page.getByRole('combobox', { name: 'Mode', exact: true }).selectOption('batch');
  else await page.getByRole('button', { name: 'Load Tracks' }).click();
  await page.getByRole('button', { name: workflow, exact: true }).click();
  return page.getByRole('region', { name: workflow, exact: true });
}
async function rows(region: Locator, values: string[]) {
  await region.getByRole('tab', { name: 'Advanced', exact: true }).click();
  const add = region
    .getByRole('heading', { name: 'CUSTOM VIDEO FILTERS', exact: true })
    .locator('..')
    .getByRole('button', { name: 'Add', exact: true });
  for (const [index, value] of values.entries()) {
    await add.click();
    await region.getByLabel(`Filter ${index + 1}`, { exact: true }).fill(value);
  }
}
async function request(page: Page, command = 'mock_start_encode', count = 1) {
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
      (globalThis as any).__sourceCalls.filter((call: any) => call.command === command).at(-1)
        .payload.request,
    command,
  );
}
const raw = [
  '  negate  ',
  '',
  'ab',
  'éé',
  '😀',
  '\u0085ab\u0085',
  "drawtext=text='C\\:\\clips\\one':x=2",
  ...Array.from({ length: 12 }, (_, i) => `hue=h=${i}`),
  '\ufeffab',
];
const expected = raw.filter((_, index) => ![1, 2, 3, 4, 5].includes(index));
for (const workflow of ['Quick Convert', 'AV1AN'] as const)
  test(`${workflow} submits arbitrary source strings in order and omits only source-short rows`, async ({
    page,
  }) => {
    const region = await open(page, workflow);
    await rows(region, raw);
    await page
      .getByRole('button', {
        name: workflow === 'AV1AN' ? 'Run: AV1AN encode' : 'Run: Encode',
        exact: true,
      })
      .click();
    expect((await request(page)).settings).toMatchObject({
      sourceFilters: true,
      av1anFilters: expected,
    });
    for (const [index, value] of raw.entries())
      await expect(region.getByLabel(`Filter ${index + 1}`, { exact: true })).toHaveValue(value);
  });
for (const codec of ['gif', 'png', 'jpg'])
  test(`${codec} source export includes the same raw picture-filter snapshot`, async ({ page }) => {
    const region = await open(page);
    await region.getByLabel('Codec', { exact: true }).selectOption(codec);
    await rows(region, [raw[0], raw[6], '']);
    await page.getByRole('button', { name: 'Run: Encode', exact: true }).click();
    expect((await request(page, 'mock_image_execution')).processing).toMatchObject({
      sourceFilters: true,
      filters: [raw[0], raw[6]],
    });
  });
test('empty source rows remain editable but do not enable filters, and implicit images omit picture processing', async ({
  page,
}) => {
  const region = await open(page);
  await rows(region, ['', '  ', 'ab', '😀']);
  await page.getByRole('button', { name: 'Run: Encode', exact: true }).click();
  const ordinary = await request(page);
  expect(ordinary.settings.av1anFilters).toBeUndefined();
  expect(ordinary.settings.sourceFilters).toBeUndefined();
  await region.getByRole('tab', { name: 'Video', exact: true }).click();
  await region.getByLabel('Codec', { exact: true }).selectOption('png');
  await page.getByRole('button', { name: 'Run: Encode', exact: true }).click();
  const image = await request(page, 'mock_image_execution');
  expect(image.processing?.filters).toBeUndefined();
  expect(image.processing?.sourceFilters).toBeUndefined();
  await region.getByRole('tab', { name: 'Advanced', exact: true }).click();
  await region.getByLabel('Filter 1', { exact: true }).fill('negate');
  await page.getByRole('button', { name: 'Track List', exact: true }).click();
  await page.getByRole('checkbox', { name: /Include .*Video/ }).uncheck();
  await page.getByRole('button', { name: 'Quick Convert', exact: true }).click();
  await page.getByRole('button', { name: 'Run: Encode', exact: true }).click();
  const implicit = await request(page, 'mock_image_execution', 2);
  expect(implicit.operation).toBe('implicitExport');
  expect(implicit.processing).toBeUndefined();
});
test('held software request keeps its filter snapshot while the editable draft changes', async ({
  page,
}) => {
  const region = await open(page);
  await rows(region, ['negate']);
  await page.evaluate(() => {
    const h = globalThis as any;
    h.__holdResolution = new Promise((resolve) => (h.__releaseResolution = resolve));
  });
  await page.keyboard.down('Shift');
  await page.getByRole('button', { name: 'Run: Encode', exact: true }).click();
  await region.getByLabel('Filter 1', { exact: true }).fill('hflip');
  await page.evaluate(() => {
    const h = globalThis as any;
    h.__releaseResolution();
    h.__holdResolution = null;
  });
  const dialog = page.getByRole('dialog', { name: 'Edit ffmpeg Arguments' });
  await expect(dialog).toBeVisible();
  await page.keyboard.up('Shift');
  await dialog.getByRole('button', { name: 'Run', exact: true }).click();
  expect((await request(page)).settings).toMatchObject({
    sourceFilters: true,
    av1anFilters: ['negate'],
  });
  await expect(region.getByLabel('Filter 1', { exact: true })).toHaveValue('hflip');
});
for (const image of [false, true])
  test(`${image ? 'image' : 'CPU'} batch freezes the source filters for every input before asynchronous preparation`, async ({
    page,
  }) => {
    const region = await open(page, 'Quick Convert', true);
    if (image) await region.getByLabel('Codec', { exact: true }).selectOption('png');
    await rows(region, ['negate']);
    await page.evaluate((image) => {
      const h = globalThis as any;
      h[image ? '__holdBatchResolution' : '__holdBatchPreview'] = new Promise(
        (resolve) => (h.__releaseFiltersBatch = resolve),
      );
    }, image);
    await page.getByRole('button', { name: 'Run Batch: Encode', exact: true }).click();
    await region.getByLabel('Filter 1', { exact: true }).fill('hflip');
    await page.evaluate(() => {
      const h = globalThis as any;
      h.__releaseFiltersBatch();
      h.__holdBatchResolution = null;
      h.__holdBatchPreview = null;
    });
    await request(page, image ? 'mock_image_execution' : 'mock_start_encode', 2);
    const requests = await page.evaluate(
      (command) =>
        (globalThis as any).__sourceCalls
          .filter((call: any) => call.command === command)
          .map((call: any) => call.payload.request),
      image ? 'mock_image_execution' : 'mock_start_encode',
    );
    expect(
      requests.map((value: any) =>
        image ? value.processing.filters : value.settings.av1anFilters,
      ),
    ).toEqual([['negate'], ['negate']]);
    expect(
      requests.every((value: any) =>
        image ? value.processing.sourceFilters : value.settings.sourceFilters,
      ),
    ).toBe(true);
  });
const saved = {
  version: 1,
  chunkMethod: 'lsmash',
  splitMethod: 'sceneDetection',
  chunkOrder: 'sequential',
  concatMethod: 'mkvmerge',
  encoderThreads: 3,
  sceneDetectionSlices: 2,
  workers: 2,
  workerBaseline: 4,
  filters: [
    '',
    "movie='C\\:\\clips\\long.mkv'",
    ...Array.from({ length: 17 }, (_, i) => `drawtext=text='${i}${'x'.repeat(4200)}'`),
  ],
  audio: { codec: 'flac', channels: 'mono', bitrateKbps: 128 },
  stripAudio: false,
  copySubs: false,
  copyData: true,
  copyAttachments: false,
};
async function seed(page: Page) {
  await page.addInitScript(
    ({ key, saved }) => {
      if (!sessionStorage.getItem('seeded-source-filters')) {
        localStorage.setItem(key, JSON.stringify(saved));
        sessionStorage.setItem('seeded-source-filters', '1');
      }
    },
    { key: preferencesKey, saved },
  );
}
test('source preferences retain empty rows, paths, long arrays and unrelated audio settings after editing and reload', async ({
  page,
}) => {
  await seed(page);
  const region = await open(page, 'AV1AN');
  await region.getByRole('tab', { name: 'Advanced', exact: true }).click();
  await expect(region.getByLabel('Filter 19', { exact: true })).toHaveValue(saved.filters[18]);
  await expect(region.getByLabel('Filter 1', { exact: true })).toHaveValue('');
  await expect(region.getByLabel('Filter 2', { exact: true })).toHaveValue(saved.filters[1]);
  await region.getByLabel('Filter 1', { exact: true }).fill('  ');
  await expect
    .poll(() =>
      page.evaluate((key) => JSON.parse(localStorage.getItem(key)!).filters[0], preferencesKey),
    )
    .toBe('  ');
  const persisted = await page.evaluate(
    (key) => JSON.parse(localStorage.getItem(key)!),
    preferencesKey,
  );
  expect(persisted.filters).toEqual(['  ', ...saved.filters.slice(1)]);
  expect(persisted).toMatchObject({
    audio: saved.audio,
    copySubs: false,
    copyData: true,
    copyAttachments: false,
    workerBaseline: 4,
  });
  await page.reload();
  await page.evaluate((media) => (globalThis as any).__setSourceMedia(media), progressive);
  await page.getByRole('button', { name: 'File List', exact: true }).click();
  await page.getByRole('button', { name: 'Add Files…' }).click();
  await page.getByRole('button', { name: 'Load Tracks' }).click();
  await page.getByRole('button', { name: 'AV1AN', exact: true }).click();
  const reopened = page.getByRole('region', { name: 'AV1AN', exact: true });
  await reopened.getByRole('tab', { name: 'Advanced', exact: true }).click();
  await expect(reopened.getByLabel('Filter 1', { exact: true })).toHaveValue('  ');
  await expect(reopened.getByLabel('Filter 19', { exact: true })).toHaveValue(saved.filters[18]);
});
test('storage quota failure reports the save failure and retains the usable draft and existing preferences', async ({
  page,
}) => {
  await seed(page);
  const region = await open(page, 'AV1AN');
  await region.getByRole('tab', { name: 'Advanced', exact: true }).click();
  const before = await page.evaluate((key) => localStorage.getItem(key), preferencesKey);
  await page.evaluate((key) => {
    const original = Storage.prototype.setItem;
    Storage.prototype.setItem = function (name, value) {
      if (name === key)
        throw new DOMException('Fixture storage quota exceeded', 'QuotaExceededError');
      return original.call(this, name, value);
    };
  }, preferencesKey);
  await region.getByLabel('Filter 1', { exact: true }).fill('negate');
  await expect(
    page.getByText(/AV1AN preferences could not be saved:.*Fixture storage quota exceeded/).first(),
  ).toBeVisible();
  expect(await page.evaluate((key) => localStorage.getItem(key), preferencesKey)).toBe(before);
  await page.getByRole('button', { name: 'Run: AV1AN encode', exact: true }).click();
  expect((await request(page)).settings.av1anFilters[0]).toBe('negate');
  await expect(region.getByLabel('Filter 1', { exact: true })).toHaveValue('negate');
});
