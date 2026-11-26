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
  await page.getByRole('button', { name: 'Track List', exact: true }).click();
  const defaultAudio = page.getByLabel('Default Audio', { exact: true });
  await expect(defaultAudio).toBeDisabled();
  await expect(defaultAudio.locator('option')).toHaveText(['None']);

  await page.getByRole('button', { name: 'File List', exact: true }).click();
  await page.getByRole('combobox', { name: 'Mode', exact: true }).selectOption('batch');
  await expect(page.getByRole('button', { name: 'Sort…', exact: true })).toBeDisabled();
  await page.getByRole('button', { name: 'Batch naming help', exact: true }).click();
  await expect(page.getByText(/Use \{name\}/)).toBeVisible();
  await page.keyboard.press('Escape');
  await expect(page.getByText(/Use \{name\}/)).toBeHidden();
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
  await page
    .getByRole('dialog', { name: 'Argument Details' })
    .getByRole('button', { name: 'Close' })
    .last()
    .click();

  const filterInput = av1an.getByLabel('Filter arguments', { exact: true });
  await filterInput.fill('grain');
  await expect(av1an.getByRole('region', { name: /Filtered arguments matching/ })).toBeVisible();
  await firstTab.focus();
  await page.keyboard.press('ArrowRight');
  await expect(filterInput).toHaveValue('');
  await expect(secondTab).toHaveAttribute('aria-selected', 'true');
  await expect(av1an.getByRole('tabpanel', { name: 'HDR Metadata' })).toBeVisible();

  // x265 inline HDR metadata fields must not render Browse file-picker buttons.
  await page.getByRole('button', { name: 'Quick Convert', exact: true }).click();
  const quick = page.getByRole('region', { name: 'Quick Convert', exact: true });
  await quick.getByLabel('Codec', { exact: true }).selectOption('x265Standalone');
  await quick.getByRole('tab', { name: 'Advanced', exact: true }).click();
  await quick.getByRole('tab', { name: 'HDR Metadata', exact: true }).click();
  await expect(quick.getByLabel('master-display value')).toBeVisible();
  await expect(
    quick.getByRole('tabpanel', { name: 'HDR Metadata' }).getByRole('button', { name: /Browse/ }),
  ).toHaveCount(0);
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
  await expect(page.getByRole('textbox', { name: 'Save encodes to', exact: true })).toBeVisible();
});

test('active subtab stays dark on hover and Batch Mode Output Names does not overlap Mode select', async ({
  page,
}) => {
  await page.goto('/');
  await page.getByRole('button', { name: 'AV1AN', exact: true }).click();
  const av = page.getByRole('region', { name: 'AV1AN', exact: true });
  const audioTab = av.getByRole('tab', { name: 'Audio & Tracks', exact: true });
  await audioTab.click();
  await audioTab.hover();
  const colors = await audioTab.evaluate((el) => {
    const style = getComputedStyle(el);
    return { bg: style.backgroundColor, fg: style.color };
  });
  expect(colors.bg).not.toBe('rgb(207, 200, 177)');

  for (const width of [1280, 760]) {
    await page.setViewportSize({ width, height: 700 });
    await page.getByRole('button', { name: 'File List', exact: true }).click();
    await page.getByRole('combobox', { name: 'Mode', exact: true }).selectOption('batch');
    const modeBox = (await page
      .getByRole('combobox', { name: 'Mode', exact: true })
      .boundingBox())!;
    const outLabel = await page.getByText('Output Names', { exact: true }).boundingBox();
    expect(outLabel).not.toBeNull();
    const overlapsHorizontally =
      outLabel!.x < modeBox.x + modeBox.width && outLabel!.x + outLabel!.width > modeBox.x;
    const overlapsVertically =
      outLabel!.y < modeBox.y + modeBox.height && outLabel!.y + outLabel!.height > modeBox.y;
    expect(overlapsHorizontally && overlapsVertically).toBe(false);
  }
});

test('compact log splitter keeps valid ARIA bounds and ArrowDown never grows a compact log', async ({
  page,
}) => {
  await page.setViewportSize({ width: 760, height: 600 });
  await page.goto('/');
  const splitter = page.getByRole('separator', { name: 'Resize log', exact: true });
  const before = Number(await splitter.getAttribute('aria-valuenow'));
  const min = Number(await splitter.getAttribute('aria-valuemin'));
  expect(min).toBeLessThanOrEqual(before);
  await splitter.focus();
  await page.keyboard.press('ArrowDown');
  const after = Number(await splitter.getAttribute('aria-valuenow'));
  expect(after).toBeLessThanOrEqual(before);
});

