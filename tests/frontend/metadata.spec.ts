import { expect, test } from '@playwright/test';
import { installSourceEncodeFixture, source } from './helpers/source-encode-fixture';

// The old expandable metadata inspector is not in the fork. Keep the source's
// track identities/descriptions and editable title/language request assertions;
// detailed probe metadata remains covered by native media/probe tests.
test('Track List describes source streams and metadata edits retain their indices', async ({
  page,
}) => {
  await installSourceEncodeFixture(page);
  const video = {
    ...source.streams[0],
    index: 0,
    width: 720,
    height: 480,
    frameRate: '24000/1001',
    language: 'eng',
    title: 'Main feature',
  };
  const audio = {
    ...source.streams[1],
    index: 2,
    codec: 'dts',
    channelLayout: '5.1(side)',
    language: 'eng',
    title: 'Commentary',
  };
  const subtitle = {
    ...source.streams[2],
    index: 4,
    codec: 'hdmv_pgs_subtitle',
    language: 'jpn',
    title: 'Japanese',
  };
  const unknown = {
    ...source.streams[2],
    index: 6,
    codec: 'vendor_subtitle',
    language: null,
    title: null,
  };
  const attachment = {
    ...source.streams[2],
    index: 7,
    kind: 'attachment',
    codec: 'ttf',
    attachmentFilename: 'captions-font.ttf',
    language: null,
    title: null,
  };
  const fixture = {
    ...source,
    sizeBytes: '9007199254740993',
    streams: [video, audio, subtitle, unknown, attachment],
  };
  await page.evaluate((media) => (globalThis as any).__setSourceMedia(media), fixture);
  await page.getByRole('button', { name: 'Add Files…' }).click();
  await page.getByRole('button', { name: 'Load Tracks' }).click();
  const tracks = page.getByRole('region', { name: 'Track List' });
  await expect(
    tracks.getByRole('option', { name: /Video \(H264\).*720x480.*Main feature/ }),
  ).toBeVisible();
  await expect(tracks.getByRole('option', { name: /Audio \(DTS\).*Commentary/ })).toBeVisible();
  await expect(tracks.getByRole('option', { name: /Subtitles \(PGS\).*Japanese/ })).toBeVisible();
  await expect(tracks.getByRole('option', { name: /vendor_subtitle/ })).toBeVisible();
  await expect(tracks.getByRole('option', { name: /captions-font.ttf/ })).toBeVisible();
  await page.getByRole('button', { name: 'Quick Convert', exact: true }).click();
  const quick = page.getByRole('region', { name: 'Quick Convert', exact: true });
  await quick.getByRole('tab', { name: 'Metadata' }).click();
  await quick.getByLabel('Title for track 2').fill('Director commentary');
  await quick.getByLabel('Language for track 2').fill('fra');
  await quick.getByLabel('Output Path', { exact: true }).fill('/media/metadata');
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
  expect(request.source.streamIndices).toEqual([0, 2, 4, 6, 7]);
  expect(request.settings.trackOverrides).toContainEqual(
    expect.objectContaining({ streamIndex: 2, title: 'Director commentary', language: 'fra' }),
  );
});

test('audio-only input disables thumbnail requests on Track List', async ({ page }) => {
  await installSourceEncodeFixture(page);
  const fixture = {
    ...source,
    id: 'audio-only',
    path: '/media/voice.m4a',
    name: 'voice.m4a',
    streams: source.streams.filter((stream) => stream.kind === 'audio'),
  };
  await page.evaluate((media) => (globalThis as any).__setSourceMedia(media), fixture);
  await page.getByRole('button', { name: 'Add Files…' }).click();
  await page.getByRole('button', { name: 'Load Tracks' }).click();
  await expect(page.getByRole('button', { name: 'Show next thumbnail' })).toBeDisabled();
  expect(
    await page.evaluate(
      () =>
        (globalThis as any).__sourceCalls.filter((c: any) => c.command === 'preview_frame').length,
    ),
  ).toBe(0);
});

