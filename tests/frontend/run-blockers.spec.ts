import { expect, test } from '@playwright/test';
import { installSourceEncodeFixture, source } from './helpers/source-encode-fixture';

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
