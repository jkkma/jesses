import { expect, test } from '@playwright/test';
import { installSourceEncodeFixture, source } from './helpers/source-encode-fixture';

for (const workflow of ['Quick Convert', 'AV1AN'] as const) {
  test(`${workflow} uses requested defaults after loading a source`, async ({ page }) => {
    await page.setViewportSize({ width: 800, height: 720 });
    await installSourceEncodeFixture(page);
    await page.getByRole('button', { name: 'Add Files…' }).click();
    await page.getByRole('button', { name: 'Load Tracks' }).click();
    await page.getByRole('button', { name: workflow, exact: true }).click();
    const region = page.getByRole('region', { name: workflow, exact: true });
    await expect(
      region.getByLabel(workflow === 'AV1AN' ? 'Encoder' : 'Codec', { exact: true }),
    ).toHaveValue('svtAv1Hdr');
    // Quick Convert: MP4, CRF 20. AV1AN: WebM, CRF 35. Both: SVT-AV1-HDR preset 4.
    await expect(region.getByLabel('Container', { exact: true })).toHaveValue(
      workflow === 'AV1AN' ? 'webm' : 'mp4',
    );
    await expect(region.getByLabel('Quality Mode', { exact: true })).toHaveValue(
      workflow === 'AV1AN' ? 'crf' : 'quality',
    );
    await expect(region.getByLabel('Quality', { exact: true })).toHaveValue(
      workflow === 'AV1AN' ? '35' : '20',
    );
    await expect(region.getByLabel('Preset / Effort', { exact: true })).toHaveValue('4');
    await expect(region.getByLabel('Tune', { exact: true })).toHaveValue('visualQuality');
    await expect(region.getByLabel('Color Format', { exact: true })).toHaveValue('yuv420p10le');
    await expect(region.getByLabel('Frame Rate', { exact: true })).toHaveValue('');
    await expect(region.getByLabel('Crop', { exact: true })).toHaveValue('disabled');
    await expect(region.getByLabel('Resize', { exact: true })).toHaveValue('none');
    await expect(region.getByLabel('Borders', { exact: true })).toHaveValue('');
    await expect(region.getByLabel('Grain Synthesis', { exact: true })).toHaveValue('disabled');
    if (workflow === 'Quick Convert')
      await expect(region.getByRole('checkbox', { name: 'One frame per field' })).not.toBeChecked();
    await region.getByLabel('Output Path', { exact: true }).fill('/media/defaults');
    const area = await region.locator('.tab-content').evaluate((el) => ({
      width: el.clientWidth,
      scrollWidth: el.scrollWidth,
      height: el.clientHeight,
      scrollHeight: el.scrollHeight,
    }));
    expect(area.scrollWidth).toBeLessThanOrEqual(area.width + 1);
    expect(area.scrollHeight).toBeLessThanOrEqual(area.height + 1);
    await page.screenshot({
      path: `target/test-artifacts/streamline-ui/${workflow.replace(' ', '-')}-defaults.png`,
    });
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
    expect(request.settings).toMatchObject({
      encoder: 'svtAv1Hdr',
      hdrTune: 'visualQuality',
      crf: workflow === 'AV1AN' ? 35 : 20,
      preset: 4,
    });
    expect(request.source.outputPath).toMatch(workflow === 'AV1AN' ? /\.webm$/ : /\.mp4$/);
    expect(
      request.settings.audio.every(
        (track: any) =>
          track.codec === 'opus' && track.bitrateKbps === 128 && track.channels === 'preserve',
      ),
    ).toBe(true);
    expect(
      workflow === 'AV1AN'
        ? request.settings.av1anOptions.pixelFormat
        : request.settings.outputPixelFormat,
    ).toBe('yuv420p10le');
    expect(request.settings.temporal?.deinterlace?.mode).not.toBe('bob');
    expect(request.settings.parameters?.some((entry: any) => entry.name === 'tune')).toBeFalsy();
    if (workflow === 'AV1AN') {
      expect(request.settings.av1anOptions).toMatchObject({
        splitMethod: 'sceneDetection',
        sourceWorkflow: true,
        maximumChunkFrames: 240,
      });
    }
  });
}

