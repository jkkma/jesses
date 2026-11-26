import { expect, test, type Page } from '@playwright/test';
import { installSourceEncodeFixture, source } from './helpers/source-encode-fixture';

function hdrSource(transfer: 'smpte2084' | 'arib-std-b67') {
  return {
    ...source,
    id: transfer,
    path: `/media/${transfer}.mkv`,
    name: `${transfer}.mkv`,
    streams: source.streams.map((stream) =>
      stream.kind === 'video'
        ? {
            ...stream,
            codec: 'hevc',
            fieldOrder: 'progressive',
            pixelFormat: 'yuv420p10le',
            colorPrimaries: 'bt2020',
            colorTransfer: transfer,
            colorSpace: 'bt2020nc',
            colorRange: 'tv',
            ...(transfer === 'smpte2084' ? { dynamicHdrFormats: ['HDR10+'] } : {}),
          }
        : stream,
    ),
  };
}

async function openHdrQuickConvert(page: Page, transfer: 'smpte2084' | 'arib-std-b67') {
  await installSourceEncodeFixture(page);
  await page.evaluate((media) => (globalThis as any).__setSourceMedia(media), hdrSource(transfer));
  await page.getByRole('button', { name: 'Add Files…' }).click();
  await page.getByRole('button', { name: 'Load Tracks' }).click();
  await page.getByRole('button', { name: 'Quick Convert', exact: true }).click();
  return page.getByRole('region', { name: 'Quick Convert', exact: true });
}

test('Quick Convert blocks 8-bit preserved HDR10 and submits 10-bit fallback', async ({ page }) => {
  const quick = await openHdrQuickConvert(page, 'smpte2084');
  const run = page.getByRole('button', { name: 'Run: Encode', exact: true });
  const color = quick.getByLabel('Color Format', { exact: true });
  const tone = quick.getByLabel('Tone Mapping', { exact: true });
  await tone.selectOption('off');
  await quick.getByLabel('HDR10 fallback (discard DV / HDR10+)').check();
  await color.selectOption('yuv420p');
  await expect(run).toBeDisabled();
  await expect(page.locator('#run-blocked-reason')).toContainText(
    'HDR10 output requires 10-bit 4:2:0',
  );

  await tone.selectOption('spline');
  await expect(run).toBeEnabled();
  await tone.selectOption('off');
  await expect(run).toBeDisabled();
  await color.selectOption('yuv420p10le');
  await quick.getByLabel('HDR10 fallback (discard DV / HDR10+)').check();
  await run.click();
  await expect
    .poll(() =>
      page.evaluate(() => {
        const settings = (globalThis as any).__sourceCalls.find(
          (call: any) => call.command === 'mock_start_encode',
        )?.payload.request.settings;
        return (
          settings && [
            settings.encoder,
            settings.outputPixelFormat,
            settings.hdr10Fallback,
            !!settings.toneMap,
          ]
        );
      }),
    )
    .toEqual(['svtAv1Hdr', 'yuv420p10le', true, false]);
});

test('Quick Convert requires Tone Mapping to encode HLG', async ({ page }) => {
  const quick = await openHdrQuickConvert(page, 'arib-std-b67');
  const run = page.getByRole('button', { name: 'Run: Encode', exact: true });
  const tone = quick.getByLabel('Tone Mapping', { exact: true });
  await expect(tone).toHaveValue('spline');
  await tone.selectOption('off');
  await expect(run).toBeDisabled();
  await expect(page.locator('#run-blocked-reason')).toHaveText(
    'Choose a Tone Mapping curve to convert HLG to SDR.',
  );
  await expect(quick.getByLabel('HDR10 fallback (discard DV / HDR10+)')).toHaveCount(0);
  await quick.getByRole('tab', { name: 'Audio', exact: true }).click();
  await page.locator('#run-blocked-reason').click();
  await tone.selectOption('reinhard');
  await run.click();
  await expect
    .poll(() =>
      page.evaluate(
        () =>
          (globalThis as any).__sourceCalls.find(
            (call: any) => call.command === 'mock_start_encode',
          )?.payload.request.settings.toneMap?.algorithm,
      ),
    )
    .toBe('reinhard');
});

