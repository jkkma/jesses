import { expect, test, type Page } from '@playwright/test';
import type { MediaFile } from '../../src/lib/ipc/generated';
import { installSourceEncodeFixture, source } from './helpers/source-encode-fixture';

const primary: MediaFile = {
  ...source,
  streams: [
    {
      ...source.streams[0],
      fieldOrder: 'progressive',
      pixelFormat: 'yuv420p',
      colorTransfer: 'bt709',
      title: 'First picture',
    },
    ...source.streams.slice(1),
  ],
};
const secondary: MediaFile = {
  ...primary,
  id: 'second',
  path: '/media/second.mkv',
  name: 'second.mkv',
  streams: [{ ...primary.streams[0], title: 'Second picture' }],
};
const hlg: MediaFile = {
  ...secondary,
  id: 'hlg',
  path: '/media/hlg.mkv',
  name: 'hlg.mkv',
  streams: [
    {
      ...secondary.streams[0],
      codec: 'hevc',
      pixelFormat: 'yuv420p10le',
      colorPrimaries: 'bt2020',
      colorTransfer: 'arib-std-b67',
      colorSpace: 'bt2020nc',
      colorRange: 'tv',
    },
  ],
};

function selectView(page: Page, view: string) {
  return page.getByRole('button', { name: view, exact: true }).click();
}

async function importMedia(page: Page, inputs: MediaFile[]) {
  await installSourceEncodeFixture(page);
  await page.evaluate((inputs) => {
    const calls = globalThis as any;
    calls.__pickerPaths = inputs.map((input) => input.path);
    calls.__mediaByPath = Object.fromEntries(inputs.map((input) => [input.path, input]));
  }, inputs);
  await page.getByRole('button', { name: 'Add Files…' }).click();
  await expect(page.getByRole('option', { name: new RegExp(inputs.at(-1)!.name) })).toBeVisible();
  await expect(page.getByRole('button', { name: 'Add Files…' })).toBeEnabled();
}

async function loadAllTracks(page: Page) {
  await page.getByRole('option', { name: /input\.mkv/ }).click();
  await page.getByRole('option', { name: /second\.mkv/ }).click({ modifiers: ['Control'] });
  await page.getByRole('button', { name: 'Load Tracks' }).click();
}

function chooseBatchMode(page: Page) {
  return page.getByRole('combobox', { name: 'Mode', exact: true }).selectOption('batch');
}

function requests(page: Page, command = 'mock_start_encode') {
  return page.evaluate(
    (name) =>
      (globalThis as any).__sourceCalls
        .filter((call: any) => call.command === name)
        .map((call: any) => call.payload.request),
    command,
  );
}

for (const edited of [false, true]) {
  test(`Quick Convert resets trim and ${edited ? 'keeps an edited path' : 'suggests the new picture name'} on source change`, async ({
    page,
  }) => {
    await importMedia(page, [primary, secondary]);
    await loadAllTracks(page);
    await selectView(page, 'Quick Convert');
    const quick = page.getByRole('region', { name: 'Quick Convert', exact: true });
    const output = quick.getByLabel('Output Path', { exact: true });
    await expect(output).toHaveValue('/media/input');
    if (edited) await output.fill('/exports/hand-picked');
    await quick.getByRole('button', { name: 'Configure…', exact: true }).first().click();
    const trim = page.getByRole('dialog', { name: 'Configure Trim' });
    await trim.getByLabel('Start', { exact: true }).fill('3');
    await trim.getByLabel('End', { exact: true }).fill('5');
    await trim.getByRole('button', { name: 'OK', exact: true }).click();
    await expect(quick.getByLabel('Trim range')).toHaveText('00:00:03–00:00:05');

    await selectView(page, 'Track List');
    await page.getByRole('checkbox', { name: /Include .*First picture/ }).uncheck();
    await page.getByRole('checkbox', { name: /Include .*Second picture/ }).check();
    await selectView(page, 'Quick Convert');
    await expect(quick.getByText('Source: second.mkv', { exact: true })).toBeVisible();
    await expect(quick.getByLabel('Trim range')).toHaveCount(0);
    const name = edited ? '/exports/hand-picked' : '/media/second';
    await expect(output).toHaveValue(name);
    await page.getByRole('button', { name: 'Run: Encode', exact: true }).click();
    await expect.poll(() => requests(page)).toHaveLength(1);
    const [request] = await requests(page);
    expect(request.source).toMatchObject({
      inputPath: secondary.path,
      outputPath: `${name}.mp4`,
    });
    expect(request.settings.trim).toBeUndefined();
  });
}

