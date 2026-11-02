import { expect, test } from '@playwright/test';
import { installSourceEncodeFixture, source } from './helpers/source-encode-fixture';

// The fork exposes full-source PNG/JPEG sequences in Quick Convert, rather
// than the former Jesses-only Utilities still-frame exporter. Preserve the
// pixel-format request assertions on the source's actual controls.
test('PNG sequence sends RGBA 16-bit and JPEG uses its own YUV format', async ({ page }) => {
  await installSourceEncodeFixture(page);
  await page.getByRole('button', { name: 'Add Files…' }).click();
  await page.getByRole('button', { name: 'Load Tracks' }).click();
  await page.getByRole('button', { name: 'Quick Convert', exact: true }).click();
  const quick = page.getByRole('region', { name: 'Quick Convert', exact: true });

  await quick.getByLabel('Codec', { exact: true }).selectOption('png');
  await quick.getByLabel('Color Format').selectOption('rgba64');
  await quick.getByLabel('Output Path', { exact: true }).fill('/media/png-frames');
  await page.getByRole('button', { name: 'Run: Encode', exact: true }).click();
  await expect
    .poll(() =>
      page.evaluate(
        () =>
          (globalThis as any).__sourceCalls.filter(
            (call: any) => call.command === 'mock_image_execution',
          ).length,
      ),
    )
    .toBe(1);
  let calls = await page.evaluate(() =>
    (globalThis as any).__sourceCalls.filter(
      (call: any) => call.command === 'mock_image_execution',
    ),
  );
  expect(calls[0].payload.request).toMatchObject({
    operation: 'export',
    inputPath: '/media/input.mkv',
    streamIndex: 0,
    format: 'pngSequence',
    fullSource: true,
    outputPath: '/media/png-frames',
    pixelFormat: 'rgba64',
  });

  await expect(page.getByRole('button', { name: 'Run: Encode', exact: true })).toBeEnabled();
  await quick.getByLabel('Codec', { exact: true }).selectOption('jpg');
  const color = quick.getByLabel('Color Format');
  await expect(color.locator('option').allTextContents()).resolves.toEqual([
    'YUV - 4:2:0 - 8 bit',
    'YUV - 4:2:2 - 8 bit',
    'YUV - 4:4:4 - 8 bit',
  ]);
  await quick.getByLabel('Output Path', { exact: true }).fill('/media/jpeg-frames');
  await page.getByRole('button', { name: 'Run: Encode', exact: true }).click();
  await expect
    .poll(() =>
      page.evaluate(
        () =>
          (globalThis as any).__sourceCalls.filter(
            (call: any) => call.command === 'mock_image_execution',
          ).length,
      ),
    )
    .toBe(2);
  calls = await page.evaluate(() =>
    (globalThis as any).__sourceCalls.filter(
      (call: any) => call.command === 'mock_image_execution',
    ),
  );
  expect(calls[1].payload.request).toMatchObject({
    operation: 'export',
    format: 'jpegSequence',
    fullSource: true,
    outputPath: '/media/jpeg-frames',
    pixelFormat: 'yuv420p',
    quality: 3,
  });
  expect(calls[1].payload.request.pixelFormat).not.toBe('rgba64');
});

