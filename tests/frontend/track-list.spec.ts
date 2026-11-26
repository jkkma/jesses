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
    if (height === 600) await panel.locator('.track-option').first().click();
    for (const name of ['Select…', 'Sort…']) {
      const trigger = panel.getByRole('button', { name, exact: true });
      await trigger.click();
      const menuId = await trigger.getAttribute('popovertarget');
      const menu = page.locator(`#${menuId}`);
      await expect(menu).toBeVisible();
      const workspace = (await page.locator('main').boundingBox())!;
      for (const button of await menu.getByRole('button').all()) {
        await button.scrollIntoViewIfNeeded();
        const box = (await button.boundingBox())!;
        expect(box.y).toBeGreaterThanOrEqual(workspace.y);
        expect(box.y + box.height).toBeLessThanOrEqual(workspace.y + workspace.height);
      }
      await page.keyboard.press('Escape');
      await expect(menu).toBeHidden();
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
    await panel.locator('.track-option').filter({ hasText: 'Audio (AAC)' }).click();
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
      .getByRole('grid', { name: 'Tracks' });
    const last = list.getByRole('row').last();
    await expect(last).toContainText('Subtitle 29');
    // Showing a row's details takes more of the panel.
    await list.locator('.track-option').first().click();
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

test('Track List supports single-tab-stop keyboard navigation, Space toggle, and multi-file source labels', async ({
  page,
}) => {
  await installSourceEncodeFixture(page);
  const second = {
    ...source,
    id: 'second-file',
    path: '/media/second.mkv',
    name: 'second.mkv',
    streams: [{ ...source.streams[1], index: 0, title: 'Commentary' }],
  };
  await page.evaluate(
    ({ first, second }) => {
      const host = globalThis as any;
      host.__pickerPaths = [first.path, second.path];
      host.__mediaByPath = { [first.path]: first, [second.path]: second };
    },
    { first: source, second },
  );
  await page.getByRole('button', { name: 'Add Files…' }).click();
  await page.getByRole('option', { name: /input\.mkv/ }).click();
  await page.getByRole('option', { name: /second\.mkv/ }).click({ modifiers: ['Control'] });
  await page.getByRole('button', { name: 'Load Tracks' }).click();
  const tracks = page.getByRole('region', { name: 'Track List' });
  const grid = tracks.getByRole('grid', { name: 'Tracks' });
  const rows = grid.getByRole('row');
  const options = grid.locator('.track-option');
  await expect(options.first()).toContainText('· input.mkv');
  const lastRow = rows.last();
  const lastOption = options.last();
  await expect(lastOption).toContainText('· second.mkv');
  await expect(grid.locator('.track-option[tabindex="0"]')).toHaveCount(1);
  await options.first().focus();
  await page.keyboard.press('End');
  await expect(lastOption).toBeFocused();
  await expect(lastRow).toHaveAttribute('aria-selected', 'true');
  const lastCheckbox = tracks.getByRole('checkbox').last();
  await expect(lastCheckbox).not.toBeChecked();
  await page.keyboard.press(' ');
  await expect(lastCheckbox).toBeChecked();
  await expect(tracks.getByLabel('Default Audio').locator('option').nth(0)).toContainText(
    '· input.mkv',
  );
  await expect(tracks.getByLabel('Default Subtitles').locator('option').nth(1)).toContainText(
    '· input.mkv',
  );
});

test('Track List disambiguates duplicate basenames across directories', async ({ page }) => {
  await installSourceEncodeFixture(page);
  const first = {
    ...source,
    id: 'dir-a-ep',
    path: '/media/season-1/episode.mkv',
    name: 'episode.mkv',
  };
  const second = {
    ...source,
    id: 'dir-b-ep',
    path: '/media/season-2/episode.mkv',
    name: 'episode.mkv',
  };
  await page.evaluate(
    ({ first, second }) => {
      const host = globalThis as any;
      host.__pickerPaths = [first.path, second.path];
      host.__mediaByPath = { [first.path]: first, [second.path]: second };
    },
    { first, second },
  );
  await page.getByRole('button', { name: 'Add Files…' }).click();
  const fileOptions = page.getByRole('listbox', { name: 'Files' }).getByRole('option');
  await fileOptions.first().click();
  await fileOptions.nth(1).click({ modifiers: ['Control'] });
  await page.getByRole('button', { name: 'Load Tracks' }).click();
  const tracks = page.getByRole('region', { name: 'Track List' });
  const options = tracks.getByRole('grid', { name: 'Tracks' }).locator('.track-option');
  await expect(options.nth(1)).toContainText('· season-1/episode.mkv');
  await expect(options.last()).toContainText('· season-2/episode.mkv');
  await tracks.getByRole('checkbox', { name: /Include .*Audio .*season-2\/episode\.mkv/ }).check();
  const audioOptions = await tracks.getByLabel('Default Audio').locator('option').allTextContents();
  expect(audioOptions).toHaveLength(2);
  expect(new Set(audioOptions).size).toBe(2);
  expect(audioOptions[0]).toContain('season-1/episode.mkv');
  expect(audioOptions[1]).toContain('season-2/episode.mkv');
});
