import { expect, test } from '@playwright/test';
import { installSourceEncodeFixture, source } from './helpers/source-encode-fixture';

for (const workflow of ['Quick Convert', 'AV1AN'] as const) {
  test(`${workflow} starts with the first track of each type and keeps manual changes`, async ({
    page,
  }) => {
    await installSourceEncodeFixture(page);
    await page.evaluate(
      (media) => {
        const host = globalThis as any;
        host.__setSourceMedia(media);
        host.__mediaByPath[media.path] = media;
      },
      {
        ...source,
        streams: [
          source.streams[0],
          source.streams[1],
          { ...source.streams[1], index: 3, title: 'Second audio' },
          source.streams[2],
          { ...source.streams[2], index: 4, title: 'Second subtitle' },
          { ...source.streams[2], index: 5, kind: 'attachment', codec: 'ttf', title: 'Font' },
        ],
      },
    );
    await page.getByRole('button', { name: 'Add Files…' }).click();
    await page.getByRole('button', { name: 'Load Tracks' }).click();
    const region = page.getByRole('region', { name: 'Track List' });
    const boxes = region.getByRole('checkbox', { name: /^Include / });
    await expect(boxes).toHaveCount(6);
    const initial = await boxes.evaluateAll((elements) =>
      elements.map((e) => (e as HTMLInputElement).checked),
    );
    await region.getByText('Select…').click();
    await region.getByRole('button', { name: 'First Track Of Each Type', exact: true }).click();
    const menuSelection = await boxes.evaluateAll((elements) =>
      elements.map((e) => (e as HTMLInputElement).checked),
    );
    expect(initial).toEqual([true, true, false, true, false, false]);
    expect(initial).toEqual(menuSelection);
    await boxes.nth(2).check();
    await page.getByRole('button', { name: workflow, exact: true }).click();
    await page
      .getByRole('region', { name: workflow, exact: true })
      .getByLabel('Output Path', { exact: true })
      .fill('/media/tracks');
    await page
      .getByRole('button', {
        name: workflow === 'AV1AN' ? 'Run: AV1AN encode' : 'Run: Encode',
        exact: true,
      })
      .click();
    await expect
      .poll(() =>
        page.evaluate(() =>
          (globalThis as any).__sourceCalls.some((c: any) => c.command === 'mock_start_encode'),
        ),
      )
      .toBe(true);
    const request = await page.evaluate(
      () =>
        (globalThis as any).__sourceCalls.find((c: any) => c.command === 'mock_start_encode')
          .payload.request,
    );
    expect(request.source.streamIndices).toEqual([0, 1, 3, 2]);
    await page.getByRole('button', { name: 'Track List', exact: true }).click();
    await expect(boxes.nth(2)).toBeChecked();
  });
}

for (const workflow of ['Quick Convert', 'AV1AN'] as const) {
  test(`${workflow} submits the default first tracks`, async ({ page }) => {
    await installSourceEncodeFixture(page);
    await page.evaluate(
      (media) => {
        const host = globalThis as any;
        host.__setSourceMedia(media);
        host.__mediaByPath[media.path] = media;
      },
      {
        ...source,
        streams: [
          source.streams[0],
          source.streams[1],
          { ...source.streams[1], index: 3 },
          source.streams[2],
          { ...source.streams[2], index: 4 },
          { ...source.streams[2], index: 5, kind: 'attachment', codec: 'ttf' },
        ],
      },
    );
    await page.getByRole('button', { name: 'Add Files…' }).click();
    await page.getByRole('button', { name: 'Load Tracks' }).click();
    await page.getByRole('button', { name: workflow, exact: true }).click();
    await page
      .getByRole('region', { name: workflow, exact: true })
      .getByLabel('Output Path', { exact: true })
      .fill('/media/default-tracks');
    await page
      .getByRole('button', {
        name: workflow === 'AV1AN' ? 'Run: AV1AN encode' : 'Run: Encode',
        exact: true,
      })
      .click();
    await expect
      .poll(() =>
        page.evaluate(() =>
          (globalThis as any).__sourceCalls.some((c: any) => c.command === 'mock_start_encode'),
        ),
      )
      .toBe(true);
    const request = await page.evaluate(
      () =>
        (globalThis as any).__sourceCalls.find((c: any) => c.command === 'mock_start_encode')
          .payload.request,
    );
    expect(request.source.streamIndices).toEqual([0, 1, 2]);
  });
}
