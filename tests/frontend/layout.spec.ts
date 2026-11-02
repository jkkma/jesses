import { expect, test } from '@playwright/test';
import { installSourceEncodeFixture } from './helpers/source-encode-fixture';

// The former tests opened the removed Jesses sample/Filters panels. The fork
// uses loaded files and source-shaped subtabs; keep the same viewport, compact
// numeric control and keyboard-navigation assertions on those actual surfaces.
for (const viewport of [
  { width: 800, height: 720 },
  { width: 1280, height: 850 },
  { width: 1920, height: 1080 },
]) {
  test(`source encode controls and tab keyboard navigation fit at ${viewport.width}px`, async ({
    page,
  }) => {
    await page.setViewportSize(viewport);
    await installSourceEncodeFixture(page);
    await page.getByRole('button', { name: 'Add Files…' }).click();
    await page.getByRole('button', { name: 'Load Tracks' }).click();
    for (const name of ['Quick Convert', 'AV1AN']) {
      await page.getByRole('button', { name, exact: true }).click();
      const workspace = page.getByRole('region', { name, exact: true });
      const quality = await workspace.getByLabel('Quality', { exact: true }).boundingBox();
      const preset = await workspace.getByLabel('Preset / Effort', { exact: true }).boundingBox();
      expect(quality!.width).toBeLessThanOrEqual(130);
      expect(preset!.width).toBeLessThanOrEqual(105);
      const run = await page
        .getByRole('button', {
          name: name === 'AV1AN' ? 'Run: AV1AN encode' : 'Run: Encode',
          exact: true,
        })
        .boundingBox();
      expect(run!.y).toBeGreaterThanOrEqual(0);
      expect(run!.y + run!.height).toBeLessThanOrEqual(viewport.height);
      const tabs = workspace.getByRole('tablist');
      const video = tabs.getByRole('tab', { name: 'Video', exact: true });
      await video.focus();
      await page.keyboard.press('End');
      const advanced = tabs.getByRole('tab', { name: 'Advanced', exact: true });
      await expect(advanced).toBeFocused();
      await expect(advanced).toHaveAttribute('aria-selected', 'true');
      await page.keyboard.press('Home');
      await expect(video).toBeFocused();
      await expect(video).toHaveAttribute('aria-selected', 'true');
      await page.keyboard.press('ArrowRight');
      await expect(tabs.getByRole('tab').nth(1)).toBeFocused();
      expect(await page.evaluate(() => document.documentElement.scrollWidth)).toBe(viewport.width);
    }
    for (const name of ['File List', 'Track List', 'Utilities', 'Settings']) {
      await page.getByRole('button', { name, exact: true }).click();
      expect(await page.evaluate(() => document.documentElement.scrollWidth)).toBe(viewport.width);
    }
  });
}
