import { expect, test, type Locator, type Page } from '@playwright/test';
import { showAllEncodeSettings } from './helpers/encode-settings';
import { mkdir } from 'node:fs/promises';
import { join } from 'node:path';
import type { EncodeRequest, MediaFile, MediaStream } from '../../src/lib/ipc/generated';

const stream = (index: number, kind: string, codec: string, title: string): MediaStream => ({
  index,
  kind,
  codec,
  width: kind === 'video' ? 320 : null,
  height: kind === 'video' ? 180 : null,
  frameRate: kind === 'video' ? '24/1' : null,
  sampleRate: kind === 'audio' ? 48000 : null,
  channels: kind === 'audio' ? 2 : null,
  language: kind === 'subtitle' ? 'en' : null,
  title,
});
const source = (id: string, name: string, streams: MediaStream[]): MediaFile => ({
  id,
  path: `C:\\media\\${name}`,
  name,
  sizeBytes: '123456',
  durationSeconds: 12,
  format: 'matroska,webm',
  streams,
});
const primary = source('main', 'main.mkv', [
  stream(0, 'video', 'h264', 'Main picture'),
  stream(3, 'audio', 'aac', 'Original sound'),
  stream(7, 'subtitle', 'subrip', 'Original captions'),
  stream(9, 'attachment', 'ttf', 'Original font'),
]);
const dubbed = source('dub', 'dub.mka', [
  stream(2, 'audio', 'flac', 'Dub sound'),
  stream(5, 'attachment', 'ttf', 'Dub font'),
  stream(6, 'video', 'h264', 'Unwanted picture'),
]);
const captions = source('captions', 'captions.mks', [stream(4, 'subtitle', 'ass', 'Signs')]);
const alternate = source('alternate', 'alternate.mka', [
  stream(3, 'audio', 'flac', 'Alternate language'),
]);
const second = source('second', 'second.mka', [stream(3, 'audio', 'aac', 'Commentary')]);
const surround = source('surround', 'surround.mka', [
  { ...stream(3, 'audio', 'flac', 'Surround'), channels: 6, channelLayout: '5.1' },
]);
const bitmap = source('bitmap', 'bitmap.mks', [
  stream(4, 'subtitle', 'hdmv_pgs_subtitle', 'Bitmap captions'),
]);
const movText = source('movtext', 'movtext.mp4', [stream(5, 'subtitle', 'mov_text', 'Timed text')]);
const donorOnly = source('donor', 'donor.mkv', []);
const timecodeDonor = source('timecode', 'timecode.mov', [
  { ...stream(6, 'data', 'bin_data', 'Camera timecode'), codecTag: 'tmcd' },
  { ...stream(8, 'data', 'bin_data', 'Unrelated data'), codecTag: 'gpmd' },
]);
const primaryMov: MediaFile = {
  ...primary,
  path: 'C:\\media\\main.mov',
  name: 'main.mov',
  streams: [
    ...primary.streams,
    { ...stream(6, 'data', 'bin_data', 'Camera timecode'), codecTag: 'tmcd' },
  ],
};
const hdrPrimary: MediaFile = {
  ...primary,
  streams: primary.streams.map((entry) =>
    entry.kind === 'video'
      ? {
          ...entry,
          pixelFormat: 'yuv420p10le',
          bitDepth: 10,
          colorPrimaries: 'bt2020',
          colorTransfer: 'smpte2084',
          colorSpace: 'bt2020nc',
          colorRange: 'tv',
          hdrFormat: 'HDR10',
        }
      : entry,
  ),
};
const dv5Primary: MediaFile = {
  ...primary,
  streams: primary.streams.map((entry) =>
    entry.kind === 'video'
      ? {
          ...entry,
          codec: 'hevc',
          pixelFormat: 'yuv420p10le',
          bitDepth: 10,
          colorPrimaries: 'unknown',
          colorTransfer: 'unknown',
          colorSpace: 'unknown',
          colorRange: 'pc',
          hdrFormat: 'Dolby Vision',
          dynamicHdrFormats: ['Dolby Vision'],
          dolbyVisionProfile: 5,
        }
      : entry,
  ),
};

type Call = { command: string; payload: Record<string, unknown> };
type Mock = { calls: Call[]; setMedia: (file: MediaFile) => void };

async function mockDesktop(page: Page, initialFile: MediaFile = primary) {
  await page.addInitScript(
    ({ initial, capabilities }) => {
      const state = globalThis as unknown as Record<string, unknown>;
      let selected = initial;
      let callback = 0;
      const calls: Call[] = [];
      state.__externalMock = {
        calls,
        setMedia: (file: MediaFile) => (selected = file),
      } satisfies Mock;
      state.isTauri = true;
      state.__TAURI_INTERNALS__ = {
        metadata: { currentWindow: { label: 'main' }, currentWebview: { label: 'main' } },
        transformCallback: () => ++callback,
        unregisterCallback: () => {},
        invoke: async (command: string, payload: Record<string, unknown> = {}) => {
          calls.push({ command, payload });
          if (command === 'plugin:dialog|open') return [selected.path];
          if (command === 'plugin:dialog|save') return 'C:\\exports\\output.mkv';
          if (command.startsWith('plugin:event|')) return ++callback;
          if (command === 'get_capabilities') return capabilities;
          if (command === 'get_completion_status')
            return {
              options: { notify: false, finishAction: 'none' },
              armedJobs: 0,
              secondsRemaining: null,
              error: null,
            };
          if (command === 'begin_media_analysis') return `analysis-${++callback}`;
          if (command === 'cancel_media_analysis') return;
          if (command === 'measure_loudness')
            return {
              integratedLufs: -20.1,
              truePeakDbfs: -4.2,
              loudnessRangeLu: 5.5,
              suggestedGainTenthsDb: -29,
              targetLimitedByPeak: false,
              sourceFingerprint: 'b'.repeat(64),
              message: 'Review the flat gain before applying it.',
            };
          if (command === 'probe_media') return selected;
          if (command === 'list_jobs') return [];
          if (command === 'subscribe_jobs') {
            (payload.channel as { onmessage: (jobs: unknown[]) => void }).onmessage([]);
            return;
          }
          if (command === 'preview_encode_plan')
            return {
              request: payload.request,
              sourceFingerprint: 'a'.repeat(64),
              outputFrameCount: '288',
              outputFrameRate: '24/1',
              stages: [],
              notes: [],
            };
          if (command === 'enqueue_encode' || command === 'start_encode') {
            const request = payload.request as EncodeRequest;
            return {
              id: 'job-1',
              state: command === 'enqueue_encode' ? 'queued' : 'running',
              request: request.source,
              encodeSettings: request.settings,
              progressSeconds: 0,
              durationSeconds: 12,
              logs: [],
              error: null,
              recovery: null,
              logPath: null,
            };
          }
          throw new Error(`Unexpected command ${command}`);
        },
      };
    },
    {
      initial: initialFile,
      capabilities: [
        'ffmpeg',
        'ffprobe',
        'svt-av1',
        'svt-av1-hdr',
        'svt-av1-5fish',
        'av1an',
        'x264',
        'aomenc',
        'vpxenc',
        'x265',
        'mkvmerge',
      ].map((id) => ({
        id,
        name: id,
        available: true,
        path: `C:\\tools\\${id}.exe`,
        version: 'test',
        detail: null,
      })),
    },
  );
}