for (const hdrFirst of [true, false])
  test(`Quick Convert batch skips an HLG item when Tone Mapping is off and ${hdrFirst ? 'HLG' : 'SDR'} is first`, async ({
    page,
  }) => {
    await installSourceEncodeFixture(page);
    const sdr = {
      ...source,
      streams: source.streams.map((stream) =>
        stream.kind === 'video' ? { ...stream, fieldOrder: 'progressive' } : stream,
      ),
    };
    const hlg = hdrSource('arib-std-b67');
    await page.evaluate(
      (inputs) => {
        const mock = globalThis as any;
        mock.__pickerPaths = inputs.map((item) => item.path);
        mock.__mediaByPath = Object.fromEntries(inputs.map((item) => [item.path, item]));
      },
      hdrFirst ? [hlg, sdr] : [sdr, hlg],
    );
    await page.getByRole('button', { name: 'Add Files…' }).click();
    await page.getByRole('combobox', { name: 'Mode', exact: true }).selectOption('batch');
    await page.getByRole('button', { name: 'Quick Convert', exact: true }).click();
    const quick = page.getByRole('region', { name: 'Quick Convert', exact: true });
    await quick.getByLabel('Tone Mapping', { exact: true }).selectOption('off');
    await expect(
      quick.getByText('Choose a Tone Mapping curve to convert HLG to SDR.'),
    ).toBeVisible();
    await quick.getByLabel('Color Format', { exact: true }).selectOption('yuv420p');
    await page.getByRole('button', { name: 'Run Batch: Encode' }).click();
    await page.getByRole('button', { name: 'File List', exact: true }).click();
    await expect(page.getByRole('option', { name: /arib-std-b67.mkv/ })).toContainText('Failed');
    const requests = await page.evaluate(() =>
      (globalThis as any).__sourceCalls
        .filter((call: any) => call.command === 'mock_start_encode')
        .map((call: any) => call.payload.request),
    );
    expect(requests).toHaveLength(1);
    expect(requests[0].source.inputPath).toBe(sdr.path);
    expect(requests[0].settings.outputPixelFormat).toBe('yuv420p');
  });

// Matroska cannot store MP4 timed text. With the default copy mode, those
// tracks become SubRip instead of blocking Run, and the Subtitles tab says so.
test('copied mov_text into MKV is converted to SubRip instead of blocking Run', async ({
  page,
}) => {
  await installSourceEncodeFixture(page);
  const subtitle = source.streams.find((stream) => stream.kind === 'subtitle')!;
  const media = {
    ...source,
    path: '/media/movie.mp4',
    name: 'movie.mp4',
    streams: [
      ...source.streams.filter((stream) => stream.kind !== 'subtitle'),
      { ...subtitle, index: 3, codec: 'mov_text', title: 'English' },
    ],
  };
  await page.evaluate((media) => (globalThis as any).__setSourceMedia(media), media);
  await page.getByRole('button', { name: 'Add Files…' }).click();
  await page.getByRole('button', { name: 'Load Tracks' }).click();
  await page.getByRole('button', { name: 'Quick Convert', exact: true }).click();
  const quick = page.getByRole('region', { name: 'Quick Convert', exact: true });
  await quick.getByLabel('Container', { exact: true }).selectOption('matroska');
  await quick.getByLabel('Output Path', { exact: true }).fill('/media/movie-av1');
  await expect(quick.getByRole('alert').filter({ hasText: /MKV cannot store/ })).toHaveCount(0);
  await expect(page.locator('#run-blocked-reason')).toHaveCount(0);
  const run = page.getByRole('button', { name: 'Run: Encode', exact: true });
  await expect(run).toBeEnabled();

  await quick.getByRole('tab', { name: 'Subtitles' }).click();
  await expect(quick.getByLabel('Codec', { exact: true })).toHaveValue('copy');
  await expect(
    quick.getByText(
      'The MP4 text subtitle is converted to SubRip, because MKV cannot store MP4 text.',
    ),
  ).toBeVisible();

  await run.click();
  await expect
    .poll(() =>
      page.evaluate(
        () =>
          (globalThis as any).__sourceCalls.find((c: any) => c.command === 'mock_start_encode')
            ?.payload.request.settings.subtitles,
      ),
    )
    .toEqual([{ streamIndex: 3, mode: 'subRip' }]);
});

