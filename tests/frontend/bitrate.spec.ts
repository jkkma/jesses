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
          throw new Error(`Unexpected command: ${command}`);
        },
      };
    },
    { hold },
  );
  await page.goto('/');
  await page.getByRole('button', { name: 'Add files', exact: true }).first().click();
  await expect(page.getByRole('heading', { name: 'sample.mkv', exact: true })).toBeVisible();
}

const calls = (page: Page, command: string) =>
  page.evaluate(
    (command) =>
      (globalThis as unknown as { __bitrateMock: Mock }).__bitrateMock.calls.filter(
        (call) => call.command === command,
      ),
    command,
  );

test('bitrate analysis stays lazy and uses original stream identities with adjustable windows', async ({
  page,
}) => {
  await setup(page);
  expect(await calls(page, 'analyze_bitrate')).toHaveLength(0);
  await page.getByRole('button', { name: 'Bitrate analysis', exact: true }).click();
  await page.getByLabel('Bitrate stream', { exact: true }).selectOption('3');
  await page.getByLabel('Bitrate window', { exact: true }).selectOption('5');
  await page.getByRole('button', { name: 'Analyze bitrate', exact: true }).click();
  await expect(page.getByRole('img', { name: /Packet bitrate over/ })).toBeVisible();
  const [call] = await calls(page, 'analyze_bitrate');
  expect(call.payload.request).toEqual({
    inputPath: 'C:\\media\\sample.mkv',
    streamIndex: 3,
    windowSeconds: 5,
  });
  await page.getByRole('slider', { name: 'Inspect bitrate window' }).focus();
  await page.keyboard.press('End');
  await expect(page.getByText(/5.00–10.00 s/)).toBeVisible();
  await page.getByRole('button', { name: 'Export SVG', exact: true }).click();
  expect(await calls(page, 'export_analysis')).toHaveLength(0);
  await expect(page.getByRole('alert')).toHaveCount(0);
  await page.getByLabel('Bitrate window', { exact: true }).selectOption('10');
  await expect(page.getByRole('img', { name: /Packet bitrate over/ })).toHaveCount(0);
  expect(await calls(page, 'analyze_bitrate')).toHaveLength(1);
});

test('closing cancels a packet scan and discards a late completion', async ({ page }) => {
  await setup(page, true);
  const disclosure = page.getByRole('button', { name: 'Bitrate analysis', exact: true });
  await disclosure.click();
  await page.getByRole('button', { name: 'Analyze bitrate', exact: true }).click();
  await expect.poll(async () => (await calls(page, 'analyze_bitrate')).length).toBe(1);
  await disclosure.click();
  await expect
    .poll(async () => (await calls(page, 'cancel_media_analysis')).length)
    .toBeGreaterThan(0);
  await page.evaluate(() =>
    (globalThis as unknown as { __bitrateMock: Mock }).__bitrateMock.release?.(),
  );
  await disclosure.click();
  await expect(page.getByRole('button', { name: 'Analyze bitrate', exact: true })).toBeEnabled();
  await expect(page.getByRole('img', { name: /Packet bitrate over/ })).toHaveCount(0);
  await expect(page.getByRole('alert')).toHaveCount(0);
});
