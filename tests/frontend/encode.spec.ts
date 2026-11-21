import { expect, test } from '@playwright/test';
import { installSourceEncodeFixture, source } from './helpers/source-encode-fixture';
import {
  defaultFramingDraft,
  framingDimensions,
} from '../../src/lib/components/shared/framing-options';

test('portrait percentage resize follows the source short-axis rounding', () => {
  const draft = defaultFramingDraft();
  draft.resizeEnabled = true;
  draft.resizeMode = 'percent';
  draft.resizePercent = 33;
  draft.resizeWidth = 1920;
  draft.resizeHeight = 1080;
  draft.resizeModulus = 2;
  const video = { ...source.streams[0], width: 720, height: 1280 };
  expect(framingDimensions(draft, video)).toMatchObject({ width: 238, height: 424 });
});

for (const workflow of ['Quick Convert', 'AV1AN'] as const) {
  test(`${workflow} preserves each source audio codec in the submitted track`, async ({ page }) => {
    await installSourceEncodeFixture(page);
    await page.evaluate((media) => (globalThis as any).__setSourceMedia(media), {
      ...source,
      streams: source.streams.map((stream) =>
        stream.kind === 'video' ? { ...stream, fieldOrder: 'progressive' } : stream,
      ),
    });
    await page.getByRole('button', { name: 'Add Files…' }).click();
    await page.getByRole('button', { name: 'Load Tracks' }).click();
    await page.getByRole('button', { name: workflow, exact: true }).click();
    const region = page.getByRole('region', { name: workflow, exact: true });
    await region
      .getByRole('tab', { name: workflow === 'AV1AN' ? 'Audio & Tracks' : 'Audio' })
      .click();
    const codecs = ['copy', 'aac', 'opus', 'vorbis', 'eac3', 'mp3', 'flac'];
    for (const [index, codec] of codecs.entries()) {
      await region.getByLabel('Codec', { exact: true }).selectOption(codec);
      if (codec === 'copy') {
        await expect(region.getByLabel('Channels')).toHaveValue('preserve');
        await expect(region.getByLabel('Channels')).toBeDisabled();
      }
      await page
        .getByRole('button', {
          name: workflow === 'AV1AN' ? 'Run: AV1AN encode' : 'Run: Encode',
          exact: true,
        })
        .click();
      await expect
        .poll(() =>
          page.evaluate(
            () =>
              (globalThis as any).__sourceCalls.filter(
                (c: any) => c.command === 'mock_start_encode',
              ).length,
          ),
        )
        .toBe(index + 1);
    }
    const requests = await page.evaluate(() =>
      (globalThis as any).__sourceCalls
        .filter((c: any) => c.command === 'mock_start_encode')
        .map((c: any) => c.payload.request),
    );
    expect(requests.map((request: any) => request.settings.audio[0].codec)).toEqual(codecs);
    expect(requests[0].settings.audio[0].channels).toBe('preserve');
    expect(requests.every((request: any) => request.settings.audio[0].streamIndex === 1)).toBe(
      true,
    );
  });
}

test('AV1AN validates source worker count and submits the selected segment reader', async ({
  page,
}) => {
  await installSourceEncodeFixture(page);
  await page.getByRole('button', { name: 'Add Files…' }).click();
  await page.getByRole('button', { name: 'Load Tracks' }).click();
  await page.getByRole('button', { name: 'AV1AN', exact: true }).click();
  const av = page.getByRole('region', { name: 'AV1AN', exact: true });
  await av.getByRole('tab', { name: 'AV1AN Options' }).click();
  await av.getByLabel('Chunk Method').selectOption('segment');
  await av.getByLabel('Workers', { exact: true }).fill('0');
  await expect(page.getByRole('button', { name: 'Run: AV1AN encode' })).toBeDisabled();
  await av.getByLabel('Workers', { exact: true }).fill('3');
  await page.getByRole('button', { name: 'Run: AV1AN encode' }).click();
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
  expect(settings).toMatchObject({ workers: 3, av1anOptions: { chunkMethod: 'segment' } });
});