test('AV1AN blocks an unchecked picture and opens Track List from the reason', async ({ page }) => {
  await importMedia(page, [primary]);
  await selectView(page, 'Track List');
  await page.getByRole('checkbox', { name: /Include .*First picture/ }).uncheck();
  await selectView(page, 'AV1AN');
  await expect(page.getByRole('button', { name: 'Run: AV1AN encode' })).toBeDisabled();
  const reason = page.locator('#run-blocked-reason');
  await expect(reason).toContainText('Select one video in Track List to encode with AV1AN.');
  await reason.click();
  await expect(page.getByRole('region', { name: 'Track List', exact: true })).toBeVisible();
  expect(await requests(page)).toHaveLength(0);
});

test('AV1AN uses the checked picture from a later file and preserves the explicit track order', async ({
  page,
}) => {
  await importMedia(page, [primary, secondary]);
  await loadAllTracks(page);
  await page.getByRole('checkbox', { name: /Include .*First picture/ }).uncheck();
  await page.getByRole('checkbox', { name: /Include .*Second picture/ }).check();
  await selectView(page, 'AV1AN');
  const av1an = page.getByRole('region', { name: 'AV1AN', exact: true });
  await expect(av1an.getByText('Source: second.mkv', { exact: true })).toBeVisible();
  await page.getByRole('button', { name: 'Run: AV1AN encode', exact: true }).click();
  await expect.poll(() => requests(page)).toHaveLength(1);
  const [request] = await requests(page);
  expect(request.source).toMatchObject({
    inputPath: secondary.path,
    outputPath: '/media/second.webm',
    streamIndices: [0],
  });
  expect(request.settings.videoStreamIndex).toBe(0);
  expect(request.settings.trackOrder).toEqual([
    { inputPath: primary.path, streamIndex: 1 },
    { inputPath: primary.path, streamIndex: 2 },
    { streamIndex: 0 },
  ]);
});

