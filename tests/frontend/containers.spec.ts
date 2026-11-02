import { expect, test } from '@playwright/test';
import { installSourceEncodeFixture, source } from './helpers/source-encode-fixture';

// The source box is an extensionless stem; a separate reviewed-queue page and
// its container summary are not fork controls. Keep extension and immutable
// request checks on Quick Convert and AV1AN's actual output controls.
test('selected container determines the written extension and a later edit cannot change the run', async ({
  page,
}) => {
  await installSourceEncodeFixture(page);
  await page.getByRole('button', { name: 'Add Files…' }).click();
  await page.getByRole('button', { name: 'Load Tracks' }).click();
  await page.getByRole('button', { name: 'Quick Convert', exact: true }).click();
  const quick = page.getByRole('region', { name: 'Quick Convert', exact: true });
  await quick.getByLabel('Container').selectOption('mp4');
  await quick.getByLabel('Output Path', { exact: true }).fill('/media/clip.with.dots');
  await page.evaluate(() => {
    const h = globalThis as any;
    h.__holdPrepare = new Promise((resolve) => (h.__releasePrepare = resolve));
  });
  await page.getByRole('button', { name: 'Run: Encode', exact: true }).click();
  await expect
    .poll(() =>
      page.evaluate(() =>
        (globalThis as any).__sourceCalls.some((c: any) => c.command === 'prepare_command_edit'),
      ),
    )
    .toBe(true);
  await quick.getByLabel('Container').selectOption('webm');
  await page.evaluate(() => (globalThis as any).__releasePrepare());
  await expect
    .poll(() =>
      page.evaluate(
        () =>
          (globalThis as any).__sourceCalls.find((c: any) => c.command === 'mock_start_encode')
            ?.payload.request.source.outputPath,
      ),
    )
    .toBe('/media/clip.with.dots.mp4');
  await expect(quick.getByLabel('Output Path', { exact: true })).toHaveValue(
    '/media/clip.with.dots',
  );
  await page.getByRole('button', { name: 'AV1AN', exact: true }).click();
  const av = page.getByRole('region', { name: 'AV1AN', exact: true });
  await av.getByLabel('Container').selectOption('mp4');
  await av.getByLabel('Output Path', { exact: true }).fill('/media/av1an');
  await expect(av.getByLabel('Container')).toHaveValue('mp4');
});

test('batch captures container and paths once before asynchronous preparation', async ({
  page,
}) => {
  await installSourceEncodeFixture(page);
  const second = { ...source, id: 'source-2', path: '/other/second.mkv', name: 'second.mkv' };
  await page.evaluate(
    ({ first, second }) => {
      const h = globalThis as any;
      h.__pickerPaths = [first.path, second.path];
      h.__mediaByPath = { [first.path]: first, [second.path]: second };
      h.__batchOutputPaths = ['/media/first.webm', '/other/second.webm'];
      h.__holdPrepare = new Promise((resolve) => (h.__releasePrepare = resolve));
    },
    { first: source, second },
  );
  await page.getByRole('button', { name: 'Add Files…' }).click();
  await page.getByRole('combobox', { name: 'Mode', exact: true }).selectOption('batch');
  await page.getByRole('button', { name: 'Quick Convert', exact: true }).click();
  const quick = page.getByRole('region', { name: 'Quick Convert', exact: true });
  await quick.getByLabel('Container').selectOption('webm');
  await page.getByRole('button', { name: 'Run Batch: Encode' }).click();
  await expect
    .poll(() =>
      page.evaluate(() =>
        (globalThis as any).__sourceCalls.some((c: any) => c.command === 'prepare_command_edit'),
      ),
    )
    .toBe(true);
  await quick.getByLabel('Container').selectOption('mp4');
  await page.evaluate(() => (globalThis as any).__releasePrepare());
  await expect
    .poll(() =>
      page.evaluate(
        () =>
          (globalThis as any).__sourceCalls.filter((c: any) => c.command === 'mock_start_encode')
            .length,
      ),
    )
    .toBe(2);
  const calls = await page.evaluate(() => (globalThis as any).__sourceCalls);
  expect(
    calls.find((c: any) => c.command === 'preview_encode_batch').payload.request.outputContainer,
  ).toBe('webm');
  expect(
    calls
      .filter((c: any) => c.command === 'mock_start_encode')
      .map((c: any) => c.payload.request.source.outputPath),
  ).toEqual(['/media/first.webm', '/other/second.webm']);
});