test('AV1AN sends the chosen surround layout and channel-scaled bitrate', async ({ page }) => {
  await installSourceEncodeFixture(page);
  await page.getByRole('button', { name: 'Add Files…' }).click();
  await page.getByRole('button', { name: 'Load Tracks' }).click();
  await page.getByRole('button', { name: 'AV1AN', exact: true }).click();
  const av = page.getByRole('region', { name: 'AV1AN', exact: true });
  await av.getByRole('tab', { name: 'Audio & Tracks' }).click();
  await av.getByLabel('Codec', { exact: true }).selectOption('aac');
  await av.getByLabel('Bitrate (Kbps)').fill('128');
  for (const [index, layout] of ['surround51', 'surround71'].entries()) {
    await av.getByLabel('Channels').selectOption(layout);
    await page.getByRole('button', { name: 'Run: AV1AN encode' }).click();
    await expect
      .poll(() =>
        page.evaluate(
          () =>
            (globalThis as any).__sourceCalls.filter((c: any) => c.command === 'mock_start_encode')
              .length,
        ),
      )
      .toBe(index + 1);
  }
  const audio = await page.evaluate(() =>
    (globalThis as any).__sourceCalls
      .filter((c: any) => c.command === 'mock_start_encode')
      .map((c: any) => c.payload.request.settings.audio[0]),
  );
  expect(audio).toMatchObject([
    { codec: 'aac', channels: 'surround51', bitrateKbps: 256 },
    { codec: 'aac', channels: 'surround71', bitrateKbps: 320 },
  ]);
});

test('AV1AN target mode waits for SDR tone mapping on an HDR source', async ({ page }) => {
  await installSourceEncodeFixture(page);
  const hdr = {
    ...source,
    streams: source.streams.map((stream) =>
      stream.kind === 'video'
        ? {
            ...stream,
            fieldOrder: 'progressive',
            codec: 'hevc',
            pixelFormat: 'yuv420p10le',
            bitDepth: 10,
            colorPrimaries: 'bt2020',
            colorTransfer: 'smpte2084',
            colorSpace: 'bt2020nc',
            colorRange: 'tv',
            hdrFormat: 'HDR / PQ',
          }
        : stream,
    ),
  };
  await page.evaluate((media) => (globalThis as any).__setSourceMedia(media), hdr);
  await page.getByRole('button', { name: 'Add Files…' }).click();
  await page.getByRole('button', { name: 'Load Tracks' }).click();
  await page.getByRole('button', { name: 'AV1AN', exact: true }).click();
  const av = page.getByRole('region', { name: 'AV1AN', exact: true });
  await av.getByLabel('Tone Mapping').selectOption('off');
  await av.getByLabel('Quality Mode').selectOption('vmaf');
  await expect(page.getByRole('button', { name: 'Run: AV1AN encode' })).toBeDisabled();
  await av.getByLabel('Tone Mapping').selectOption('reinhard');
  await page.getByRole('button', { name: 'Run: AV1AN encode' }).click();
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
  expect(settings).toMatchObject({
    toneMap: { algorithm: 'reinhard' },
    av1anOptions: { sourceTargetDefaults: true, targetQuality: { metric: 'vmaf' } },
  });
});

test('source quality validation blocks invalid values and permits a corrected retry', async ({
  page,
}) => {
  await installSourceEncodeFixture(page);
  await page.getByRole('button', { name: 'Add Files…' }).click();
  await page.getByRole('button', { name: 'Load Tracks' }).click();
  await page.getByRole('button', { name: 'Quick Convert', exact: true }).click();
  const quick = page.getByRole('region', { name: 'Quick Convert', exact: true });
  await quick.getByLabel('Codec', { exact: true }).selectOption('x264');
  await quick.getByLabel('Quality', { exact: true }).fill('52');
  await expect(page.getByRole('button', { name: 'Run: Encode' })).toBeDisabled();
  expect(
    await page.evaluate(() =>
      (globalThis as any).__sourceCalls.some((c: any) => c.command === 'prepare_command_edit'),
    ),
  ).toBe(false);
  await quick.getByLabel('Quality', { exact: true }).fill('20');
  await page.getByRole('button', { name: 'Run: Encode' }).click();
  await expect
    .poll(() =>
      page.evaluate(() =>
        (globalThis as any).__sourceCalls.some((c: any) => c.command === 'mock_start_encode'),
      ),
    )
    .toBe(true);
});