for (const workflow of ['Quick Convert', 'AV1AN'] as const) {
  const av1an = workflow === 'AV1AN';
  const runName = av1an ? 'Run Batch: AV1AN encode' : 'Run Batch: Encode';

  test(`${workflow} batch uses the changed default folder and naming template and keeps a single-file path draft`, async ({
    page,
  }) => {
    await importMedia(page, [primary]);
    await selectView(page, workflow);
    const panel = page.getByRole('region', { name: workflow, exact: true });
    const singlePath = panel.getByLabel('Output Path', { exact: true });
    await singlePath.fill('');
    await selectView(page, 'File List');
    await chooseBatchMode(page);
    await selectView(page, workflow);
    const batch = panel.getByRole('region', { name: 'Batch output' });
    await expect(singlePath).toHaveCount(0);
    await expect(batch).toContainText('Same folder as each source');

    await batch.getByRole('button', { name: 'Change output folder' }).click();
    const folder = page.getByPlaceholder('Leave empty to save next to the source file');
    await expect(folder).toBeFocused();
    await folder.fill('/exports');
    await folder.press('Enter');
    await expect
      .poll(() =>
        page.evaluate(() => (globalThis as any).__preferences.general.defaultOutputDirectory),
      )
      .toBe('/exports');
    await selectView(page, workflow);
    await batch.getByRole('button', { name: 'Edit output names' }).click();
    await page.getByLabel('Output Names').fill('{name}_{codec}');
    const expectedPath = av1an ? '/exports/input_av1.webm' : '/exports/input_av1.mp4';
    await page.evaluate((path) => ((globalThis as any).__batchOutputPaths = [path]), expectedPath);
    await selectView(page, workflow);
    await expect(batch).toContainText('/exports');
    await expect(batch).toContainText('{name}_{codec}');
    await expect(batch).toContainText(expectedPath);
    const preview = (await requests(page, 'resolve_batch_output_paths')).at(-1);
    expect(preview).toMatchObject({
      defaultOutputDirectory: '/exports',
      outputNameTemplate: '{name}_{codec}',
    });
    await page.getByRole('button', { name: runName, exact: true }).click();
    await expect.poll(() => requests(page)).toHaveLength(1);
    const [submitted] = await requests(page, 'preview_encode_batch');
    expect(submitted).toMatchObject({
      outputDirectory: '/exports',
      outputNameTemplate: '{name}_{codec}',
    });
    expect((await requests(page))[0].source.outputPath).toBe(expectedPath);

    await selectView(page, 'File List');
    await page.getByRole('combobox', { name: 'Mode', exact: true }).selectOption('mux');
    await selectView(page, workflow);
    await expect(singlePath).toHaveValue('');
  });

  for (const hlgFirst of [false, true]) {
    test(`${workflow} batch tone maps only HLG when ${hlgFirst ? 'HLG' : 'SDR'} is first`, async ({
      page,
    }) => {
      await importMedia(page, hlgFirst ? [hlg, primary] : [primary, hlg]);
      await chooseBatchMode(page);
      await selectView(page, workflow);
      const region = page.getByRole('region', { name: workflow, exact: true });
      await expect(region.getByLabel('Tone Mapping', { exact: true })).toHaveValue(
        av1an ? 'mobius' : 'spline',
      );
      await page.getByRole('button', { name: runName, exact: true }).click();
      await expect.poll(() => requests(page)).toHaveLength(2);
      const executed = await requests(page);
      expect(
        executed.find((entry: any) => entry.source.inputPath === primary.path).settings.toneMap,
      ).toBeUndefined();
      expect(
        executed.find((entry: any) => entry.source.inputPath === hlg.path).settings.toneMap
          .algorithm,
      ).toBe(av1an ? 'mobius' : 'spline');
    });
  }
}

test('Quick Convert offers the shared audio settings in Batch and keeps the mux draft', async ({
  page,
}) => {
  await importMedia(page, [primary]);
  await selectView(page, 'Quick Convert');
  const quick = page.getByRole('region', { name: 'Quick Convert', exact: true });
  await quick.getByRole('tab', { name: 'Audio', exact: true }).click();
  const configuration = quick.getByLabel('Configuration', { exact: true });
  await configuration.selectOption('perTrack');
  const audio = page.getByRole('dialog', { name: 'Configure Audio Tracks' });
  await audio.getByLabel('Channels for track 1').fill('1');
  await audio.getByLabel('Bitrate for track 1').fill('96');
  await audio.getByRole('button', { name: 'OK', exact: true }).click();

  await selectView(page, 'File List');
  await chooseBatchMode(page);
  await selectView(page, 'Quick Convert');
  await expect(configuration).toHaveCount(0);
  await expect(quick.getByText('Batch uses the same audio settings for all tracks.')).toBeVisible();
  await quick.getByLabel('Bitrate (Kbps)', { exact: true }).fill('192');
  await page.getByRole('button', { name: 'Run Batch: Encode', exact: true }).click();
  await expect.poll(() => requests(page)).toHaveLength(1);
  expect((await requests(page))[0].settings.audio[0]).toMatchObject({
    channels: 'preserve',
    bitrateKbps: 192,
  });

  await selectView(page, 'File List');
  await page.getByRole('combobox', { name: 'Mode', exact: true }).selectOption('mux');
  await selectView(page, 'Quick Convert');
  await expect(configuration).toHaveValue('perTrack');
  await quick.getByRole('button', { name: 'Configure…', exact: true }).click();
  await expect(audio.getByLabel('Channels for track 1')).toHaveValue('1');
  await expect(audio.getByLabel('Bitrate for track 1')).toHaveValue('96');
});

