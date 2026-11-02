import { expect, test } from '@playwright/test';
import { installSourceEncodeFixture, source } from './helpers/source-encode-fixture';

for (const externalFirst of [false, true])
  test(`Copy trim previews and submits the selected nonfirst video${externalFirst ? ' from an external file' : ''}`, async ({
    page,
  }) => {
    await installSourceEncodeFixture(page);
    const primary: any = {
      ...source,
      streams: source.streams.map((stream) =>
        stream.kind === 'video'
          ? { ...stream, title: 'Primary first', fieldOrder: 'progressive' }
          : stream,
      ),
    };
    const selected = {
      ...primary.streams[0],
      index: 4,
      title: 'Selected sixty',
      frameRate: '60/1',
      fieldOrder: 'progressive',
    };
    const other = {
      ...primary,
      id: 'other',
      path: '/other/second.mkv',
      name: 'second.mkv',
      durationSeconds: 20,
      streams: [{ ...primary.streams[0], title: 'External first', frameRate: '25/1' }, selected],
    };
    if (!externalFirst) primary.streams.push(selected);
    await page.evaluate(
      ({ primary, other, externalFirst }) => {
        const host = globalThis as any;
        host.__pickerPaths = [primary.path];
        host.__mediaByPath = { [primary.path]: primary, [other.path]: other };
      },
      { primary, other, externalFirst },
    );
    await page.getByRole('button', { name: 'Add Files…' }).click();
    await page.getByRole('button', { name: 'Load Tracks' }).click();
    if (externalFirst) {
      await page.getByRole('button', { name: 'File List', exact: true }).click();
      await page.evaluate((other) => ((globalThis as any).__pickerPaths = [other.path]), other);
      await page.getByRole('button', { name: 'Add Files…' }).click();
      await page.getByRole('button', { name: 'Add to file list' }).click();
      await page.getByRole('option', { name: /second.mkv/ }).click();
      await page.getByRole('button', { name: 'Load Tracks' }).click();
    }
    await page.getByRole('checkbox', { name: /Include .*Primary first/ }).uncheck();
    await page.getByRole('checkbox', { name: /Include .*Selected sixty/ }).check();
    await page.getByRole('button', { name: 'Quick Convert', exact: true }).click();
    const quick = page.getByRole('region', { name: 'Quick Convert', exact: true });
    await quick.getByLabel('Codec', { exact: true }).selectOption('copy');
    const start = await page.evaluate(() => (globalThis as any).__sourceCalls.length);
    await quick.getByRole('button', { name: 'Configure…' }).first().click();
    const dialog = page.getByRole('dialog', { name: 'Configure Trim' });
    await expect(dialog.getByLabel('Mode', { exact: true })).toHaveCount(0);
    await expect(dialog.getByLabel('Position')).toHaveAttribute('max', externalFirst ? '20' : '12');
    await dialog.getByRole('button', { name: '+1f', exact: true }).click();
    await expect(dialog.getByText('00:00:00.017', { exact: true })).toBeVisible();
    const path = externalFirst ? other.path : primary.path;
    await expect
      .poll(() =>
        page.evaluate(
          ({ start, path }) =>
            (globalThis as any).__sourceCalls
              .slice(start)
              .some(
                (call: any) =>
                  call.command === 'preview_frame' &&
                  call.payload.request.inputPath === path &&
                  call.payload.request.videoStreamIndex === 4,
              ),
          { start, path },
        ),
      )
      .toBe(true);
    await dialog.getByLabel('Start', { exact: true }).fill('00:00:03.400');
    await dialog.getByLabel('End', { exact: true }).fill('00:00:04.600');
    await expect(dialog.getByLabel('Start', { exact: true })).toHaveValue('00:00:02');
    await expect(dialog.getByLabel('Duration', { exact: true })).toHaveValue('00:00:02.600');
    await dialog.getByRole('button', { name: 'OK', exact: true }).click();
    await quick.getByLabel('Output Path', { exact: true }).fill('/media/copied-section');
    await page.getByRole('button', { name: 'Run: Encode', exact: true }).click();
    await expect
      .poll(() =>
        page.evaluate(() =>
          (globalThis as any).__sourceCalls.some(
            (call: any) => call.command === 'mock_start_encode',
          ),
        ),
      )
      .toBe(true);
    const calls = await page.evaluate(
      (start) => (globalThis as any).__sourceCalls.slice(start),
      start,
    );
    const request = calls.find((call: any) => call.command === 'mock_start_encode').payload.request;
    expect(request.source.inputPath).toBe(path);
    expect(request.settings).toMatchObject({
      videoMode: 'copy',
      videoStreamIndex: 4,
      sourceTrim: true,
      trim: { time: { startMilliseconds: 2000, endMilliseconds: 4600, seekStrategy: 'inputSeek' } },
    });
    const snaps = calls.filter((call: any) => call.command === 'snap_keyframe');
    expect(snaps.length).toBeGreaterThan(0);
    for (const snap of snaps)
      expect(snap.payload.request).toMatchObject({ inputPath: path, videoStreamIndex: 4 });
    const dialogPreviews = calls.filter(
      (call: any) => call.command === 'preview_frame' && call.payload.request.positionSeconds < 1,
    );
    for (const preview of dialogPreviews)
      expect(preview.payload.request).toMatchObject({ inputPath: path, videoStreamIndex: 4 });
  });