for (const [workflow, missing] of [
  ['Quick Convert', 'ffmpeg'],
  ['Quick Convert', 'ffprobe'],
  ['Quick Convert', 'svt-av1'],
  ['Quick Convert', 'x264'],
  ['AV1AN', 'ffmpeg'],
  ['AV1AN', 'ffprobe'],
  ['AV1AN', 'svt-av1'],
  ['AV1AN', 'av1an'],
] as const) {
  test(`${workflow} needs its selected ${missing} tool before Run`, async ({ page }) => {
    await page.addInitScript((missing) => {
      (globalThis as any).__unavailableTools = [missing];
    }, missing);
    await installSourceEncodeFixture(page);
    await page.getByRole('button', { name: 'Add Files…' }).click();
    await page.getByRole('button', { name: 'Load Tracks' }).click();
    await page.getByRole('button', { name: workflow, exact: true }).click();
    const region = page.getByRole('region', { name: workflow, exact: true });
    if (missing === 'x264') await region.getByLabel('Codec', { exact: true }).selectOption('x264');
    if (missing === 'svt-av1')
      await region
        .getByLabel(workflow === 'AV1AN' ? 'Encoder' : 'Codec', { exact: true })
        .selectOption('svtAv1');
    await expect(
      page.getByRole('button', {
        name: workflow === 'AV1AN' ? 'Run: AV1AN encode' : 'Run: Encode',
        exact: true,
      }),
    ).toBeDisabled();
    expect(
      await page.evaluate(() =>
        (globalThis as any).__sourceCalls.some((c: any) => c.command === 'prepare_command_edit'),
      ),
    ).toBe(false);
  });
}

for (const workflow of ['Quick Convert', 'AV1AN'] as const)
  for (const outcome of ['path', 'error'] as const) {
    test(`${workflow} ignores a stale destination picker ${outcome} after changing codec`, async ({
      page,
    }) => {
      await installSourceEncodeFixture(page);
      await page.getByRole('button', { name: 'Add Files…' }).click();
      await page.getByRole('button', { name: 'Load Tracks' }).click();
      await page.getByRole('button', { name: workflow, exact: true }).click();
      const region = page.getByRole('region', { name: workflow, exact: true });
      await page.evaluate((outcome) => {
        const h = globalThis as any;
        h.__holdSave = new Promise((resolve) => (h.__releaseSave = resolve));
        h.__savePath = '/media/stale.mkv';
        h.__saveFailure = outcome === 'error' ? 'Stale picker failure' : null;
      }, outcome);
      await region.getByRole('button', { name: 'Browse output path' }).click();
      await expect
        .poll(() =>
          page.evaluate(() =>
            (globalThis as any).__sourceCalls.some((c: any) => c.command === 'plugin:dialog|save'),
          ),
        )
        .toBe(true);
      await region
        .getByLabel(workflow === 'AV1AN' ? 'Encoder' : 'Codec', { exact: true })
        .selectOption('x264');
      await region.getByLabel('Output Path', { exact: true }).fill('/media/current');
      await page.evaluate(() => {
        const h = globalThis as any;
        h.__holdSave = null;
        h.__releaseSave();
      });
      await expect(region.getByLabel('Output Path', { exact: true })).toHaveValue('/media/current');
      await expect(region.getByText('Stale picker failure')).toHaveCount(0);
    });
  }

