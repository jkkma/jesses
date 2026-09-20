import { test, expect, type Page } from '@playwright/test';
import type { UserPreferences, SavePreferencesRequest } from '../../src/lib/ipc/generated';

async function setup(page: Page) {
  await page.addInitScript(() => {
    const host = globalThis as unknown as Record<string, unknown>;
    host.isTauri = true;
    let prefs: UserPreferences = JSON.parse(localStorage.getItem('preferences-test') ?? 'null') ?? {
      general: { defaultOutputDirectory: 'C:\\exports', recursiveImport: true },
      recentPaths: ['C:\\media\\sample.mkv', 'C:\\media\\second.mkv', 'C:\\offline\\missing.mkv'],
      revision: 0,
    };
    const calls: { command: string; payload: Record<string, unknown> }[] = [];
    host.__preferenceCalls = calls;
    let callback = 0;
    const save = () => {
      localStorage.setItem('preferences-test', JSON.stringify(prefs));
      return structuredClone(prefs);
    };
    host.__TAURI_INTERNALS__ = {
      metadata: { currentWindow: { label: 'main' }, currentWebview: { label: 'main' } },
      transformCallback: () => ++callback,
      unregisterCallback: () => {},
      invoke: async (command: string, payload: Record<string, unknown> = {}) => {
        calls.push({ command, payload });
        if (command.startsWith('plugin:event|')) return ++callback;
        if (command === 'get_capabilities')
          return [
            'ffmpeg',
            'ffprobe',
            'svt-av1',
            'svt-av1-hdr',
            'svt-av1-5fish',
            'x264',
            'av1an',
          ].map((id) => ({
            id,
            name: id,
            available: true,
            path: `C:\\tools\\${id}.exe`,
            version: 'test',
            detail: null,
          }));
        if (command === 'subscribe_jobs') {
          (payload.channel as { onmessage: (value: unknown[]) => void }).onmessage([]);
          return;
        }
        if (command === 'get_storage_locations') return [['Preferences', 'C:\\data\\config']];
        if (command === 'get_preferences') return structuredClone(prefs);
        if (command === 'save_preferences') {
          const request = JSON.parse(JSON.stringify(payload.request)) as SavePreferencesRequest;
          prefs = {
            general: request.general,
            recentPaths: request.recentPaths ?? prefs.recentPaths,
            revision: prefs.revision + 1,
          };
          return save();
        }
        if (command === 'remember_recent_media') {
          prefs = {
            ...prefs,
            recentPaths: [...new Set([...(payload.paths as string[]), ...prefs.recentPaths])].slice(
              0,
              15,
            ),
            revision: prefs.revision + 1,
          };
          return save();
        }
        if (command === 'recent_path_is_folder') {
          if (String(payload.path).includes('offline'))
            throw {
              code: 'RECENT_MEDIA_UNAVAILABLE',
              message: 'The recent media is unavailable.',
            };
          return false;
        }
        if (command === 'probe_media')
          return {
            id: String(payload.path),
            path: payload.path,
            name: String(payload.path).split('\\').pop(),
            sizeBytes: '1000',
            durationSeconds: 10,
            format: 'matroska',
            streams: [
              {
                index: 0,
                kind: 'video',
                codec: 'h264',
                width: 320,
                height: 180,
                frameRate: '24/1',
                sampleRate: null,
                channels: null,
                language: null,
                title: null,
              },
            ],
          };
        if (command === 'plugin:dialog|open') return 'C:\\saved\\config.json';
        if (command === 'preview_preference_import')
          return {
            request: {
              general: { defaultOutputDirectory: 'D:\\imported', recursiveImport: true },
              recentPaths: ['D:\\media\\older.mkv'],
            },
            acceptedKeys: ['DefaultOutputDir', 'RecentFiles'],
            ignoredKeyCount: 12,
            warnings: [],
          };
        if (command === 'get_completion_status')
          return {
            options: { notify: false, finishAction: 'none' },
            armedJobs: 0,
            secondsRemaining: null,
            error: null,
          };
        throw new Error(`Unexpected command ${command}`);
      },
    };
  });
}
async function count(page: Page, command: string) {
  return page.evaluate(
    (name) =>
      (
        globalThis as unknown as { __preferenceCalls: { command: string }[] }
      ).__preferenceCalls.filter((c) => c.command === name).length,
    command,
  );
}

