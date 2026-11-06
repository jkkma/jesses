import { expect, test } from '@playwright/test';
import { installSourceEncodeFixture, source } from './helpers/source-encode-fixture';

// Copying MP4 timed text into Matroska fails in the runtime preflight. The
// form must say so before Run, both in the panel and beside the Run button.
test('copied mov_text into MKV is reported before Run and cleared by a text conversion', async ({
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
  await quick.getByLabel('Output Path', { exact: true }).fill('/media/movie-av1');
  const message = /Subtitle #3 is MP4 text \(mov_text\), which MKV cannot store/;
  await expect(quick.getByRole('alert').filter({ hasText: message })).toBeVisible();
  const reason = page.locator('#run-blocked-reason');
  await expect(reason).toHaveText(message);
  const run = page.getByRole('button', { name: 'Run: Encode', exact: true });
  await expect(run).toBeDisabled();
  await expect(run).toHaveAttribute('aria-describedby', 'run-blocked-reason');
  await expect(reason).toHaveCSS(
    'color',
    await quick
      .getByRole('alert')
      .first()
      .evaluate((element) => getComputedStyle(element).color),
  );

  await quick.getByRole('tab', { name: 'Subtitles' }).click();
  await quick.getByLabel('Codec', { exact: true }).selectOption('subRip');
  await expect(quick.getByRole('alert').filter({ hasText: message })).toHaveCount(0);
  await expect(reason).toHaveCount(0);
  await expect(run).toBeEnabled();
});

// Cover art is an attached picture, not a second video track to encode.
test('attached cover art starts unchecked and does not block Run', async ({ page }) => {
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
  await expect(page.getByRole('checkbox', { name: /Include .*Cover/ })).not.toBeChecked();
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
  const indices = await page.evaluate(
    () =>
      (globalThis as any).__sourceCalls.find((c: any) => c.command === 'mock_start_encode').payload
        .request.source.streamIndices,
  );
  expect(indices).toContain(video.index);
});