test('copy-video trim snaps the selected start to a keyframe and preserves the end', async ({
  page,
}) => {
  await installSourceEncodeFixture(page);
  await page.getByRole('button', { name: 'Add Files…' }).click();
  await page.getByRole('button', { name: 'Load Tracks' }).click();
  await page.getByRole('button', { name: 'Quick Convert', exact: true }).click();
  const q = page.getByRole('region', { name: 'Quick Convert', exact: true });
  await q.getByLabel('Codec', { exact: true }).selectOption('copy');
  await q.getByRole('button', { name: 'Configure…' }).first().click();
  const d = page.getByRole('dialog', { name: 'Configure Trim' });
  await d.getByLabel('Start', { exact: true }).fill('00:00:05');
  await d.getByLabel('End', { exact: true }).fill('00:00:10');
  await d.getByRole('button', { name: 'OK' }).click();
  await expect(d).toHaveCount(0);
  await page.getByRole('button', { name: 'Run: Encode' }).click();
  await expect
    .poll(() =>
      page.evaluate(() =>
        (globalThis as any).__sourceCalls.some((c: any) => c.command === 'mock_start_encode'),
      ),
    )
    .toBe(true);
  const request = await page.evaluate(
    () =>
      (globalThis as any).__sourceCalls.find((c: any) => c.command === 'mock_start_encode').payload
        .request,
  );
  expect(request.settings.videoMode).toBe('copy');
  expect(request.settings.trim.time).toMatchObject({
    startMilliseconds: 4000,
    endMilliseconds: 10000,
  });
});

test('source permits copied audio with encoded video trim', async ({ page }) => {
  await installSourceEncodeFixture(page);
  await page.getByRole('button', { name: 'Add Files…' }).click();
  await page.getByRole('button', { name: 'Load Tracks' }).click();
  await page.getByRole('button', { name: 'Quick Convert', exact: true }).click();
  const q = page.getByRole('region', { name: 'Quick Convert', exact: true });
  await q.getByRole('tab', { name: 'Audio' }).click();
  await q.getByLabel('Codec', { exact: true }).selectOption('copy');
  await q.getByRole('tab', { name: 'Video', exact: true }).click();
  await q.getByRole('button', { name: 'Configure…' }).first().click();
  const d = page.getByRole('dialog', { name: 'Configure Trim' });
  await d.getByLabel('Mode').selectOption('frames');
  await d.getByLabel('Start', { exact: true }).fill('24');
  await d.getByLabel('End', { exact: true }).fill('72');
  await d.getByRole('button', { name: 'OK' }).click();
  await expect(page.getByRole('button', { name: 'Run: Encode' })).toBeEnabled();
  await page.getByRole('button', { name: 'Run: Encode' }).click();
  await expect
    .poll(() =>
      page.evaluate(() =>
        (globalThis as any).__sourceCalls.some((c: any) => c.command === 'mock_start_encode'),
      ),
    )
    .toBe(true);
  const request = await page.evaluate(
    () =>
      (globalThis as any).__sourceCalls.find((c: any) => c.command === 'mock_start_encode').payload
        .request,
  );
  expect(request.settings.audio[0].codec).toBe('copy');
  expect(request.settings.trim).toMatchObject({ startFrame: 24, endFrameExclusive: 72 });
  expect(request.settings.sourceTrim).toBe(true);
});

