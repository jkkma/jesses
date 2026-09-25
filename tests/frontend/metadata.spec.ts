import { expect, test, type Page } from '@playwright/test';
import type { MediaFile, UserPreferences } from '../../src/lib/ipc/generated';

type MetadataMock = {
  pickFiles: (paths: string[]) => void;
};

async function desktopMock(page: Page, files: [string, MediaFile][], pickerPaths: string[]) {
  await page.addInitScript(
    ({ files, pickerPaths }) => {
      const host = globalThis as unknown as Record<string, unknown>;
      const knownFiles = new Map(files);
      let selectedPaths = pickerPaths;
      let nextCallback = 0;
      let nextEvent = 0;
      const callbacks = new Map<number, (value: unknown) => void>();
      let prefs: UserPreferences = {
        general: { defaultOutputDirectory: '', recursiveImport: false },
        recentPaths: [],
        revision: 0,
      };
      host.__metadataMock = {
        pickFiles: (paths) => (selectedPaths = paths),
      } satisfies MetadataMock;
      host.isTauri = true;
      host.__TAURI_INTERNALS__ = {
        metadata: { currentWindow: { label: 'main' }, currentWebview: { label: 'main' } },
        transformCallback: (callback: (value: unknown) => void) => {
          const id = ++nextCallback;
          callbacks.set(id, callback);
          return id;
        },
        unregisterCallback: (id: number) => callbacks.delete(id),
        invoke: async (command: string, payload: Record<string, unknown> = {}) => {
          if (command === 'plugin:dialog|open') return selectedPaths;
          if (command === 'plugin:event|listen') {
            callbacks.set(payload.handler as number, () => undefined);
            return ++nextEvent;
          }
          if (command === 'plugin:event|unlisten') return;
          if (command === 'get_capabilities') return [];
          if (command === 'subscribe_jobs') {
            (payload.channel as { onmessage: (value: unknown[]) => void }).onmessage([]);
            return;
          }
          if (command === 'get_preferences') return structuredClone(prefs);
          if (command === 'remember_recent_media') {
            prefs = {
              ...prefs,
              recentPaths: [...(payload.paths as string[]), ...prefs.recentPaths].slice(0, 15),
              revision: prefs.revision + 1,
            };
            return structuredClone(prefs);
          }
          if (command === 'probe_media') {
            const value = knownFiles.get(payload.path as string);
            if (value) return value;
            throw {
              code: 'FILE_NOT_FOUND',
              message: 'The selected file is unavailable.',
              path: payload.path,
            };
          }
          throw new Error(`Unexpected command: ${command}`);
        },
      };
    },
    { files, pickerPaths },
  );
}

function media(path: string, streams: Record<string, unknown>[], metadata = {}): MediaFile {
  return {
    id: path,
    path,
    name: path.split(/[\\/]/).at(-1)!,
    sizeBytes: '9007199254740993',
    durationSeconds: 123.45,
    format: 'matroska,webm',
    streams,
    ...metadata,
  } as unknown as MediaFile;
}

async function importFile(page: Page) {
  await page.getByRole('button', { name: 'Add files', exact: true }).first().click();
}

