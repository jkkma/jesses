import { expect, test, type Page } from '@playwright/test';
import type { FolderScanResult, MediaFile, UserPreferences } from '../../src/lib/ipc/generated';

const videoStream = {
  index: 0,
  kind: 'video' as const,
  codec: 'ffv1',
  width: 320,
  height: 180,
  frameRate: '24/1',
  sampleRate: null,
  channels: null,
  language: null,
  title: null,
};

function media(id: string, path: string): MediaFile {
  return {
    id,
    path,
    name: path.split(/[\\/]/).at(-1)!,
    sizeBytes: '1024',
    durationSeconds: 1,
    format: 'matroska',
    streams: [videoStream],
  };
}

type FilesMock = {
  calls: { command: string; payload: Record<string, unknown> }[];
  drop: (paths: string[]) => void;
  dropReady: () => boolean;
  pickFiles: (paths: string[]) => void;
  release: (key: string) => void;
};

async function desktopMock(
  page: Page,
  options: {
    files: [string, MediaFile][];
    folders?: string[];
    scans?: [string, FolderScanResult][];
    pickerPaths?: string[];
    unreadable?: string[];
    held?: string[];
  },
) {
  await page.addInitScript(
    ({ options }) => {
      const host = globalThis as unknown as Record<string, unknown>;
      let pickerPaths = options.pickerPaths ?? [];
      const knownFiles = new Map(options.files);
      const folders = new Set(options.folders ?? []);
      const scans = new Map(options.scans ?? []);
      const unreadable = new Set(options.unreadable ?? []);
      const held = new Set(options.held ?? []);
      const waits = new Map<string, (() => void)[]>();
      const callbacks = new Map<number, (value: unknown) => void>();
      const calls: FilesMock['calls'] = [];
      let nextCallback = 0;
      let nextEvent = 0;
      let dragDrop: ((value: unknown) => void) | undefined;
      let prefs: UserPreferences = {
        general: { defaultOutputDirectory: '', recursiveImport: true },
        recentPaths: [],
        revision: 0,
      };
      const wait = async (key: string) => {
        if (held.has(key))
          await new Promise<void>((resolve) =>
            waits.set(key, [...(waits.get(key) ?? []), resolve]),
          );
      };
      const remember = (paths: string[]) => {
        const key = (path: string) =>
          path
            .replace(/^\\\\\?\\UNC\\/i, '\\\\')
            .replace(/^\\\\\?\\/i, '')
            .replaceAll('/', '\\')
            .replace(/\\+$/, '')
            .toLowerCase();
        const seen = new Set<string>();
        prefs = {
          ...prefs,
          recentPaths: [...paths, ...prefs.recentPaths]
            .filter((path) => {
              const normalized = key(path);
              if (seen.has(normalized)) return false;
              seen.add(normalized);
              return true;
            })
            .slice(0, 15),
          revision: prefs.revision + 1,
        };
        return structuredClone(prefs);
      };
      host.__filesMock = {
        calls,
        drop: (paths) => {
          if (!dragDrop) throw new Error('Drag/drop listener is not ready.');
          dragDrop({
            event: 'tauri://drag-drop',
            id: 1,
            payload: { paths, position: { x: 10, y: 10 } },
          });
        },
        dropReady: () => dragDrop !== undefined,
        pickFiles: (paths) => (pickerPaths = paths),
        release: (key) => {
          held.delete(key);
          waits.get(key)?.forEach((resolve) => resolve());
          waits.delete(key);
        },
      } satisfies FilesMock;
      host.isTauri = true;
      host.__TAURI_INTERNALS__ = {
        metadata: { currentWindow: { label: 'main' }, currentWebview: { label: 'main' } },
        transformCallback: (callback: (value: unknown) => void) => {
          const id = ++nextCallback;
          callbacks.set(id, callback);
          return id;
        },
        unregisterCallback: (id: number) => callbacks.delete(id),
        invoke: async (command: string, payload: Record<string, unknown> = {}) => {
          calls.push({ command, payload });
          if (command === 'plugin:event|listen') {
            if (payload.event === 'tauri://drag-drop')
              dragDrop = callbacks.get(payload.handler as number);
            return ++nextEvent;
          }
          if (command === 'plugin:event|unlisten') return;
          if (command === 'plugin:dialog|open') return pickerPaths;
          if (command === 'get_capabilities') return [];
          if (command === 'subscribe_jobs') {
            (payload.channel as { onmessage: (value: unknown[]) => void }).onmessage([]);
            return;
          }
          if (command === 'get_completion_status')
            return {
              options: { notify: false, finishAction: 'none' },
              armedJobs: 0,
              secondsRemaining: null,
              error: null,
            };
          if (command === 'get_preferences') return structuredClone(prefs);
          if (command === 'remember_recent_media') return remember(payload.paths as string[]);
          if (command === 'save_preferences') {
            const request = payload.request as {
              general: UserPreferences['general'];
              recentPaths: string[] | null;
            };
            prefs = {
              ...prefs,
              general: request.general,
              recentPaths: request.recentPaths ?? prefs.recentPaths,
              revision: prefs.revision + 1,
            };
            return structuredClone(prefs);
          }
          if (command === 'recent_path_is_folder') {
            const path = payload.path as string;
            await wait(`classify:${path}`);
            if (folders.has(path)) return true;
            if (knownFiles.has(path) || unreadable.has(path)) return false;
            throw {
              code: 'RECENT_MEDIA_UNAVAILABLE',
              message: 'The dropped path is unavailable.',
              path,
            };
          }
          if (command === 'scan_media_folder') {
            const request = payload.request as { path: string };
            await wait(`scan:${request.path}`);
            const result = scans.get(request.path);
            if (!result)
              throw {
                code: 'FOLDER_UNREADABLE',
                message: 'The dropped folder could not be read.',
                path: request.path,
              };
            return result;
          }
          if (command === 'probe_media') {
            const path = payload.path as string;
            await wait(`probe:${path}`);
            if (unreadable.has(path))
              throw {
                code: 'FILE_UNREADABLE',
                message: 'The dropped file could not be read.',
                path,
              };
            const value = knownFiles.get(path);
            if (value) return value;
            throw { code: 'FILE_NOT_FOUND', message: 'The dropped file is unavailable.', path };
          }
          throw new Error(`Unexpected command: ${command}`);
        },
      };
    },
    { options },
  );
}

