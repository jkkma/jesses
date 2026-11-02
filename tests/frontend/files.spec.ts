import { expect, test } from '@playwright/test';
import { installSourceEncodeFixture, source } from './helpers/source-encode-fixture';

test('mixed import retains valid Unicode media, reports failures and deduplicates Windows aliases', async ({
  page,
}) => {
  await installSourceEncodeFixture(page);
  const canonical = 'C:\\Media Files\\café 東京.mkv';
  const alias = 'c:\\media files\\CAFÉ 東京.MKV';
  const shortAlias = 'C:\\MEDIAF~1\\CAFE~1.MKV';
  const broken = 'C:\\Media Files\\broken.mkv';
  const media = { ...source, id: 'same-canonical-media', path: canonical, name: 'café 東京.mkv' };
  await page.evaluate(
    ({ canonical, alias, shortAlias, broken, media }) => {
      const host = globalThis as any;
      host.__pickerPaths = [canonical, alias, shortAlias, broken];
      host.__mediaByPath = { [canonical]: media, [shortAlias]: media };
      host.__probeFailures = [broken];
    },
    { canonical, alias, shortAlias, broken, media },
  );
  await page.getByRole('button', { name: 'Add Files…', exact: true }).click();
  await expect(page.getByRole('option', { name: /café 東京.mkv/ })).toHaveCount(1);
  await expect(page.getByRole('log', { name: 'Recent activity' })).toContainText(
    'Invalid media fixture.',
  );
  const calls = await page.evaluate(() => (globalThis as any).__sourceCalls);
  expect(
    calls.filter((c: any) => c.command === 'probe_media').map((c: any) => c.payload.path),
  ).toEqual([canonical, shortAlias, broken]);
  await page.getByRole('button', { name: 'Load Tracks', exact: true }).click();
  await expect(page.getByRole('checkbox', { name: /Include .*Audio/ })).toBeVisible();
});

test('copy conversion keeps colliding stream indices source-scoped and preserves reordered tracks', async ({
  page,
}) => {
  await installSourceEncodeFixture(page);
  const external = {
    ...source,
    id: 'external',
    path: '/other/commentary.mka',
    name: 'commentary.mka',
    streams: [{ ...source.streams[1], title: 'External commentary' }],
  };
  await page.evaluate(
    ({ first, external }) => {
      const host = globalThis as any;
      host.__pickerPaths = [first.path, external.path];
      host.__mediaByPath = { [first.path]: first, [external.path]: external };
    },
    { first: source, external },
  );
  await page.getByRole('button', { name: 'Add Files…' }).click();
  await page.getByRole('option', { name: /input.mkv/ }).click();
  await page.getByRole('option', { name: /commentary.mka/ }).click({ modifiers: ['Control'] });
  await page.getByRole('button', { name: 'Load Tracks' }).click();
  const tracks = page.getByRole('region', { name: 'Track List' });
  await tracks
    .getByRole('listbox', { name: 'Tracks' })
    .getByRole('option', { name: /External commentary/ })
    .click();
  await tracks.getByRole('button', { name: 'Move track up' }).click();
  await page.getByRole('button', { name: 'Quick Convert', exact: true }).click();
  const quick = page.getByRole('region', { name: 'Quick Convert', exact: true });
  await quick.getByLabel('Codec', { exact: true }).selectOption('copy');
  await quick.getByRole('tab', { name: 'Audio' }).click();
  await quick.getByLabel('Codec', { exact: true }).selectOption('copy');
  await page.getByRole('button', { name: 'Run: Encode', exact: true }).click();
  await expect
    .poll(() =>
      page.evaluate(
        () =>
          (globalThis as any).__sourceCalls.find((c: any) => c.command === 'mock_start_encode')
            ?.payload.request,
      ),
    )
    .toBeTruthy();
  const request = await page.evaluate(
    () =>
      (globalThis as any).__sourceCalls.find((c: any) => c.command === 'mock_start_encode').payload
        .request,
  );
  expect(request.source).toMatchObject({ inputPath: '/media/input.mkv', streamIndices: [0, 1, 2] });
  expect(request.settings.externalTracks).toContainEqual(
    expect.objectContaining({ inputPath: '/other/commentary.mka', streamIndex: 1 }),
  );
  const order = request.settings.trackOrder;
  const externalPosition = order.findIndex(
    (track: any) => track.inputPath === '/other/commentary.mka' && track.streamIndex === 1,
  );
  const subtitlePosition = order.findIndex(
    (track: any) => !track.inputPath && track.streamIndex === 2,
  );
  expect(externalPosition).toBeGreaterThanOrEqual(0);
  expect(subtitlePosition).toBeGreaterThanOrEqual(0);
  expect(externalPosition).toBeLessThan(subtitlePosition);
});
