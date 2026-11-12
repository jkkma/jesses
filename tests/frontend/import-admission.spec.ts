import { expect, test } from '@playwright/test';
import { installSourceEncodeFixture, source } from './helpers/source-encode-fixture';

test('a multi-file import keeps the same native reservation until all probes finish', async ({
  page,
}) => {
  await installSourceEncodeFixture(page);
  await page.goto('/');
  await page.evaluate((media) => {
    const host = globalThis as any;
    const invoke = host.__TAURI_INTERNALS__.invoke;
    const paths = ['/media/first.mkv', '/media/second.mkv'];
    host.__pickerPaths = paths;
    host.__mediaByPath = Object.fromEntries(
      paths.map((path, i) => [
        path,
        { ...media, id: 'import-' + i, name: path.split('/').at(-1), path },
      ]),
    );
    const secondProbe = new Promise<void>((resolve) => {
      host.__releaseSecondProbe = resolve;
    });
    host.__TAURI_INTERNALS__.invoke = async (command: string, payload: any = {}) => {
      if (command === 'begin_media_analysis' && payload.forImport) {
        host.__sourceCalls.push({ command, payload });
        return 'import-reservation';
      }
      const result = await invoke(command, payload);
      if (command === 'probe_media' && payload.path === paths[1]) await secondProbe;
      return result;
    };
  }, source);
  const importCalls = (): Promise<Array<[string, string | null]>> =>
    page.evaluate(() =>
      (globalThis as any).__sourceCalls
        .filter(
          ({ command, payload }: any) =>
            (command === 'begin_media_analysis' && payload.forImport) ||
            command === 'claim_media_import' ||
            command === 'probe_media' ||
            (command === 'cancel_media_analysis' && payload.id === 'import-reservation'),
        )
        .map(({ command, payload }: any) => [command, payload.importId ?? payload.id ?? null]),
    );

  await page.getByRole('button', { name: 'Add Files…' }).click();
  await expect
    .poll(async () => (await importCalls()).filter(([command]) => command === 'probe_media').length)
    .toBe(2);
  expect(await importCalls()).toEqual([
    ['begin_media_analysis', null],
    ['claim_media_import', 'import-reservation'],
    ['probe_media', 'import-reservation'],
    ['probe_media', 'import-reservation'],
  ]);
  await page.evaluate(() => (globalThis as any).__releaseSecondProbe());
  await expect.poll(importCalls).toEqual([
    ['begin_media_analysis', null],
    ['claim_media_import', 'import-reservation'],
    ['probe_media', 'import-reservation'],
    ['probe_media', 'import-reservation'],
    ['cancel_media_analysis', 'import-reservation'],
  ]);
  await expect(page.getByRole('option', { name: /second\.mkv/ })).toBeVisible();
});
