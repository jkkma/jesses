import { expect, type Page } from '@playwright/test';

export async function showAllEncodeSettings(page: Page) {
  const toggle = page.getByRole('button', { name: /^(Show all settings|Use tabs)$/ });
  await expect(toggle).toBeVisible();
  if ((await toggle.textContent())?.trim() === 'Show all settings') await toggle.click();
  await expect(toggle).toHaveText('Use tabs');
}