async function calls(page: Page, command: string): Promise<Call[]> {
  return page.evaluate(
    (name) =>
      (globalThis as unknown as { __externalMock: Mock }).__externalMock.calls.filter(
        (call) => call.command === name,
      ),
    command,
  );
}

async function importFile(page: Page, file: MediaFile) {
  await page.evaluate(
    (value) => (globalThis as unknown as { __externalMock: Mock }).__externalMock.setMedia(value),
    file,
  );
  await page
    .getByRole('navigation', { name: 'Workspace' })
    .getByRole('button', { name: /^Files/ })
    .click();
  await page.getByRole('button', { name: 'Add files', exact: true }).first().click();
  await expect(page.getByRole('heading', { name: file.name, exact: true })).toBeVisible();
}

const workspace = (page: Page, tab: 'Quick Convert' | 'av1an') =>
  page.getByRole('region', {
    name: tab === 'av1an' ? 'av1an workspace' : 'Quick Convert workspace',
    exact: true,
  });

async function choosePrimary(page: Page, tab: 'Quick Convert' | 'av1an') {
  await page.getByRole('button', { name: tab, exact: true }).click();
  await showAllEncodeSettings(page);
  await workspace(page, tab)
    .getByLabel('Source for this encode', { exact: true })
    .selectOption(primary.id);
}

async function capturePanelEvidence(page: Page, panel: Locator, name: string) {
  const evidence = process.env.JESSES_UI_EVIDENCE;
  if (!evidence) return;
  await mkdir(evidence, { recursive: true });
  for (const [width, height] of [
    [1280, 850],
    [1000, 800],
  ] as const) {
    await page.setViewportSize({ width, height });
    await panel.scrollIntoViewIfNeeded();
    await panel.locator('.layout-content').evaluate((element) => {
      element.scrollTop = 0;
    });
    await page.screenshot({ path: join(evidence, `${name}-${width}x${height}.png`) });
    await page.screenshot({
      path: join(evidence, `${name}-${width}x${height}-full.png`),
      fullPage: true,
    });
    await panel.locator('.layout-content').evaluate((element) => {
      element.scrollTop = element.scrollHeight;
    });
    await page.screenshot({ path: join(evidence, `${name}-${width}x${height}-bottom.png`) });
  }
}