test('explicit image export freezes all input identities and source metadata before resolution', async ({
  page,
}) => {
  await installSourceEncodeFixture(page);
  const first = { ...source, title: 'Original', language: 'eng' };
  const donor = { ...source, id: 'donor', path: '/other/donor.mkv', name: 'donor.mkv' };
  await page.evaluate(
    ({ first, donor }) => {
      const h = globalThis as any;
      h.__pickerPaths = [first.path, donor.path];
      h.__mediaByPath = { [first.path]: first, [donor.path]: donor };
    },
    { first, donor },
  );
  await page.getByRole('button', { name: 'Add Files…' }).click();
  await page.getByRole('option', { name: /input.mkv/ }).click();
  await page.getByRole('button', { name: 'Load Tracks' }).click();
  await page.getByRole('button', { name: 'Quick Convert', exact: true }).click();
  const quick = page.getByRole('region', { name: 'Quick Convert', exact: true });
  await quick.getByLabel('Codec', { exact: true }).selectOption('png');
  await quick.getByRole('tab', { name: 'Metadata' }).click();
  await quick.getByLabel('Copy Metadata From').selectOption('/other/donor.mkv');
  await quick.getByLabel('Copy Chapters From').selectOption('');
  await quick.getByLabel('Title for output file').fill('  Image title  ');
  await quick.getByLabel('Language for output file').fill(' jpn ');
  await quick.getByLabel('Title for track 1').fill('Dub');
  await quick.getByLabel('Output Path', { exact: true }).fill('/media/images');
  await page.evaluate(() => {
    const h = globalThis as any;
    h.__holdResolution = new Promise((resolve) => (h.__releaseResolution = resolve));
  });
  await page.getByRole('button', { name: 'Run: Encode', exact: true }).click();
  await quick.getByLabel('Title for output file').fill('New draft');
  await page.evaluate(() => (globalThis as any).__releaseResolution());
  await expect
    .poll(() =>
      page.evaluate(
        () =>
          (globalThis as any).__sourceCalls.find((c: any) => c.command === 'mock_image_execution')
            ?.payload.request,
      ),
    )
    .toBeTruthy();
  const request = await page.evaluate(
    () =>
      (globalThis as any).__sourceCalls.find((c: any) => c.command === 'mock_image_execution')
        .payload.request,
  );
  expect(request.processing.metadata).toEqual({
    inputs: ['/media/input.mkv', '/other/donor.mkv'],
    options: {
      sourceInput: 1,
      attachmentInputs: [],
      title: 'Image title',
      language: 'jpn',
      tracks: [
        { title: '', language: '' },
        { title: 'Dub', language: 'eng' },
        { title: 'Subtitles', language: 'eng' },
      ],
      audioDispositions: [true],
      subtitleDispositions: [true],
    },
  });
  expect(request.processing.metadata.options.chaptersInput).toBeUndefined();
  await expect(quick.getByLabel('Title for output file')).toHaveValue('New draft');
  await quick.getByLabel('Apply the titles and languages below').uncheck();
  await page.getByRole('button', { name: 'Run: Encode', exact: true }).click();
  await expect
    .poll(() =>
      page.evaluate(
        () =>
          (globalThis as any).__sourceCalls.filter((c: any) => c.command === 'mock_image_execution')
            .length,
      ),
    )
    .toBe(2);
  const secondRequest = await page.evaluate(
    () =>
      (globalThis as any).__sourceCalls.filter((c: any) => c.command === 'mock_image_execution')[1]
        .payload.request,
  );
  expect(secondRequest.processing.metadata.options.tracks).toEqual([]);
  expect(secondRequest.processing.metadata.options.title).toBeUndefined();
  expect(secondRequest.processing.metadata.options.language).toBeUndefined();
});

