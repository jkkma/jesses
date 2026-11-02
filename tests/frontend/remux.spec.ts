import { expect, test } from '@playwright/test';
import { installSourceEncodeFixture, source } from './helpers/source-encode-fixture';

test('Strip Video allows a selected subtitle without requiring audio or mapping checked pictures', async ({
  page,
}) => {
  await installSourceEncodeFixture(page);
  const media = {
    ...source,
    streams: [...source.streams, { ...source.streams[0], index: 3, title: 'Alternate picture' }],
  };
  await page.evaluate((media) => (globalThis as any).__setSourceMedia(media), media);
  await page.getByRole('button', { name: 'Add Files…' }).click();
  await page.getByRole('button', { name: 'Load Tracks' }).click();
  await page.getByRole('checkbox', { name: /Include .*Audio/ }).uncheck();
  await page.getByRole('button', { name: 'Quick Convert', exact: true }).click();
  const quick = page.getByRole('region', { name: 'Quick Convert', exact: true });
  await quick.getByLabel('Codec', { exact: true }).selectOption('drop');
  await expect(page.getByRole('button', { name: 'Run: Encode', exact: true })).toBeEnabled();
  await page.getByRole('button', { name: 'Run: Encode', exact: true }).click();
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
  expect(request.source.streamIndices).toEqual([2]);
  expect(request.settings.videoMode).toBe('drop');
});

test('copy conversion reports a preparation failure and permits correction and retry', async ({
  page,
}) => {
  await installSourceEncodeFixture(page);
  await page.getByRole('button', { name: 'Add Files…' }).click();
  await page.getByRole('button', { name: 'Load Tracks' }).click();
  await page.getByRole('button', { name: 'Quick Convert', exact: true }).click();
  const quick = page.getByRole('region', { name: 'Quick Convert', exact: true });
  await quick.getByLabel('Codec', { exact: true }).selectOption('copy');
  await quick.getByRole('tab', { name: 'Audio' }).click();
  await quick.getByLabel('Codec', { exact: true }).selectOption('copy');
  await page.evaluate(() => {
    (globalThis as any).__prepareFailure = 'Output cannot be written';
  });
  await page.getByRole('button', { name: 'Run: Encode', exact: true }).click();
  await expect(quick.getByRole('alert')).toContainText('Output cannot be written');
  expect(
    await page.evaluate(() =>
      (globalThis as any).__sourceCalls.some((c: any) => c.command === 'mock_start_encode'),
    ),
  ).toBe(false);
  await page.evaluate(() => {
    (globalThis as any).__prepareFailure = '';
  });
  await quick.getByLabel('Output Path', { exact: true }).fill('/media/corrected');
  await page.getByRole('button', { name: 'Run: Encode', exact: true }).click();
  await expect
    .poll(() =>
      page.evaluate(
        () =>
          (globalThis as any).__sourceCalls.find((c: any) => c.command === 'mock_start_encode')
            ?.payload.request.source.outputPath,
      ),
    )
    .toBe('/media/corrected.mkv');
});

test('a webview reload follows the existing copy job through failure without reporting success', async ({
  page,
}) => {
  await installSourceEncodeFixture(page);
  const job = {
    id: 'copy-job',
    state: 'running',
    request: {
      inputPath: '/media/input.mkv',
      outputPath: '/media/copy.mkv',
      streamIndices: [0, 1],
    },
    encodeSettings: { backend: 'standalone' },
    progressSeconds: 3,
    durationSeconds: 12,
    logs: [],
    error: null,
    recovery: null,
    logPath: null,
  };
  await page.evaluate(
    (job) => sessionStorage.setItem('test-initial-jobs', JSON.stringify([job])),
    job,
  );
  await page.reload();
  await expect(page.getByLabel('Current action')).toHaveText('Running: Encode...');
  await expect(page.getByLabel('Task progress')).toHaveAttribute('value', '3');
  await page.evaluate(() => {
    const host = globalThis as any;
    host.__testJobs = host.__testJobs.map((job: any) => ({
      ...job,
      state: 'failed',
      error: { code: 'ENCODE_FAILED', message: 'copy failed' },
    }));
    host.__emitJobs();
  });
  await expect(page.getByLabel('Current action')).toHaveText(
    'Did not finish - the log has the details.',
  );
  await expect(page.getByLabel('Current action')).not.toHaveText('Done.');
});