for (const tab of ['Quick Convert', 'av1an'] as const) {
  test(`${tab} copies a separate MOV timecode only with full unchanged cadence`, async ({
    page,
  }) => {
    await mockDesktop(page);
    await page.goto('/');
    await page.getByRole('button', { name: 'Add files', exact: true }).first().click();
    await importFile(page, timecodeDonor);
    await choosePrimary(page, tab);
    const form = workspace(page, tab);
    await form.getByLabel('Output container').selectOption('mov');
    const layout = form.locator('details.track-layout');
    await layout.locator('summary').first().click();
    const selector = layout.getByLabel('MOV timecode track');
    await expect(selector.locator('option')).toHaveCount(2);
    await selector.selectOption({ label: 'timecode.mov · stream #6' });
    await capturePanelEvidence(
      page,
      layout,
      `${tab === 'av1an' ? 'av1an' : 'quick-convert'}-timecode`,
    );
    await form.getByRole('button', { name: 'Add to queue', exact: true }).click();
    const request = (await calls(page, 'enqueue_encode'))[0].payload.request as EncodeRequest;
    expect(request.settings.movTimecodeTrack).toEqual({
      inputPath: timecodeDonor.path,
      streamIndex: 6,
    });
    expect(request.source.streamIndices).not.toContain(6);
    expect(request.settings).not.toHaveProperty('externalTracks');
    expect(request.settings).not.toHaveProperty('trackOrder');
    await form.getByLabel('Trim video interval', { exact: true }).check();
    await expect(form.getByRole('button', { name: 'Add to queue', exact: true })).toBeDisabled();
    await expect(form).toContainText(
      'A copied MOV timecode track cannot be combined with trimming',
    );
    await form.getByLabel('Trim video interval', { exact: true }).uncheck();
    await form.locator('details.temporal-options summary').click();
    await form.getByLabel('Source reconstruction').selectOption('frame');
    await expect(form.getByRole('button', { name: 'Add to queue', exact: true })).toBeDisabled();
    await expect(form).toContainText(
      'cannot be combined with frame-rate, deinterlace or cadence changes',
    );
    await form.getByLabel('Source reconstruction').selectOption('off');
    await form.getByLabel('Output container').selectOption('matroska');
    await expect(form.getByRole('button', { name: 'Add to queue', exact: true })).toBeDisabled();
    await expect(form).toContainText('A copied timecode track requires MOV output');
    await selector.selectOption('');
    await expect(form.getByRole('button', { name: 'Add to queue', exact: true })).toBeEnabled();
  });

  test(`${tab} keeps donor choices, track labels and a full cross-file order`, async ({ page }) => {
    await mockDesktop(page);
    await page.goto('/');
    await page.getByRole('button', { name: 'Add files', exact: true }).first().click();
    await importFile(page, dubbed);
    await importFile(page, captions);
    await importFile(page, donorOnly);
    await choosePrimary(page, tab);
    const form = workspace(page, tab);
    const tracks = form.locator('details.external-tracks');
    await tracks.locator('summary').click();
    await tracks.getByLabel('Add dub.mka audio stream #2').check();
    await tracks.getByLabel('Add captions.mks subtitle stream #4').check();
    await tracks.getByLabel('Add dub.mka attachment stream #5').check();
    const layout = form.locator('details.track-layout');
    await layout.locator('summary').first().click();
    await layout.getByLabel('Container metadata source').selectOption(donorOnly.id);
    await layout.getByLabel('Chapters source').selectOption(captions.id);
    await layout.getByRole('button', { name: 'Move dub.mka stream #2 up' }).click();
    await layout.getByRole('button', { name: 'Move dub.mka stream #5 up' }).click();
    const primaryAudio = layout.getByRole('group', { name: 'Output track main.mkv stream #3' });
    await primaryAudio.getByText('Labels & flags').click();
    await primaryAudio.getByLabel('Change language').check();
    await primaryAudio.getByLabel('Track language').fill('fr');
    await expect(form.getByRole('button', { name: 'Add to queue', exact: true })).toBeDisabled();
    await expect(form).toContainText('Track language must be a three-letter code');
    await primaryAudio.getByLabel('Track language').fill('spa');
    await primaryAudio.getByLabel('Track default flag').selectOption('no');
    const dubAudio = layout.getByRole('group', { name: 'Output track dub.mka stream #2' });
    await dubAudio.getByText('Labels & flags').click();
    await dubAudio.getByLabel('Change title').check();
    await dubAudio.getByLabel('Track title').fill('');
    await dubAudio.getByLabel('Change language').check();
    await dubAudio.getByLabel('Track language').fill('fra');
    await dubAudio.getByLabel('Track default flag').selectOption('yes');
    await dubAudio.getByLabel('Track forced flag').selectOption('no');
    await form.getByLabel('Video encoder', { exact: true }).selectOption('svtAv1');
    await form.getByLabel('Video encoder', { exact: true }).selectOption('svtAv1Hdr');
    await expect(layout.getByLabel('Container metadata source')).toHaveValue(donorOnly.id);
    await expect(dubAudio.getByLabel('Track language')).toHaveValue('fra');
    await capturePanelEvidence(
      page,
      layout,
      `${tab === 'av1an' ? 'av1an' : 'quick-convert'}-layout`,
    );
    await form.getByRole('button', { name: 'Preview command plan', exact: true }).click();
    await expect.poll(() => calls(page, 'preview_encode_plan')).toHaveLength(1);
    const preview = (await calls(page, 'preview_encode_plan'))[0].payload.request as EncodeRequest;
    expect(preview.settings.metadataSourcePath).toBe(donorOnly.path);
    expect(preview.settings.chaptersSourcePath).toBe(captions.path);
    expect(preview.settings.trackOverrides).toEqual([
      { streamIndex: 3, language: 'spa', default: false },
    ]);
    expect(preview.settings.externalTracks).toEqual([
      {
        inputPath: dubbed.path,
        streamIndex: 2,
        title: '',
        language: 'fra',
        default: true,
        forced: false,
      },
      { inputPath: captions.path, streamIndex: 4 },
      { inputPath: dubbed.path, streamIndex: 5 },
    ]);
    expect(preview.settings.trackOrder).toEqual([
      { streamIndex: 0 },
      { streamIndex: 3 },
      { inputPath: dubbed.path, streamIndex: 2 },
      { streamIndex: 7 },
      { inputPath: captions.path, streamIndex: 4 },
      { inputPath: dubbed.path, streamIndex: 5 },
      { streamIndex: 9 },
    ]);
    await form.getByRole('button', { name: 'Add to queue', exact: true }).click();
    const queued = (await calls(page, 'enqueue_encode'))[0].payload.request as EncodeRequest;
    expect(queued.settings.trackOrder).toEqual(preview.settings.trackOrder);
    await layout.evaluate((element: HTMLDetailsElement) => {
      element.open = true;
    });
    await dubAudio.locator('details.track-tags').evaluate((element: HTMLDetailsElement) => {
      element.open = true;
    });
    await dubAudio.getByLabel('Track language').fill('deu');
    expect(queued.settings.externalTracks?.[0].language).toBe('fra');
  });

  test(`${tab} blocks a removed or refreshed donor until reselected`, async ({ page }) => {
    await mockDesktop(page);
    await page.goto('/');
    await page.getByRole('button', { name: 'Add files', exact: true }).first().click();
    await importFile(page, donorOnly);
    await choosePrimary(page, tab);
    const form = workspace(page, tab);
    const layout = form.locator('details.track-layout');
    await layout.locator('summary').first().click();
    await layout.getByLabel('Container metadata source').selectOption(donorOnly.id);
    await page
      .getByRole('navigation', { name: 'Workspace' })
      .getByRole('button', { name: /^Files/ })
      .click();
    await page.getByRole('button', { name: `Remove ${donorOnly.name}`, exact: true }).click();
    await page.getByRole('button', { name: tab, exact: true }).click();
    await showAllEncodeSettings(page);
    await expect(form.getByRole('button', { name: 'Add to queue', exact: true })).toBeDisabled();
    await expect(layout).toContainText('Container metadata source was removed or changed');
    await importFile(page, { ...donorOnly, title: 'Updated donor' });
    await page.getByRole('button', { name: tab, exact: true }).click();
    await showAllEncodeSettings(page);
    await expect(form.getByRole('button', { name: 'Add to queue', exact: true })).toBeDisabled();
    await layout.getByLabel('Container metadata source').selectOption(donorOnly.id);
    await expect(form.getByRole('button', { name: 'Add to queue', exact: true })).toBeEnabled();
  });

  test(`${tab} converts source-local external audio and restores its draft`, async ({ page }) => {
    await mockDesktop(page);
    await page.goto('/');
    await page.getByRole('button', { name: 'Add files', exact: true }).first().click();
    await importFile(page, alternate);
    await importFile(page, second);
    await importFile(page, dubbed);
    await choosePrimary(page, tab);
    const form = workspace(page, tab);
    const tracks = form.locator('details.external-tracks');
    await tracks.locator('summary').click();
    await tracks.getByLabel('Add alternate.mka audio stream #3').check();
    await tracks.getByLabel('Add second.mka audio stream #3').check();
    const first = tracks.getByRole('group', { name: 'alternate.mka' });
    await first.getByLabel('Audio codec').selectOption('aac');
    await first.getByLabel('Audio bitrate').fill('192');
    await first.getByLabel('Audio channels').selectOption('mono');
    const loudness = first.getByRole('group', { name: 'Loudness and gain for stream #3' });
    await loudness.getByRole('button', { name: 'Measure loudness', exact: false }).click();
    await loudness.getByRole('button', { name: 'Measure audio track', exact: true }).click();
    await expect(loudness).toContainText('Suggested gain: -2.9 dB');
    expect((await calls(page, 'measure_loudness'))[0].payload).toMatchObject({
      request: { inputPath: alternate.path, streamIndex: 3, channels: 'mono' },
    });
    await loudness.getByRole('button', { name: 'Apply measured gain', exact: true }).click();
    await first.getByLabel('Timing offset in seconds for alternate.mka stream #3').fill('1.234');
    await tracks.getByLabel('Timing offset in seconds for second.mka stream #3').fill('-0.5');
    await form.getByLabel('Video encoder', { exact: true }).selectOption('svtAv1');
    await expect(tracks.getByLabel('Add alternate.mka audio stream #3')).not.toBeChecked();
    await form.getByLabel('Video encoder', { exact: true }).selectOption('svtAv1Hdr');
    await expect(tracks.getByLabel('Add alternate.mka audio stream #3')).toBeChecked();
    await expect(first.getByLabel('Audio codec')).toHaveValue('aac');
    await expect(first.getByLabel('Audio bitrate')).toHaveValue('192');
    await expect(first.getByLabel('Audio channels')).toHaveValue('mono');
    await expect(loudness.getByLabel('Audio gain (dB)')).toHaveValue('-2.9');
    await expect(
      first.getByLabel('Timing offset in seconds for alternate.mka stream #3'),
    ).toHaveValue('1.234');
    await form.getByLabel('Source for this encode', { exact: true }).selectOption(dubbed.id);
    await form.getByLabel('Source for this encode', { exact: true }).selectOption(primary.id);
    await expect(first.getByLabel('Audio codec')).toHaveValue('aac');
    await expect(loudness.getByLabel('Audio gain (dB)')).toHaveValue('-2.9');
    await first
      .getByLabel('Timing offset in seconds for alternate.mka stream #3')
      .fill('86400.001');
    await expect(form.getByRole('button', { name: 'Add to queue', exact: true })).toBeDisabled();
    await expect(form).toContainText('Enter a timing offset from');
    await first.getByLabel('Timing offset in seconds for alternate.mka stream #3').fill('1.234');
    if (process.env.JESSES_UI_EVIDENCE) {
      const evidence = process.env.JESSES_UI_EVIDENCE;
      await mkdir(evidence, { recursive: true });
      for (const [width, height] of [
        [1280, 850],
        [1000, 800],
      ] as const) {
        await page.setViewportSize({ width, height });
        await tracks.scrollIntoViewIfNeeded();
        await tracks.locator('.external-content').evaluate((element) => {
          element.scrollTop = 0;
        });
        const name = `${tab === 'av1an' ? 'av1an' : 'quick-convert'}-converted-${width}x${height}`;
        await page.screenshot({ path: join(evidence, `${name}.png`) });
        await page.screenshot({ path: join(evidence, `${name}-full.png`), fullPage: true });
        await tracks.locator('.external-content').evaluate((element) => {
          element.scrollTop = element.scrollHeight;
        });
        await page.screenshot({ path: join(evidence, `${name}-bottom.png`) });
      }
      await page.setViewportSize({ width: 1280, height: 850 });
    }
    await form.getByRole('button', { name: 'Preview command plan', exact: true }).click();
    await expect.poll(() => calls(page, 'preview_encode_plan')).toHaveLength(1);
    const preview = (await calls(page, 'preview_encode_plan'))[0].payload.request as EncodeRequest;
    expect(preview.source.streamIndices).toEqual([0, 3, 7, 9]);
    expect(preview.settings.audio).toContainEqual({
      streamIndex: 3,
      codec: 'copy',
      bitrateKbps: 128,
      channels: 'preserve',
    });
    expect(preview.settings.externalTracks).toEqual([
      {
        inputPath: alternate.path,
        streamIndex: 3,
        offsetMilliseconds: 1234,
        audio: {
          codec: 'aac',
          bitrateKbps: 192,
          channels: 'mono',
          gain: { tenthsDb: -29, sourceFingerprint: 'b'.repeat(64) },
        },
      },
      { inputPath: second.path, streamIndex: 3, offsetMilliseconds: -500 },
    ]);
    await form.getByRole('button', { name: 'Add to queue', exact: true }).click();
    const queued = (await calls(page, 'enqueue_encode'))[0].payload.request as EncodeRequest;
    expect(queued.settings.externalTracks).toEqual(preview.settings.externalTracks);
    await loudness.getByLabel('Audio gain (dB)').fill('-3.5');
    expect(queued.settings.externalTracks?.[0].audio?.gain).toEqual({
      tenthsDb: -29,
      sourceFingerprint: 'b'.repeat(64),
    });
    await first.getByLabel('Audio codec').selectOption('opus');
    expect(queued.settings.externalTracks?.[0].audio?.codec).toBe('aac');
    await first.getByLabel('Audio codec').selectOption('copy');
    await form.getByRole('button', { name: 'Add to queue', exact: true }).click();
    const reset = (await calls(page, 'enqueue_encode'))[1].payload.request as EncodeRequest;
    expect(reset.settings.externalTracks?.[0]).toEqual({
      inputPath: alternate.path,
      streamIndex: 3,
      offsetMilliseconds: 1234,
    });
  });

  test(`${tab} blocks incompatible external audio and invalid bitrate`, async ({ page }) => {
    await mockDesktop(page);
    await page.goto('/');
    await page.getByRole('button', { name: 'Add files', exact: true }).first().click();
    await importFile(page, surround);
    await choosePrimary(page, tab);
    const form = workspace(page, tab);
    const tracks = form.locator('details.external-tracks');
    await tracks.locator('summary').click();
    await tracks.getByLabel('Add surround.mka audio stream #3').check();
    await tracks.getByLabel('Audio codec').selectOption('mp3');
    await expect(form.getByRole('button', { name: 'Add to queue', exact: true })).toBeDisabled();
    await expect(tracks).toContainText('MP3 supports mono or stereo');
    await tracks.getByLabel('Audio channels').selectOption('stereo');
    await expect(form.getByRole('button', { name: 'Add to queue', exact: true })).toBeEnabled();
    await tracks.getByLabel('Audio codec').selectOption('eac3');
    await tracks.getByLabel('Audio channels').selectOption('surround71');
    await expect(form.getByRole('button', { name: 'Add to queue', exact: true })).toBeDisabled();
    await expect(tracks).toContainText('E-AC-3 conversion supports up to 5.1 channels');
    await tracks.getByLabel('Audio codec').selectOption('aac');
    await tracks.getByLabel('Audio bitrate').fill('9999');
    await expect(form.getByRole('button', { name: 'Add to queue', exact: true })).toBeDisabled();
    await tracks.getByLabel('Audio bitrate').fill('192');
    await expect(form.getByRole('button', { name: 'Add to queue', exact: true })).toBeEnabled();
    await tracks.getByLabel('Audio gain (dB)').fill('-3.5');
    await form.getByRole('button', { name: 'Add to queue', exact: true }).click();
    const request = (await calls(page, 'enqueue_encode'))[0].payload.request as EncodeRequest;
    expect(request.settings.externalTracks?.[0].audio?.gain).toEqual({ tenthsDb: -35 });
  });

  test(`${tab} keeps subtitle offsets and enforces one burn across sources`, async ({ page }) => {
    await mockDesktop(page);
    await page.goto('/');
    await page.getByRole('button', { name: 'Add files', exact: true }).first().click();
    await importFile(page, captions);
    await importFile(page, dubbed);
    await choosePrimary(page, tab);
    const form = workspace(page, tab);
    const tracks = form.locator('details.external-tracks');
    await tracks.locator('summary').click();
    await tracks.getByLabel('Add captions.mks subtitle stream #4').check();
    const caption = tracks.getByRole('group', { name: 'captions.mks' });
    const action = caption.getByLabel('Subtitle action');
    await caption.getByLabel('Timing offset in seconds for captions.mks stream #4').fill('-0.125');
    if (tab === 'av1an') {
      await expect(action.locator('option[value="webVtt"]')).toBeDisabled();
      await expect(action.locator('option[value="burnIn"]')).toBeDisabled();
      if (process.env.JESSES_UI_EVIDENCE) {
        const evidence = process.env.JESSES_UI_EVIDENCE;
        await mkdir(evidence, { recursive: true });
        for (const [width, height] of [
          [1280, 850],
          [1000, 800],
        ] as const) {
          await page.setViewportSize({ width, height });
          await tracks.scrollIntoViewIfNeeded();
          await page.screenshot({ path: join(evidence, `av1an-subtitles-${width}x${height}.png`) });
        }
      }
      await form.getByRole('button', { name: 'Add to queue', exact: true }).click();
      const request = (await calls(page, 'enqueue_encode'))[0].payload.request as EncodeRequest;
      expect(request.settings.externalTracks).toEqual([
        { inputPath: captions.path, streamIndex: 4, offsetMilliseconds: -125 },
      ]);
      return;
    }
    await action.selectOption('webVtt');
    await form.getByLabel('Video encoder', { exact: true }).selectOption('svtAv1');
    await form.getByLabel('Video encoder', { exact: true }).selectOption('svtAv1Hdr');
    await expect(action).toHaveValue('webVtt');
    await form.getByLabel('Source for this encode', { exact: true }).selectOption(dubbed.id);
    await form.getByLabel('Source for this encode', { exact: true }).selectOption(primary.id);
    await expect(action).toHaveValue('webVtt');
    if (process.env.JESSES_UI_EVIDENCE) {
      const evidence = process.env.JESSES_UI_EVIDENCE;
      await mkdir(evidence, { recursive: true });
      for (const [width, height] of [
        [1280, 850],
        [1000, 800],
      ] as const) {
        await page.setViewportSize({ width, height });
        await tracks.scrollIntoViewIfNeeded();
        await page.screenshot({
          path: join(evidence, `quick-convert-subtitles-${width}x${height}.png`),
        });
      }
    }
    await form.getByRole('button', { name: 'Preview command plan', exact: true }).click();
    await expect.poll(() => calls(page, 'preview_encode_plan')).toHaveLength(1);
    const preview = (await calls(page, 'preview_encode_plan'))[0].payload.request as EncodeRequest;
    expect(preview.settings.externalTracks).toEqual([
      {
        inputPath: captions.path,
        streamIndex: 4,
        offsetMilliseconds: -125,
        subtitleMode: 'webVtt',
      },
    ]);
    await action.selectOption('burnIn');
    const primaryAction = form
      .getByRole('group', { name: 'Subtitle settings for stream #7' })
      .getByLabel('Subtitle action');
    await primaryAction.selectOption('burnIn');
    await expect(form.getByRole('button', { name: 'Add to queue', exact: true })).toBeDisabled();
    await expect(form).toContainText(
      'Choose at most one subtitle track to burn into the video across all source files',
    );
    await primaryAction.selectOption('copy');
    await expect(form.getByRole('button', { name: 'Add to queue', exact: true })).toBeEnabled();
    await form.getByRole('button', { name: 'Add to queue', exact: true }).click();
    const queued = (await calls(page, 'enqueue_encode'))[0].payload.request as EncodeRequest;
    expect(queued.settings.externalTracks?.[0]).toMatchObject({
      subtitleMode: 'burnIn',
      offsetMilliseconds: -125,
    });
    await action.selectOption('copy');
    await form.getByRole('button', { name: 'Add to queue', exact: true }).click();
    const reset = (await calls(page, 'enqueue_encode'))[1].payload.request as EncodeRequest;
    expect(reset.settings.externalTracks).toEqual([
      { inputPath: captions.path, streamIndex: 4, offsetMilliseconds: -125 },
    ]);
  });

  test(`${tab} permits trimmed converted audio and text subtitles`, async ({ page }) => {
    await mockDesktop(page);
    await page.goto('/');
    await page.getByRole('button', { name: 'Add files', exact: true }).first().click();
    await importFile(page, dubbed);
    await importFile(page, captions);
    await importFile(page, movText);
    await importFile(page, bitmap);
    await choosePrimary(page, tab);
    const form = workspace(page, tab);
    const tracks = form.locator('details.external-tracks');
    await tracks.locator('summary').click();
    await tracks.getByLabel('Add dub.mka audio stream #2').check();
    await tracks.getByLabel('Add captions.mks subtitle stream #4').check();
    await tracks.getByLabel('Add movtext.mp4 subtitle stream #5').check();
    await tracks.getByLabel('Add bitmap.mks subtitle stream #4').check();
    await form
      .getByRole('group', { name: 'Audio settings for stream #3' })
      .getByLabel('Audio codec')
      .selectOption('flac');
    await form.getByLabel('Trim video interval', { exact: true }).check();
    await form.getByLabel('End frame (excluded)', { exact: true }).fill('120');
    await expect(form.getByRole('button', { name: 'Add to queue', exact: true })).toBeDisabled();
    await expect(form).toContainText(
      'Choose audio conversion for every external audio track when trimming',
    );
    await tracks
      .getByRole('group', { name: 'dub.mka' })
      .getByLabel('Audio codec')
      .selectOption('aac');
    await expect(form.getByRole('button', { name: 'Add to queue', exact: true })).toBeDisabled();
    await expect(form).toContainText(
      'Trimming supports text subtitles and burned bitmap subtitles',
    );
    const bitmapAction = tracks
      .getByRole('group', { name: 'bitmap.mks' })
      .getByLabel('Subtitle action');
    if (tab === 'Quick Convert') {
      await bitmapAction.selectOption('burnIn');
    } else {
      await expect(bitmapAction.locator('option[value="burnIn"]')).toBeDisabled();
      await tracks.getByLabel('Add bitmap.mks subtitle stream #4').uncheck();
    }
    await expect(form.getByRole('button', { name: 'Add to queue', exact: true })).toBeEnabled();
    await form.getByRole('button', { name: 'Add to queue', exact: true }).click();
    const request = (await calls(page, 'enqueue_encode'))[0].payload.request as EncodeRequest;
    expect(request.settings.trim).toMatchObject({ startFrame: 0, endFrameExclusive: 120 });
    expect(request.settings.externalTracks).toContainEqual({
      inputPath: dubbed.path,
      streamIndex: 2,
      audio: { codec: 'aac', bitrateKbps: 128, channels: 'preserve' },
    });
    expect(request.settings.externalTracks).toContainEqual({
      inputPath: movText.path,
      streamIndex: 5,
    });
    if (tab === 'Quick Convert')
      expect(request.settings.externalTracks).toContainEqual({
        inputPath: bitmap.path,
        streamIndex: 4,
        subtitleMode: 'burnIn',
      });
  });

  test(`${tab} copies ordered external tracks in preview and immutable encode request`, async ({
    page,
  }) => {
    await mockDesktop(page);
    await page.goto('/');
    await page.getByRole('button', { name: 'Add files', exact: true }).first().click();
    await importFile(page, dubbed);
    await importFile(page, captions);
    await choosePrimary(page, tab);
    const form = workspace(page, tab);
    const tracks = form.locator('details.external-tracks');
    await tracks.locator('summary').click();
    await expect(tracks).toContainText('dub.mka');
    await expect(tracks).toContainText('flac');
    await expect(tracks).toContainText('Signs');
    await expect(tracks.getByLabel('Add main.mkv audio stream #3')).toHaveCount(0);
    await expect(tracks.getByLabel('Add dub.mka video stream #6')).toHaveCount(0);
    await tracks.getByLabel('Add dub.mka audio stream #2').check();
    await tracks.getByLabel('Add captions.mks subtitle stream #4').check();
    await tracks.getByLabel('Add dub.mka attachment stream #5').check();
    if (process.env.JESSES_UI_EVIDENCE) {
      const evidence = process.env.JESSES_UI_EVIDENCE;
      await mkdir(evidence, { recursive: true });
      for (const [width, height] of [
        [1280, 850],
        [1000, 800],
      ] as const) {
        await page.setViewportSize({ width, height });
        await tracks.scrollIntoViewIfNeeded();
        const name = `${tab === 'av1an' ? 'av1an' : 'quick-convert'}-${width}x${height}`;
        await page.screenshot({ path: join(evidence, `${name}.png`) });
        await page.screenshot({ path: join(evidence, `${name}-full.png`), fullPage: true });
      }
      await page.setViewportSize({ width: 1280, height: 850 });
    }
    await form.getByRole('button', { name: 'Preview command plan', exact: true }).click();
    await expect.poll(() => calls(page, 'preview_encode_plan')).toHaveLength(1);
    const preview = (await calls(page, 'preview_encode_plan'))[0].payload.request as EncodeRequest;
    expect(preview.source).toMatchObject({ inputPath: primary.path, streamIndices: [0, 3, 7, 9] });
    expect(preview.settings.externalTracks).toEqual([
      { inputPath: dubbed.path, streamIndex: 2 },
      { inputPath: captions.path, streamIndex: 4 },
      { inputPath: dubbed.path, streamIndex: 5 },
    ]);
    await form.getByRole('button', { name: 'Add to queue', exact: true }).click();
    const queued = (await calls(page, 'enqueue_encode'))[0].payload.request as EncodeRequest;
    expect(queued.settings.externalTracks).toEqual(preview.settings.externalTracks);
    await tracks.getByLabel('Add dub.mka audio stream #2').uncheck();
    expect(queued.settings.externalTracks).toHaveLength(3);
  });

  test(`${tab} keeps source drafts and blocks trim or removed external tracks`, async ({
    page,
  }) => {
    await mockDesktop(page);
    await page.goto('/');
    await page.getByRole('button', { name: 'Add files', exact: true }).first().click();
    await importFile(page, dubbed);
    await choosePrimary(page, tab);
    const form = workspace(page, tab);
    const tracks = form.locator('details.external-tracks');
    await tracks.locator('summary').click();
    await tracks.getByLabel('Add dub.mka audio stream #2').check();
    await form.getByLabel('Video encoder', { exact: true }).selectOption('svtAv1');
    await expect(tracks.getByLabel('Add dub.mka audio stream #2')).not.toBeChecked();
    await form.getByLabel('Video encoder', { exact: true }).selectOption('svtAv1Hdr');
    await expect(tracks.getByLabel('Add dub.mka audio stream #2')).toBeChecked();
    await form.getByLabel('Source for this encode', { exact: true }).selectOption(dubbed.id);
    await expect(tracks.getByLabel('Add dub.mka audio stream #2')).toHaveCount(0);
    await form.getByLabel('Source for this encode', { exact: true }).selectOption(primary.id);
    await expect(tracks.getByLabel('Add dub.mka audio stream #2')).toBeChecked();
    await form.getByLabel('Trim video interval', { exact: true }).check();
    await expect(form.getByRole('button', { name: 'Add to queue', exact: true })).toBeDisabled();
    await expect(form).toContainText(
      'Choose audio conversion for every external audio track when trimming',
    );
    await form.getByLabel('Trim video interval', { exact: true }).uncheck();
    await expect(form.getByRole('button', { name: 'Add to queue', exact: true })).toBeEnabled();
    await tracks.getByLabel('Audio codec').selectOption('aac');
    await page
      .getByRole('navigation', { name: 'Workspace' })
      .getByRole('button', { name: /^Files/ })
      .click();
    await page.getByRole('button', { name: `Remove ${dubbed.name}`, exact: true }).click();
    await page.getByRole('button', { name: tab, exact: true }).click();
    await showAllEncodeSettings(page);
    await expect(form.getByRole('button', { name: 'Add to queue', exact: true })).toBeDisabled();
    await expect(form).toContainText('A selected track source was removed');
    await expect(tracks).toContainText('dub.mka · stream #2 needs to be selected again.');
    await importFile(page, {
      ...dubbed,
      streams: [stream(2, 'audio', 'opus', 'Updated dub'), ...dubbed.streams.slice(1)],
    });
    await page.getByRole('button', { name: tab, exact: true }).click();
    await showAllEncodeSettings(page);
    await expect(form.getByRole('button', { name: 'Add to queue', exact: true })).toBeDisabled();
    await expect(tracks).toContainText('Updated dub');
    await tracks
      .getByRole('button', { name: `Remove unavailable track ${dubbed.path} stream #2` })
      .click();
    await expect(form.getByRole('button', { name: 'Add to queue', exact: true })).toBeEnabled();
    await form.getByRole('button', { name: 'Add to queue', exact: true }).click();
    const request = (await calls(page, 'enqueue_encode'))[0].payload.request as EncodeRequest;
    expect(request.settings).not.toHaveProperty('externalTracks');
  });
}

