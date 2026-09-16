import { expect, test, type Page } from '@playwright/test';
import type { JobSnapshot, MediaFile, MediaStream, MuxRequest } from '../../src/lib/ipc/generated';

function stream(index: number, kind: string, title: string | null): MediaStream {
  return {
    index,
    kind,
    codec:
      kind === 'video'
        ? 'ffv1'
        : kind === 'audio'
          ? 'flac'
          : kind === 'subtitle'
            ? 'subrip'
            : 'ttf',
    width: kind === 'video' ? 320 : null,
    height: kind === 'video' ? 180 : null,
    frameRate: kind === 'video' ? '24/1' : null,
    sampleRate: kind === 'audio' ? 48000 : null,
    channels: kind === 'audio' ? 2 : null,
    language: kind === 'audio' ? 'eng' : null,
    title,
  };
}
const media: MediaFile[] = [
  {
    id: 'picture',
    path: 'C:\\media\\picture.mkv',
    name: 'picture.mkv',
    sizeBytes: '10000',
    durationSeconds: 12,
    format: 'matroska',
    streams: [stream(0, 'video', 'Picture'), stream(8, 'attachment', 'Font')],
  },
  {
    id: 'sound',
    path: "C:\\media\\音声's $.mka",
    name: "音声's $.mka",
    sizeBytes: '2000',
    durationSeconds: 12,
    format: 'matroska',
    streams: [stream(0, 'audio', 'Original sound')],
  },
  {
    id: 'captions',
    path: 'C:\\media\\captions.srt',
    name: 'captions.srt',
    sizeBytes: '300',
    durationSeconds: 12,
    format: 'srt',
    streams: [stream(0, 'subtitle', null)],
  },
];
type Call = { command: string; payload: Record<string, unknown> };
type Mock = { calls: Call[]; fail: (value: boolean) => void };
async function setup(page: Page, options: { terminalReply?: boolean } = {}) {
  await page.addInitScript(
    ({ media, terminalReply }) => {
      const state = globalThis as unknown as Record<string, unknown>;
      let callback = 0;
      const calls: Call[] = [];
      let fail = false;
      let jobs: JobSnapshot[] = [];
      let channel: { onmessage: (jobs: JobSnapshot[]) => void } | undefined;
      state.__muxMock = {
        calls,
        fail: (value) => {
          fail = value;
        },
      } satisfies Mock;
      state.isTauri = true;
      state.__TAURI_INTERNALS__ = {
        metadata: { currentWindow: { label: 'main' }, currentWebview: { label: 'main' } },
        transformCallback: () => ++callback,
        unregisterCallback: () => {},
        invoke: async (command: string, payload: Record<string, unknown> = {}) => {
          calls.push({
            command,
            payload: structuredClone(command === 'subscribe_jobs' ? {} : payload),
          });
          if (command.startsWith('plugin:event|')) return ++callback;
          if (command === 'get_capabilities')
            return ['ffmpeg', 'ffprobe'].map((id) => ({
              id,
              name: id,
              available: true,
              path: `C:\\tools\\${id}.exe`,
              version: 'fixture',
              detail: null,
            }));
          if (command === 'plugin:dialog|open') return media.map((file) => file.path);
          if (command === 'plugin:dialog|save') return "C:\\exports\\combined's $.mkv";
          if (command === 'probe_media') return media.find((file) => file.path === payload.path);
          if (command === 'list_jobs') return jobs;
          if (command === 'subscribe_jobs') {
            channel = payload.channel as typeof channel;
            channel?.onmessage(jobs);
            return;
          }
          if (command === 'start_mux') {
            if (fail)
              throw {
                code: 'OUTPUT_EXISTS',
                message: 'Choose a destination that does not exist.',
                path: null,
              };
            const request = structuredClone(payload.request) as MuxRequest;
            const first = request.tracks[0].sourceId;
            const job: JobSnapshot = {
              id: `mux-${jobs.length + 1}`,
              state: 'succeeded',
              request: {
                inputPath: request.sources.find((source) => source.id === first)!.inputPath,
                outputPath: request.outputPath,
                streamIndices: request.tracks
                  .filter((track) => track.sourceId === first)
                  .map((track) => track.streamIndex),
              },
              muxRequest: request,
              encodeSettings: null,
              progressSeconds: 12,
              durationSeconds: 12,
              logs: ['Verified all packet payloads.'],
              error: null,
              recovery: null,
              logPath: null,
            };
            jobs = [job, ...jobs];
            channel?.onmessage(jobs);
            return terminalReply ? { ...job, state: 'queued' } : job;
          }
          if (command === 'get_completion_status')
            return {
              options: { notify: false, finishAction: 'none' },
              armedJobs: 0,
              secondsRemaining: null,
              error: null,
            };
          throw new Error(`Unexpected command ${command}`);
        },
      };
    },
    { media, terminalReply: options.terminalReply ?? false },
  );
  await page.goto('/');
  await page.getByRole('button', { name: 'Add files', exact: true }).first().click();
  await expect(page.getByRole('heading', { name: media[2].name, exact: true })).toBeVisible();
  await page.getByRole('button', { name: 'Remux', exact: true }).click();
  await page.getByLabel('Combine tracks from multiple files').check();
}
async function submitted(page: Page) {
  return page.evaluate(() =>
    (globalThis as unknown as { __muxMock: Mock }).__muxMock.calls
      .filter((call) => call.command === 'start_mux')
      .map((call) => call.payload.request as MuxRequest),
  );
}

