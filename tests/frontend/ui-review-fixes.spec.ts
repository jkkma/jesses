import { expect, test } from '@playwright/test';
import { installSourceEncodeFixture } from './helpers/source-encode-fixture';

test('minimum window size 760x600 keeps the footer and Run button on screen without outer overflow', async ({
  page,
}) => {
  await page.setViewportSize({ width: 760, height: 600 });
  await page.goto('/');
  for (const tab of [
    'File List',
    'Track List',
    'AV1AN',
    'Quick Convert',
    'Utilities',
    'Settings',
  ]) {
    await page.getByRole('button', { name: tab, exact: true }).click();
    const metrics = await page.evaluate(() => ({
      scrollWidth: document.documentElement.scrollWidth,
      innerWidth: window.innerWidth,
      scrollHeight: document.documentElement.scrollHeight,
      innerHeight: window.innerHeight,
    }));
    expect(metrics.scrollWidth, `${tab} horizontal overflow`).toBeLessThanOrEqual(
      metrics.innerWidth,
    );
    expect(metrics.scrollHeight, `${tab} vertical overflow`).toBeLessThanOrEqual(
      metrics.innerHeight,
    );
    const run = await page.locator('footer .run-button').boundingBox();
    expect(run, `${tab} Run button`).not.toBeNull();
    expect(run!.y + run!.height, `${tab} Run bottom`).toBeLessThanOrEqual(600);
  }
});

test('empty Track List shows None for Default Audio and File List popovers light-dismiss', async ({
  page,
}) => {
  await page.goto('/');
  await page.getByRole('combobox', { name: 'Mode', exact: true }).selectOption('batch');
  await page.getByRole('button', { name: 'Batch naming help', exact: true }).click();
  await expect(page.getByText(/Use \{name\}/)).toBeVisible();
  // Opening Sort… must auto-dismiss the open help popover so they never overlap.
  await page.getByRole('button', { name: 'Sort…', exact: true }).click();
  await expect(page.getByText(/Use \{name\}/)).toBeHidden();
  await expect(page.getByRole('button', { name: 'Name (A-Z)', exact: true })).toBeVisible();
  await page.keyboard.press('Escape');
  await expect(page.getByRole('button', { name: 'Name (A-Z)', exact: true })).toBeHidden();

  await page.getByRole('button', { name: 'Track List', exact: true }).click();
  const defaultAudio = page.getByLabel('Default Audio', { exact: true });
  await expect(defaultAudio).toBeDisabled();
  await expect(defaultAudio.locator('option')).toHaveText(['None']);
});

test('Advanced tab selects its initial category, supports keyboard tabs, and exposes accessible row help', async ({
  page,
}) => {
  await page.goto('/');
  await page.getByRole('button', { name: 'AV1AN', exact: true }).click();
  const av1an = page.getByRole('region', { name: 'AV1AN', exact: true });
  await av1an.getByRole('tab', { name: 'Advanced', exact: true }).click();
  const categories = av1an.getByRole('tablist', { name: 'Encoder argument categories' });
  const firstTab = categories.getByRole('tab', { name: 'Film Grain & Noise', exact: true });
  const secondTab = categories.getByRole('tab', { name: 'HDR Metadata', exact: true });
  await expect(firstTab).toHaveAttribute('aria-selected', 'true');
  await expect(av1an.getByLabel('fgs-table value')).toBeVisible();
  await firstTab.focus();
  await page.keyboard.press('ArrowRight');
  await expect(secondTab).toBeFocused();
  await expect(secondTab).toHaveAttribute('aria-selected', 'true');
  await expect(av1an.getByLabel('dolby-vision-rpu value')).toBeVisible();
  // Right-clicking an input should not hijack the native editing context menu.
  await av1an.getByLabel('dolby-vision-rpu value').click({ button: 'right' });
  await expect(page.getByRole('dialog', { name: 'Argument Details' })).toHaveCount(0);
  // Keyboard-accessible ? details button opens Argument Details.
  await av1an.getByRole('button', { name: 'Details for dolby-vision-rpu' }).click();
  await expect(page.getByRole('dialog', { name: 'Argument Details' })).toBeVisible();
});

test('Trim dialog hides selection fill when no file is loaded and disambiguates playhead actions', async ({
  page,
}) => {
  await page.goto('/');
  await page.getByRole('button', { name: 'Quick Convert', exact: true }).click();
  const quick = page.getByRole('region', { name: 'Quick Convert', exact: true });
  await quick.getByRole('button', { name: 'Configure…', exact: true }).first().click();
  const dialog = page.getByRole('dialog', { name: 'Configure Trim' });
  await expect(dialog.locator('.range > div')).toHaveCount(0);
  await expect(dialog.getByRole('button', { name: 'Jump to start point' })).toBeVisible();
  await expect(dialog.getByRole('button', { name: 'Jump to end point' })).toBeVisible();
  await expect(dialog.getByRole('button', { name: 'Set start to playhead' })).toBeVisible();
  await expect(dialog.getByRole('button', { name: 'Set end to playhead' })).toBeVisible();
});

test('Utilities avoids nested buttons, shows Batch Mode status, and Settings has monotonic headings', async ({
  page,
}) => {
  await installSourceEncodeFixture(page);
  await page.getByRole('combobox', { name: 'Mode', exact: true }).selectOption('batch');
  await page.getByRole('button', { name: 'Utilities', exact: true }).click();
  const utilities = page.getByRole('region', { name: 'Utilities', exact: true });
  await expect(utilities.getByRole('status')).toContainText('Batch Mode');
  expect(
    await utilities.evaluate(
      (root) => root.querySelectorAll('button button, [role="button"] button').length,
    ),
  ).toBe(0);

  await page.getByRole('button', { name: 'Settings', exact: true }).click();
  const headingLevels = await page
    .locator('main')
    .evaluate((main) =>
      [...main.querySelectorAll('h1, h2, h3, h4, h5, h6')].map((el) => Number(el.tagName[1])),
    );
  expect(headingLevels[0]).toBe(1);
  expect(headingLevels.filter((level) => level === 1)).toHaveLength(1);
  await expect(
    page.getByRole('button', { name: 'Browse default output folder', exact: true }),
  ).toBeVisible();
});