// Cover art is an attached picture, not a second video track to encode. It is
// kept beside the encoded video rather than mapped as one of its streams.
test('attached cover art is kept beside the encode and does not block Run', async ({ page }) => {
  await installSourceEncodeFixture(page);
  const video = source.streams.find((stream) => stream.kind === 'video')!;
  const media = {
    ...source,
    streams: [
      ...source.streams.filter((stream) => stream.kind !== 'subtitle'),
      {
        ...video,
        index: 28,
        codec: 'mjpeg',
        width: 600,
        height: 900,
        title: 'Cover',
        attachedPic: true,
      },
    ],
  };
  await page.evaluate((media) => (globalThis as any).__setSourceMedia(media), media);
  await page.getByRole('button', { name: 'Add Files…' }).click();
  await page.getByRole('button', { name: 'Load Tracks' }).click();
  await expect(page.getByRole('checkbox', { name: /Include .*Cover/ })).toBeChecked();
  await page.getByRole('button', { name: 'Quick Convert', exact: true }).click();
  const quick = page.getByRole('region', { name: 'Quick Convert', exact: true });
  await quick.getByLabel('Output Path', { exact: true }).fill('/media/cover-av1');
  await expect(
    quick.getByText('This encoder supports one selected video track per encode.'),
  ).toHaveCount(0);
  await expect(page.locator('#run-blocked-reason')).toHaveCount(0);
  await page.getByRole('button', { name: 'Run: Encode', exact: true }).click();
  await expect
    .poll(() =>
      page.evaluate(
        () =>
          (globalThis as any).__sourceCalls.find((c: any) => c.command === 'mock_start_encode')
            ?.payload.request.source.streamIndices,
      ),
    )
    .not.toContain(28);
  const request = await page.evaluate(
    () =>
      (globalThis as any).__sourceCalls.find((c: any) => c.command === 'mock_start_encode').payload
        .request,
  );
  expect(request.source.streamIndices).toContain(video.index);
  expect(request.settings.videoStreamIndex).toBe(video.index);
  expect(request.settings.coverStreamIndex).toBe(28);
});

test('a cover listed before the movie is not chosen as the video', async ({ page }) => {
  await installSourceEncodeFixture(page);
  const video = source.streams.find((stream) => stream.kind === 'video')!;
  const media = {
    ...source,
    streams: [
      {
        ...video,
        index: 0,
        codec: 'mjpeg',
        width: 600,
        height: 900,
        title: 'Cover',
        attachedPic: true,
      },
      { ...video, index: 3 },
      ...source.streams.filter((stream) => stream.kind !== 'video' && stream.kind !== 'subtitle'),
    ],
  };
  await page.evaluate((media) => (globalThis as any).__setSourceMedia(media), media);
  await page.getByRole('button', { name: 'Add Files…' }).click();
  await page.getByRole('button', { name: 'Load Tracks' }).click();
  const cover = page.getByRole('checkbox', { name: /Include .*Cover/ });
  await expect(cover).toBeChecked();
  // Leaving the cover out is a Track List choice.
  await cover.uncheck();
  await page.getByRole('button', { name: 'Quick Convert', exact: true }).click();
  const quick = page.getByRole('region', { name: 'Quick Convert', exact: true });
  await quick.getByLabel('Output Path', { exact: true }).fill('/media/cover-first-av1');
  await expect(page.locator('#run-blocked-reason')).toHaveCount(0);
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
  expect(request.settings.videoStreamIndex).toBe(3);
  expect(request.source.streamIndices).not.toContain(0);
  expect(request.settings.coverStreamIndex).toBeUndefined();
});