for (const workflow of ['Quick Convert', 'AV1AN'] as const) {
  test(`${workflow} leaves image subtitles out of its default container`, async ({ page }) => {
    await installSourceEncodeFixture(page);
    // A Blu-ray rip: the first (default-selected) subtitle is PGS.
    const withPgs = {
      ...source,
      streams: source.streams.map((stream) =>
        stream.kind === 'subtitle'
          ? { ...stream, codec: 'hdmv_pgs_subtitle', title: 'PGS' }
          : stream,
      ),
    };
    await page.evaluate((entry) => (globalThis as any).__setSourceMedia(entry), withPgs);
    await page.getByRole('button', { name: 'Add Files…' }).click();
    await page.getByRole('button', { name: 'Load Tracks' }).click();
    await page.getByRole('button', { name: workflow, exact: true }).click();
    const region = page.getByRole('region', { name: workflow, exact: true });
    const tracksTab = region.getByRole('tab', {
      name: workflow === 'AV1AN' ? 'Audio & Tracks' : 'Subtitles',
      exact: true,
    });
    const note = `${workflow === 'AV1AN' ? 'WebM' : 'MP4'} cannot store image subtitles (PGS, VobSub), so one subtitle track is left out. Choose MKV to keep them.`;
    await tracksTab.click();
    await expect(region.getByLabel('Image subtitles')).toHaveText(note);
    await region.getByLabel('Output Path', { exact: true }).fill('/media/pgs');
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
    // The PGS track (2) is left out instead of the job being refused.
    expect(request.source.streamIndices).toEqual([0, 1]);
    await expect(page.getByRole('log')).toContainText(note);
    // MKV keeps image subtitles, so the note goes away.
    await region.getByRole('tab', { name: 'Video', exact: true }).click();
    await region.getByLabel('Container', { exact: true }).selectOption('matroska');
    await tracksTab.click();
    await expect(region.getByLabel('Image subtitles')).toHaveCount(0);
  });
}

test('explicit AV1AN scene detection survives reload and a source change', async ({ page }) => {
  await installSourceEncodeFixture(page);
  await page.getByRole('button', { name: 'Add Files…' }).click();
  await page.getByRole('button', { name: 'AV1AN', exact: true }).click();
  let av = page.getByRole('region', { name: 'AV1AN', exact: true });
  await av.getByRole('tab', { name: 'Av1an Options' }).click();
  await av.getByLabel('Split Method').selectOption('sceneDetection');
  await expect
    .poll(() =>
      page.evaluate(
        () => JSON.parse(localStorage.getItem('jesses.av1an.preferences.v1') ?? '{}').splitMethod,
      ),
    )
    .toBe('sceneDetection');
  await page.getByRole('button', { name: 'Settings', exact: true }).click();
  await page.getByRole('button', { name: 'Reset Now', exact: true }).click();
  await page.getByRole('button', { name: 'AV1AN', exact: true }).click();
  await av.getByRole('tab', { name: 'Av1an Options' }).click();
  await expect(av.getByLabel('Split Method')).toHaveValue('sceneDetection');
  await page.reload();
  const second = { ...source, id: 'source-2', path: '/media/other.mkv', name: 'other.mkv' };
  await page.evaluate((entry) => {
    const host = globalThis as any;
    host.__setSourceMedia(entry);
    host.__mediaByPath[entry.path] = entry;
  }, second);
  await page.getByRole('button', { name: 'File List', exact: true }).click();
  await page.getByRole('button', { name: 'Add Files…' }).click();
  await page.getByRole('button', { name: 'Load Tracks' }).click();
  await page.getByRole('button', { name: 'AV1AN', exact: true }).click();
  av = page.getByRole('region', { name: 'AV1AN', exact: true });
  await av.getByRole('tab', { name: 'Av1an Options' }).click();
  await expect(av.getByLabel('Split Method')).toHaveValue('sceneDetection');
  await page.getByRole('button', { name: 'Run: AV1AN encode', exact: true }).click();
  await expect
    .poll(() =>
      page.evaluate(
        () =>
          (globalThis as any).__sourceCalls.find((c: any) => c.command === 'mock_start_encode')
            ?.payload.request.settings.av1anOptions.splitMethod,
      ),
    )
    .toBe('sceneDetection');
});

