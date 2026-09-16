import { expect, test, type Page } from '@playwright/test';
import type { MediaFile, ToolInfo } from '../../src/lib/ipc/generated';

const fixture: MediaFile = {
  id: 'fixture-a',
  path: 'C:\\media\\café 東京.mkv',
  name: 'café 東京.mkv',
  sizeBytes: '9007199254740993',
  durationSeconds: 1.001,
  format: 'matroska,webm',
  streams: [
    {
      index: 0,
      kind: 'video',
      codec: 'ffv1',
      width: 320,
      height: 180,
      frameRate: '24000/1001',
      sampleRate: null,
      channels: null,
      language: null,
      title: null,
    },
    {
      index: 3,
      kind: 'audio',
      codec: 'pcm_s16le',
      width: null,
      height: null,
      frameRate: null,
      sampleRate: 48000,
      channels: 2,
      language: 'eng',
      title: 'Stereo fixture',
    },
  ],
};
const capabilities: ToolInfo[] = [
  {
    id: 'ffprobe',
    name: 'FFprobe',
    available: true,
    path: 'C:\\tools\\ffprobe.exe',
    version: 'ffprobe test version',
    detail: null,
  },
  {
    id: 'svt-av1',
    name: 'SVT-AV1',
    available: false,
    path: null,
    version: null,
    detail: 'Install the standalone encoder.',
  },
  {
    id: 'svt-av1-5fish',
    name: 'SVT-AV1 5fish',
    available: true,
    path: 'C:\\tools\\5fish\\SvtAv1EncApp.exe',
    version: 'SVT-AV1 5fish test version',
    detail: null,
  },
  {
    id: 'svt-av1-hdr',
    name: 'SVT-AV1-HDR',
    available: false,
    path: null,
    version: null,
    detail: 'Install the HDR build in its own tool location.',
  },
];

async function desktopMock(page: Page, paths: string[], media = fixture) {
  await page.addInitScript(
    ({ media, tools, pickedPaths }) => {
      const state = globalThis as unknown as Record<string, unknown>;
      state.isTauri = true;
      state.__TAURI_INTERNALS__ = {
        metadata: { currentWindow: { label: 'main' }, currentWebview: { label: 'main' } },
        transformCallback: () => 1,
        invoke: async (
          command: string,
          payload: { path?: string; channel?: { onmessage: (value: unknown[]) => void } },
        ) => {
          if (command === 'subscribe_jobs') {
            payload.channel?.onmessage([]);
            return;
          }
          if (command === 'get_capabilities') return tools;
          if (command === 'plugin:dialog|open') return pickedPaths;
          if (command.startsWith('plugin:event|')) return 1;
          if (command === 'probe_media') {
            if (payload.path?.includes('broken'))
              throw { code: 'probe_failed', message: 'Invalid media fixture.' };
            return media;
          }
          if (command === 'get_completion_status')
            return {
              options: { notify: false, finishAction: 'none' },
              armedJobs: 0,
              secondsRemaining: null,
              error: null,
            };
          throw new Error(`Unexpected IPC command: ${command}`);
        },
      };
    },
    { media, tools: capabilities, pickedPaths: paths },
  );
}

test('browser preview starts empty, identifies sample data, and keeps encoding unavailable', async ({
  page,
}) => {
  await page.goto('/');
  await expect(page.getByRole('heading', { name: 'Your media starts here.' })).toBeVisible();
  await expect(page.getByRole('button', { name: 'Add files', exact: true }).first()).toBeDisabled();
  await page.getByRole('button', { name: 'Load sample', exact: true }).click();
  await expect(page.getByRole('heading', { name: 'Coastal walk.mkv' })).toBeVisible();
  await expect(
    page.getByText('Sample values for interface review. No file has been read.'),
  ).toBeVisible();
  await page.getByRole('button', { name: 'Quick Convert', exact: true }).click();
  await expect(page.getByRole('button', { name: 'Start encode' })).toBeDisabled();
  await expect(
    page
      .getByRole('region', { name: 'Quick Convert workspace' })
      .getByLabel('Quality', { exact: true }),
  ).toHaveValue('30');
  await page.getByRole('button', { name: 'av1an', exact: true }).click();
  const av1an = page.getByRole('region', { name: 'av1an workspace', exact: true });
  await expect(av1an).toBeVisible();
  await expect(av1an.getByRole('button', { name: 'Start encode', exact: true })).toBeDisabled();
  await expect(av1an.getByLabel('Parallel chunks', { exact: true })).toBeDisabled();
  await page.getByRole('button', { name: 'Change source' }).click();
  await page.getByRole('button', { name: 'Remove Coastal walk.mkv' }).click();
  await expect(page.getByRole('heading', { name: 'Your media starts here.' })).toBeVisible();
});

