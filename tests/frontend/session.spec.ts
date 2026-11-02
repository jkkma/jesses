import { test, expect } from '@playwright/test';

// The fork keeps the activity log visible; its resizable splitter replaces the
// former Jesses-only log disclosure and numeric height input.
test('active source tab and keyboard-adjusted log height survive a restart', async ({ page }) => {
  await page.goto('/');
  await page.getByRole('button', { name: 'Utilities', exact: true }).click();
  const splitter = page.getByRole('slider', { name: 'Resize log' });
  await splitter.focus();
  for (let index = 0; index < 8; index++) await page.keyboard.press('ArrowUp');
  await expect(splitter).toHaveAttribute('aria-valuenow', '220');
  await expect(page.getByRole('log', { name: 'Recent activity' })).toHaveCSS('height', '220px');
  await page.reload();
  await expect(page.getByRole('region', { name: 'Utilities', exact: true })).toBeVisible();
  await expect(splitter).toHaveAttribute('aria-valuenow', '220');
  await expect(page.getByRole('log', { name: 'Recent activity' })).toHaveCSS('height', '220px');
});