test('preferences persist, recent media reopens, and new destinations use the saved folder', async ({
  page,
}) => {
  await setup(page);
  await page.goto('/');
  await expect(
    page.getByRole('checkbox', { name: 'Include subfolders', exact: true }),
  ).toBeChecked();
  await page.getByLabel('Recent media', { exact: true }).selectOption('C:\\media\\sample.mkv');
  await page.getByRole('button', { name: 'Open recent', exact: true }).click();
  await page.getByRole('button', { name: 'Quick Convert', exact: true }).click();
  await expect(page.getByRole('textbox', { name: 'Encode destination', exact: true })).toHaveValue(
    'C:\\exports\\sample_av1_hdr.mkv',
  );
  await page.getByRole('button', { name: 'Tools & settings', exact: true }).click();
  await page.getByLabel('Default output folder', { exact: true }).fill('D:\\new exports');
  await page.getByRole('button', { name: 'Save preferences', exact: true }).click();
  await expect(page.getByText('Preferences saved.', { exact: false })).toBeVisible();
  await page.getByRole('button', { name: 'Quick Convert', exact: true }).click();
  await expect(page.getByRole('textbox', { name: 'Encode destination', exact: true })).toHaveValue(
    'C:\\exports\\sample_av1_hdr.mkv',
  );
  await page.reload();
  await page.getByRole('button', { name: 'Tools & settings', exact: true }).click();
  await expect(page.getByLabel('Default output folder', { exact: true })).toHaveValue(
    'D:\\new exports',
  );
  await page.getByRole('button', { name: 'Files 00', exact: true }).click();
  await page.getByLabel('Recent media', { exact: true }).selectOption('C:\\media\\sample.mkv');
  await page.getByRole('button', { name: 'Open recent', exact: true }).click();
  await page.getByRole('button', { name: 'Quick Convert', exact: true }).click();
  await expect(page.getByRole('textbox', { name: 'Encode destination', exact: true })).toHaveValue(
    'D:\\new exports\\sample_av1_hdr.mkv',
  );
});

test('import waits for explicit Apply and removes an unavailable recent only after open fails', async ({
  page,
}) => {
  await setup(page);
  await page.goto('/');
  await page.getByLabel('Recent media', { exact: true }).selectOption('C:\\offline\\missing.mkv');
  await page.getByRole('button', { name: 'Open recent', exact: true }).click();
  await expect(page.getByRole('alert')).toContainText('recent media is unavailable');
  await expect(
    page
      .getByLabel('Recent media', { exact: true })
      .locator('option[value="C:\\\\offline\\\\missing.mkv"]'),
  ).toHaveCount(0);
  await page.getByRole('button', { name: 'Tools & settings', exact: true }).click();
  await page.getByRole('button', { name: 'Import saved preferences', exact: true }).click();
  await expect(
    page.getByRole('heading', { name: 'Review preferences', exact: true }),
  ).toBeVisible();
  expect(await count(page, 'save_preferences')).toBe(1);
  await expect(page.getByText('12 unsupported keys skipped', { exact: false })).toBeVisible();
  await page.getByRole('button', { name: 'Apply imported preferences', exact: true }).click();
  await expect(page.getByLabel('Default output folder', { exact: true })).toHaveValue(
    'D:\\imported',
  );
  expect(await count(page, 'save_preferences')).toBe(2);
  await page.getByRole('button', { name: 'Clear recent media', exact: true }).click();
  await expect(page.getByText('Recent-media history cleared.', { exact: true })).toBeVisible();
  await page.getByRole('button', { name: 'Files 00', exact: true }).click();
  await expect(page.getByLabel('Recent media', { exact: true })).toHaveCount(0);
});

test('reopening an already loaded recent file selects it without duplicating or probing it', async ({
  page,
}) => {
  await setup(page);
  await page.goto('/');
  for (const path of ['C:\\media\\sample.mkv', 'C:\\media\\second.mkv', 'C:\\media\\sample.mkv']) {
    await page.getByLabel('Recent media', { exact: true }).selectOption(path);
    await page.getByRole('button', { name: 'Open recent', exact: true }).click();
    await expect(
      page.getByRole('button', {
        name: path.endsWith('second.mkv') ? /^second\.mkv/ : /^sample\.mkv/,
      }),
    ).toHaveAttribute('aria-pressed', 'true');
  }
  expect(await count(page, 'probe_media')).toBe(2);
  await expect(page.getByRole('button', { name: 'Files 02', exact: true })).toBeVisible();
});

test('clearing recent media preserves unsaved general edits until Save', async ({ page }) => {
  await setup(page);
  await page.goto('/');
  await page.getByRole('button', { name: 'Tools & settings', exact: true }).click();
  const folder = page.getByLabel('Default output folder', { exact: true });
  const recursive = page.getByRole('checkbox', {
    name: 'Include subfolders when adding a folder',
    exact: true,
  });
  await expect(folder).toHaveValue('C:\\exports');
  await folder.fill('D:\\unsaved exports');
  await recursive.uncheck();
  await page.getByRole('button', { name: 'Clear recent media', exact: true }).click();
  await expect(page.getByText('Recent-media history cleared.', { exact: true })).toBeVisible();
  await expect(folder).toHaveValue('D:\\unsaved exports');
  await expect(recursive).not.toBeChecked();
  const before = await page.evaluate(() => JSON.parse(localStorage.getItem('preferences-test')!));
  expect(before.general).toEqual({ defaultOutputDirectory: 'C:\\exports', recursiveImport: true });
  expect(before.recentPaths).toEqual([]);
  await page.getByRole('button', { name: 'Save preferences', exact: true }).click();
  await expect(page.getByText('Preferences saved.', { exact: false })).toBeVisible();
  await page.reload();
  await page.getByRole('button', { name: 'Tools & settings', exact: true }).click();
  await expect(folder).toHaveValue('D:\\unsaved exports');
  await expect(recursive).not.toBeChecked();
});