test('image batch freezes per-source metadata donors and suppresses the editable grid', async ({
  page,
}) => {
  await installSourceEncodeFixture(page);
  const second = { ...source, id: 'second', path: '/other/second.mkv', name: 'second.mkv' };
  await page.evaluate(
    ({ first, second }) => {
      const h = globalThis as any;
      h.__pickerPaths = [first.path, second.path];
      h.__mediaByPath = { [first.path]: first, [second.path]: second };
    },
    { first: source, second },
  );
  await page.getByRole('button', { name: 'Add Files…' }).click();
  await page.getByRole('option', { name: /input.mkv/ }).click();
  await page.getByRole('button', { name: 'Load Tracks' }).click();
  await page.getByRole('button', { name: 'Quick Convert', exact: true }).click();
  const quick = page.getByRole('region', { name: 'Quick Convert', exact: true });
  await quick.getByLabel('Codec', { exact: true }).selectOption('png');
  await quick.getByRole('tab', { name: 'Metadata' }).click();
  await quick.getByLabel('Title for output file').fill('Not a batch override');
  await page.getByRole('button', { name: 'File List', exact: true }).click();
  await page.getByRole('combobox', { name: 'Mode', exact: true }).selectOption('batch');
  await page.getByRole('button', { name: 'Quick Convert', exact: true }).click();
  await quick.getByRole('tab', { name: 'Metadata' }).click();
  await expect(quick.getByLabel('Copy Metadata From')).toHaveValue('current');
  await quick.getByLabel('Copy Chapters From').selectOption('');
  await page.getByRole('button', { name: 'Run Batch: Encode', exact: true }).click();
  await expect
    .poll(() =>
      page.evaluate(
        () =>
          (globalThis as any).__sourceCalls.filter((c: any) => c.command === 'mock_image_execution')
            .length,
      ),
    )
    .toBe(2);
  const requests = await page.evaluate(() =>
    (globalThis as any).__sourceCalls
      .filter((c: any) => c.command === 'mock_image_execution')
      .map((c: any) => c.payload.request),
  );
  for (const request of requests) {
    expect(request.processing.metadata).toEqual({
      inputs: [request.inputPath],
      options: {
        sourceInput: 0,
        attachmentInputs: [],
        tracks: [],
        audioDispositions: [true],
        subtitleDispositions: [true],
      },
    });
  }
  await quick.getByLabel('Copy Metadata From').selectOption('');
  await quick.getByLabel('Copy Chapters From').selectOption('current');
  await page.getByRole('button', { name: 'Run Batch: Encode', exact: true }).click();
  await expect
    .poll(() =>
      page.evaluate(
        () =>
          (globalThis as any).__sourceCalls.filter((c: any) => c.command === 'mock_image_execution')
            .length,
      ),
    )
    .toBe(4);
  const secondRun = await page.evaluate(() =>
    (globalThis as any).__sourceCalls
      .filter((c: any) => c.command === 'mock_image_execution')
      .slice(2)
      .map((c: any) => c.payload.request),
  );
  for (const request of secondRun) {
    expect(request.processing.metadata).toEqual({
      inputs: [request.inputPath],
      options: {
        chaptersInput: 0,
        attachmentInputs: [],
        tracks: [],
        audioDispositions: [true],
        subtitleDispositions: [true],
      },
    });
  }
});

test('shared footer follows image and video codec plus mux/batch mode transitions', async ({
  page,
}) => {
  await installSourceEncodeFixture(page);
  await page.getByRole('button', { name: 'Add Files…' }).click();
  await page.getByRole('button', { name: 'Load Tracks' }).click();
  await page.getByRole('button', { name: 'Quick Convert', exact: true }).click();
  const quick = page.getByRole('region', { name: 'Quick Convert', exact: true });
  await expect(page.getByRole('button', { name: 'Run: Encode', exact: true })).toBeVisible();
  await quick.getByLabel('Codec', { exact: true }).selectOption('png');
  await page.getByRole('button', { name: 'File List', exact: true }).click();
  await page.getByRole('combobox', { name: 'Mode', exact: true }).selectOption('batch');
  await page.getByRole('button', { name: 'Quick Convert', exact: true }).click();
  await expect(page.getByRole('button', { name: 'Run Batch: Encode', exact: true })).toBeVisible();
  await quick.getByLabel('Codec', { exact: true }).selectOption('svtAv1');
  await expect(page.getByRole('button', { name: 'Run Batch: Encode', exact: true })).toBeVisible();
  await page.getByRole('button', { name: 'File List', exact: true }).click();
  await page.getByRole('combobox', { name: 'Mode', exact: true }).selectOption('mux');
  await page.getByRole('button', { name: 'Quick Convert', exact: true }).click();
  await expect(page.getByRole('button', { name: 'Run: Encode', exact: true })).toBeVisible();
});
