import { expect, test } from '@playwright/test';
import { installSourceEncodeFixture, source } from './helpers/source-encode-fixture';

test('implicit image request freezes all inputs, checked tracks and source nonvideo settings', async ({
  page,
}) => {
  await installSourceEncodeFixture(page);
  const first = { ...source, title: 'Original title', language: 'eng' };
  const external = {
    ...source,
    id: 'external-audio',
    path: '/other/voice.mka',
    name: 'voice.mka',
    streams: [{ ...source.streams[1], title: 'External voice' }],
  };
  await page.evaluate(
    ({ first, external }) => {
      const h = globalThis as any;
      h.__pickerPaths = [first.path, external.path];
      h.__mediaByPath = { [first.path]: first, [external.path]: external };
    },
    { first, external },
  );
  await page.getByRole('button', { name: 'Add Files…' }).click();
  await page.getByRole('option', { name: /input.mkv/ }).click();
  await page.getByRole('option', { name: /voice.mka/ }).click({ modifiers: ['Control'] });
  await page.getByRole('button', { name: 'Load Tracks' }).click();
  await page.getByRole('checkbox', { name: /Include .*Video/ }).uncheck();
  await page.getByRole('button', { name: 'Quick Convert', exact: true }).click();
  const quick = page.getByRole('region', { name: 'Quick Convert', exact: true });
  await quick.getByLabel('Codec', { exact: true }).selectOption('png');
  await quick.getByLabel('Frame Rate', { exact: true }).fill('30');
  await quick.getByRole('button', { name: 'Configure…' }).first().click();
  const trim = page.getByRole('dialog', { name: 'Configure Trim' });
  await trim.getByLabel('Mode').selectOption('frames');
  await trim.getByLabel('Start', { exact: true }).fill('2');
  await trim.getByLabel('End', { exact: true }).fill('10');
  await trim.getByRole('button', { name: 'OK' }).click();
  await quick.getByRole('tab', { name: 'Audio', exact: true }).click();
  await quick.getByLabel('Configuration').selectOption('perTrack');
  const audio = page.getByRole('dialog', { name: 'Configure Audio Tracks' });
  await audio.getByLabel('Channels for track 2').fill('6');
  await audio.getByLabel('Bitrate for track 2').fill('384');
  await audio.getByRole('button', { name: 'OK' }).click();
  await quick.getByRole('tab', { name: 'Metadata', exact: true }).click();
  await expect(quick.getByLabel('Title for output file')).toHaveValue('Original title');
  await expect(quick.getByLabel('Language for output file')).toHaveValue('eng');
  await quick.getByLabel('Title for output file').fill('  Final title  ');
  await quick.getByLabel('Language for output file').fill('  jpn  ');
  await quick.getByLabel('Copy Metadata From').selectOption('/other/voice.mka');
  await quick.getByLabel('Copy Chapters From').selectOption('/media/input.mkv');
  const voice = quick.getByRole('row').filter({ hasText: 'voice.mka #1' });
  await voice.getByLabel('Title for track 1').fill('Dub');
  await voice.getByLabel('Language for track 1').fill('spa');
  await quick.getByLabel('Output Path', { exact: true }).fill('/media/images');
  await page.evaluate(() => {
    const h = globalThis as any;
    h.__holdResolution = new Promise((resolve) => (h.__releaseResolution = resolve));
  });
  await page.getByRole('button', { name: 'Run: Encode', exact: true }).click();
  await quick.getByLabel('Title for output file').fill('Newer draft');
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
  expect(request).toMatchObject({
    operation: 'implicitExport',
    inputs: ['/media/input.mkv', '/other/voice.mka'],
    checked: [
      { inputIndex: 0, streamIndex: 1 },
      { inputIndex: 0, streamIndex: 2 },
      { inputIndex: 1, streamIndex: 1 },
    ],
    currentInput: 0,
    format: 'pngSequence',
    outputPath: '/media/images',
    keepFrameCount: false,
    trim: { startFrame: 2, endFrameExclusive: 10 },
    subtitleCodec: 'copy',
    metadata: {
      sourceInput: 1,
      chaptersInput: 0,
      title: 'Final title',
      language: 'jpn',
      tracks: [
        { title: 'Audio', language: 'eng' },
        { title: 'Subtitles', language: 'eng' },
        { title: 'Dub', language: 'spa' },
      ],
    },
  });
  expect(request.audio).toMatchObject({
    mode: 'encode',
    codec: 'opus',
    tracks: [
      { source: { inputIndex: 0, streamIndex: 1 }, channels: 2, bitrateKbps: 128 },
      { source: { inputIndex: 1, streamIndex: 1 }, channels: 6, bitrateKbps: 384 },
    ],
  });
  expect(request).not.toHaveProperty('processing');
  expect(request).not.toHaveProperty('quality');
  expect(
    await page.evaluate(() =>
      (globalThis as any).__sourceCalls.some((c: any) =>
        ['detect_crop', 'resolve_deinterlace'].includes(c.command),
      ),
    ),
  ).toBe(false);
  await expect(quick.getByLabel('Title for output file')).toHaveValue('Newer draft');
});