test('copy conversion requires a destination and selected tracks', async ({ page }) => {
  await installSourceEncodeFixture(page);
  await page.getByRole('button', { name: 'Add Files…' }).click();
  await page.getByRole('button', { name: 'Load Tracks' }).click();
  await page.getByRole('button', { name: 'Quick Convert', exact: true }).click();
  const quick = page.getByRole('region', { name: 'Quick Convert', exact: true });
  await quick.getByLabel('Codec', { exact: true }).selectOption('copy');
  await quick.getByLabel('Output Path', { exact: true }).fill('');
  await expect(page.getByRole('button', { name: 'Run: Encode', exact: true })).toBeDisabled();
  await quick.getByLabel('Output Path', { exact: true }).fill('/media/copy');
  await page.getByRole('button', { name: 'Track List', exact: true }).click();
  const tracks = page.getByRole('region', { name: 'Track List' });
  await tracks.getByText('Select…', { exact: true }).click();
  await tracks.getByRole('button', { name: 'Check None', exact: true }).click();
  await page.getByRole('button', { name: 'Quick Convert', exact: true }).click();
  await page.getByRole('button', { name: 'Run: Encode', exact: true }).click();
  await expect(tracks).toBeVisible();
  await expect(page.getByRole('log', { name: 'Recent activity' })).toContainText(
    'No tracks are selected. Please select at least one track to use.',
  );
  await expect(page.getByLabel('Current action')).not.toHaveText('Done.');
  expect(
    await page.evaluate(() =>
      (globalThis as any).__sourceCalls.some((c: any) => c.command === 'mock_start_encode'),
    ),
  ).toBe(false);
  await tracks.getByRole('checkbox', { name: /Include .*Video/ }).check();
  await page.getByRole('button', { name: 'Quick Convert', exact: true }).click();
  await page.getByRole('button', { name: 'Run: Encode', exact: true }).click();
  await expect
    .poll(() =>
      page.evaluate(
        () =>
          (globalThis as any).__sourceCalls.find((c: any) => c.command === 'mock_start_encode')
            ?.payload.request.source.streamIndices,
      ),
    )
    .toEqual([0]);
});

test('image export also rejects an entirely unchecked Track List', async ({ page }) => {
  await installSourceEncodeFixture(page);
  await page.getByRole('button', { name: 'Add Files…' }).click();
  await page.getByRole('button', { name: 'Load Tracks' }).click();
  const tracks = page.getByRole('region', { name: 'Track List' });
  await tracks.getByText('Select…', { exact: true }).click();
  await tracks.getByRole('button', { name: 'Check None', exact: true }).click();
  await page.getByRole('button', { name: 'Quick Convert', exact: true }).click();
  const quick = page.getByRole('region', { name: 'Quick Convert', exact: true });
  await quick.getByLabel('Codec', { exact: true }).selectOption('gif');
  await page.getByRole('button', { name: 'Run: Encode', exact: true }).click();
  await expect(tracks).toBeVisible();
  await expect(page.getByRole('log', { name: 'Recent activity' })).toContainText(
    'No tracks are selected.',
  );
  expect(
    await page.evaluate(() =>
      (globalThis as any).__sourceCalls.some((c: any) => c.command === 'prepare_command_edit'),
    ),
  ).toBe(false);
  await expect(page.getByLabel('Current action')).not.toHaveText('Done.');
});

