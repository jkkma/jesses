import { expect, test } from '@playwright/test';
import { installSourceEncodeFixture } from './helpers/source-encode-fixture';

// Preparation runs the real pre-encode work and can take minutes. Shift held
// when Run was clicked must still open the editor once the command is ready.
test('Shift at the Run click opens the editor after a long preparation', async ({ page }) => {
  await installSourceEncodeFixture(page);
  await page.getByRole('button', { name: 'Add Files…' }).click();
  await page.getByRole('button', { name: 'Load Tracks' }).click();
  await page.getByRole('button', { name: 'Quick Convert', exact: true }).click();
  const quick = page.getByRole('region', { name: 'Quick Convert', exact: true });
  await quick.getByLabel('Output Path', { exact: true }).fill('/media/review');
  await page.evaluate(() => {
    const host = globalThis as any;
    host.__holdPrepare = new Promise((resolve) => (host.__releasePrepare = resolve));
  });
  await page
    .getByRole('button', { name: 'Run: Encode', exact: true })
    .click({ modifiers: ['Shift'] });
  await expect
    .poll(() =>
      page.evaluate(() =>
        (globalThis as any).__sourceCalls.some((c: any) => c.command === 'prepare_command_edit'),
      ),
    )
    .toBe(true);
  // Shift was released long before the held command is ready.
  await page.evaluate(() => (globalThis as any).__releasePrepare());
  const dialog = page.getByRole('dialog', { name: 'Edit ffmpeg Arguments' });
  await expect(dialog).toBeVisible();
  expect(
    await page.evaluate(() =>
      (globalThis as any).__sourceCalls.some((c: any) => c.command === 'confirm_command_edit'),
    ),
  ).toBe(false);
  await dialog.getByRole('button', { name: 'Run', exact: true }).click();
  await expect
    .poll(() =>
      page.evaluate(() =>
        (globalThis as any).__sourceCalls.some((c: any) => c.command === 'confirm_command_edit'),
      ),
    )
    .toBe(true);
});

test('a plain Run click confirms the prepared command without the editor', async ({ page }) => {
  await installSourceEncodeFixture(page);
  await page.getByRole('button', { name: 'Add Files…' }).click();
  await page.getByRole('button', { name: 'Load Tracks' }).click();
  await page.getByRole('button', { name: 'Quick Convert', exact: true }).click();
  const quick = page.getByRole('region', { name: 'Quick Convert', exact: true });
  await quick.getByLabel('Output Path', { exact: true }).fill('/media/plain');
  await page.getByRole('button', { name: 'Run: Encode', exact: true }).click();
  await expect
    .poll(() =>
      page.evaluate(() =>
        (globalThis as any).__sourceCalls.some((c: any) => c.command === 'confirm_command_edit'),
      ),
    )
    .toBe(true);
  await expect(page.getByRole('dialog', { name: 'Edit ffmpeg Arguments' })).toHaveCount(0);
});