test('combined remux maps colliding stream indices by source, preserves order and saves independent owners and edits', async ({
  page,
}) => {
  await setup(page, { terminalReply: true });
  // The first imported source initializes the independent combined draft.
  await expect(page.getByLabel('Use source picture.mkv')).toBeChecked();
  await expect(page.getByLabel(`Use source ${media[1].name}`)).not.toBeChecked();
  await page.getByLabel(`Use source ${media[1].name}`).check();
  await page.getByLabel('Use source captions.srt').check();
  await page
    .getByLabel(`Title for ${media[1].name} #0`, { exact: true })
    .fill("Dub's ${title} 日本語");
  await page.getByLabel(`Language for ${media[1].name} #0`, { exact: true }).fill('spa');
  await page.getByLabel(`Default for ${media[1].name} #0`, { exact: true }).selectOption('yes');
  await page.getByLabel('Forced for captions.srt #0', { exact: true }).selectOption('yes');
  await page.getByRole('button', { name: `Move ${media[1].name} #0 up`, exact: true }).click();
  await page.getByLabel('Container metadata from').selectOption('sound');
  await page.getByLabel('Chapters from').selectOption('picture');
  await page.getByRole('button', { name: 'Choose combined destination', exact: true }).click();
  await page.getByRole('button', { name: 'Start combined remux', exact: true }).click();
  await expect(page.getByText('Succeeded', { exact: true })).toBeVisible();
  const [request] = await submitted(page);
  expect(request.sources.map((source) => source.id)).toEqual(['picture', 'sound', 'captions']);
  expect(request.tracks.map((track) => [track.sourceId, track.streamIndex])).toEqual([
    ['sound', 0],
    ['picture', 0],
    ['captions', 0],
    ['picture', 8],
  ]);
  expect(request.tracks[0]).toEqual({
    sourceId: 'sound',
    streamIndex: 0,
    title: "Dub's ${title} 日本語",
    language: 'spa',
    default: true,
    forced: null,
  });
  expect(request.tracks[2].forced).toBe(true);
  expect(request).toMatchObject({
    metadataSourceId: 'sound',
    chaptersSourceId: 'picture',
    outputPath: "C:\\exports\\combined's $.mkv",
  });
  await page.getByLabel(`Title for ${media[1].name} #0`, { exact: true }).fill('Second draft');
  await page.getByLabel('Combined destination').fill('C:\\exports\\second.mkv');
  await page.getByRole('button', { name: 'Start combined remux', exact: true }).click();
  const history = page.getByRole('article', { name: 'Job mux-1', exact: true });
  await history.getByText('Saved settings and log', { exact: true }).click();
  await expect(history.getByText('3 sources · 4 selected tracks', { exact: true })).toBeVisible();
  await expect(history.getByText(/Dub's \$\{title\} 日本語/)).toBeVisible();
  expect((await submitted(page))[0]).toEqual(request);
});

test('removing another source preserves remaining track choices and edits while attachment order stays valid', async ({
  page,
}) => {
  await setup(page);
  await page.getByLabel(`Use source ${media[1].name}`).check();
  await page.getByLabel('Use source captions.srt').check();
  await page.getByLabel(`Title for ${media[1].name} #0`, { exact: true }).fill('Keep my edit');
  await page.getByLabel('Include captions.srt #0', { exact: true }).uncheck();
  await page.getByLabel('Chapters from').selectOption('');
  await expect(
    page.getByRole('button', { name: 'Move picture.mkv #8 up', exact: true }),
  ).toBeDisabled();
  await page.getByLabel('Use source picture.mkv').uncheck();
  await expect(page.getByLabel(`Title for ${media[1].name} #0`, { exact: true })).toHaveValue(
    'Keep my edit',
  );
  await expect(page.getByLabel('Include captions.srt #0', { exact: true })).not.toBeChecked();
  await expect(page.getByLabel('Container metadata from')).toHaveValue('sound');
  await expect(page.getByLabel('Chapters from')).toHaveValue('');
  await page.getByLabel('Combine tracks from multiple files').uncheck();
  await page.getByLabel('Combine tracks from multiple files').check();
  await expect(page.getByLabel(`Title for ${media[1].name} #0`, { exact: true })).toHaveValue(
    'Keep my edit',
  );
  await page.getByRole('button', { name: 'Start combined remux', exact: true }).click();
  expect((await submitted(page))[0]).toMatchObject({
    metadataSourceId: 'sound',
    chaptersSourceId: null,
    tracks: [{ sourceId: 'sound', streamIndex: 0, title: 'Keep my edit' }],
  });
});

test('combined validation and runtime refusal keep drafts editable and never submit an empty media selection', async ({
  page,
}) => {
  await setup(page);
  await page.setViewportSize({ width: 760, height: 650 });
  await page.getByLabel('Include picture.mkv #0', { exact: true }).uncheck();
  await expect(
    page.getByRole('button', { name: 'Start combined remux', exact: true }),
  ).toBeDisabled();
  await page.getByLabel(`Use source ${media[1].name}`).check();
  const language = page.getByLabel(`Language for ${media[1].name} #0`, { exact: true });
  await language.fill('english');
  await expect(
    page.getByRole('button', { name: 'Start combined remux', exact: true }),
  ).toBeDisabled();
  await language.fill('');
  await page.evaluate(() => (globalThis as unknown as { __muxMock: Mock }).__muxMock.fail(true));
  await page.getByRole('button', { name: 'Start combined remux', exact: true }).click();
  await expect(page.getByRole('alert')).toContainText('Choose a destination that does not exist.');
  await expect(language).toHaveValue('');
  await expect(language).toBeEnabled();
  expect((await submitted(page))[0].tracks[0].language).toBe('');
  expect(await page.evaluate(() => document.documentElement.scrollWidth <= window.innerWidth)).toBe(
    true,
  );
});
