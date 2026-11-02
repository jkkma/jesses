import { expect, test } from '@playwright/test';
import { installSourceEncodeFixture } from './helpers/source-encode-fixture';

test('Utilities exposes the fork cards in source order and uses the shared Run action', async ({
  page,
}) => {
  await installSourceEncodeFixture(page);
  await page.getByRole('button', { name: 'Utilities', exact: true }).click();
  const utilities = page.getByRole('region', { name: 'Utilities' });
  await expect(utilities.locator('.card strong')).toHaveText([
    'Read Bitrates',
    'Get Metrics',
    'Transfer Color Metadata',
    'Deinterlace Video',
    'Repair Frame Cadence',
    'Film Grain (AV1)',
    'Sample Encodes',
    'Cut Video',
    'Concatenate Into Single MKV',
    'Show Bitrate Chart',
    'OCR Bitmap Subtitles',
  ]);
  await utilities.getByRole('button', { name: /Repair Frame Cadence/ }).click();
  await expect(page.getByRole('button', { name: 'Run: Cadence repair' })).toBeVisible();
});