for (const kind of ['Audio', 'Subtitle'] as const) {
  test(`Quick retains the codec draft but omits unchecked video for ${kind.toLowerCase()}-only selection`, async ({
    page,
  }) => {
    await installSourceEncodeFixture(page);
    await page.getByRole('button', { name: 'Add Files…' }).click();
    await page.getByRole('button', { name: 'Load Tracks' }).click();
    await page.getByRole('button', { name: 'Quick Convert', exact: true }).click();
    await page
      .getByRole('region', { name: 'Quick Convert', exact: true })
      .getByLabel('Quality', { exact: true })
      .fill('');
    await page.getByRole('button', { name: 'Track List', exact: true }).click();
    const tracks = page.getByRole('region', { name: 'Track List' });
    await tracks.getByText('Select…', { exact: true }).click();
    await tracks.getByRole('button', { name: 'Check None', exact: true }).click();
    await tracks.getByRole('checkbox', { name: new RegExp(`Include .*${kind}`) }).check();
    await page.getByRole('button', { name: 'Quick Convert', exact: true }).click();
    const quick = page.getByRole('region', { name: 'Quick Convert', exact: true });
    await expect(quick.getByLabel('Codec', { exact: true })).toHaveValue('svtAv1');
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
        (globalThis as any).__sourceCalls.find((c: any) => c.command === 'mock_start_encode')
          .payload.request,
    );
    expect(request.source.streamIndices).toEqual([kind === 'Audio' ? 1 : 2]);
    expect(request.settings.videoMode).toBe('drop');
    expect(request.settings.temporal).toBeUndefined();
    expect(request.settings.hdrMetadata).toBeUndefined();
    expect(request.settings.outputPixelFormat).toBeUndefined();
    expect(
      await page.evaluate(() =>
        (globalThis as any).__sourceCalls.some((c: any) =>
          ['detect_crop', 'resolve_deinterlace'].includes(c.command),
        ),
      ),
    ).toBe(false);
    await expect(quick.getByLabel('Codec', { exact: true })).toHaveValue('svtAv1');
  });
}

test('Quick sends an external-only audio selection without mapping the unchecked primary video', async ({
  page,
}) => {
  await installSourceEncodeFixture(page);
  const first = {
    id: 'first',
    path: '/media/main.mkv',
    name: 'main.mkv',
    sizeBytes: '1000',
    durationSeconds: 12,
    format: 'matroska',
    streams: [
      {
        index: 0,
        kind: 'video',
        codec: 'h264',
        width: 640,
        height: 360,
        frameRate: '24/1',
        sampleRate: null,
        channels: null,
        language: null,
        title: null,
      },
    ],
  };
  const external = {
    id: 'external',
    path: '/media/voice.mka',
    name: 'voice.mka',
    sizeBytes: '1000',
    durationSeconds: 12,
    format: 'matroska',
    streams: [
      {
        index: 0,
        kind: 'audio',
        codec: 'aac',
        width: null,
        height: null,
        frameRate: null,
        sampleRate: 48000,
        channels: 2,
        language: 'eng',
        title: 'External voice',
      },
    ],
  };
  await page.evaluate(
    ({ first, external }) => {
      const h = globalThis as any;
      h.__pickerPaths = [first.path, external.path];
      h.__mediaByPath = { [first.path]: first, [external.path]: external };
    },
    { first, external },
  );
  await page.getByRole('button', { name: 'Add Files…' }).click();
  await page.getByRole('option', { name: /main.mkv/ }).click();
  await page.getByRole('option', { name: /voice.mka/ }).click({ modifiers: ['Control'] });
  await page.getByRole('button', { name: 'Load Tracks' }).click();
  await page.getByRole('checkbox', { name: /Include .*Video/ }).uncheck();
  await page.getByRole('button', { name: 'Quick Convert', exact: true }).click();
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
  expect(request.source).toMatchObject({ inputPath: '/media/main.mkv', streamIndices: [] });
  expect(request.settings.videoMode).toBe('drop');
  expect(request.settings.externalTracks).toContainEqual(
    expect.objectContaining({ inputPath: '/media/voice.mka', streamIndex: 0 }),
  );
});