for (const [choice, format] of [
  ['gif', 'gif'],
  ['jpg', 'jpegSequence'],
] as const) {
  test(`${format} forwards subtitle-only selection without inventing a video map`, async ({
    page,
  }) => {
    await installSourceEncodeFixture(page);
    await page.getByRole('button', { name: 'Add Files…' }).click();
    await page.getByRole('button', { name: 'Load Tracks' }).click();
    const tracks = page.getByRole('region', { name: 'Track List' });
    await tracks.getByText('Select…', { exact: true }).click();
    await tracks.getByRole('button', { name: 'Check None', exact: true }).click();
    await tracks.getByRole('checkbox', { name: /Include .*Subtitle/ }).check();
    await page.getByRole('button', { name: 'Quick Convert', exact: true }).click();
    const quick = page.getByRole('region', { name: 'Quick Convert', exact: true });
    await quick.getByLabel('Codec', { exact: true }).selectOption(choice);
    await quick.getByRole('tab', { name: 'Subtitles', exact: true }).click();
    await quick.getByLabel('Codec', { exact: true }).selectOption('strip');
    await quick.getByRole('tab', { name: 'Metadata', exact: true }).click();
    await quick.getByLabel('Apply the titles and languages below').uncheck();
    await page.getByRole('button', { name: 'Run: Encode', exact: true }).click();
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
    expect(request).toMatchObject({
      operation: 'implicitExport',
      inputs: ['/media/input.mkv'],
      checked: [{ inputIndex: 0, streamIndex: 2 }],
      currentInput: 0,
      format,
      subtitleCodec: 'strip',
      metadata: { tracks: [], audioDispositions: [], subtitleDispositions: [] },
    });
    expect(request.audio).toBeUndefined();
    expect(request.metadata.title).toBeUndefined();
    expect(request.metadata.language).toBeUndefined();
    expect(request).not.toHaveProperty('processing');
  });
}

test('implicit image blank global metadata clears it and Shift reviews the actual native request', async ({
  page,
}) => {
  await installSourceEncodeFixture(page);
  const media = { ...source, title: 'Original', language: 'eng' };
  await page.evaluate((media) => (globalThis as any).__setSourceMedia(media), media);
  await page.getByRole('button', { name: 'Add Files…' }).click();
  await page.getByRole('button', { name: 'Load Tracks' }).click();
  await page.getByRole('checkbox', { name: /Include .*Video/ }).uncheck();
  await page.getByRole('button', { name: 'Quick Convert', exact: true }).click();
  const quick = page.getByRole('region', { name: 'Quick Convert', exact: true });
  await quick.getByLabel('Codec', { exact: true }).selectOption('png');
  await quick.getByRole('tab', { name: 'Metadata' }).click();
  await quick.getByLabel('Title for output file').fill('   ');
  await quick.getByLabel('Language for output file').fill('');
  await page.keyboard.down('Shift');
  await page.getByRole('button', { name: 'Run: Encode', exact: true }).click();
  const editor = page.getByRole('dialog', { name: 'Edit ffmpeg Arguments' });
  await expect(editor).toBeVisible();
  await page.keyboard.up('Shift');
  await editor.getByRole('textbox').fill('-i edited input');
  await editor.getByRole('button', { name: 'Run', exact: true }).click();
  await expect
    .poll(() =>
      page.evaluate(
        () =>
          (globalThis as any).__sourceCalls.find((c: any) => c.command === 'confirm_command_edit')
            ?.payload.request.arguments,
      ),
    )
    .toBe('-i edited input');
  const request = await page.evaluate(
    () =>
      (globalThis as any).__sourceCalls.find((c: any) => c.command === 'prepare_command_edit')
        .payload.request.request,
  );
  expect(request.metadata).toMatchObject({ title: '', language: '' });
});