for (const workflow of ['Quick Convert', 'AV1AN'] as const) {
  test(`${workflow} does not surface a stale preparation error after the input changes`, async ({
    page,
  }) => {
    await installSourceEncodeFixture(page);
    const first = {
      ...source,
      streams: source.streams.map((stream) =>
        stream.kind === 'video' ? { ...stream, fieldOrder: 'progressive' } : stream,
      ),
    };
    const second = { ...first, id: 'second', path: '/media/second.mkv', name: 'second.mkv' };
    await page.evaluate(
      (inputs) => {
        const h = globalThis as any;
        h.__pickerPaths = inputs.map((item) => item.path);
        h.__mediaByPath = Object.fromEntries(inputs.map((item) => [item.path, item]));
        h.__holdPrepare = new Promise((resolve) => (h.__releasePrepare = resolve));
        h.__prepareFailure = 'Old preparation failed';
      },
      [first, second],
    );
    await page.getByRole('button', { name: 'Add Files…' }).click();
    await page.getByRole('option', { name: /input.mkv/ }).click();
    await page.getByRole('button', { name: 'Load Tracks' }).click();
    await page.getByRole('button', { name: workflow, exact: true }).click();
    await page
      .getByRole('button', {
        name: workflow === 'AV1AN' ? 'Run: AV1AN encode' : 'Run: Encode',
        exact: true,
      })
      .click();
    await expect
      .poll(() =>
        page.evaluate(() =>
          (globalThis as any).__sourceCalls.some((c: any) => c.command === 'prepare_command_edit'),
        ),
      )
      .toBe(true);
    await page.getByRole('button', { name: 'File List', exact: true }).click();
    await page.getByRole('option', { name: /input.mkv/ }).click();
    await page.getByRole('button', { name: 'Remove', exact: true }).click();
    await page.getByRole('option', { name: /second.mkv/ }).click();
    await page.getByRole('button', { name: 'Load Tracks' }).click();
    await page.getByRole('button', { name: workflow, exact: true }).click();
    const region = page.getByRole('region', { name: workflow, exact: true });
    await expect(
      page.getByRole('button', {
        name: workflow === 'AV1AN' ? 'Run: AV1AN encode' : 'Run: Encode',
        exact: true,
      }),
    ).toBeDisabled();
    await page.evaluate(() => {
      const h = globalThis as any;
      h.__holdPrepare = null;
      h.__releasePrepare();
    });
    await expect(
      page.getByRole('button', {
        name: workflow === 'AV1AN' ? 'Run: AV1AN encode' : 'Run: Encode',
        exact: true,
      }),
    ).toBeEnabled();
    await expect(region.getByText('Old preparation failed')).toHaveCount(0);
  });
}

for (const workflow of ['Quick Convert', 'AV1AN'] as const) {
  test(`${workflow} explains invalid quality and returns to the video settings`, async ({
    page,
  }) => {
    await installSourceEncodeFixture(page);
    await page.getByRole('button', { name: 'Add Files…' }).click();
    await page.getByRole('button', { name: 'Load Tracks' }).click();
    await page.getByRole('button', { name: workflow, exact: true }).click();
    const region = page.getByRole('region', { name: workflow, exact: true });
    await region.getByLabel('Quality', { exact: true }).fill('999');
    await region
      .getByRole('tab', { name: workflow === 'AV1AN' ? 'Audio & Tracks' : 'Audio' })
      .click();
    const reason = page.getByRole('button', { name: /CRF/ });
    await expect(reason).toBeVisible();
    await reason.click();
    await expect(region.getByRole('tab', { name: 'Video', exact: true })).toHaveAttribute(
      'aria-selected',
      'true',
    );
    await region.getByLabel('Quality', { exact: true }).fill('20');
    await expect(
      page.getByRole('button', {
        name: workflow === 'AV1AN' ? 'Run: AV1AN encode' : 'Run: Encode',
        exact: true,
      }),
    ).toBeEnabled();
  });

  test(`${workflow} explains an empty output path`, async ({ page }) => {
    await installSourceEncodeFixture(page);
    await page.getByRole('button', { name: 'Add Files…' }).click();
    await page.getByRole('button', { name: 'Load Tracks' }).click();
    await page.getByRole('button', { name: workflow, exact: true }).click();
    const region = page.getByRole('region', { name: workflow, exact: true });
    await region.getByLabel('Output Path', { exact: true }).fill('');
    await expect(
      page.getByText('Choose an output path to continue.', { exact: true }),
    ).toBeVisible();
    await region.getByLabel('Output Path', { exact: true }).fill('/media/output');
    await expect(
      page.getByRole('button', {
        name: workflow === 'AV1AN' ? 'Run: AV1AN encode' : 'Run: Encode',
        exact: true,
      }),
    ).toBeEnabled();
  });
}