test('Quick Convert keeps QTGMC and external subtitle burn in one request', async ({ page }) => {
  await mockDesktop(page);
  await page.goto('/');
  await page.getByRole('button', { name: 'Add files', exact: true }).first().click();
  await importFile(page, captions);
  await choosePrimary(page, 'Quick Convert');
  const form = workspace(page, 'Quick Convert');
  const tracks = form.locator('details.external-tracks');
  await tracks.locator('summary').click();
  await tracks.getByLabel('Add captions.mks subtitle stream #4').check();
  await tracks
    .getByRole('group', { name: 'captions.mks' })
    .getByLabel('Subtitle action')
    .selectOption('burnIn');
  await form.locator('details.temporal-options summary').click();
  await form.getByLabel('Source reconstruction').selectOption('qtgmcFrame');
  await form.getByLabel('QTGMC preset').selectOption('medium');
  await expect(form.getByRole('button', { name: 'Add to queue', exact: true })).toBeEnabled();
  await form.getByRole('button', { name: 'Add to queue', exact: true }).click();
  const request = (await calls(page, 'enqueue_encode'))[0].payload.request as EncodeRequest;
  expect(request.settings.temporal?.qtgmc).toMatchObject({ mode: 'frame', preset: 'medium' });
  expect(request.settings.externalTracks).toEqual([
    { inputPath: captions.path, streamIndex: 4, subtitleMode: 'burnIn' },
  ]);
});