test('desktop import retains good files through errors, deduplicates, and exposes source stream indices', async ({
  page,
}) => {
  await desktopMock(page, [fixture.path, 'C:\\media\\broken.mkv', fixture.path]);
  await page.goto('/');
  await expect(page.getByRole('button', { name: 'Add files', exact: true }).first()).toHaveCSS(
    'color',
    'rgb(255, 249, 238)',
  );
  await expect(page.getByRole('button', { name: 'Add files', exact: true }).first()).toHaveCSS(
    'background-color',
    'rgb(173, 83, 38)',
  );
  await page.getByRole('button', { name: 'Add files', exact: true }).first().click();
  await expect(page.getByRole('heading', { name: fixture.name })).toBeVisible();
  await expect(page.getByRole('alert')).toContainText('Invalid media fixture.');
  await expect(page.getByRole('button', { name: `Remove ${fixture.name}` })).toHaveCount(1);
  await expect(page.getByText('#3', { exact: true })).toBeVisible();
  await expect(page.getByText('23.976 fps', { exact: true }).first()).toBeVisible();
  await page.getByRole('button', { name: 'Dismiss import errors' }).click();
  await expect(page.getByRole('alert')).toHaveCount(0);
  await page.getByRole('button', { name: 'Tools & settings', exact: true }).click();
  await expect(page.getByText('ffprobe test version', { exact: true })).toBeVisible();
  await expect(page.getByText('Install the standalone encoder.')).toBeVisible();
  await expect(page.getByRole('row').filter({ hasText: 'svt-av1-5fish' })).toContainText(
    'SVT-AV1 5fish test version',
  );
  await expect(page.getByRole('row').filter({ hasText: 'svt-av1-5fish' })).toContainText(
    'C:\\tools\\5fish\\SvtAv1EncApp.exe',
  );
  await expect(page.getByRole('row').filter({ hasText: 'svt-av1-hdr' })).toContainText('Not found');
  await expect(page.getByRole('row').filter({ hasText: 'svt-av1-hdr' })).toContainText(
    'Install the HDR build in its own tool location.',
  );
});

test('palette survives dark OS theme and minimum-size layout remains usable', async ({ page }) => {
  await page.emulateMedia({ colorScheme: 'dark', reducedMotion: 'reduce' });
  await page.setViewportSize({ width: 760, height: 600 });
  await page.goto('/');
  await expect(page.locator('html')).toHaveCSS('background-color', 'rgb(240, 238, 230)');
  await expect(page.getByRole('complementary', { name: 'Media inspector' })).toHaveCSS(
    'background-color',
    'rgb(227, 218, 204)',
  );
  await page.getByRole('button', { name: 'Load sample', exact: true }).click();
  expect(await page.evaluate(() => document.documentElement.scrollWidth <= window.innerWidth)).toBe(
    true,
  );
  await page.getByRole('button', { name: 'Quick Convert', exact: true }).click();
  expect(await page.evaluate(() => document.documentElement.scrollWidth <= window.innerWidth)).toBe(
    true,
  );
  await page.getByRole('button', { name: 'av1an', exact: true }).click();
  await expect(page.getByRole('heading', { name: 'av1an', exact: true })).toBeVisible();
  expect(await page.evaluate(() => document.documentElement.scrollWidth <= window.innerWidth)).toBe(
    true,
  );
  await page.screenshot({ path: test.info().outputPath('av1an-minimum-size.png'), fullPage: true });
});

test('keyboard opens native picker and activity disclosure retains state on reload', async ({
  page,
}) => {
  await desktopMock(page, [fixture.path]);
  await page.goto('/');
  await page.keyboard.press('Control+o');
  await expect(page.getByRole('heading', { name: fixture.name })).toBeVisible();
  await page.getByRole('button', { name: /Activity log/ }).click();
  await expect(page.getByRole('log')).toContainText(`Imported ${fixture.name}`);
  await page.reload();
  await expect(page.getByRole('button', { name: /Activity log/ })).toHaveAttribute(
    'aria-expanded',
    'true',
  );
});

test('inspector exposes HDR detection and color precision without treating missing header metadata as absent', async ({
  page,
}) => {
  const hdr: MediaFile = {
    ...fixture,
    streams: [
      {
        ...fixture.streams[0],
        codec: 'hevc',
        width: 3840,
        height: 2160,
        pixelFormat: 'yuv420p10le',
        bitDepth: 10,
        colorPrimaries: 'bt2020',
        colorTransfer: 'smpte2084',
        colorSpace: 'bt2020nc',
        colorRange: 'tv',
        hdrFormat: 'HDR / PQ',
        hasHdrStaticMetadata: false,
        dynamicHdrFormats: ['Dolby Vision'],
      },
    ],
  };
  await desktopMock(page, [hdr.path], hdr);
  await page.setViewportSize({ width: 760, height: 600 });
  await page.goto('/');
  await page.getByRole('button', { name: 'Add files', exact: true }).first().click();
  const inspector = page.getByRole('complementary', { name: 'Media inspector' });
  await expect(inspector).toContainText('yuv420p10le · 10-bit');
  await expect(inspector).toContainText('bt2020');
  await expect(inspector).toContainText('smpte2084');
  await expect(inspector).toContainText('bt2020nc · tv');
  await expect(inspector).toContainText('HDR / PQ');
  await expect(inspector).toContainText('Dolby Vision');
  await expect(inspector).toContainText('Not reported in stream headers');
  await expect(inspector).toContainText('Frame metadata may contain additional HDR information');
  expect(await page.evaluate(() => document.documentElement.scrollWidth <= innerWidth)).toBe(true);
  await page.screenshot({ path: test.info().outputPath('hdr-inspector.png'), fullPage: true });
  await page.getByRole('button', { name: 'Quick Convert', exact: true }).click();
  await expect(
    page
      .getByRole('region', { name: 'Quick Convert workspace' })
      .getByLabel('Allow HDR10 fallback', { exact: true }),
  ).not.toBeChecked();
});
