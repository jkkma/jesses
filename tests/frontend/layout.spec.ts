import { expect, test } from '@playwright/test';

for (const viewport of [
  { width: 800, height: 720 },
  { width: 1280, height: 850 },
  { width: 1920, height: 1080 },
]) {
  test(`common encode controls and actions fit at ${viewport.width}px`, async ({ page }) => {
    await page.setViewportSize(viewport);
    await page.goto('/');
    expect(await page.evaluate(() => document.documentElement.scrollWidth)).toBe(viewport.width);
    await page.getByRole('button', { name: 'Load sample', exact: true }).click();
    await expect(page.getByRole('searchbox', { name: 'Search source files' })).toBeVisible();
    expect(await page.evaluate(() => document.documentElement.scrollWidth)).toBe(viewport.width);

    for (const name of ['Quick Convert', 'av1an']) {
      await page.getByRole('button', { name, exact: true }).click();
      const workspace = page.getByRole('region', { name: `${name} workspace`, exact: true });
      const quality = workspace.getByLabel('Quality', { exact: true });
      const bounds = await quality.boundingBox();
      expect(bounds?.width).toBeLessThanOrEqual(130);
      const preset = await workspace.getByLabel('Encoder preset', { exact: true }).boundingBox();
      expect(preset?.width).toBeLessThanOrEqual(130);
      const start = await workspace.getByRole('button', { name: 'Start encode' }).boundingBox();
      expect(start!.y).toBeGreaterThanOrEqual(0);
      expect(start!.y + start!.height).toBeLessThanOrEqual(viewport.height);

      const framing = workspace.locator('.framing-options');
      await expect(framing).not.toHaveAttribute('open', '');
      await framing.locator('summary').focus();
      await page.keyboard.press('Enter');
      await expect(framing.getByLabel('Crop top (pixels)', { exact: true })).toBeVisible();
      expect(await page.evaluate(() => document.documentElement.scrollWidth)).toBe(viewport.width);
      await framing.locator('summary').click();
      await expect(framing.getByLabel('Crop top (pixels)', { exact: true })).not.toBeVisible();
    }

    for (const name of ['Batch encode', 'Remux', 'Utilities']) {
      await page.getByRole('button', { name, exact: true }).click();
      expect(await page.evaluate(() => document.documentElement.scrollWidth)).toBe(viewport.width);
    }
  });
}