async function calls(page: Page, command: string) {
  return page.evaluate(
    (name) =>
      (globalThis as unknown as { __filesMock: FilesMock }).__filesMock.calls.filter(
        (call) => call.command === name,
      ),
    command,
  );
}

async function drop(page: Page, paths: string[]) {
  await expect
    .poll(() =>
      page.evaluate(() =>
        (globalThis as unknown as { __filesMock: FilesMock }).__filesMock.dropReady(),
      ),
    )
    .toBe(true);
  await page.evaluate(
    (value) => (globalThis as unknown as { __filesMock: FilesMock }).__filesMock.drop(value),
    paths,
  );
}

async function release(page: Page, key: string) {
  await page.evaluate(
    (value) => (globalThis as unknown as { __filesMock: FilesMock }).__filesMock.release(value),
    key,
  );
}

async function pickFiles(page: Page, paths: string[]) {
  await page.evaluate(
    (value) => (globalThis as unknown as { __filesMock: FilesMock }).__filesMock.pickFiles(value),
    paths,
  );
}

test('mixed file and folder drops scan folders, retain per-entry errors, and preserve long Unicode paths', async ({
  page,
}) => {
  const firstFolder = 'C:\\drop\\set-a';
  const secondFolder = 'C:\\drop\\set-b';
  const firstNested = 'C:\\drop\\set-a\\Episode 01.mkv';
  const secondNested = 'C:\\drop\\set-b\\Episode 02.mkv';
  const longUnicode = `C:\\drop\\${'非常に長い名前'.repeat(40)}\\café 東京.mkv`;
  const unreadable = 'C:\\drop\\unreadable.mkv';
  await desktopMock(page, {
    files: [
      [firstNested, media('episode-1', firstNested)],
      [secondNested, media('episode-2', secondNested)],
      [longUnicode, media('unicode-long', longUnicode)],
    ],
    folders: [firstFolder, secondFolder],
    scans: [
      [firstFolder, { paths: [firstNested], errors: [], skippedCount: 0, truncated: false }],
      [
        secondFolder,
        {
          paths: [secondNested],
          errors: [
            {
              code: 'FOLDER_ENTRY_UNREADABLE',
              message: 'A protected child could not be read.',
              path: `${secondFolder}\\protected`,
            },
          ],
          skippedCount: 1,
          truncated: false,
        },
      ],
    ],
    unreadable: [unreadable],
  });
  await page.goto('/');
  await drop(page, [firstFolder, longUnicode, secondFolder, unreadable]);

  await expect(page.getByRole('button', { name: 'Files 03', exact: true })).toBeVisible();
  await expect(page.getByRole('heading', { name: 'Episode 02.mkv', exact: true })).toBeVisible();
  await expect(page.getByRole('alert')).toContainText('A protected child could not be read.');
  await expect(page.getByRole('alert')).toContainText('The dropped file could not be read.');
  expect((await calls(page, 'scan_media_folder')).map((call) => call.payload.request)).toEqual([
    { path: firstFolder, recursive: true },
    { path: secondFolder, recursive: true },
  ]);
  expect((await calls(page, 'probe_media')).map((call) => call.payload.path)).toContain(
    longUnicode,
  );
  const remembered = (await calls(page, 'remember_recent_media')).at(-1)!.payload.paths as string[];
  expect(remembered.slice(0, 2)).toEqual([firstFolder, secondFolder]);
  expect(remembered).toContain(longUnicode);
});

