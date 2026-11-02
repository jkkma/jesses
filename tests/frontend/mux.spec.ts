import { expect, test } from '@playwright/test';
import { installSourceEncodeFixture } from './helpers/source-encode-fixture';

test('selected tracks with colliding indices retain independent metadata, order and source owners', async ({
  page,
}) => {
  await installSourceEncodeFixture(page);
  const first = {
    id: 'picture',
    path: '/media/picture.mkv',
    name: 'picture.mkv',
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
        sampleRate: null,
        channels: null,
        language: null,
        title: 'Picture',
      },
    ],
  };
  const second = {
    id: 'sound',
    path: '/media/voice.mka',
    name: 'voice.mka',
    sizeBytes: '1000',
    durationSeconds: 12,
    format: 'matroska',
    streams: [
      {
        index: 0,
        kind: 'audio',
        codec: 'aac',
        width: null,
        height: null,
        frameRate: null,
        sampleRate: 48000,
        channels: 2,
        language: 'eng',
        title: 'Voice',
      },
    ],
  };
  await page.evaluate(
    ({ first, second }) => {
      const host = globalThis as any;
      host.__pickerPaths = [first.path, second.path];
      host.__mediaByPath = { [first.path]: first, [second.path]: second };
    },
    { first, second },
  );
  await page.getByRole('button', { name: 'Add Files…' }).click();
  await page.getByRole('option', { name: /picture.mkv/ }).click();
  await page.getByRole('option', { name: /voice.mka/ }).click({ modifiers: ['Control'] });
  await page.getByRole('button', { name: 'Load Tracks' }).click();
  const tracks = page.getByRole('region', { name: 'Track List' });
  await tracks
    .getByRole('listbox', { name: 'Tracks' })
    .getByRole('option', { name: /Voice/ })
    .click();
  await tracks.getByRole('button', { name: 'Move track up' }).click();
  await page.getByRole('button', { name: 'Quick Convert', exact: true }).click();
  const quick = page.getByRole('region', { name: 'Quick Convert', exact: true });
  await quick.getByLabel('Codec', { exact: true }).selectOption('copy');
  await quick.getByRole('tab', { name: 'Audio' }).click();
  await quick.getByLabel('Codec', { exact: true }).selectOption('copy');
  await quick.getByRole('tab', { name: 'Metadata' }).click();
  await quick.getByLabel('Copy Metadata From').selectOption('/media/voice.mka');
  await quick.getByLabel('Copy Chapters From').selectOption('/media/picture.mkv');
  const voice = quick.getByRole('row').filter({ hasText: 'voice.mka #0' });
  await voice.getByLabel('Title for track 0').fill("Dub's ${title} 日本語");
  await voice.getByLabel('Language for track 0').fill('spa');
  await quick.getByLabel('Output Path', { exact: true }).fill('/media/combined');
  await page.getByRole('button', { name: 'Run: Encode', exact: true }).click();
  await expect
    .poll(() =>
      page.evaluate(
        () =>
          (globalThis as any).__sourceCalls.find((c: any) => c.command === 'mock_start_encode')
            ?.payload.request,
      ),
    )
    .toBeTruthy();
  const request = await page.evaluate(
    () =>
      (globalThis as any).__sourceCalls.find((c: any) => c.command === 'mock_start_encode').payload
        .request,
  );
  expect(request.source).toMatchObject({
    inputPath: '/media/picture.mkv',
    outputPath: '/media/combined.mkv',
    streamIndices: [0],
  });
  expect(request.settings.externalTracks).toContainEqual(
    expect.objectContaining({
      inputPath: '/media/voice.mka',
      streamIndex: 0,
      title: "Dub's ${title} 日本語",
      language: 'spa',
    }),
  );
  expect(request.settings.trackOrder).toEqual([
    { inputPath: '/media/voice.mka', streamIndex: 0 },
    { streamIndex: 0 },
  ]);
  expect(request.settings).toMatchObject({
    metadataSourcePath: '/media/voice.mka',
    chaptersSourcePath: '/media/picture.mkv',
  });
  await page.getByRole('button', { name: 'File List', exact: true }).click();
  await page.getByRole('option', { name: /picture.mkv/ }).click();
  await page.getByRole('button', { name: 'Remove', exact: true }).click();
  await page.getByRole('button', { name: 'Quick Convert', exact: true }).click();
  await quick.getByRole('tab', { name: 'Metadata' }).click();
  await expect(
    quick.getByRole('row').filter({ hasText: 'voice.mka #0' }).getByLabel('Title for track 0'),
  ).toHaveValue("Dub's ${title} 日本語");
});