test('stopping implicit image name resolution prevents late native preparation', async ({
  page,
}) => {
  await installSourceEncodeFixture(page);
  await page.getByRole('button', { name: 'Add Files…' }).click();
  await page.getByRole('button', { name: 'Load Tracks' }).click();
  await page.getByRole('checkbox', { name: /Include .*Video/ }).uncheck();
  await page.getByRole('button', { name: 'Quick Convert', exact: true }).click();
  const quick = page.getByRole('region', { name: 'Quick Convert', exact: true });
  await quick.getByLabel('Codec', { exact: true }).selectOption('png');
  await expect(quick.getByLabel('Output Path', { exact: true })).not.toHaveValue('');
  await page.evaluate(() => {
    const h = globalThis as any;
    h.__holdResolution = new Promise((resolve) => (h.__releaseResolution = resolve));
  });
  await page.getByRole('button', { name: 'Run: Encode', exact: true }).click();
  await page.getByRole('button', { name: 'Stop', exact: true }).click();
  await page.evaluate(() => (globalThis as any).__releaseResolution());
  await expect(page.getByLabel('Current action')).toHaveText('Canceled.');
  expect(
    await page.evaluate(() =>
      (globalThis as any).__sourceCalls.some((c: any) => c.command === 'prepare_command_edit'),
    ),
  ).toBe(false);
});

test('implicit image preserves attachment metadata input and surfaces native failure', async ({
  page,
}) => {
  await installSourceEncodeFixture(page);
  const attachment = {
    ...source.streams[2],
    index: 5,
    kind: 'attachment',
    codec: 'ttf',
    title: 'Font',
    language: null,
  };
  const media = { ...source, streams: [...source.streams, attachment] };
  await page.evaluate((media) => {
    const h = globalThis as any;
    h.__setSourceMedia(media);
    h.__implicitFailure = true;
  }, media);
  await page.getByRole('button', { name: 'Add Files…' }).click();
  await page.getByRole('button', { name: 'Load Tracks' }).click();
  await page.getByRole('checkbox', { name: /Include .*Video/ }).uncheck();
  await page.getByRole('button', { name: 'Quick Convert', exact: true }).click();
  const quick = page.getByRole('region', { name: 'Quick Convert', exact: true });
  await quick.getByLabel('Codec', { exact: true }).selectOption('gif');
  await quick.getByRole('tab', { name: 'Audio' }).click();
  await quick.getByLabel('Codec', { exact: true }).selectOption('strip');
  await quick.getByRole('tab', { name: 'Metadata' }).click();
  await quick.getByLabel('Copy Metadata From').selectOption('');
  await page.getByRole('button', { name: 'Run: Encode', exact: true }).click();
  await expect(quick.getByRole('alert')).toContainText('image export failed');
  await expect(page.getByLabel('Current action')).toHaveText(
    'Did not finish - the log has the details.',
  );
  const request = await page.evaluate(
    () =>
      (globalThis as any).__sourceCalls.find((c: any) => c.command === 'mock_image_execution')
        .payload.request,
  );
  expect(request).toMatchObject({
    audio: { mode: 'strip' },
    metadata: {
      attachmentInputs: [0],
      tracks: [
        { title: 'Subtitles', language: 'eng' },
        { title: 'Font', language: '' },
      ],
      audioDispositions: [],
      subtitleDispositions: [true],
    },
  });
  expect(request.metadata.sourceInput).toBeUndefined();
});

test('implicit image samples Shift after preparation and closing the editor cancels the held command', async ({
  page,
}) => {
  await installSourceEncodeFixture(page);
  await page.getByRole('button', { name: 'Add Files…' }).click();
  await page.getByRole('button', { name: 'Load Tracks' }).click();
  await page.getByRole('checkbox', { name: /Include .*Video/ }).uncheck();
  await page.getByRole('button', { name: 'Quick Convert', exact: true }).click();
  await page
    .getByRole('region', { name: 'Quick Convert', exact: true })
    .getByLabel('Codec', { exact: true })
    .selectOption('png');
  await page.evaluate(() => {
    const h = globalThis as any;
    h.__holdPrepare = new Promise((resolve) => (h.__releasePrepare = resolve));
  });
  await page.getByRole('button', { name: 'Run: Encode', exact: true }).click();
  await expect
    .poll(() =>
      page.evaluate(() =>
        (globalThis as any).__sourceCalls.some((c: any) => c.command === 'prepare_command_edit'),
      ),
    )
    .toBe(true);
  await page.keyboard.down('Shift');
  await page.evaluate(() => (globalThis as any).__releasePrepare());
  const dialog = page.getByRole('dialog', { name: 'Edit ffmpeg Arguments' });
  await expect(dialog).toBeVisible();
  await page.keyboard.up('Shift');
  await dialog.getByRole('button', { name: 'Close' }).click();
  await expect(page.getByLabel('Current action')).toHaveText('Canceled.');
  expect(
    await page.evaluate(() =>
      (globalThis as any).__sourceCalls.some((c: any) => c.command === 'cancel_command_edit'),
    ),
  ).toBe(true);
  expect(
    await page.evaluate(() =>
      (globalThis as any).__sourceCalls.some((c: any) => c.command === 'confirm_command_edit'),
    ),
  ).toBe(false);
});