test('primary MOV timecode omits its input path and is absent from media order', async ({
  page,
}) => {
  await mockDesktop(page, primaryMov);
  await page.goto('/');
  await page.getByRole('button', { name: 'Add files', exact: true }).first().click();
  await choosePrimary(page, 'Quick Convert');
  const form = workspace(page, 'Quick Convert');
  await form.getByLabel('Output container').selectOption('mov');
  const layout = form.locator('details.track-layout');
  await layout.locator('summary').first().click();
  await layout.getByLabel('MOV timecode track').selectOption({ label: 'main.mov · stream #6' });
  await form.getByRole('button', { name: 'Add to queue', exact: true }).click();
  const request = (await calls(page, 'enqueue_encode'))[0].payload.request as EncodeRequest;
  expect(request.settings.movTimecodeTrack).toEqual({ streamIndex: 6 });
  expect(request.source.streamIndices).not.toContain(6);
  expect(request.settings).not.toHaveProperty('trackOrder');
});

test('Quick Convert preserves Auto/GPU HDR routes and manual CPU tone mapping', async ({
  page,
}) => {
  await mockDesktop(page, hdrPrimary);
  await page.goto('/');
  await page.getByRole('button', { name: 'Add files', exact: true }).first().click();
  await choosePrimary(page, 'Quick Convert');
  const form = workspace(page, 'Quick Convert');
  await form.getByLabel('Video encoder', { exact: true }).selectOption('x265');
  await form.getByLabel('HDR / HLG to SDR', { exact: true }).check();
  await expect(form.getByLabel('Processing route')).toHaveValue('auto');
  await expect(form.getByLabel('Signal peak mode', { exact: true })).toHaveValue('measured');
  await form.getByLabel('Tone mapping curve').selectOption('spline');
  if (process.env.JESSES_UI_EVIDENCE) {
    const evidence = process.env.JESSES_UI_EVIDENCE;
    await mkdir(evidence, { recursive: true });
    for (const [width, height] of [
      [1280, 850],
      [1000, 800],
    ] as const) {
      await page.setViewportSize({ width, height });
      await form.getByLabel('Processing route').scrollIntoViewIfNeeded();
      await page.screenshot({ path: join(evidence, `quick-convert-hdr-${width}x${height}.png`) });
    }
  }
  await form.getByRole('button', { name: 'Add to queue', exact: true }).click();
  const auto = (await calls(page, 'enqueue_encode'))[0].payload.request as EncodeRequest;
  expect(auto.settings.toneMap).toEqual({
    algorithm: 'spline',
    backend: 'auto',
    peakMode: 'measured',
    sourcePeakNits: 1000,
    hdr10BaseLayer: false,
  });
  await form.getByLabel('Processing route').selectOption('cpu');
  await expect(form.getByLabel('Tone mapping curve')).toHaveValue('hable');
  await expect(
    form.getByLabel('Tone mapping curve').locator('option[value="spline"]'),
  ).toHaveAttribute('disabled', '');
  await form.getByLabel('Tone mapping curve').selectOption('reinhard');
  await form.getByLabel('Signal peak mode', { exact: true }).selectOption('manual');
  await form.getByLabel('Signal peak (nits)').fill('1500');
  await form.getByRole('button', { name: 'Add to queue', exact: true }).click();
  const cpu = (await calls(page, 'enqueue_encode'))[1].payload.request as EncodeRequest;
  expect(cpu.settings.toneMap).toEqual({
    algorithm: 'reinhard',
    sourcePeakNits: 1500,
    hdr10BaseLayer: false,
  });
  await form.getByLabel('Processing route').selectOption('gpu');
  await expect(form.getByLabel('Signal peak mode', { exact: true })).toHaveValue('measured');
  await form.getByLabel('Tone mapping curve').selectOption('spline');
  await form.getByRole('button', { name: 'Add to queue', exact: true }).click();
  const gpu = (await calls(page, 'enqueue_encode'))[2].payload.request as EncodeRequest;
  expect(gpu.settings.toneMap).toMatchObject({
    algorithm: 'spline',
    backend: 'gpu',
    peakMode: 'measured',
  });
  expect(auto.settings.toneMap?.sourcePeakNits).toBe(1000);
});