test('copy maps multiple checked primary and external video tracks in source order', async ({
  page,
}) => {
  await installSourceEncodeFixture(page);
  const first = {
    id: 'picture',
    path: '/media/picture.mkv',
    name: 'picture.mkv',
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
        title: 'Main picture',
      },
      {
        index: 1,
        kind: 'video',
        codec: 'h264',
        width: 640,
        height: 360,
        frameRate: '24/1',
        fieldOrder: 'progressive',
        sampleRate: null,
        channels: null,
        language: null,
        title: 'Alternate picture',
      },
    ],
  };
  const second = {
    ...first,
    id: 'external',
    path: '/media/external.mkv',
    name: 'external.mkv',
    streams: [{ ...first.streams[0], title: 'External picture' }],
  };
  await page.evaluate(
    (inputs) => {
      const h = globalThis as any;
      h.__pickerPaths = inputs.map((item) => item.path);
      h.__mediaByPath = Object.fromEntries(inputs.map((item) => [item.path, item]));
    },
    [first, second],
  );
  await page.getByRole('button', { name: 'Add Files…' }).click();
  await page.getByRole('option', { name: /picture.mkv/ }).click();
  await page.getByRole('option', { name: /external.mkv/ }).click({ modifiers: ['Control'] });
  await page.getByRole('button', { name: 'Load Tracks' }).click();
  await page.getByRole('checkbox', { name: /Include .*External picture/ }).check();
  await page.getByRole('button', { name: 'Quick Convert', exact: true }).click();
  const quick = page.getByRole('region', { name: 'Quick Convert', exact: true });
  await expect(
    quick.getByText('This encoder supports one selected video track per encode.'),
  ).toBeVisible();
  await quick.getByLabel('Codec', { exact: true }).selectOption('copy');
  await page.getByRole('button', { name: 'Run: Encode' }).click();
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
  expect(request.source.streamIndices).toEqual([0, 1]);
  expect(request.settings.externalTracks).toContainEqual(
    expect.objectContaining({ inputPath: second.path, streamIndex: 0 }),
  );
  expect(request.settings.trackOrder).toEqual([
    { streamIndex: 0 },
    { streamIndex: 1 },
    { inputPath: second.path, streamIndex: 0 },
  ]);
});

test('Strip Video ignores checked external video before validating a retained subtitle', async ({
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
        sampleRate: null,
        channels: null,
        language: null,
        title: 'Main picture',
      },
      {
        index: 1,
        kind: 'subtitle',
        codec: 'subrip',
        width: null,
        height: null,
        frameRate: null,
        sampleRate: null,
        channels: null,
        language: 'eng',
        title: 'Caption',
      },
    ],
  };
  const second = {
    ...first,
    id: 'external',
    path: '/media/external.mkv',
    name: 'external.mkv',
    streams: [{ ...first.streams[0], title: 'External picture' }],
  };
  await page.evaluate(
    (inputs) => {
      const h = globalThis as any;
      h.__pickerPaths = inputs.map((item) => item.path);
      h.__mediaByPath = Object.fromEntries(inputs.map((item) => [item.path, item]));
    },
    [first, second],
  );
  await page.getByRole('button', { name: 'Add Files…' }).click();
  await page.getByRole('option', { name: /first.mkv/ }).click();
  await page.getByRole('option', { name: /external.mkv/ }).click({ modifiers: ['Control'] });
  await page.getByRole('button', { name: 'Load Tracks' }).click();
  await page.getByRole('checkbox', { name: /Include .*External picture/ }).check();
  await page.getByRole('button', { name: 'Quick Convert', exact: true }).click();
  await page
    .getByRole('region', { name: 'Quick Convert', exact: true })
    .getByLabel('Codec', { exact: true })
    .selectOption('drop');
  await expect(page.getByRole('button', { name: 'Run: Encode' })).toBeEnabled();
  await page.getByRole('button', { name: 'Run: Encode' }).click();
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
  expect(request.source.streamIndices).toEqual([1]);
  expect(request.settings.externalTracks ?? []).toEqual([]);
  expect(request.settings.trackOrder).toEqual([{ streamIndex: 1 }]);
});

