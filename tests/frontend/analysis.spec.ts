import { expect, test } from '@playwright/test';
import { installSourceEncodeFixture, source } from './helpers/source-encode-fixture';

// The source has a Track List thumbnail and a separate trim editor, not the
// former inspector with manually applied crop/field proposals. Trim bounds,
// snapping, registration cancellation and stale preview/keyframe replies are
// exercised in source-encode-parity.spec.ts.
test('changing the loaded source ignores the previous thumbnail and keeps display orientation', async ({
  page,
}) => {
  await installSourceEncodeFixture(page);
  const second = { ...source, id: 'second', path: '/other/second.mkv', name: 'second.mkv' };
  await page.evaluate(
    ({ first, second }) => {
      const host = globalThis as any;
      host.__pickerPaths = [first.path, second.path];
      host.__mediaByPath = { [first.path]: first, [second.path]: second };
      host.__holdPreviews = true;
    },
    { first: source, second },
  );
  await page.getByRole('button', { name: 'Add Files…' }).click();
  await page.getByRole('option', { name: /input.mkv/ }).click();
  await page.getByRole('button', { name: 'Load Tracks' }).click();
  await expect
    .poll(() =>
      page.evaluate(
        () =>
          (globalThis as any).__pendingPreviews?.some(
            (p: any) =>
              p.request.inputPath === '/media/input.mkv' && p.request.positionSeconds === 2.4,
          ) ?? false,
      ),
    )
    .toBe(true);
  await page.getByRole('button', { name: 'File List', exact: true }).click();
  await page.getByRole('option', { name: /input.mkv/ }).click();
  await page.getByRole('button', { name: 'Remove', exact: true }).click();
  await page.getByRole('option', { name: /second.mkv/ }).click();
  await page.getByRole('button', { name: 'Load Tracks' }).click();
  await expect
    .poll(() =>
      page.evaluate(
        () =>
          (globalThis as any).__pendingPreviews?.some(
            (p: any) =>
              p.request.inputPath === '/other/second.mkv' && p.request.positionSeconds === 2.4,
          ) ?? false,
      ),
    )
    .toBe(true);
  const requests = await page.evaluate(() =>
    (globalThis as any).__pendingPreviews
      .filter((p: any) => p.request.positionSeconds === 2.4)
      .map((p: any) => p.request),
  );
  expect(requests).toContainEqual(
    expect.objectContaining({
      inputPath: '/media/input.mkv',
      videoStreamIndex: 0,
      displayOrientation: true,
    }),
  );
  expect(requests).toContainEqual(
    expect.objectContaining({
      inputPath: '/other/second.mkv',
      videoStreamIndex: 0,
      displayOrientation: true,
    }),
  );
  await page.evaluate(() => {
    const pending = (globalThis as any).__pendingPreviews;
    const result = (suffix: string) => ({
      imageDataUrl: `data:image/png;base64,aQ==#${suffix}`,
      width: 1,
      height: 1,
      sourceWidth: 1,
      sourceHeight: 1,
      positionSeconds: 2.4,
      sourceFingerprint: 'test',
    });
    for (const item of pending.filter(
      (p: any) => p.request.inputPath === '/other/second.mkv' && p.request.positionSeconds === 2.4,
    ))
      item.resolve(result('new'));
    for (const item of pending.filter(
      (p: any) => p.request.inputPath === '/media/input.mkv' && p.request.positionSeconds === 2.4,
    ))
      item.resolve(result('old'));
  });
  await expect(page.getByAltText('Thumbnail of second.mkv')).toHaveAttribute(
    'src',
    'data:image/png;base64,aQ==#new',
  );
});