test('Output File metadata starts from the loaded file, retains edits across tabs and resets after removal', async ({
  page,
}) => {
  if (process.env.JESSES_CAPTURE_UI) await page.setViewportSize({ width: 800, height: 720 });
  await installSourceEncodeFixture(page);
  const first = { ...source, title: 'Original title', language: 'eng' };
  const second = {
    ...source,
    id: 'second',
    path: '/other/second.mkv',
    name: 'second.mkv',
    title: 'Second title',
    language: 'jpn',
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
  await page.getByRole('option', { name: /input.mkv/ }).click();
  await page.getByRole('button', { name: 'Load Tracks' }).click();
  await page.getByRole('button', { name: 'Quick Convert', exact: true }).click();
  const quick = page.getByRole('region', { name: 'Quick Convert', exact: true });
  await quick.getByRole('tab', { name: 'Metadata' }).click();
  const title = quick.getByLabel('Title for output file');
  const language = quick.getByLabel('Language for output file');
  await expect(title).toHaveValue('Original title');
  await expect(language).toHaveValue('eng');
  const sizes = await title.evaluate((element) => {
    const row = element.closest('tr')!;
    return {
      title: element.getBoundingClientRect().width,
      language: row
        .querySelector<HTMLInputElement>('[aria-label="Language for output file"]')!
        .getBoundingClientRect().width,
      scrolls: document.documentElement.scrollWidth > innerWidth,
    };
  });
  expect(sizes.language).toBeLessThanOrEqual(100);
  expect(sizes.title).toBeGreaterThan(sizes.language * 2);
  expect(sizes.scrolls).toBe(false);
  if (process.env.JESSES_CAPTURE_UI)
    await page.screenshot({
      path: 'target/migration-validation/frontend/metadata-output-file-800.png',
    });
  await title.fill('Edited title');
  await language.fill('spa');
  await quick.getByLabel('Apply the titles and languages below').uncheck();
  await expect(title).toBeDisabled();
  await quick.getByRole('tab', { name: 'Video', exact: true }).click();
  await quick.getByRole('tab', { name: 'Metadata' }).click();
  await quick.getByLabel('Apply the titles and languages below').check();
  await expect(title).toHaveValue('Edited title');
  await expect(language).toHaveValue('spa');
  await page.getByRole('button', { name: 'File List', exact: true }).click();
  await page.getByRole('option', { name: /input.mkv/ }).click();
  await page.getByRole('button', { name: 'Remove', exact: true }).click();
  await page.getByRole('button', { name: 'Quick Convert', exact: true }).click();
  await quick.getByRole('tab', { name: 'Metadata' }).click();
  await expect(title).toHaveValue('Second title');
  await expect(language).toHaveValue('jpn');
  await page.getByRole('button', { name: 'File List', exact: true }).click();
  await page.getByRole('option', { name: /second.mkv/ }).click();
  await page.getByRole('button', { name: 'Remove', exact: true }).click();
  await page.evaluate((first) => {
    const host = globalThis as any;
    host.__pickerPaths = [first.path];
    host.__mediaByPath[first.path] = first;
  }, first);
  await page.getByRole('button', { name: 'Add Files…' }).click();
  await page.getByRole('button', { name: 'Load Tracks' }).click();
  await page.getByRole('button', { name: 'Quick Convert', exact: true }).click();
  await quick.getByRole('tab', { name: 'Metadata' }).click();
  await expect(title).toHaveValue('Original title');
  await expect(language).toHaveValue('eng');
});

test('source None donors and Output File clear values map separately from unchecked Apply', async ({
  page,
}) => {
  await installSourceEncodeFixture(page);
  const media = { ...source, title: 'Original', language: 'eng' };
  await page.evaluate((media) => (globalThis as any).__setSourceMedia(media), media);
  await page.getByRole('button', { name: 'Add Files…' }).click();
  await page.getByRole('button', { name: 'Load Tracks' }).click();
  await page.getByRole('button', { name: 'Quick Convert', exact: true }).click();
  const quick = page.getByRole('region', { name: 'Quick Convert', exact: true });
  await quick.getByRole('tab', { name: 'Metadata' }).click();
  await quick.getByLabel('Copy Metadata From').selectOption('');
  await quick.getByLabel('Copy Chapters From').selectOption('');
  await quick.getByLabel('Title for output file').fill('   ');
  await quick.getByLabel('Language for output file').fill('  fra  ');
  await page.getByRole('button', { name: 'Run: Encode', exact: true }).click();
  await expect
    .poll(() =>
      page.evaluate(
        () =>
          (globalThis as any).__sourceCalls.filter((c: any) => c.command === 'mock_start_encode')
            .length,
      ),
    )
    .toBe(1);
  let requests = await page.evaluate(() =>
    (globalThis as any).__sourceCalls
      .filter((c: any) => c.command === 'mock_start_encode')
      .map((c: any) => c.payload.request),
  );
  expect(requests[0].settings).toMatchObject({
    copyMetadata: false,
    copyChapters: false,
    outputMetadata: { title: '', language: 'fra' },
  });
  expect(requests[0].settings.metadataSourcePath).toBeUndefined();
  expect(requests[0].settings.chaptersSourcePath).toBeUndefined();
  await quick.getByLabel('Copy Metadata From').selectOption('/media/input.mkv');
  await quick.getByLabel('Copy Chapters From').selectOption('/media/input.mkv');
  await quick.getByLabel('Apply the titles and languages below').uncheck();
  await page.getByRole('button', { name: 'Run: Encode', exact: true }).click();
  await expect
    .poll(() =>
      page.evaluate(
        () =>
          (globalThis as any).__sourceCalls.filter((c: any) => c.command === 'mock_start_encode')
            .length,
      ),
    )
    .toBe(2);
  requests = await page.evaluate(() =>
    (globalThis as any).__sourceCalls
      .filter((c: any) => c.command === 'mock_start_encode')
      .map((c: any) => c.payload.request),
  );
  expect(requests[1].settings).toMatchObject({
    metadataSourcePath: '/media/input.mkv',
    chaptersSourcePath: '/media/input.mkv',
  });
  expect(requests[1].settings.copyMetadata).toBeUndefined();
  expect(requests[1].settings.copyChapters).toBeUndefined();
  expect(requests[1].settings.outputMetadata).toBeUndefined();
  expect(
    (requests[1].settings.trackOverrides ?? []).some(
      (item: any) => 'title' in item || 'language' in item,
    ),
  ).toBe(false);
});

test('Quick batch defaults to each source donor, respects None, and excludes the metadata grid', async ({
  page,
}) => {
  await installSourceEncodeFixture(page);
  const first = { ...source, title: 'First' };
  const second = {
    ...source,
    id: 'second',
    path: '/other/second.mkv',
    name: 'second.mkv',
    title: 'Second',
  };
  await page.evaluate(
    ({ first, second }) => {
      const h = globalThis as any;
      h.__pickerPaths = [first.path, second.path];
      h.__mediaByPath = { [first.path]: first, [second.path]: second };
    },
    { first, second },
  );
  await page.getByRole('button', { name: 'Add Files…' }).click();
  await page.getByRole('option', { name: /input.mkv/ }).click();
  await page.getByRole('button', { name: 'Load Tracks' }).click();
  await page.getByRole('button', { name: 'Quick Convert', exact: true }).click();
  const quick = page.getByRole('region', { name: 'Quick Convert', exact: true });
  await quick.getByRole('tab', { name: 'Metadata' }).click();
  await quick.getByLabel('Title for output file').fill('Do not batch this title');
  await quick.getByLabel('Title for track 1').fill('Do not batch this track title');
  await page.getByRole('button', { name: 'File List', exact: true }).click();
  await page.getByRole('combobox', { name: 'Mode', exact: true }).selectOption('batch');
  await page.getByRole('button', { name: 'Quick Convert', exact: true }).click();
  await quick.getByRole('tab', { name: 'Metadata' }).click();
  await expect(quick.getByLabel('Copy Metadata From')).toHaveValue('current');
  await expect(quick.getByLabel('Copy Chapters From')).toHaveValue('current');
  await expect(quick.getByLabel('Title for output file')).toHaveCount(0);
  await quick.getByLabel('Copy Metadata From').selectOption('');
  await page.getByRole('button', { name: 'Run Batch: Encode', exact: true }).click();
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
  expect(preview.copyMetadata).toBe(false);
  expect(preview.copyChapters).toBeUndefined();
  expect(preview.outputMetadata).toBeUndefined();
  const requests = calls
    .filter((c: any) => c.command === 'mock_start_encode')
    .map((c: any) => c.payload.request);
  for (const request of requests) {
    expect(request.settings.copyMetadata).toBe(false);
    expect(request.settings.metadataSourcePath).toBeUndefined();
    expect(request.settings.chaptersSourcePath).toBe(request.source.inputPath);
    expect(request.settings.outputMetadata).toBeUndefined();
    expect(
      (request.settings.trackOverrides ?? []).some(
        (item: any) => 'title' in item || 'language' in item,
      ),
    ).toBe(false);
  }
  await quick.getByLabel('Copy Metadata From').selectOption('current');
  await quick.getByLabel('Copy Chapters From').selectOption('');
  await page.getByRole('button', { name: 'Run Batch: Encode', exact: true }).click();
  await expect
    .poll(() =>
      page.evaluate(
        () =>
          (globalThis as any).__sourceCalls.filter((c: any) => c.command === 'mock_start_encode')
            .length,
      ),
    )
    .toBe(4);
  const secondRun = await page.evaluate(() =>
    (globalThis as any).__sourceCalls
      .filter((c: any) => c.command === 'mock_start_encode')
      .slice(2)
      .map((c: any) => c.payload.request),
  );
  for (const request of secondRun) {
    expect(request.settings.metadataSourcePath).toBe(request.source.inputPath);
    expect(request.settings.copyMetadata).toBeUndefined();
    expect(request.settings.copyChapters).toBe(false);
    expect(request.settings.chaptersSourcePath).toBeUndefined();
    expect(request.settings.outputMetadata).toBeUndefined();
  }
});

test('copy-video conversion carries trimmed Output File metadata and explicit donor removal', async ({
  page,
}) => {
  await installSourceEncodeFixture(page);
  await page.getByRole('button', { name: 'Add Files…' }).click();
  await page.getByRole('button', { name: 'Load Tracks' }).click();
  await page.getByRole('button', { name: 'Quick Convert', exact: true }).click();
  const quick = page.getByRole('region', { name: 'Quick Convert', exact: true });
  await quick.getByLabel('Codec', { exact: true }).selectOption('copy');
  await quick.getByRole('tab', { name: 'Metadata' }).click();
  await quick.getByLabel('Copy Metadata From').selectOption('');
  await quick.getByLabel('Copy Chapters From').selectOption('');
  await quick.getByLabel('Title for output file').fill('  Copied Title  ');
  await quick.getByLabel('Language for output file').fill('  jpn  ');
  await page.getByRole('button', { name: 'Run: Encode', exact: true }).click();
  await expect
    .poll(() =>
      page.evaluate(
        () =>
          (globalThis as any).__sourceCalls.find((c: any) => c.command === 'mock_start_encode')
            ?.payload.request.settings,
      ),
    )
    .toMatchObject({
      videoMode: 'copy',
      copyMetadata: false,
      copyChapters: false,
      outputMetadata: { title: 'Copied Title', language: 'jpn' },
    });
});
