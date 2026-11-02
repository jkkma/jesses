import { expect, test } from '@playwright/test';
import { installSourceEncodeFixture } from './helpers/source-encode-fixture';

test('browser preview starts without media and does not offer desktop import or encoding', async ({
  page,
}) => {
  await page.goto('/');
  await expect(page.getByRole('button', { name: 'Add Files…' })).toBeDisabled();
  await expect(page.getByRole('button', { name: 'Run', exact: true })).toBeDisabled();
  await page.getByRole('button', { name: 'Quick Convert', exact: true }).click();
  await expect(page.getByRole('button', { name: 'Run: Encode', exact: true })).toBeDisabled();
  await expect(page.getByRole('log', { name: 'Recent activity' })).toContainText(
    'Local file access requires the desktop app.',
  );
});

test('Ctrl+O opens the desktop source picker and the persistent log records the import', async ({
  page,
}) => {
  await installSourceEncodeFixture(page);
  await page.keyboard.press('Control+o');
  await expect(page.getByRole('option', { name: /input.mkv/ })).toBeVisible();
  await expect(page.getByRole('log', { name: 'Recent activity' })).toContainText(
    'Imported input.mkv',
  );
  expect(
    await page.evaluate(
      () =>
        (globalThis as any).__sourceCalls.filter((c: any) => c.command === 'plugin:dialog|open')
          .length,
    ),
  ).toBe(1);
});

test('Paper-and-ink palette and minimum-width shell persist under a dark OS preference', async ({
  page,
}) => {
  await page.emulateMedia({ colorScheme: 'dark', reducedMotion: 'reduce' });
  await page.setViewportSize({ width: 760, height: 600 });
  await page.goto('/');
  await expect(page.locator('html')).toHaveCSS('background-color', 'rgb(201, 193, 169)');
  for (const tab of ['File List', 'Quick Convert', 'AV1AN']) {
    await page.getByRole('button', { name: tab, exact: true }).click();
    expect(await page.evaluate(() => document.documentElement.scrollWidth <= innerWidth)).toBe(
      true,
    );
  }
});