test('a missing required encoder is named beside Run', async ({ page }) => {
  await page.addInitScript(() => {
    (globalThis as any).__unavailableTools = ['svt-av1-hdr'];
  });
  await installSourceEncodeFixture(page);
  await page.getByRole('button', { name: 'Add Files…' }).click();
  await page.getByRole('button', { name: 'Load Tracks' }).click();
  await page.getByRole('button', { name: 'AV1AN', exact: true }).click();
  await expect(page.getByText(/Missing required tool.*svt-av1-hdr/i)).toBeVisible();
});

test('AV1AN exposes scene slices only when the prepass prerequisites are selected', async ({
  page,
}) => {
  await installSourceEncodeFixture(page);
  await page.getByRole('button', { name: 'Add Files…' }).click();
  await page.getByRole('button', { name: 'AV1AN', exact: true }).click();
  const av = page.getByRole('region', { name: 'AV1AN', exact: true });
  await av.getByRole('tab', { name: 'AV1AN Options' }).click();
  const slices = av.getByLabel('Detection Slices');
  await expect(av.getByText(/Detects scenes in this many parallel parts/)).toBeVisible();
  await av.getByLabel('Split Method').selectOption('sceneDetection');
  await av.getByLabel('Chunk Method').selectOption('lsmash');
  await expect(slices).toBeEnabled();
  await av.getByLabel('Chunk Method').selectOption('ffms2');
  await expect(slices).toBeDisabled();
  await av.getByLabel('Chunk Method').selectOption('lsmash');
  await av.getByLabel('Split Method').selectOption('fixedChunks');
  await expect(slices).toBeDisabled();
});

test('AV1AN accepts 64 workers and rejects 65', async ({ page }) => {
  await installSourceEncodeFixture(page);
  await page.getByRole('button', { name: 'Add Files…' }).click();
  await page.getByRole('button', { name: 'Load Tracks' }).click();
  await page.getByRole('button', { name: 'AV1AN', exact: true }).click();
  const av = page.getByRole('region', { name: 'AV1AN', exact: true });
  await av.getByRole('tab', { name: 'AV1AN Options' }).click();
  const workers = av.getByLabel('Workers', { exact: true });
  await expect(workers).toHaveAttribute('max', '64');
  await workers.fill('65');
  await expect(page.getByRole('button', { name: 'Run: AV1AN encode', exact: true })).toBeDisabled();
  await workers.fill('64');
  await page.getByRole('button', { name: 'Run: AV1AN encode', exact: true }).click();
  await expect
    .poll(() =>
      page.evaluate(
        () =>
          (globalThis as any).__sourceCalls.find(
            (call: any) => call.command === 'mock_start_encode',
          )?.payload.request.settings.workers,
      ),
    )
    .toBe(64);
});