test('without checked video a selected subtitle is retained, not burned into an unchecked picture', async ({
  page,
}) => {
  await installSourceEncodeFixture(page);
  await page.getByRole('button', { name: 'Add Files…' }).click();
  await page.getByRole('button', { name: 'Load Tracks' }).click();
  await page.getByRole('button', { name: 'Quick Convert', exact: true }).click();
  const quick = page.getByRole('region', { name: 'Quick Convert', exact: true });
  await quick.getByRole('tab', { name: 'Subtitles' }).click();
  await quick.getByLabel('Burn In').selectOption('2');
  await page.getByRole('button', { name: 'Track List', exact: true }).click();
  const tracks = page.getByRole('region', { name: 'Track List' });
  await tracks.getByRole('checkbox', { name: /Include .*Video/ }).uncheck();
  await tracks.getByRole('checkbox', { name: /Include .*Audio/ }).uncheck();
  await page.getByRole('button', { name: 'Quick Convert', exact: true }).click();
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
  expect(request.source.streamIndices).toEqual([2]);
  expect(request.settings.subtitles ?? []).toEqual([]); // Unspecified subtitle mode is native stream copy.
  expect(request.settings.videoMode).toBe('drop');
});

test('Stop cancels the running copy job and reflects the terminal snapshot', async ({ page }) => {
  await installSourceEncodeFixture(page);
  await page.evaluate(() => {
    (globalThis as any).__holdTerminal = true;
  });
  await page.getByRole('button', { name: 'Add Files…' }).click();
  await page.getByRole('button', { name: 'Load Tracks' }).click();
  await page.getByRole('button', { name: 'Quick Convert', exact: true }).click();
  const quick = page.getByRole('region', { name: 'Quick Convert', exact: true });
  await quick.getByLabel('Codec', { exact: true }).selectOption('copy');
  await page.getByRole('button', { name: 'Run: Encode', exact: true }).click();
  await expect(page.getByRole('button', { name: 'Stop', exact: true })).toBeVisible();
  await expect
    .poll(() =>
      page.evaluate(() =>
        (globalThis as any).__sourceCalls.some((c: any) => c.command === 'mock_start_encode'),
      ),
    )
    .toBe(true);
  await expect(page.getByRole('button', { name: 'Pause', exact: true })).toBeEnabled();
  await page.getByRole('button', { name: 'Settings', exact: true }).click();
  await expect(page.getByLabel('Current action')).toHaveText('Running: Encode...');
  await expect(page.getByRole('button', { name: 'Pause', exact: true })).toBeEnabled();
  await page.getByRole('button', { name: 'Stop', exact: true }).click();
  await expect
    .poll(() =>
      page.evaluate(() =>
        (globalThis as any).__sourceCalls.some((c: any) => c.command === 'cancel_all_jobs'),
      ),
    )
    .toBe(true);
  await expect(page.getByLabel('Current action')).toHaveText('Canceled.');
});

test('a rejected empty selection replaces a previous successful status', async ({ page }) => {
  await installSourceEncodeFixture(page);
  await page.getByRole('button', { name: 'Add Files…' }).click();
  await page.getByRole('button', { name: 'Load Tracks' }).click();
  await page.getByRole('button', { name: 'Quick Convert', exact: true }).click();
  await page.getByRole('button', { name: 'Run: Encode', exact: true }).click();
  await expect(page.getByLabel('Current action')).toHaveText('Done.');
  await page.getByRole('button', { name: 'Track List', exact: true }).click();
  const tracks = page.getByRole('region', { name: 'Track List' });
  await tracks.getByText('Select…', { exact: true }).click();
  await tracks.getByRole('button', { name: 'Check None', exact: true }).click();
  await page.getByRole('button', { name: 'Quick Convert', exact: true }).click();
  await page.getByRole('button', { name: 'Run: Encode', exact: true }).click();
  await expect(tracks).toBeVisible();
  await expect(page.getByLabel('Current action')).toHaveText('Canceled.');
  expect(
    await page.evaluate(
      () =>
        (globalThis as any).__sourceCalls.filter((c: any) => c.command === 'prepare_command_edit')
          .length,
    ),
  ).toBe(1);
});
