import { expect, test } from '@playwright/test';
import { installSourceEncodeFixture, source } from './helpers/source-encode-fixture';

test('explicitly disabling default subtitles survives track edits and reloads', async ({
  page,
}) => {
  await installSourceEncodeFixture(page);
  await page.getByRole('button', { name: 'Add Files…' }).click();
  await page.getByRole('button', { name: 'Load Tracks' }).click();
  const subtitles = page.getByLabel('Default Subtitles');
  await subtitles.selectOption('');
  await page.getByRole('checkbox', { name: /Include .*Audio/ }).uncheck();
  await expect(subtitles).toHaveValue('');
  await page.getByRole('button', { name: 'File List', exact: true }).click();
  await page.getByRole('button', { name: 'Load Tracks' }).click();
  await expect(subtitles).toHaveValue('');
  await page.getByRole('button', { name: 'Quick Convert', exact: true }).click();
  await page.getByRole('button', { name: 'Run: Encode', exact: true }).click();
  await expect
    .poll(() =>
      page.evaluate(() =>
        (globalThis as any).__sourceCalls
          .find((call: any) => call.command === 'mock_start_encode')
          ?.payload.request.settings.trackOverrides.find((track: any) => track.streamIndex === 2),
      ),
    )
    .toMatchObject({ streamIndex: 2, default: false });
});

for (const height of [600, 720]) {
  test(`track action menus stay inside the workspace at ${height} px`, async ({ page }) => {
    await page.setViewportSize({ width: 1152, height });
    await installSourceEncodeFixture(page);
    await page.getByRole('button', { name: 'Add Files…' }).click();
    await page.getByRole('button', { name: 'Load Tracks' }).click();
    const panel = page.getByRole('region', { name: 'Track List' });
    if (height === 600) await panel.getByRole('option').first().click();
    for (const name of ['Select…', 'Sort…']) {
      const menu = panel
        .locator('details')
        .filter({ has: page.locator('summary', { hasText: name }) });
      await menu.locator('summary').click();
      const workspace = (await page.locator('main').boundingBox())!;
      for (const button of await menu.getByRole('button').all()) {
        await button.scrollIntoViewIfNeeded();
        const box = (await button.boundingBox())!;
        expect(box.y).toBeGreaterThanOrEqual(workspace.y);
        expect(box.y + box.height).toBeLessThanOrEqual(workspace.y + workspace.height);
      }
      await menu.locator('summary').click();
    }
    await panel.getByText('Select…', { exact: true }).click();
    await panel.getByRole('button', { name: 'Check None', exact: true }).click();
    for (const checkbox of await panel.getByRole('checkbox').all())
      await expect(checkbox).not.toBeChecked();
  });
}

for (const [sampleRate, displayed] of [
  [48000, '48 kHz'],
  [44100, '44.1 kHz'],
] as const) {
  test(`audio details display ${sampleRate} Hz in the correct units`, async ({ page }) => {
    await installSourceEncodeFixture(page);
    await page.evaluate((media) => (globalThis as any).__setSourceMedia(media), {
      ...source,
      streams: source.streams.map((stream) =>
        stream.kind === 'audio' ? { ...stream, sampleRate } : stream,
      ),
    });
    await page.getByRole('button', { name: 'Add Files…' }).click();
    await page.getByRole('button', { name: 'Load Tracks' }).click();
    const panel = page.getByRole('region', { name: 'Track List' });
    await panel.getByRole('option').filter({ hasText: 'Audio (AAC)' }).click();
    await expect(panel.locator('pre')).toContainText(`Sample Rate: ${displayed}`);
  });
}

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