test('Windows spelling aliases are queued once and canonical media identity deduplicates short aliases', async ({
  page,
}) => {
  const canonical = 'C:\\Media Files\\Café 東京.mkv';
  const caseAlias = 'c:\\media files\\CAFÉ 東京.MKV';
  const verbatimAlias = '\\\\?\\C:\\MEDIA FILES\\CAFÉ 東京.MKV';
  const shortAlias = 'C:\\MEDIAF~1\\CAFÉ東~1.MKV';
  const value = media('canonical-media-id', canonical);
  await desktopMock(page, {
    files: [
      [canonical, value],
      [caseAlias, value],
      [verbatimAlias, value],
      [shortAlias, value],
    ],
    pickerPaths: [canonical, caseAlias, verbatimAlias, shortAlias],
  });
  await page.goto('/');
  await page.getByRole('button', { name: 'Add files', exact: true }).first().click();

  await expect(page.getByRole('button', { name: 'Files 01', exact: true })).toBeVisible();
  await expect(page.getByRole('button', { name: 'Remove Café 東京.mkv', exact: true })).toHaveCount(
    1,
  );
  expect((await calls(page, 'probe_media')).map((call) => call.payload.path)).toEqual([
    canonical,
    shortAlias,
  ]);
});

test('stopping a dropped folder scan suppresses its late result and restores import controls', async ({
  page,
}) => {
  const folder = 'C:\\drop\\slow-folder';
  const nested = `${folder}\\Episode.mkv`;
  await desktopMock(page, {
    files: [[nested, media('slow-episode', nested)]],
    folders: [folder],
    scans: [[folder, { paths: [nested], errors: [], skippedCount: 0, truncated: false }]],
    held: [`scan:${folder}`],
  });
  await page.goto('/');
  await drop(page, [folder]);
  await expect.poll(() => calls(page, 'scan_media_folder').then((value) => value.length)).toBe(1);
  await page.getByRole('button', { name: 'Stop import', exact: true }).click();
  await release(page, `scan:${folder}`);

  await expect(page.getByText(/Stopped importing. Completed files are kept/)).toBeVisible();
  await expect(page.getByRole('button', { name: 'Add folder', exact: true })).toBeEnabled();
  expect(await calls(page, 'probe_media')).toHaveLength(0);
  expect(await calls(page, 'remember_recent_media')).toHaveLength(0);
});

test('stopping during dropped-file classification never starts its probe after the classifier settles', async ({
  page,
}) => {
  const stopped = 'C:\\drop\\stopped.mkv';
  const replacement = 'C:\\drop\\replacement.mkv';
  await desktopMock(page, {
    files: [
      [stopped, media('stopped', stopped)],
      [replacement, media('replacement', replacement)],
    ],
    pickerPaths: [replacement],
    held: [`classify:${stopped}`],
  });
  await page.goto('/');
  await drop(page, [stopped]);
  await expect
    .poll(() => calls(page, 'recent_path_is_folder').then((value) => value.length))
    .toBe(1);
  await page.getByRole('button', { name: 'Stop import', exact: true }).click();
  await page.getByRole('button', { name: 'Add files', exact: true }).first().click();
  await expect(page.getByRole('heading', { name: 'replacement.mkv', exact: true })).toBeVisible();
  await release(page, `classify:${stopped}`);

  await expect.poll(() => calls(page, 'probe_media').then((value) => value.length)).toBe(1);
  expect((await calls(page, 'probe_media')).map((call) => call.payload.path)).toEqual([
    replacement,
  ]);
  await expect(page.getByRole('button', { name: 'Files 01', exact: true })).toBeVisible();
});

test('a stopped slow probe cannot reorder completed paths above a newer import in recents', async ({
  page,
}) => {
  const completed = 'C:\\drop\\completed-before-stop.mkv';
  const stopped = 'C:\\drop\\stopped-late.mkv';
  const replacement = 'C:\\drop\\newer.mkv';
  await desktopMock(page, {
    files: [
      [completed, media('completed', completed)],
      [stopped, media('stopped', stopped)],
      [replacement, media('replacement', replacement)],
    ],
    pickerPaths: [completed, stopped],
    held: [`probe:${stopped}`],
  });
  await page.goto('/');
  await page.getByRole('button', { name: 'Add files', exact: true }).first().click();
  await expect.poll(() => calls(page, 'probe_media').then((value) => value.length)).toBe(2);
  await page.getByRole('button', { name: 'Stop import', exact: true }).click();
  await pickFiles(page, [replacement]);
  await page.getByRole('button', { name: 'Add files', exact: true }).first().click();
  await expect(page.getByRole('heading', { name: 'newer.mkv', exact: true })).toBeVisible();
  await release(page, `probe:${stopped}`);

  await expect
    .poll(() => calls(page, 'remember_recent_media').then((value) => value.length))
    .toBe(2);
  const recentOptions = page.getByLabel('Recent media', { exact: true }).locator('option');
  await expect(recentOptions).toHaveCount(3);
  await expect(recentOptions.nth(1)).toHaveText(replacement);
  await expect(recentOptions.nth(2)).toHaveText(completed);
  await expect(page.getByRole('button', { name: 'Files 02', exact: true })).toBeVisible();
  await expect(page.getByText('stopped-late.mkv', { exact: true })).toHaveCount(0);
});