test('saved AV1AN scene detection and other preferences remain unchanged', async ({ page }) => {
  await page.addInitScript(() => {
    if (sessionStorage.getItem('migration-seeded')) return;
    sessionStorage.setItem('migration-seeded', 'yes');
    localStorage.setItem(
      'jesses.av1an.preferences.v1',
      JSON.stringify({
        version: 1,
        splitMethod: 'sceneDetection',
        chunkMethod: 'ffms2',
        chunkOrder: 'sequential',
        concatMethod: 'ffmpeg',
        encoderThreads: 3,
        sceneDetectionSlices: 4,
        workers: 8,
        workerBaseline: 10,
        filters: ['hflip'],
        audio: { codec: 'aac', channels: 'stereo', bitrateKbps: 192 },
        stripAudio: true,
        copySubs: false,
        copyData: true,
        copyAttachments: false,
      }),
    );
  });
  await installSourceEncodeFixture(page);
  await page.getByRole('button', { name: 'AV1AN', exact: true }).click();
  const av = page.getByRole('region', { name: 'AV1AN', exact: true });
  await av.getByRole('tab', { name: 'Av1an Options' }).click();
  await expect(av.getByLabel('Split Method')).toHaveValue('sceneDetection');
  await expect(av.getByLabel('Split Method').locator('option[value="fixedChunks"]')).toHaveText(
    'Fixed chunks',
  );
  await expect
    .poll(() =>
      page.evaluate(() => JSON.parse(localStorage.getItem('jesses.av1an.preferences.v1') ?? '{}')),
    )
    .toMatchObject({
      version: 1,
      splitMethod: 'sceneDetection',
      chunkMethod: 'ffms2',
      chunkOrder: 'sequential',
      concatMethod: 'ffmpeg',
      encoderThreads: 3,
      sceneDetectionSlices: 4,
      workers: 8,
      filters: ['hflip'],
      audio: { codec: 'aac', channels: 'stereo', bitrateKbps: 192 },
      stripAudio: true,
      copySubs: false,
      copyData: true,
      copyAttachments: false,
    });
  await av.getByLabel('Split Method').selectOption('sceneDetection');
  await expect
    .poll(() =>
      page.evaluate(
        () => JSON.parse(localStorage.getItem('jesses.av1an.preferences.v1') ?? '{}').splitMethod,
      ),
    )
    .toBe('sceneDetection');
  await page.reload();
  await page.getByRole('button', { name: 'AV1AN', exact: true }).click();
  await av.getByRole('tab', { name: 'Av1an Options' }).click();
  await expect(av.getByLabel('Split Method')).toHaveValue('sceneDetection');
});

test('HDR Tune follows advanced override and the Video selector can replace it', async ({
  page,
}) => {
  await installSourceEncodeFixture(page);
  await page.getByRole('button', { name: 'Add Files…' }).click();
  await page.getByRole('button', { name: 'Load Tracks' }).click();
  await page.getByRole('button', { name: 'Quick Convert', exact: true }).click();
  const region = page.getByRole('region', { name: 'Quick Convert', exact: true });
  await region.getByRole('tab', { name: 'Advanced' }).click();
  await region.getByRole('tab', { name: 'Quality & Tuning' }).click();
  await region.getByLabel('tune value', { exact: true }).fill('6');
  await region.getByRole('tab', { name: 'Video', exact: true }).click();
  await expect(region.getByLabel('Tune', { exact: true })).toHaveValue('filmGrain');
  await region.getByLabel('Tune', { exact: true }).selectOption('visualQuality');
  await region.getByRole('tab', { name: 'Advanced' }).click();
  await region.getByRole('tab', { name: 'Quality & Tuning' }).click();
  await expect(region.getByLabel('tune value', { exact: true })).toHaveValue('');
  await region.getByLabel('tune value', { exact: true }).fill('2');
  await region.getByRole('tab', { name: 'Video', exact: true }).click();
  await expect(region.getByLabel('Tune', { exact: true })).toHaveValue('advanced');
  await expect(region.getByLabel('Tune', { exact: true }).locator('option:checked')).toHaveText(
    'Advanced tune (2)',
  );
  await region.getByLabel('Codec', { exact: true }).selectOption('x264');
  await expect(region.getByLabel('Tune', { exact: true })).toHaveCount(0);
});