test('MOV maps checked external data and drops an incompatible attachment before ordering metadata', async ({
  page,
}) => {
  await installSourceEncodeFixture(page);
  const base = {
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
    title: 'Picture',
  };
  const first = {
    id: 'first',
    path: '/media/first.mkv',
    name: 'first.mkv',
    sizeBytes: '1000',
    durationSeconds: 12,
    format: 'matroska',
    streams: [
      base,
      { ...base, index: 2, kind: 'attachment', codec: 'ttf', title: 'Font' },
      {
        ...base,
        index: 3,
        kind: 'audio',
        codec: 'aac',
        channels: 2,
        sampleRate: 48000,
        title: 'Audio',
      },
    ],
  };
  const second = {
    ...first,
    id: 'external',
    path: '/media/data.mov',
    name: 'data.mov',
    streams: [
      { ...base, index: 0, kind: 'data', codec: null, codecTag: 'gpmd', title: 'Telemetry' },
    ],
  };
  await page.evaluate(
    (inputs) => {
      const h = globalThis as any;
      h.__pickerPaths = inputs.map((item) => item.path);
      h.__mediaByPath = Object.fromEntries(inputs.map((item) => [item.path, item]));
    },
    [first, second],
  );
  await page.getByRole('button', { name: 'Add Files…' }).click();
  await page.getByRole('option', { name: /first.mkv/ }).click();
  await page.getByRole('option', { name: /data.mov/ }).click({ modifiers: ['Control'] });
  await page.getByRole('button', { name: 'Load Tracks' }).click();
  await page.getByRole('button', { name: 'Quick Convert', exact: true }).click();
  const quick = page.getByRole('region', { name: 'Quick Convert', exact: true });
  await quick.getByLabel('Codec', { exact: true }).selectOption('copy');
  await quick.getByLabel('Container').selectOption('mov');
  await quick.getByRole('tab', { name: 'Metadata' }).click();
  await quick
    .getByRole('row')
    .filter({ hasText: 'first.mkv #3' })
    .getByLabel('Title for track 3')
    .fill('Main audio');
  await quick
    .getByRole('row')
    .filter({ hasText: 'data.mov #0' })
    .getByLabel('Title for track 0')
    .fill('GPS');
  await page.getByRole('button', { name: 'Run: Encode' }).click();
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
  expect(request.source.streamIndices).toEqual([0, 3]);
  expect(request.settings.trackOrder).toEqual([
    { streamIndex: 0 },
    { streamIndex: 3 },
    { inputPath: second.path, streamIndex: 0 },
  ]);
  expect(request.settings.trackOverrides).toContainEqual(
    expect.objectContaining({ streamIndex: 3, title: 'Main audio' }),
  );
  expect(request.settings.externalTracks).toContainEqual(
    expect.objectContaining({ inputPath: second.path, streamIndex: 0, title: 'GPS' }),
  );
  await expect(page.getByRole('log', { name: 'Recent activity' })).toContainText(
    '1 attachment track left out: MOV cannot store attachment streams.',
  );
});
