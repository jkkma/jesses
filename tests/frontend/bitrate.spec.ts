import { test, expect, type Page } from '@playwright/test';
import type { BitrateRequest, BitrateResult } from '../../src/lib/ipc/generated';

type Call = { command: string; payload: Record<string, unknown> };
type Mock = { calls: Call[]; release: (() => void) | null };

async function setup(page: Page, hold = false) {
  await page.addInitScript(
    ({ hold }) => {
      const state = globalThis as unknown as Record<string, unknown>;
      const mock: Mock = { calls: [], release: null };
      state.__bitrateMock = mock;
      state.isTauri = true;
      let callback = 0;
      state.__TAURI_INTERNALS__ = {
        metadata: { currentWindow: { label: 'main' }, currentWebview: { label: 'main' } },
        transformCallback: () => ++callback,
        unregisterCallback: () => {},
        invoke: async (command: string, payload: Record<string, unknown> = {}) => {
          mock.calls.push({ command, payload });
          if (command.startsWith('plugin:event|')) return ++callback;
          if (command === 'get_capabilities' || command === 'list_jobs') return [];
          if (command === 'list_av1an_recoveries') return [];
          if (command === 'resolve_output_path') {
            const request = payload.request as any,
              stem = request.outputStem ?? 'C:\\media\\sample';
            const ext =
              request.target.kind === 'container'
                ? request.target.format === 'matroska'
                  ? 'mkv'
                  : request.target.format
                : 'gif';
            return {
              outputStem: stem,
              outputPath: `${stem}.${ext}`,
              isDirectory: false,
              numbered: false,
            };
          }
          if (command === 'subscribe_jobs') {
            (payload.channel as { onmessage: (jobs: unknown[]) => void }).onmessage([]);
            return;
          }
          if (command === 'plugin:dialog|open') return ['C:\\media\\sample.mkv'];
          if (command === 'plugin:dialog|save') return null;
          if (command === 'probe_media')
            return {
              id: 'one',
              path: 'C:\\media\\sample.mkv',
              name: 'sample.mkv',
              sizeBytes: '10000',
              durationSeconds: 2,
              format: 'matroska',
              streams: [0, 3].map((index) => ({
                index,
                kind: index === 0 ? 'video' : 'audio',
                codec: index === 0 ? 'h264' : 'flac',
                width: 320,
                height: 180,
                frameRate: '24/1',
                channels: 2,
                sampleRate: 48000,
                language: null,
                title: null,
              })),
            };
          if (command === 'begin_media_analysis') return 'bitrate-ticket';
          if (command === 'cancel_media_analysis') return;
          if (command === 'analyze_bitrate') {
            if (hold)
              await new Promise<void>((resolve) => {
                mock.release = resolve;
              });
            const request = payload.request as BitrateRequest;
            return {
              streamIndex: request.streamIndex,
              windowSeconds: request.windowSeconds,
              packetBytes: '2000000',
              packetCount: '48',
              untimedPacketBytes: '0',
              untimedPacketCount: '0',
              dtsFallbackCount: '0',
              startSeconds: 0,
              endSeconds: 2,
              averageMegabitsPerSecond: 8,
              peakWindowMegabitsPerSecond: 12,
              sourceFingerprint: 'original',
              points: [
                { startSeconds: 0, packetBytes: '1500000', megabitsPerSecond: 12 },
                {
                  startSeconds: request.windowSeconds,
                  packetBytes: '500000',
                  megabitsPerSecond: 4,
                },
              ],
            } satisfies BitrateResult;
          }
          if (command === 'get_completion_status')
            return {
              options: { notify: false, finishAction: 'none' },
              armedJobs: 0,
              secondsRemaining: null,
              error: null,
            };
          throw new Error(`Unexpected command: ${command}`);
        },
      };
    },
    { hold },
  );
  await page.goto('/');
  await page.getByRole('button', { name: 'Add Files…', exact: true }).click();
  await page.getByRole('button', { name: 'Load Tracks', exact: true }).click();
}

const calls = (page: Page, command: string) =>
  page.evaluate(
    (command) =>
      (globalThis as unknown as { __bitrateMock: Mock }).__bitrateMock.calls.filter(
        (call) => call.command === command,
      ),
    command,
  );

// The fork exposes Read Bitrates and Show Bitrate Chart in Utilities. Its
// chart uses one-second windows; the old adjustable-window inspector and SVG
// export controls are not source screens.
test('Read Bitrates stays lazy and analyzes the selected original stream identity', async ({
  page,
}) => {
  await setup(page);
  expect(await calls(page, 'analyze_bitrate')).toHaveLength(0);
  await page.getByRole('checkbox', { name: /Include .*Video/ }).uncheck();
  await page.getByRole('button', { name: 'Utilities', exact: true }).click();
  await page
    .getByRole('region', { name: 'Utilities' })
    .getByRole('button', { name: /Read Bitrates/ })
    .click();
  await page.getByRole('button', { name: 'Run: Bitrate reading' }).click();
  await expect.poll(async () => (await calls(page, 'analyze_bitrate')).length).toBe(1);
  const [call] = await calls(page, 'analyze_bitrate');
  expect(call.payload.request).toEqual({
    inputPath: 'C:\\media\\sample.mkv',
    streamIndex: 3,
    windowSeconds: 1,
  });
  await expect(page.getByRole('log', { name: 'Recent activity' })).toContainText(
    'Stream #3 (audio) - Bitrate: 8000 kbps - Size: 1.9 MB (20000.0%)',
  );
});

test('Stop cancels a packet scan and discards a late chart completion', async ({ page }) => {
  await setup(page, true);
  await page.getByRole('button', { name: 'Utilities', exact: true }).click();
  await page
    .getByRole('region', { name: 'Utilities' })
    .getByRole('button', { name: /Show Bitrate Chart/ })
    .click();
  await page.getByRole('button', { name: 'Run: Bitrate chart' }).click();
  await expect.poll(async () => (await calls(page, 'analyze_bitrate')).length).toBe(1);
  await page.getByRole('button', { name: 'Stop', exact: true }).click();
  await expect
    .poll(async () => (await calls(page, 'cancel_media_analysis')).length)
    .toBeGreaterThan(0);
  await page.evaluate(() =>
    (globalThis as unknown as { __bitrateMock: Mock }).__bitrateMock.release?.(),
  );
  await expect(page.getByRole('dialog', { name: 'Bitrate Chart' })).toHaveCount(0);
  await expect(page.getByRole('alert')).toHaveCount(0);
});
