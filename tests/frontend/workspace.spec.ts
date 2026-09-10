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
];

async function desktopMock(page: Page, paths: string[]) {
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
          throw new Error(`Unexpected IPC command: ${command}`);
        },
      };
    },
    { media: fixture, tools: capabilities, pickedPaths: paths },
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
  await expect(page.getByLabel('Quality', { exact: true })).toHaveValue('30');
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