test('changing batch settings cancels an older destination lookup', async ({ page }) => {
  await importMedia(page, [primary, secondary]);
  await chooseBatchMode(page);
  await page.evaluate(() => {
    const host = globalThis as any;
    host.__holdBatchResolution = new Promise(
      (resolve) => (host.__releaseBatchResolution = resolve),
    );
  });
  await selectView(page, 'Quick Convert');
  const analysisId = () =>
    page.evaluate(
      () =>
        (globalThis as any).__sourceCalls.find(
          (call: any) => call.command === 'resolve_batch_output_paths',
        )?.payload.id as string | undefined,
    );
  await expect.poll(analysisId).toBeTruthy();
  const previous = await analysisId();

  const quick = page.getByRole('region', { name: 'Quick Convert', exact: true });
  await quick.getByLabel('Codec', { exact: true }).selectOption('copy');
  await expect
    .poll(() =>
      page.evaluate(
        (id) =>
          (globalThis as any).__sourceCalls.some(
            (call: any) => call.command === 'cancel_media_analysis' && call.payload.id === id,
          ),
        previous,
      ),
    )
    .toBe(true);
  await expect
    .poll(async () => (await requests(page, 'resolve_batch_output_paths')).at(-1)?.naming.codec)
    .toBe('copy');
  await page.evaluate(() => (globalThis as any).__releaseBatchResolution());
  await expect(
    quick.getByRole('region', { name: 'Batch output' }).locator('dd').last(),
  ).not.toHaveText('Resolving…');
});

test('Batch image export keeps a cover-only video stream and its naming dimensions', async ({
  page,
}) => {
  const cover = {
    ...primary.streams[0],
    index: 7,
    codec: 'mjpeg',
    attachedPic: true,
    width: 320,
    height: 180,
  };
  await importMedia(page, [{ ...primary, streams: [cover, primary.streams[1]] }]);
  await chooseBatchMode(page);
  await selectView(page, 'Quick Convert');
  const quick = page.getByRole('region', { name: 'Quick Convert', exact: true });
  await quick.getByLabel('Codec', { exact: true }).selectOption('png');
  await expect
    .poll(async () => (await requests(page, 'resolve_batch_output_paths')).at(-1)?.inputs)
    .toEqual([{ inputPath: primary.path, width: 320, height: 180 }]);
  await page.getByRole('button', { name: 'Run Batch: Encode', exact: true }).click();
  await expect
    .poll(async () =>
      (await requests(page, 'mock_image_execution')).map((item: any) => item.streamIndex),
    )
    .toEqual([7]);
});

test('AV1AN Batch shows the movie example when the first file is audio-only', async ({ page }) => {
  await importMedia(page, [
    { ...primary, name: 'audio.mka', path: '/media/audio.mka', streams: [primary.streams[1]] },
    secondary,
  ]);
  await chooseBatchMode(page);
  await page.getByLabel('Output Names').fill('{index}_{name}');
  await page.evaluate(() => {
    (globalThis as any).__batchOutputPaths = ['/media/1_audio.webm', '/media/2_second.webm'];
  });
  await selectView(page, 'AV1AN');
  const batch = page.getByRole('region', { name: 'Batch output' });
  await expect(batch).toContainText('/media/2_second.webm');
  expect((await requests(page, 'resolve_batch_output_paths'))[0].inputs).toMatchObject([
    { inputPath: '/media/audio.mka' },
    { inputPath: secondary.path },
  ]);
});