test('a cover the container cannot store is reported as left out', async ({ page }) => {
  await installSourceEncodeFixture(page);
  const video = source.streams.find((stream) => stream.kind === 'video')!;
  const media = {
    ...source,
    streams: [
      ...source.streams.filter((stream) => stream.kind !== 'subtitle'),
      { ...video, index: 28, codec: 'mjpeg', title: 'Cover', attachedPic: true },
    ],
  };
  await page.evaluate((media) => (globalThis as any).__setSourceMedia(media), media);
  await page.getByRole('button', { name: 'Add Files…' }).click();
  await page.getByRole('button', { name: 'Load Tracks' }).click();
  await page.getByRole('button', { name: 'Quick Convert', exact: true }).click();
  const quick = page.getByRole('region', { name: 'Quick Convert', exact: true });
  await quick.getByLabel('Container', { exact: true }).selectOption('mov');
  await quick.getByLabel('Output Path', { exact: true }).fill('/media/cover-mov');
  await page.getByRole('button', { name: 'Run: Encode', exact: true }).click();
  await expect
    .poll(() =>
      page.evaluate(
        () =>
          (globalThis as any).__sourceCalls.find((c: any) => c.command === 'mock_start_encode')
            ?.payload.request.settings,
      ),
    )
    .toBeTruthy();
  const settings = await page.evaluate(
    () =>
      (globalThis as any).__sourceCalls.find((c: any) => c.command === 'mock_start_encode').payload
        .request.settings,
  );
  expect(settings.coverStreamIndex).toBeUndefined();
  await expect(
    page.getByText('Cover image left out: MOV cannot store cover art.', { exact: false }),
  ).not.toHaveCount(0);
});

test('invalid audio bitrate explains the blocker and jumps to Audio settings', async ({ page }) => {
  await installSourceEncodeFixture(page);
  await page.getByRole('button', { name: 'Add Files…' }).click();
  await page.getByRole('button', { name: 'Load Tracks' }).click();
  await page.getByRole('button', { name: 'Quick Convert', exact: true }).click();
  const quick = page.getByRole('region', { name: 'Quick Convert', exact: true });
  await quick.getByRole('tab', { name: 'Audio', exact: true }).click();
  const bitrate = quick.getByLabel('Bitrate (Kbps)', { exact: true });
  await bitrate.fill('0');
  await quick.getByRole('tab', { name: 'Video', exact: true }).click();
  const run = page.getByRole('button', { name: 'Run: Encode', exact: true });
  await expect(run).toBeDisabled();
  const reason = page.locator('#run-blocked-reason');
  await expect(reason).toContainText('Enter an audio bitrate between 8 and 8192 kbps in Audio.');
  await reason.click();
  await expect(bitrate).toBeVisible();
  await bitrate.fill('160');
  await expect(run).toBeEnabled();
});

