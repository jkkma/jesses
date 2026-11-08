import { expect, test } from '@playwright/test';
import {
  adjustedOutputStem,
  adjustedOutputTitle,
} from '../../src/lib/components/shared/output-title';
import { installSourceEncodeFixture, source } from './helpers/source-encode-fixture';

test('the default output file name is adjusted, not its folder', () => {
  expect(
    adjustedOutputStem(
      'C:\\Users\\me\\4K HDR\\Resident.Evil.2026.2160p.iT.WEB-DL.DV.HDR10+[Ben The Men]',
      {
        toneMapped: true,
        height: 1080,
      },
    ),
  ).toBe('C:\\Users\\me\\4K HDR\\Resident.Evil.2026.1080p.iT.WEB-DL[Ben The Men]');
  expect(adjustedOutputStem('/media/HDR/Movie.2160p.HDR', { toneMapped: false })).toBe(
    '/media/HDR/Movie.2160p.HDR',
  );
  // A name made only of picture tokens keeps its source name.
  expect(adjustedOutputStem('/media/HDR', { toneMapped: true })).toBe('/media/HDR');
});

test('an unedited output path follows the output picture until it is edited', async ({ page }) => {
  await installSourceEncodeFixture(page);
  const name = 'Resident.Evil.2026.2160p.iT.WEB-DL.DV.HDR10+[Ben The Men]';
  await page.evaluate((media) => (globalThis as any).__setSourceMedia(media), {
    ...source,
    path: `/media/${name}.mp4`,
    name: `${name}.mp4`,
    streams: source.streams.map((stream) =>
      stream.kind === 'video'
        ? {
            ...stream,
            fieldOrder: 'progressive',
            codec: 'hevc',
            width: 3832,
            height: 1600,
            pixelFormat: 'yuv420p10le',
            bitDepth: 10,
            colorPrimaries: 'bt2020',
            colorTransfer: 'smpte2084',
            colorSpace: 'bt2020nc',
            colorRange: 'tv',
            hdrFormat: 'HDR / PQ',
          }
        : stream,
    ),
  });
  await page.getByRole('button', { name: 'Add Files…' }).click();
  await page.getByRole('button', { name: 'Load Tracks' }).click();
  await page.getByRole('button', { name: 'Quick Convert', exact: true }).click();
  const quick = page.getByRole('region', { name: 'Quick Convert', exact: true });
  const output = quick.getByLabel('Output Path', { exact: true });
  // HDR sources default to Spline tone mapping, so the HDR tokens go first.
  await expect(output).toHaveValue('/media/Resident.Evil.2026.2160p.iT.WEB-DL[Ben The Men]');
  await quick.getByLabel('Resize', { exact: true }).selectOption('1080p');
  await expect(output).toHaveValue('/media/Resident.Evil.2026.1080p.iT.WEB-DL[Ben The Men]');
  await quick.getByLabel('Tone Mapping').selectOption('off');
  await expect(output).toHaveValue(
    '/media/Resident.Evil.2026.1080p.iT.WEB-DL.DV.HDR10+[Ben The Men]',
  );

  await output.fill('/media/my-clip');
  await quick.getByLabel('Resize', { exact: true }).selectOption('720p');
  await quick.getByLabel('Tone Mapping').selectOption('spline');
  await expect(output).toHaveValue('/media/my-clip');
});

test('a resized SDR rendering does not keep the source resolution and HDR tokens', () => {
  const source = 'Resident.Evil.2026.2160p.iT.WEB-DL.DV.HDR10+[Ben The Men]';
  expect(adjustedOutputTitle(source, { toneMapped: true, height: 1080 })).toBe(
    'Resident.Evil.2026.1080p.iT.WEB-DL[Ben The Men]',
  );
  expect(adjustedOutputTitle(source, { toneMapped: false, height: 720 })).toBe(
    'Resident.Evil.2026.720p.iT.WEB-DL.DV.HDR10+[Ben The Men]',
  );
  expect(adjustedOutputTitle(source, { toneMapped: true })).toBe(
    'Resident.Evil.2026.2160p.iT.WEB-DL[Ben The Men]',
  );
  // A resize to an unknown height drops the stale resolution.
  expect(adjustedOutputTitle('Film 4K UHD HDR (2019)', { toneMapped: true, height: null })).toBe(
    'Film (2019)',
  );
  expect(adjustedOutputTitle('Movie [Dolby Vision]', { toneMapped: true })).toBe('Movie');
});

test('unchanged pictures and ordinary words keep the source title', () => {
  const title = 'DVD Extras - Driving Lessons 1080p';
  expect(adjustedOutputTitle(title, { toneMapped: false })).toBe(title);
  expect(adjustedOutputTitle(title, { toneMapped: true })).toBe(title);
  expect(adjustedOutputTitle('Hdrive.Story', { toneMapped: true })).toBe('Hdrive.Story');
  expect(adjustedOutputTitle('', { toneMapped: true, height: 1080 })).toBe('');
});
