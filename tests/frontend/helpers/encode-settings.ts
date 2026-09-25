import type { Page } from '@playwright/test';

export async function showAllEncodeSettings(page: Page) {
  const expand = page.getByRole('button', { name: 'Show all settings', exact: true });
  if (await expand.count()) await expand.click();
}