test('audio bitrate scaling below 8 kbps blocks both audio-only and external mux tracks', async ({
  page,
}) => {
  await installSourceEncodeFixture(page);
  const audioOnly = {
    ...source,
    id: 'audio-only',
    path: '/media/voice.mka',
    name: 'voice.mka',
    streams: [{ ...source.streams.find((s) => s.kind === 'audio')!, channels: 1 }],
  };
  await page.evaluate((media) => (globalThis as any).__setSourceMedia(media), audioOnly);
  await page.getByRole('button', { name: 'Add Files…' }).click();
  await page.getByRole('button', { name: 'Load Tracks' }).click();
  await page.getByRole('button', { name: 'Quick Convert', exact: true }).click();
  const quick = page.getByRole('region', { name: 'Quick Convert', exact: true });
  await quick.getByRole('tab', { name: 'Audio', exact: true }).click();
  await quick.getByLabel('Codec', { exact: true }).selectOption('opus');
  await quick.getByLabel('Bitrate (Kbps)', { exact: true }).fill('8');
  const run = page.getByRole('button', { name: 'Run: Encode', exact: true });
  await expect(run).toBeDisabled();
  await expect(page.locator('#run-blocked-reason')).toContainText(
    'Enter an audio bitrate between 8 and 8192 kbps in Audio.',
  );

  // Switch to a video-only primary plus external mono audio in mux mode.
  const videoOnly = {
    ...source,
    id: 'video-only',
    path: '/media/silent.mkv',
    name: 'silent.mkv',
    streams: source.streams.filter((s) => s.kind === 'video'),
  };
  await page.getByRole('button', { name: 'File List', exact: true }).click();
  await page.getByRole('option', { name: /voice\.mka/ }).click();
  await page.getByRole('button', { name: 'Remove', exact: true }).click();
  await page.evaluate(
    ({ videoOnly, audioOnly }) => {
      const host = globalThis as any;
      host.__pickerPaths = [videoOnly.path, audioOnly.path];
      host.__mediaByPath = { [videoOnly.path]: videoOnly, [audioOnly.path]: audioOnly };
    },
    { videoOnly, audioOnly },
  );
  await page.getByRole('button', { name: 'Add Files…' }).click();
  await page.getByRole('option', { name: /silent\.mkv/ }).click();
  await page.getByRole('option', { name: /voice\.mka/ }).click({ modifiers: ['Control'] });
  await page.getByRole('button', { name: 'Load Tracks' }).click();
  await page.getByRole('button', { name: 'Quick Convert', exact: true }).click();
  const encodeRun = page.getByRole('button', { name: 'Run: Encode', exact: true });
  await expect(encodeRun).toBeDisabled();
  await expect(page.locator('#run-blocked-reason')).toContainText(
    'Enter an audio bitrate between 8 and 8192 kbps in Audio.',
  );
  await quick.getByRole('tab', { name: 'Audio', exact: true }).click();
  await quick.getByLabel('Bitrate (Kbps)', { exact: true }).fill('16');
  await expect(encodeRun).toBeEnabled();
});

test('switching a mono 8 kbps track to Copy or FLAC enables Run and normalizes emitted bitrateKbps', async ({
  page,
}) => {
  await installSourceEncodeFixture(page);
  const monoSource = {
    ...source,
    id: 'mono-video',
    path: '/media/mono.mkv',
    name: 'mono.mkv',
    streams: [
      ...source.streams.filter((s) => s.kind === 'video'),
      { ...source.streams.find((s) => s.kind === 'audio')!, channels: 1 },
    ],
  };
  await page.evaluate((media) => (globalThis as any).__setSourceMedia(media), monoSource);
  await page.getByRole('button', { name: 'Add Files…' }).click();
  await page.getByRole('button', { name: 'Load Tracks' }).click();
  await page.getByRole('button', { name: 'Quick Convert', exact: true }).click();
  const quick = page.getByRole('region', { name: 'Quick Convert', exact: true });
  await quick.getByLabel('Container', { exact: true }).selectOption('matroska');
  await quick.getByRole('tab', { name: 'Audio', exact: true }).click();
  await quick.getByLabel('Codec', { exact: true }).selectOption('opus');
  await quick.getByLabel('Bitrate (Kbps)', { exact: true }).fill('8');
  const run = page.getByRole('button', { name: 'Run: Encode', exact: true });
  await expect(run).toBeDisabled();

  for (const codec of ['copy', 'flac'] as const) {
    await quick.getByLabel('Codec', { exact: true }).selectOption(codec);
    await expect(run).toBeEnabled();
    await page.evaluate(() => ((globalThis as any).__sourceCalls.length = 0));
    await run.click();
    await expect
      .poll(async () =>
        page.evaluate(
          () =>
            (globalThis as any).__sourceCalls.find((c: any) => c.command === 'mock_start_encode')
              ?.payload.request.settings.audio,
        ),
      )
      .toMatchObject([{ streamIndex: 1, codec, bitrateKbps: 128 }]);
    // Reset job list back to idle so the next codec submission can start cleanly.
    await page.evaluate(() => {
      const host = globalThis as any;
      host.__testJobs = [];
      host.__emitJobs();
    });
  }
});