test('SVT shows an inline error for a time-varying grain table', async ({ page }) => {
  await installSourceEncodeFixture(page);
  await page.evaluate(
    () => ((globalThis as any).__grainTable = 'filmgrn1\nE 0 50 1 2 1\nE 50 100 1 2 1\n'),
  );
  await page.getByRole('button', { name: 'Add Files…' }).click();
  await page.getByRole('button', { name: 'Load Tracks' }).click();
  await page.getByRole('button', { name: 'Quick Convert', exact: true }).click();
  const quick = page.getByRole('region', { name: 'Quick Convert', exact: true });
  await quick.getByLabel('Grain Synthesis').selectOption('table');
  await quick.getByPlaceholder('Path to a .tbl grain table').fill('/media/grain.tbl');
  await quick.getByPlaceholder('Path to a .tbl grain table').blur();
  await expect(
    quick.getByRole('alert').filter({ hasText: 'one segment starting at zero' }),
  ).toBeVisible();
  await expect(page.getByRole('button', { name: 'Run: Encode', exact: true })).toBeDisabled();
});
test('SVT shows an inline error for a grain table that ends before the video', async ({ page }) => {
  await installSourceEncodeFixture(page);
  // One second of grain for the fixture's 12-second source.
  await page.evaluate(() => ((globalThis as any).__grainTable = 'filmgrn1\nE 0 10000000 1 2 1\n'));
  await page.getByRole('button', { name: 'Add Files…' }).click();
  await page.getByRole('button', { name: 'Load Tracks' }).click();
  await page.getByRole('button', { name: 'Quick Convert', exact: true }).click();
  const quick = page.getByRole('region', { name: 'Quick Convert', exact: true });
  await quick.getByLabel('Grain Synthesis').selectOption('table');
  await quick.getByPlaceholder('Path to a .tbl grain table').fill('/media/grain.tbl');
  await quick.getByPlaceholder('Path to a .tbl grain table').blur();
  await expect(
    quick
      .getByRole('alert')
      .filter({ hasText: 'ends at 1.000 s but the video needs about 12.000 s' }),
  ).toBeVisible();
  await expect(page.getByRole('button', { name: 'Run: Encode', exact: true })).toBeDisabled();
});
for (const workflow of ['Quick Convert', 'AV1AN'] as const) {
  test(`${workflow} applies the AOM timed-grain rule for its chunking`, async ({ page }) => {
    await installSourceEncodeFixture(page);
    await page.evaluate(
      () => ((globalThis as any).__grainTable = 'filmgrn1\nE 0 50 1 2 1\nE 50 100 1 2 1\n'),
    );
    await page.getByRole('button', { name: 'Add Files…' }).click();
    await page.getByRole('button', { name: 'Load Tracks' }).click();
    await page.getByRole('button', { name: workflow, exact: true }).click();
    const region = page.getByRole('region', { name: workflow, exact: true });
    await region
      .getByLabel(workflow === 'AV1AN' ? 'Encoder' : 'Codec', { exact: true })
      .selectOption('aomAv1');
    await region.getByLabel('Grain Synthesis').selectOption('table');
    await region.getByPlaceholder('Path to a .tbl grain table').fill('/media/grain.tbl');
    await region.getByPlaceholder('Path to a .tbl grain table').blur();
    const warning = region.getByRole('alert').filter({ hasText: 'standalone AOM' });
    const run = page.getByRole('button', {
      name: workflow === 'AV1AN' ? 'Run: AV1AN encode' : 'Run: Encode',
      exact: true,
    });
    if (workflow === 'AV1AN') {
      await expect(warning).toBeVisible();
      await expect(run).toBeDisabled();
    } else {
      await expect(warning).toHaveCount(0);
      await expect(run).toBeEnabled();
    }
  });
}

test('AOM reports an incompatible CRLF grain table', async ({ page }) => {
  await installSourceEncodeFixture(page);
  await page.evaluate(() => ((globalThis as any).__grainTable = 'filmgrn1\r\nE 0 100 1 2 1\r\n'));
  await page.getByRole('button', { name: 'Add Files…' }).click();
  await page.getByRole('button', { name: 'Load Tracks' }).click();
  await page.getByRole('button', { name: 'Quick Convert', exact: true }).click();
  const quick = page.getByRole('region', { name: 'Quick Convert', exact: true });
  await quick.getByLabel('Codec', { exact: true }).selectOption('aomAv1');
  await quick.getByLabel('Grain Synthesis').selectOption('table');
  await quick.getByPlaceholder('Path to a .tbl grain table').fill('/media/grain.tbl');
  await quick.getByPlaceholder('Path to a .tbl grain table').blur();
  await expect(
    quick.getByRole('alert').filter({ hasText: 'AOM requires LF line endings' }),
  ).toBeVisible();
  await expect(page.getByRole('button', { name: 'Run: Encode', exact: true })).toBeDisabled();
});