test('inspector preserves exact stream and container metadata, including absent and unknown values', async ({
  page,
}) => {
  const path = 'C:\\media\\metadata-sample.mkv';
  const fixture = media(
    path,
    [
      {
        index: 0,
        kind: 'video',
        codec: 'h264',
        codecLongName: 'H.264 / AVC / MPEG-4 AVC / MPEG-4 part 10',
        profile: 'High',
        bitRate: '9007199254740995',
        durationSeconds: 123.4,
        width: 720,
        height: 480,
        sampleAspectRatio: '8:9',
        displayAspectRatio: '4:3',
        rotationDegrees: '90',
        frameRate: '24000/1001',
        averageFrameRate: '24000/1001',
        nominalFrameRate: '30000/1001',
        fieldOrder: 'progressive',
        pixelFormat: 'yuv420p10le',
        bitDepth: 10,
        colorPrimaries: 'bt2020',
        colorTransfer: 'smpte2084',
        colorSpace: 'bt2020nc',
        colorRange: 'tv',
        hdrFormat: 'HDR / PQ',
        hasHdrStaticMetadata: true,
        dynamicHdrFormats: ['HDR10+'],
        isDefault: true,
        sampleRate: null,
        channels: null,
        channelLayout: null,
        language: 'eng',
        title: 'Main feature',
      },
      {
        index: 2,
        kind: 'audio',
        codec: 'dts',
        codecLongName: 'DCA (DTS Coherent Acoustics)',
        profile: 'DTS-HD MA',
        bitRate: '1500000',
        sampleRate: 48000,
        channels: 6,
        channelLayout: '5.1(side)',
        language: 'eng',
        title: 'Commentary',
        isDefault: false,
        width: null,
        height: null,
        frameRate: null,
        sampleAspectRatio: null,
        displayAspectRatio: null,
        rotationDegrees: undefined,
        fieldOrder: undefined,
        pixelFormat: undefined,
        bitDepth: undefined,
        colorPrimaries: undefined,
        colorTransfer: undefined,
        colorSpace: undefined,
        colorRange: undefined,
        hdrFormat: undefined,
        hasHdrStaticMetadata: undefined,
        dynamicHdrFormats: undefined,
      },
      {
        index: 4,
        kind: 'subtitle',
        codec: 'hdmv_pgs_subtitle',
        codecLongName: 'HDMV Presentation Graphic Stream subtitles',
        language: 'jpn',
        title: 'Japanese',
        isDefault: false,
      },
      {
        index: 5,
        kind: 'subtitle',
        codec: 'subrip',
        codecLongName: 'SubRip subtitle',
        language: 'eng',
        title: 'English',
      },
      {
        index: 6,
        kind: 'subtitle',
        codec: 'vendor_subtitle',
      },
      {
        index: 7,
        kind: 'attachment',
        codec: 'ttf',
        codecLongName: 'TrueType Font',
        attachmentFilename: 'captions-font.ttf',
        attachmentMimeType: 'application/x-truetype-font',
      },
      { index: 9, kind: 'data', codec: 'scte_35', codecLongName: 'SCTE 35 section' },
    ],
    {
      title: 'Evening feature',
      language: 'eng',
      bitRate: '18014398509481987',
    },
  );
  await desktopMock(page, [[path, fixture]], [path]);
  await page.goto('/');
  await importFile(page);
  await expect(
    page.getByRole('heading', { name: 'metadata-sample.mkv', exact: true }),
  ).toBeVisible();

  const fileDisclosure = page
    .locator('details')
    .filter({ has: page.getByText('More file metadata', { exact: true }) });
  await expect(fileDisclosure).not.toHaveAttribute('open', '');
  await expect(page.getByText('File size (exact bytes)', { exact: true })).toBeHidden();
  await fileDisclosure.locator('summary').click();
  const fileMetadata = page.locator('details').filter({
    has: page.getByText('File size (exact bytes)', { exact: true }),
  });
  await expect(fileMetadata.getByText('Evening feature', { exact: true })).toBeVisible();
  await expect(fileMetadata.getByText('123.45 s', { exact: true })).toBeVisible();
  await expect(fileMetadata.getByText('18014398509481987 bit/s', { exact: true })).toBeVisible();
  await expect(fileMetadata.getByText('9007199254740993', { exact: true })).toBeVisible();

  const video = page.locator('.stream-card').filter({ hasText: '#0' });
  await expect(video.locator('details')).not.toHaveAttribute('open', '');
  await video.getByText('More stream metadata', { exact: true }).click();
  const averageRate = video.getByText('24000/1001', { exact: true });
  await expect(averageRate).toHaveCount(2);
  await expect(averageRate.first()).toBeVisible();
  await expect(averageRate.nth(1)).toBeVisible();
  await expect(video.getByText('30000/1001', { exact: true })).toBeVisible();
  await expect(video.getByText('8:9', { exact: true })).toBeVisible();
  await expect(video.getByText('90°', { exact: true })).toBeVisible();
  await expect(video.getByText('123.4 s', { exact: true })).toBeVisible();
  await expect(video.getByText('9007199254740995 bit/s', { exact: true })).toBeVisible();
  await expect(video.getByText('Yes', { exact: true })).toBeVisible();

  const audio = page.locator('.stream-card').filter({ hasText: '#2' });
  await audio.getByText('More stream metadata', { exact: true }).click();
  await expect(audio.getByText('dts', { exact: true })).toBeVisible();
  await expect(audio.getByText('DTS-HD MA', { exact: true })).toBeVisible();
  await expect(audio.getByText('5.1(side)', { exact: true })).toBeVisible();
  await expect(audio.getByText('No', { exact: true })).toBeVisible();
  await expect(audio.getByText('Not reported', { exact: true })).toBeVisible();

  const pgs = page.locator('.stream-card').filter({ hasText: '#4' });
  await pgs.getByText('More stream metadata', { exact: true }).click();
  await expect(pgs.getByText('Bitmap', { exact: true })).toBeVisible();
  const text = page.locator('.stream-card').filter({ hasText: '#5' });
  await text.getByText('More stream metadata', { exact: true }).click();
  await expect(text.getByText('Text', { exact: true })).toBeVisible();
  const unknownSubtitle = page.locator('.stream-card').filter({ hasText: '#6' });
  await unknownSubtitle.getByText('More stream metadata', { exact: true }).click();
  await expect(unknownSubtitle.getByText('Unknown', { exact: true })).toBeVisible();

  const attachment = page.locator('.stream-card').filter({ hasText: '#7' });
  await attachment.getByText('More stream metadata', { exact: true }).click();
  await expect(attachment.getByText('captions-font.ttf', { exact: true })).toBeVisible();
  await expect(attachment.getByText('application/x-truetype-font', { exact: true })).toBeVisible();
  await expect(page.locator('.stream-card').filter({ hasText: '#9' })).toContainText('data');
});

test('audio-only sources do not show video thumbnail controls', async ({ page }) => {
  const path = 'C:\\media\\voice-note.m4a';
  const fixture = media(path, [
    {
      index: 0,
      kind: 'audio',
      codec: 'aac',
      codecLongName: 'AAC (Advanced Audio Coding)',
      sampleRate: 44100,
      channels: 2,
      channelLayout: 'stereo',
      language: null,
      title: null,
    },
  ]);
  await desktopMock(page, [[path, fixture]], [path]);
  await page.goto('/');
  await importFile(page);
  await expect(page.getByRole('heading', { name: 'voice-note.m4a', exact: true })).toBeVisible();
  await expect(page.getByRole('button', { name: 'Video thumbnail & scrubbing' })).toHaveCount(0);
});
