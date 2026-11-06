import { expect, test } from '@playwright/test';
import { installSourceEncodeFixture, source } from './helpers/source-encode-fixture';

// A long track list scrolls inside its own box. That box must fit the visible
// workspace, or its last rows sit below the page edge while the list is
// already fully scrolled and the mouse wheel cannot reach them.
for (const height of [600, 850]) {
  test(`the mouse wheel reaches the last of many tracks at ${height} px`, async ({ page }) => {
    await page.setViewportSize({ width: 1280, height });
    await installSourceEncodeFixture(page);
    const subtitle = source.streams.find((stream) => stream.kind === 'subtitle')!;
    const media = {
      ...source,
      streams: [
        ...source.streams.filter((stream) => stream.kind !== 'subtitle'),
        ...Array.from({ length: 27 }, (_, offset) => ({
          ...subtitle,
          index: 3 + offset,
          title: `Subtitle ${3 + offset}`,
        })),
      ],
    };
    await page.evaluate((media) => (globalThis as any).__setSourceMedia(media), media);
    await page.getByRole('button', { name: 'Add Files…' }).click();
    await page.getByRole('button', { name: 'Load Tracks' }).click();
    const list = page
      .getByRole('region', { name: 'Track List' })
      .getByRole('listbox', { name: 'Tracks' });
    const last = list.getByRole('option').last();
    await expect(last).toContainText('Subtitle 29');
    // Showing a row's details takes more of the panel.
    await list.getByRole('option').first().click();
    const workspace = page.locator('main');
    expect(await workspace.evaluate((node) => node.scrollHeight <= node.clientHeight + 1)).toBe(
      true,
    );
    const visible = (await workspace.boundingBox())!;
    const box = (await list.boundingBox())!;
    expect(box.y + box.height).toBeLessThanOrEqual(visible.y + visible.height + 1);
    await list.hover();
    for (let step = 0; step < 12; step++) await page.mouse.wheel(0, 400);
    await expect
      .poll(() =>
        list.evaluate((node) => node.scrollTop + node.clientHeight >= node.scrollHeight - 1),
      )
      .toBe(true);
    const row = (await last.boundingBox())!;
    expect(row.y + row.height).toBeLessThanOrEqual(box.y + box.height + 1);
    await expect(page.getByLabel('Default Subtitles')).toBeInViewport();
  });
}
