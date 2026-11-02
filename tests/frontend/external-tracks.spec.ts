import { expect, test } from '@playwright/test';
import { installSourceEncodeFixture, source } from './helpers/source-encode-fixture';

for (const workflow of ['Quick Convert', 'AV1AN'] as const) {
  test(`${workflow} exposes the fork HDR tone curves and submits its selected curve`, async ({
    page,
  }) => {
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
    await page.evaluate((hdr) => (globalThis as any).__setSourceMedia(hdr), hdr);
    await page.getByRole('button', { name: 'Add Files…' }).click();
    await page.getByRole('button', { name: 'Load Tracks' }).click();
    await page.getByRole('button', { name: workflow, exact: true }).click();
    const region = page.getByRole('region', { name: workflow, exact: true });
    const tone = region.getByLabel('Tone Mapping');
    await expect(tone).toHaveValue(workflow === 'AV1AN' ? 'mobius' : 'spline');
    await expect(tone.locator('option')).toHaveText(
      workflow === 'AV1AN'
        ? ['No tone mapping', 'Hable', 'Mobius', 'Reinhard']
        : ['No tone mapping', 'Hable', 'Mobius', 'Reinhard', 'Spline (GPU)'],
    );
    await tone.selectOption('reinhard');
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
            (globalThis as any).__sourceCalls.find((c: any) => c.command === 'mock_start_encode')
              ?.payload.request.settings.toneMap,
        ),
      )
      .toMatchObject({ algorithm: 'reinhard', backend: 'auto', peakMode: 'measured' });
  });
}

test('per-track audio settings retain separate source identities with colliding stream indices', async ({
  page,
}) => {
  await installSourceEncodeFixture(page);
  const external = {
    ...source,
    id: 'external-audio',
    path: '/other/voice.mka',
    name: 'voice.mka',
    streams: [{ ...source.streams[1], title: 'External voice' }],
  };
  await page.evaluate(
    ({ first, external }) => {
      const h = globalThis as any;
      h.__pickerPaths = [first.path, external.path];
      h.__mediaByPath = { [first.path]: first, [external.path]: external };
    },
    { first: source, external },
  );
  await page.getByRole('button', { name: 'Add Files…' }).click();
  await page.getByRole('option', { name: /input.mkv/ }).click();
  await page.getByRole('option', { name: /voice.mka/ }).click({ modifiers: ['Control'] });
  await page.getByRole('button', { name: 'Load Tracks' }).click();
  await page.getByRole('button', { name: 'Quick Convert', exact: true }).click();
  const quick = page.getByRole('region', { name: 'Quick Convert', exact: true });
  await quick.getByRole('tab', { name: 'Audio' }).click();
  await quick.getByLabel('Configuration').selectOption('perTrack');
  const dialog = page.getByRole('dialog', { name: 'Configure Audio Tracks' });
  await dialog.getByLabel('Channels for track 1').fill('2');
  await dialog.getByLabel('Bitrate for track 1').fill('128');
  await dialog.getByLabel('Channels for track 2').fill('6');
  await dialog.getByLabel('Bitrate for track 2').fill('384');
  await dialog.getByRole('button', { name: 'OK' }).click();
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
  expect(request.settings.audio).toContainEqual(
    expect.objectContaining({
      streamIndex: 1,
      channels: { explicit: { count: 2, layout: 'ffmpegDefault' } },
      bitrateKbps: 128,
    }),
  );
  expect(request.settings.externalTracks).toContainEqual(
    expect.objectContaining({
      inputPath: '/other/voice.mka',
      streamIndex: 1,
      audio: expect.objectContaining({
        channels: { explicit: { count: 6, layout: 'ffmpegDefault' } },
        bitrateKbps: 384,
      }),
    }),
  );
});

test('AV1AN preserves a selected external audio track and applies the source audio codec', async ({
  page,
}) => {
  await installSourceEncodeFixture(page);
  const external = {
    ...source,
    id: 'external-audio',
    path: '/other/voice.mka',
    name: 'voice.mka',
    streams: [{ ...source.streams[1], title: 'External voice' }],
  };
  await page.evaluate(
    ({ first, external }) => {
      const h = globalThis as any;
      h.__pickerPaths = [first.path, external.path];
      h.__mediaByPath = { [first.path]: first, [external.path]: external };
    },
    { first: source, external },
  );
  await page.getByRole('button', { name: 'Add Files…' }).click();
  await page.getByRole('option', { name: /input.mkv/ }).click();
  await page.getByRole('option', { name: /voice.mka/ }).click({ modifiers: ['Control'] });
  await page.getByRole('button', { name: 'Load Tracks' }).click();
  await page.getByRole('button', { name: 'AV1AN', exact: true }).click();
  const av = page.getByRole('region', { name: 'AV1AN', exact: true });
  await av.getByRole('tab', { name: 'Audio & Tracks' }).click();
  await av.getByLabel('Codec', { exact: true }).selectOption('aac');
  await page.getByRole('button', { name: 'Run: AV1AN encode', exact: true }).click();
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
  expect(request.settings.audio).toContainEqual(
    expect.objectContaining({ streamIndex: 1, codec: 'aac' }),
  );
  expect(request.settings.externalTracks).toContainEqual(
    expect.objectContaining({
      inputPath: '/other/voice.mka',
      streamIndex: 1,
      audio: expect.objectContaining({ codec: 'aac' }),
    }),
  );
});
