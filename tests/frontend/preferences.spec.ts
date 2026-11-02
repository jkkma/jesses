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
        if (command === 'list_av1an_recoveries') return [];
        if (command === 'resolve_output_path') {
          const request = payload.request as any;
          const name =
            String(request.inputPath)
              .split(/[\\/]/)
              .at(-1)
              ?.replace(/\.[^.]+$/, '') ?? '';
          const stem =
            request.outputStem ?? `${request.defaultOutputDirectory || 'C:\\media'}\\${name}`;
          const ext =
            request.target.kind === 'container'
              ? request.target.format === 'matroska'
                ? 'mkv'
                : request.target.format
              : 'gif';
          return {
            outputStem: stem,
            outputPath: `${stem}.${ext}`,
            isDirectory: false,
            numbered: false,
          };
        }
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

// The fork uses a Recent menu and autosaves Settings on blur. The old import,
// clear-history and recursive-folder controls are not source screens.
async function openRecent(page: Page, path: string) {
  await page.getByText('Recent…', { exact: true }).click();
  await page.getByRole('button', { name: path, exact: true }).click();
}

test('saved output folder applies to new sources without rewriting an existing draft', async ({
  page,
}) => {
  await setup(page);
  await page.goto('/');
  await openRecent(page, 'C:\\media\\sample.mkv');
  await page.getByRole('button', { name: 'Load Tracks' }).click();
  await page.getByRole('button', { name: 'Quick Convert', exact: true }).click();
  const quick = page.getByRole('region', { name: 'Quick Convert', exact: true });
  await expect(quick.getByLabel('Output Path', { exact: true })).toHaveValue('C:\\exports\\sample');
  await page.getByRole('button', { name: 'Settings', exact: true }).click();
  const folder = page.getByPlaceholder('Leave empty to save next to the source file');
  await folder.fill('D:\\new exports');
  await folder.blur();
  await expect.poll(() => count(page, 'save_preferences')).toBe(1);
  await page.getByRole('button', { name: 'Quick Convert', exact: true }).click();
  await expect(quick.getByLabel('Output Path', { exact: true })).toHaveValue('C:\\exports\\sample');
  await page.reload();
  await page.getByRole('button', { name: 'Settings', exact: true }).click();
  await expect(folder).toHaveValue('D:\\new exports');
  await page.getByRole('button', { name: 'File List', exact: true }).click();
  await openRecent(page, 'C:\\media\\sample.mkv');
  await page.getByRole('button', { name: 'Load Tracks' }).click();
  await page.getByRole('button', { name: 'Quick Convert', exact: true }).click();
  await expect(quick.getByLabel('Output Path', { exact: true })).toHaveValue(
    'D:\\new exports\\sample',
  );
});

test('an unavailable recent is forgotten only after opening it fails', async ({ page }) => {
  await setup(page);
  await page.goto('/');
  expect(await count(page, 'save_preferences')).toBe(0);
  await openRecent(page, 'C:\\offline\\missing.mkv');
  await expect.poll(() => count(page, 'save_preferences')).toBe(1);
  await page.getByText('Recent…', { exact: true }).click();
  await expect(
    page.getByRole('button', { name: 'C:\\offline\\missing.mkv', exact: true }),
  ).toHaveCount(0);
  await expect(
    page.getByRole('button', { name: 'C:\\media\\sample.mkv', exact: true }),
  ).toBeVisible();
});

test('reopening a loaded recent selects it without duplicate probing', async ({ page }) => {
  await setup(page);
  await page.goto('/');
  await openRecent(page, 'C:\\media\\sample.mkv');
  await openRecent(page, 'C:\\media\\second.mkv');
  await expect(page.getByRole('option', { name: /second.mkv/ })).toBeVisible();
  await openRecent(page, 'C:\\media\\sample.mkv');
  await expect(page.getByRole('option', { name: /sample.mkv/ })).toHaveAttribute(
    'aria-selected',
    'true',
  );
  expect(await count(page, 'probe_media')).toBe(2);
  await expect(page.getByRole('option', { name: /sample.mkv/ })).toHaveCount(1);
});

test('source settings save the chosen default folder on blur', async ({ page }) => {
  await setup(page);
  await page.goto('/');
  await page.getByRole('button', { name: 'Settings', exact: true }).click();
  const folder = page.getByPlaceholder('Leave empty to save next to the source file');
  await folder.fill('D:\\renders');
  await folder.blur();
  await expect.poll(() => count(page, 'save_preferences')).toBe(1);
  await page.reload();
  await page.getByRole('button', { name: 'Settings', exact: true }).click();
  await expect(folder).toHaveValue('D:\\renders');
  await expect(page.getByRole('button', { name: 'Save preferences', exact: true })).toHaveCount(0);
});