test('av1an HDR rendering keeps CPU-compatible routes and curves', async ({ page }) => {
  await mockDesktop(page, hdrPrimary);
  await page.goto('/');
  await page.getByRole('button', { name: 'Add files', exact: true }).first().click();
  await choosePrimary(page, 'av1an');
  const form = workspace(page, 'av1an');
  await form.getByLabel('Video encoder', { exact: true }).selectOption('x264');
  await form.getByLabel('HDR / HLG to SDR', { exact: true }).check();
  await expect(form.getByLabel('Processing route').locator('option[value="gpu"]')).toHaveAttribute(
    'disabled',
    '',
  );
  await expect(
    form.getByLabel('Tone mapping curve').locator('option[value="spline"]'),
  ).toHaveAttribute('disabled', '');
  await form.getByLabel('Tone mapping curve').selectOption('reinhard');
  await form.getByRole('button', { name: 'Add to queue', exact: true }).click();
  const request = (await calls(page, 'enqueue_encode'))[0].payload.request as EncodeRequest;
  expect(request.settings.toneMap).toMatchObject({
    algorithm: 'reinhard',
    backend: 'auto',
    peakMode: 'measured',
  });
});

test('Dolby Vision profile 5 requires standalone Auto/GPU rendering', async ({ page }) => {
  await mockDesktop(page, dv5Primary);
  await page.goto('/');
  await page.getByRole('button', { name: 'Add files', exact: true }).first().click();
  await choosePrimary(page, 'Quick Convert');
  const quick = workspace(page, 'Quick Convert');
  await quick.getByLabel('Video encoder', { exact: true }).selectOption('x265');
  await quick.getByLabel('HDR / HLG to SDR', { exact: true }).check();
  await expect(quick.getByRole('button', { name: 'Add to queue', exact: true })).toBeEnabled();
  await quick.getByLabel('Signal peak mode', { exact: true }).selectOption('manual');
  await expect(quick.getByRole('button', { name: 'Add to queue', exact: true })).toBeDisabled();
  await expect(quick).toContainText('Dolby Vision profile 5 requires measured peak detection');
  await quick.getByLabel('Signal peak mode', { exact: true }).selectOption('measured');
  await quick.getByLabel('Processing route').selectOption('cpu');
  await expect(quick.getByRole('button', { name: 'Add to queue', exact: true })).toBeDisabled();
  await expect(quick).toContainText('Dolby Vision profile 5 requires standalone Auto or GPU');
  await quick.getByLabel('Processing route').selectOption('gpu');
  await expect(quick.getByRole('button', { name: 'Add to queue', exact: true })).toBeEnabled();
  await page.getByRole('button', { name: 'av1an', exact: true }).click();
  await showAllEncodeSettings(page);
  const av1an = workspace(page, 'av1an');
  await av1an.getByLabel('Video encoder', { exact: true }).selectOption('x264');
  await av1an.getByLabel('HDR / HLG to SDR', { exact: true }).check();
  await expect(av1an.getByRole('button', { name: 'Add to queue', exact: true })).toBeDisabled();
  await expect(av1an).toContainText('Dolby Vision profile 5 requires standalone Auto or GPU');
});

test('Dolby Vision profile 5 rejects a reported limited-range input', async ({ page }) => {
  await mockDesktop(page, {
    ...dv5Primary,
    streams: dv5Primary.streams.map((entry) =>
      entry.kind === 'video' ? { ...entry, colorRange: 'tv' } : entry,
    ),
  });
  await page.goto('/');
  await page.getByRole('button', { name: 'Add files', exact: true }).first().click();
  await choosePrimary(page, 'Quick Convert');
  const form = workspace(page, 'Quick Convert');
  await form.getByLabel('Video encoder', { exact: true }).selectOption('x265');
  await form.getByLabel('HDR / HLG to SDR', { exact: true }).check();
  await expect(form.getByRole('button', { name: 'Add to queue', exact: true })).toBeDisabled();
  await expect(form).toContainText('Dolby Vision profile 5 rendering requires full-range input');
});
