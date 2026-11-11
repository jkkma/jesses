import { expect, test } from '@playwright/test';
import { installSourceEncodeFixture, source } from './helpers/source-encode-fixture';

for (const example of [
  { view: 'Quick Convert', container: 'mp4', typed: '/media/clip.mp4', want: '/media/clip.mp4' },
  {
    view: 'Quick Convert',
    container: 'mp4',
    typed: 'C:\\Videos\\My.Movie.MP4',
    want: 'C:\\Videos\\My.Movie.mp4',
  },
  { view: 'Quick Convert', container: 'mp4', typed: '/media/clip.mkv', want: '/media/clip.mp4' },
  { view: 'AV1AN', container: 'matroska', typed: '/media/clip.mkv', want: '/media/clip.mkv' },
  { view: 'Quick Convert', codec: 'gif', typed: '/media/clip.gif', want: '/media/clip.gif' },
  { view: 'Quick Convert', codec: 'png', typed: '/media/frames.png', want: '/media/frames.png' },
]) {
  test(`${example.view} accepts a typed output filename ${example.typed} for ${example.codec ?? example.container}`, async ({
    page,
  }) => {
    await installSourceEncodeFixture(page);
    await page.evaluate((media) => (globalThis as any).__setSourceMedia(media), {
      ...source,
      streams: source.streams.filter((stream) => stream.kind !== 'subtitle'),
    });
    await page.getByRole('button', { name: 'Add Files…' }).click();
    await page.getByRole('button', { name: 'Load Tracks' }).click();
    await page.getByRole('button', { name: example.view, exact: true }).click();
    const panel = page.getByRole('region', { name: example.view, exact: true });
    if (example.container) await panel.getByLabel('Container').selectOption(example.container);
    if (example.codec) await panel.getByLabel('Codec', { exact: true }).selectOption(example.codec);
    await panel.getByLabel('Output Path', { exact: true }).fill(example.typed);
    await page
      .getByRole('button', {
        name: example.view === 'AV1AN' ? 'Run: AV1AN encode' : 'Run: Encode',
        exact: true,
      })
      .click();
    await expect
      .poll(() =>
        page.evaluate(() => {
          const call = (globalThis as any).__sourceCalls.find((c: any) =>
            ['mock_start_encode', 'mock_image_execution'].includes(c.command),
          );
          return call?.payload.request.source?.outputPath ?? call?.payload.request.outputPath;
        }),
      )
      .toBe(example.want);
  });
}

test('typed output extensions are removed before collision checks and container changes', async ({
  page,
}) => {
  await installSourceEncodeFixture(page);
  await page.evaluate(
    (media) => {
      (globalThis as any).__setSourceMedia(media);
      (globalThis as any).__occupied = ['/media/clip.mp4', '/media/clip.mkv'];
    },
    { ...source, streams: source.streams.filter((stream) => stream.kind !== 'subtitle') },
  );
  await page.getByRole('button', { name: 'Add Files…' }).click();
  await page.getByRole('button', { name: 'Load Tracks' }).click();
  await page.getByRole('button', { name: 'Quick Convert', exact: true }).click();
  const panel = page.getByRole('region', { name: 'Quick Convert', exact: true });
  await panel.getByLabel('Output Path', { exact: true }).fill('/media/clip.mkv');
  await panel.getByLabel('Container').selectOption('mp4');
  await expect(panel.getByLabel('Output Path', { exact: true })).toHaveValue('/media/clip (1)');
  await panel.getByLabel('Output Path', { exact: true }).fill('/media/clip.mp4');
  await page.getByRole('button', { name: 'Run: Encode', exact: true }).click();
  await expect
    .poll(() =>
      page.evaluate(
        () =>
          (globalThis as any).__sourceCalls.find((c: any) => c.command === 'mock_start_encode')
            ?.payload.request.source.outputPath,
      ),
    )
    .toBe('/media/clip (1).mp4');
});