test('Settings storage omits Open on Storage mode and shows row errors without hiding directories', async ({
  page,
}) => {
  await installSourceEncodeFixture(page);
  await page.evaluate(() => {
    const orig = (globalThis as any).__TAURI_INTERNALS__.invoke;
    (globalThis as any).__TAURI_INTERNALS__.invoke = async (
      command: string,
      payload: Record<string, any> = {},
    ) => {
      if (command === 'get_storage_locations')
        return [
          ['Storage mode', 'portable'],
          ['Preferences', 'C:\\Users\\Test\\jesses\\prefs'],
          ['Cache', 'C:\\Users\\Test\\jesses\\cache'],
        ];
      if (command === 'open_directory') throw new Error('Directory unavailable');
      return orig(command, payload);
    };
  });
  await page.getByRole('button', { name: 'Settings', exact: true }).click();
  await page.getByText('APPLICATION STORAGE', { exact: true }).click();
  await expect(page.getByText('portable', { exact: true })).toBeVisible();
  await expect(page.getByRole('button', { name: 'Open Storage mode folder' })).toHaveCount(0);
  await page.getByRole('button', { name: 'Open Preferences folder' }).click();
  await expect(page.getByRole('alert')).toContainText('Directory unavailable');
  await expect(page.getByText('C:\\Users\\Test\\jesses\\cache')).toBeVisible();
  // Retrying an action must keep the disclosure open even while clearing/replacing the error.
  await page.getByRole('button', { name: 'Copy Preferences path' }).click();
  await expect(page.getByRole('button', { name: 'Open Preferences folder' })).toBeVisible();
});

test('compact log height between 64 and 89 px persists across reload', async ({ page }) => {
  await page.setViewportSize({ width: 1152, height: 720 });
  await page.goto('/');
  const splitter = page.getByRole('separator', { name: 'Resize log', exact: true });
  await splitter.focus();
  // Adjust height into the 64..89 px range using keyboard steps (starts at 140 px -> 80 px).
  for (let i = 0; i < 6; i++) await page.keyboard.press('ArrowDown');
  await expect(splitter).toHaveAttribute('aria-valuenow', '80');
  await page.reload();
  await expect(page.getByRole('separator', { name: 'Resize log', exact: true })).toHaveAttribute(
    'aria-valuenow',
    '80',
  );
});

test('Job history labels video-copy with transcoded audio as Quick Convert rather than Remux', async ({
  page,
}) => {
  await installSourceEncodeFixture(page);
  await page.evaluate(() => {
    const host = globalThis as any;
    host.__testJobs = [
      {
        id: 'remux-1',
        state: 'succeeded',
        request: {
          inputPath: '/media/pure.mkv',
          outputPath: '/media/pure-remux.mkv',
          streamIndices: [0, 1],
        },
        encodeSettings: {
          backend: 'standalone',
          encoder: 'svtAv1',
          videoMode: 'copy',
          audio: [{ streamIndex: 1, codec: 'copy', bitrateKbps: 0, channels: 'preserve' }],
          framing: {
            crop: { left: 0, right: 0, top: 0, bottom: 0 },
            resizeWidth: null,
            borders: { left: 0, right: 0, top: 0, bottom: 0 },
          },
          workers: 2,
          videoStreamIndex: 0,
          crf: 20,
          preset: 4,
          lossless: false,
          filmGrain: 0,
          lineartPsyBias: 0,
          texturePsyBias: 0,
          hdrTune: 'visualQuality',
          hdr10Fallback: false,
        },
        progressSeconds: 12,
        durationSeconds: 12,
        logs: [],
        error: null,
        logPath: null,
      },
      {
        id: 'transcode-audio-1',
        state: 'succeeded',
        request: {
          inputPath: '/media/audio-conv.mkv',
          outputPath: '/media/audio-conv-out.mkv',
          streamIndices: [0, 1],
        },
        encodeSettings: {
          backend: 'standalone',
          encoder: 'svtAv1',
          videoMode: 'copy',
          audio: [{ streamIndex: 1, codec: 'opus', bitrateKbps: 160, channels: 'stereo' }],
          framing: {
            crop: { left: 0, right: 0, top: 0, bottom: 0 },
            resizeWidth: null,
            borders: { left: 0, right: 0, top: 0, bottom: 0 },
          },
          workers: 2,
          videoStreamIndex: 0,
          crf: 20,
          preset: 4,
          lossless: false,
          filmGrain: 0,
          lineartPsyBias: 0,
          texturePsyBias: 0,
          hdrTune: 'visualQuality',
          hdr10Fallback: false,
        },
        progressSeconds: 12,
        durationSeconds: 12,
        logs: [],
        error: null,
        logPath: null,
      },
    ];
    host.__emitJobs();
  });
  await page.getByRole('button', { name: 'Job history', exact: true }).click();
  const options = await page
    .getByRole('region', { name: 'Job history', exact: true })
    .getByRole('combobox', { name: 'Job from history' })
    .locator('option')
    .allTextContents();
  expect(options.find((text) => text.includes('pure-remux.mkv'))).toContain('· Remux ·');
  expect(options.find((text) => text.includes('audio-conv-out.mkv'))).toContain(
    '· Quick Convert ·',
  );
});
