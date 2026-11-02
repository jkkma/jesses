import { expect, test } from '@playwright/test';
import { installSourceEncodeFixture } from './helpers/source-encode-fixture';

async function load(page: import('@playwright/test').Page) {
  await installSourceEncodeFixture(page);
  await page.getByRole('button', { name: 'Add Files…' }).click();
  await page.getByRole('button', { name: 'Load Tracks' }).click();
}

test('Quick submits the source QTGMC preset and one-frame-per-field choice with its frame rate', async ({
  page,
}) => {
  await load(page);
  await page.getByRole('button', { name: 'Quick Convert', exact: true }).click();
  const quick = page.getByRole('region', { name: 'Quick Convert', exact: true });
  await quick.getByLabel('Deinterlace').selectOption('qtgmc');
  await quick.getByLabel('QTGMC preset').selectOption('medium');
  await quick.getByLabel('One frame per field').uncheck();
  await quick.getByLabel('Frame Rate', { exact: true }).fill('30000/1001');
  await page.getByRole('button', { name: 'Run: Encode', exact: true }).click();
  await expect
    .poll(() =>
      page.evaluate(
        () =>
          (globalThis as any).__sourceCalls.find((c: any) => c.command === 'mock_start_encode')
            ?.payload.request,
      ),
    )
    .toMatchObject({
      settings: {
        temporal: {
          qtgmc: { mode: 'frame', preset: 'medium', fieldOrder: 'topFirst' },
          frameRate: { numerator: 30000, denominator: 1001 },
        },
      },
    });
});

test('Quick and AV1AN retain independent source deinterlace drafts', async ({ page }) => {
  await load(page);
  await page.getByRole('button', { name: 'Quick Convert', exact: true }).click();
  const quick = page.getByRole('region', { name: 'Quick Convert', exact: true });
  await quick.getByLabel('Deinterlace').selectOption('yadif');
  await quick.getByLabel('One frame per field').uncheck();
  await page.getByRole('button', { name: 'AV1AN', exact: true }).click();
  const av1an = page.getByRole('region', { name: 'AV1AN', exact: true });
  await expect(av1an.getByLabel('Deinterlace')).toHaveValue('automatic');
  await av1an.getByLabel('Deinterlace').selectOption('bwdif');
  await page.getByRole('button', { name: 'Quick Convert', exact: true }).click();
  await expect(quick.getByLabel('Deinterlace')).toHaveValue('yadif');
  await expect(quick.getByLabel('One frame per field')).not.toBeChecked();
  await page.getByRole('button', { name: 'AV1AN', exact: true }).click();
  await page.getByRole('button', { name: 'Run: AV1AN encode', exact: true }).click();
  await expect
    .poll(() =>
      page.evaluate(
        () =>
          (globalThis as any).__sourceCalls.find((c: any) => c.command === 'mock_start_encode')
            ?.payload.request,
      ),
    )
    .toMatchObject({
      settings: {
        backend: 'av1an',
        temporal: { deinterlace: { mode: 'frame', fieldOrder: 'topFirst' } },
      },
    });
});

test('invalid frame rates prevent submission and corrected values are accepted', async ({
  page,
}) => {
  await load(page);
  await page.getByRole('button', { name: 'Quick Convert', exact: true }).click();
  const quick = page.getByRole('region', { name: 'Quick Convert', exact: true });
  await quick.getByLabel('Frame Rate', { exact: true }).fill('30000/0');
  await expect(page.getByRole('button', { name: 'Run: Encode', exact: true })).toBeDisabled();
  await quick.getByLabel('Frame Rate', { exact: true }).fill('30000/1001');
  await expect(page.getByRole('button', { name: 'Run: Encode', exact: true })).toBeEnabled();
});