test('high-bit-depth mode decision encodes an 8-bit source at 10 bits', async ({ page }) => {
  await installSourceEncodeFixture(page);
  const eightBit = {
    ...source,
    streams: source.streams.map((stream) =>
      stream.kind === 'video' ? { ...stream, pixelFormat: 'yuv420p', bitDepth: 8 } : stream,
    ),
  };
  await page.evaluate((entry) => (globalThis as any).__setSourceMedia(entry), eightBit);
  await page.getByRole('button', { name: 'Add Files…' }).click();
  await page.getByRole('button', { name: 'Load Tracks' }).click();
  await page.getByRole('button', { name: 'Quick Convert', exact: true }).click();
  const region = page.getByRole('region', { name: 'Quick Convert', exact: true });
  const color = region.getByLabel('Color Format', { exact: true });
  await expect(color).toHaveValue('yuv420p10le');
  await expect(color).toHaveAttribute(
    'title',
    'The 8-bit source is converted to 10-bit before encoding.',
  );
  await color.selectOption('yuv420p');
  await expect(color).not.toHaveAttribute('title');
  await page.evaluate(() => ((globalThis as any).__holdCatalogs = true));
  await region.getByRole('tab', { name: 'Advanced' }).click();
  await expect
    .poll(() => page.evaluate(() => (globalThis as any).__pendingCatalogs?.length ?? 0))
    .toBeGreaterThan(0);
  await page.evaluate(() => {
    for (const pending of (globalThis as any).__pendingCatalogs.splice(0))
      pending.resolve({
        encoder: pending.request.encoder,
        backend: pending.request.backend,
        route: 'test',
        toolPath: '/bin/test',
        toolVersion: 'test',
        notes: [],
        parameters: [
          {
            name: 'hbd-mds',
            label: 'hbd-mds',
            argument: '--hbd-mds',
            minimum: 0,
            maximum: 2,
            valueKind: 'integer',
            minimumValue: '0',
            maximumValue: '2',
            choices: [],
            group: 'Quality & Tuning',
            description: 'High-bit-depth mode decision',
            example: '1',
          },
        ],
      });
  });
  await region.getByRole('tab', { name: 'Quality & Tuning' }).click();
  await region.getByLabel('hbd-mds value', { exact: true }).fill('1');
  await region.getByRole('tab', { name: 'Video', exact: true }).click();
  // Choosing high-bit-depth mode decision moves the 8-bit choice to 10-bit.
  await expect(color).toHaveValue('yuv420p10le');
  await expect(region.getByLabel('Bit depth')).toHaveText(
    'High-bit-depth mode decision runs at 10 bits: the 8-bit source is converted to 10-bit first.',
  );
  // Picking 8-bit again is still encoded at 10 bits, and the note says so.
  await color.selectOption('yuv420p');
  await expect(region.getByLabel('Bit depth')).toHaveText(
    'High-bit-depth mode decision runs at 10 bits: this encode uses 10-bit 4:2:0.',
  );
  await region.getByLabel('Output Path', { exact: true }).fill('/media/hbd');
  await page.getByRole('button', { name: 'Run: Encode', exact: true }).click();
  await expect
    .poll(() =>
      page.evaluate(() =>
        (globalThis as any).__sourceCalls.some((c: any) => c.command === 'mock_start_encode'),
      ),
    )
    .toBe(true);
  const settings = await page.evaluate(
    () =>
      (globalThis as any).__sourceCalls.find((c: any) => c.command === 'mock_start_encode').payload
        .request.settings,
  );
  expect(settings.parameters).toContainEqual({ name: 'hbd-mds', value: '1' });
  expect(settings.outputPixelFormat).toBe('yuv420p');
});

test('source changes retain explicit quality and Tune choices', async ({ page }) => {
  await installSourceEncodeFixture(page);
  const second = { ...source, id: 'source-2', path: '/media/other.mkv', name: 'other.mkv' };
  await page.evaluate(
    ({ first, second }) => {
      const host = globalThis as any;
      host.__pickerPaths = [first.path, second.path];
      host.__mediaByPath = { [first.path]: first, [second.path]: second };
    },
    { first: source, second },
  );
  await page.getByRole('button', { name: 'Add Files…' }).click();
  await page.getByRole('button', { name: 'Quick Convert', exact: true }).click();
  const region = page.getByRole('region', { name: 'Quick Convert', exact: true });
  await region.getByLabel('Quality', { exact: true }).fill('27');
  await region.getByLabel('Tune', { exact: true }).selectOption('filmGrain');
  await page.getByRole('button', { name: 'File List', exact: true }).click();
  await page.getByRole('option', { name: /input.mkv/ }).click();
  await page.getByRole('button', { name: 'Remove', exact: true }).click();
  await page.getByRole('button', { name: 'Quick Convert', exact: true }).click();
  await expect(region.getByLabel('Quality', { exact: true })).toHaveValue('27');
  await expect(region.getByLabel('Tune', { exact: true })).toHaveValue('filmGrain');
});
