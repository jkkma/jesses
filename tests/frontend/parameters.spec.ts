import { expect, test, type Page } from '@playwright/test';
import type { BatchEncodeRequest, EncodeRequest, MediaFile } from '../../src/lib/ipc/generated';

const source: MediaFile = {
  id: 'subtitle-source',
  path: 'C:\\media\\captions.mkv',
  name: 'captions.mkv',
  sizeBytes: '1000',
  durationSeconds: 12,
  format: 'matroska',
  streams: [
    {
      index: 2,
      kind: 'video',
      codec: 'h264',
      width: 320,
      height: 180,
      frameRate: '24/1',
      sampleRate: null,
      channels: null,
      language: null,
      title: null,
    },
    ...[7, 9].map((index) => ({
      index,
      kind: 'subtitle' as const,
      codec: 'ass',
      width: null,
      height: null,
      frameRate: null,
      sampleRate: null,
      channels: null,
      language: 'eng',
      title: `Captions ${index}`,
    })),
  ],
};
type Call = { command: string; payload: Record<string, unknown> };
async function mock(page: Page, corruptPresets = false) {
  await page.addInitScript(
    ({ source, corruptPresets }) => {
      const state = globalThis as unknown as Record<string, unknown>;
      const calls: Call[] = [];
      state.__subtitleCalls = calls;
      let nextId = 0;
      let presets: import('../../src/lib/ipc/generated').EncoderParameterPreset[] = [];
      state.isTauri = true;
      state.__TAURI_INTERNALS__ = {
        metadata: { currentWindow: { label: 'main' }, currentWebview: { label: 'main' } },
        transformCallback: () => ++nextId,
        unregisterCallback: () => {},
        invoke: async (command: string, payload: Record<string, unknown> = {}) => {
          calls.push({ command, payload });
          if (command.startsWith('plugin:event|')) return ++nextId;
          if (command === 'get_capabilities')
            return [
              'ffmpeg',
              'ffprobe',
              'svt-av1',
              'svt-av1-5fish',
              'svt-av1-hdr',
              'x264',
              'av1an',
              'mkvmerge',
            ].map((id) => ({
              id,
              name: id,
              available: true,
              path: `C:\\tools\\${id}.exe`,
              version: 'mock',
              detail: null,
            }));
          if (command === 'begin_media_analysis') return `analysis-${++nextId}`;
          if (command === 'cancel_media_analysis') return;
          if (command === 'get_parameter_presets') {
            if (corruptPresets)
              throw { message: 'Preferences could not be read. Existing files were preserved.' };
            return structuredClone(presets);
          }
          if (command === 'save_parameter_preset' || command === 'remove_parameter_preset') {
            const item =
              payload.request as import('../../src/lib/ipc/generated').EncoderParameterPreset;
            presets = presets.filter(
              (value) =>
                value.name !== item.name ||
                value.encoder !== item.encoder ||
                value.backend !== item.backend,
            );
            if (command === 'save_parameter_preset') presets.push(structuredClone(item));
            return structuredClone(presets);
          }
          if (command === 'get_encoder_parameters') {
            const request = payload.request as { encoder: string; backend: string };
            return {
              ...request,
              route: request.encoder === 'x265' ? 'FFmpeg library' : 'Standalone encoder CLI',
              toolPath: 'C:\\tools\\encoder.exe',
              toolVersion: 'qualified test build',
              parameters:
                request.backend === 'av1an' && request.encoder === 'x264'
                  ? [
                      {
                        name: 'tune',
                        label: 'Tune',
                        argument: '--tune',
                        minimum: 0,
                        maximum: 0,
                        valueKind: 'choiceList',
                        minimumValue: '',
                        maximumValue: '',
                        choices: ['film', 'grain'],
                        group: 'Psychovisual',
                        description: 'Source tuning',
                        example: 'film',
                      },
                      {
                        name: 'psy-rd',
                        label: 'Psycho-visual RD',
                        argument: '--psy-rd',
                        minimum: 0,
                        maximum: 10,
                        valueKind: 'pairDecimal',
                        minimumValue: '0',
                        maximumValue: '10',
                        choices: [],
                        group: 'Psychovisual',
                        description: 'Rate distortion and trellis tuning',
                        example: '1.0:0.0',
                      },
                      {
                        name: 'pbratio',
                        label: 'P-to-B QP ratio',
                        argument: '--pbratio',
                        minimum: 1,
                        maximum: 10,
                        valueKind: 'decimal',
                        minimumValue: '1',
                        maximumValue: '10',
                        choices: [],
                        group: 'Rate control',
                        description: 'B-frame quantizer ratio',
                        example: '1.30',
                      },
                    ]
                  : request.backend === 'av1an' && request.encoder.startsWith('svtAv1')
                    ? [
                        {
                          name: 'aq-mode',
                          label: 'Adaptive quantization mode',
                          argument: '--aq-mode',
                          minimum: 0,
                          maximum: 2,
                          valueKind: 'whole',
                          minimumValue: '0',
                          maximumValue: '2',
                          choices: [],
                          group: 'Quantization',
                          description: 'Adaptive quantization',
                          example: '2',
                        },
                        {
                          name: 'complex-hvs',
                          label: 'Complex visual model',
                          argument: '--complex-hvs',
                          minimum: 0,
                          maximum: 1,
                          valueKind: 'whole',
                          minimumValue: '0',
                          maximumValue: '1',
                          choices: [],
                          group: 'Psychovisual',
                          description: 'Perceptual detail model',
                          example: '1',
                        },
                        {
                          name: 'noise',
                          label: 'Noise override',
                          argument: '--noise',
                          minimum: 0,
                          maximum: 200,
                          valueKind: 'whole',
                          minimumValue: '0',
                          maximumValue: '200',
                          choices: [],
                          group: 'Film grain',
                          description: 'Advanced grain source',
                          example: '10',
                        },
                      ]
                    : [
                        {
                          name: 'ref',
                          label: 'Reference frames',
                          argument: request.encoder === 'x265' ? 'ref' : '--ref',
                          minimum: 1,
                          maximum: request.encoder === 'x264' ? 16 : 6,
                          valueKind: 'whole',
                          minimumValue: '1',
                          maximumValue: request.encoder === 'x264' ? '16' : '6',
                          choices: [],
                          group: 'General',
                          description: 'Reference frames',
                          example: '3',
                        },
                      ],
              notes: ['Speed preset first; overrides afterward.'],
            };
          }
          if (command === 'preview_encode_plan') {
            const request = payload.request as EncodeRequest;
            return new Promise((resolve) => {
              state.__resolvePlan = () =>
                resolve({
                  request,
                  sourceFingerprint: 'fixture-hash',
                  outputFrameCount: '288',
                  outputFrameRate: '24/1',
                  notes: ['Temporary paths belong to this preview.'],
                  stages: [
                    {
                      label: 'Video encoder',
                      executable: 'C:\\tools\\encoder.exe',
                      arguments: [
                        '--ref',
                        request.settings.parameters?.[0]?.value ?? '1',
                        '--source-label',
                        request.source.inputPath,
                      ],
                      workingDirectory: null,
                      notes: [],
                    },
                  ],
                });
            });
          }
          if (command === 'get_storage_locations') return [];
          if (command === 'plugin:dialog|open')
            return (payload.options as { directory?: boolean })?.directory
              ? 'C:\\output'
              : [source.path];
          if (command === 'import_paths') return { files: [source], errors: [] };
          if (command === 'probe_media') return source;
          if (command === 'list_jobs') return [];
          if (command === 'subscribe_jobs') {
            (payload.channel as { onmessage: (jobs: unknown[]) => void }).onmessage([]);
            return;
          }
          if (command === 'preview_encode_batch') {
            const request = payload.request as BatchEncodeRequest;
            return {
              items: request.inputs.map((input) => ({
                inputPath: input.inputPath,
                outputPath: 'C:\\output\\captions.mkv',
                error: null,
                request: {
                  source: {
                    inputPath: input.inputPath,
                    outputPath: 'C:\\output\\captions.mkv',
                    streamIndices: input.streamIndices,
                  },
                  settings: {
                    ...request,
                    videoStreamIndex: input.videoStreamIndex,
                    audio: input.audio,
                    framing: input.framing,
                    subtitles: input.subtitles,
                    parameters: request.parameters,
                  },
                },
              })),
            };
          }
          if (
            command === 'start_encode' ||
            command === 'enqueue_encode' ||
            command === 'enqueue_encode_batch'
          ) {
            const jobs = (
              (payload.requests as EncodeRequest[]) ?? [payload.request as EncodeRequest]
            ).map((request) => ({
              id: `job-${++nextId}`,
              state: 'queued',
              request: request.source,
              encodeSettings: request.settings,
              progressSeconds: 0,
              durationSeconds: 12,
              logs: [],
              error: null,
              recovery: null,
              logPath: null,
            }));
            return command === 'enqueue_encode_batch' ? jobs : jobs[0];
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
    { source, corruptPresets },
  );
  await page.goto('/');
  await page.getByRole('button', { name: 'Add files', exact: true }).first().click();
  await expect(page.getByRole('heading', { name: source.name, exact: true })).toBeVisible();
  await page.getByRole('button', { name: 'Quick Convert', exact: true }).click();
}
const calls = (page: Page, command: string) =>
  page.evaluate(
    (command) =>
      (globalThis as unknown as { __subtitleCalls: Call[] }).__subtitleCalls.filter(
        (call) => call.command === command,
      ),
    command,
  );

test('qualified parameter presets stay separate across encoders and apply after speed defaults', async ({
  page,
}) => {
  await mock(page);
  const quick = page.getByRole('region', { name: 'Quick Convert workspace', exact: true });
  await quick.getByLabel('Video encoder', { exact: true }).selectOption('x264');
  await quick.getByText('Advanced encoder parameters', { exact: true }).click();
  await quick.getByLabel('Override Reference frames', { exact: true }).check();
  await quick.getByLabel('Reference frames value', { exact: true }).fill('17');
  await expect(quick.getByRole('button', { name: 'Start encode', exact: true })).toBeDisabled();
  await quick.getByLabel('Reference frames value', { exact: true }).fill('3');
  await quick.getByLabel('Parameter preset name', { exact: true }).fill('Animation');
  await quick.getByRole('button', { name: 'Save parameter preset', exact: true }).click();
  await expect(quick.getByLabel('Saved parameter preset', { exact: true })).toHaveValue(
    'Animation',
  );
  await quick.getByLabel('Video encoder', { exact: true }).selectOption('x265');
  await expect(
    quick.getByLabel('Saved parameter preset', { exact: true }).locator('option'),
  ).toHaveCount(1);
  await quick.getByLabel('Video encoder', { exact: true }).selectOption('x264');
  await expect(quick.getByLabel('Reference frames value', { exact: true })).toHaveValue('3');
  await quick.getByRole('button', { name: 'Clear overrides', exact: true }).click();
  await quick.getByLabel('Saved parameter preset', { exact: true }).selectOption('Animation');
  await quick.getByRole('button', { name: 'Apply parameter preset', exact: true }).click();
  await expect(quick.getByLabel('Reference frames value', { exact: true })).toHaveValue('3');
  await quick.getByRole('button', { name: 'Start encode', exact: true }).click();
  const request = (await calls(page, 'start_encode'))[0].payload.request as EncodeRequest;
  expect(request.settings.parameters).toEqual([{ name: 'ref', value: '3' }]);
  await quick.getByLabel('Reference frames value', { exact: true }).fill('4');
  expect(request.settings.parameters).toEqual([{ name: 'ref', value: '3' }]);
});

test('AV1AN x264 exposes named tune and decimal pair overrides in the queued request', async ({
  page,
}) => {
  await mock(page);
  await page.getByRole('button', { name: 'av1an', exact: true }).click();
  const workspace = page.getByRole('region', { name: 'av1an workspace', exact: true });
  await workspace.getByLabel('Video encoder', { exact: true }).selectOption('x264');
  await workspace.getByText('Advanced encoder parameters', { exact: true }).click();
  await expect
    .poll(async () => (await calls(page, 'get_encoder_parameters')).at(-1)?.payload.request)
    .toEqual({
      encoder: 'x264',
      backend: 'av1an',
    });
  await workspace.getByLabel('Override Tune', { exact: true }).check();
  await workspace.getByLabel('Tune value', { exact: true }).fill('grain');
  await workspace.getByLabel('Override Psycho-visual RD', { exact: true }).check();
  await workspace.getByLabel('Psycho-visual RD value', { exact: true }).fill('1.2:0.1500');
  const queue = workspace.getByRole('button', { name: 'Add to queue', exact: true });
  await expect(queue).toBeDisabled();
  await workspace.getByLabel('Psycho-visual RD value', { exact: true }).fill('1.2:0.15');
  await workspace.getByLabel('Override P-to-B QP ratio', { exact: true }).check();
  await workspace.getByLabel('P-to-B QP ratio value', { exact: true }).fill('1.30');
  await expect(queue).toBeEnabled();
  await queue.click();
  const request = (await calls(page, 'enqueue_encode'))[0].payload.request as EncodeRequest;
  expect(request.settings).toMatchObject({ backend: 'av1an', encoder: 'x264' });
  expect(request.settings.parameters).toEqual([
    { name: 'tune', value: 'grain' },
    { name: 'psy-rd', value: '1.2:0.15' },
    { name: 'pbratio', value: '1.30' },
  ]);
});

test('AV1AN SVT accepts adaptive quantization and complex visual-model overrides', async ({
  page,
}) => {
  await mock(page);
  await page.getByRole('button', { name: 'av1an', exact: true }).click();
  const workspace = page.getByRole('region', { name: 'av1an workspace', exact: true });
  await workspace.getByText('Advanced encoder parameters', { exact: true }).click();
  await workspace.getByLabel('Override Adaptive quantization mode', { exact: true }).check();
  await workspace.getByLabel('Adaptive quantization mode value', { exact: true }).fill('3');
  const queue = workspace.getByRole('button', { name: 'Add to queue', exact: true });
  await expect(queue).toBeDisabled();
  await workspace.getByLabel('Adaptive quantization mode value', { exact: true }).fill('2');
  await workspace.getByLabel('Override Complex visual model', { exact: true }).check();
  await workspace.getByLabel('Complex visual model value', { exact: true }).fill('1');
  await expect(queue).toBeEnabled();
  await queue.click();
  const request = (await calls(page, 'enqueue_encode'))[0].payload.request as EncodeRequest;
  expect(request.settings.parameters).toEqual([
    { name: 'aq-mode', value: '2' },
    { name: 'complex-hvs', value: '1' },
  ]);
});

test('AV1AN blocks conflicting SVT grain sources before queueing', async ({ page }) => {
  await mock(page);
  await page.getByRole('button', { name: 'av1an', exact: true }).click();
  const workspace = page.getByRole('region', { name: 'av1an workspace', exact: true });
  await workspace.getByLabel('Film grain synthesis', { exact: true }).fill('8');
  await workspace.getByText('Advanced encoder parameters', { exact: true }).click();
  await workspace.getByLabel('Override Noise override', { exact: true }).check();
  await workspace.getByLabel('Noise override value', { exact: true }).fill('10');
  const queue = workspace.getByRole('button', { name: 'Add to queue', exact: true });
  await expect(queue).toBeDisabled();
  await expect(workspace.getByRole('alert')).toContainText(
    'Use one grain source: a grain table, film-grain synthesis, or the advanced noise override.',
  );
  await workspace.getByLabel('Film grain synthesis', { exact: true }).fill('0');
  await expect(queue).toBeEnabled();
});

test('command preview cancels on edits and discards late replies before showing native argument arrays', async ({
  page,
}) => {
  await mock(page);
  const quick = page.getByRole('region', { name: 'Quick Convert workspace', exact: true });
  const button = quick.getByRole('button', { name: 'Preview command plan', exact: true });
  await button.click();
  await expect(
    quick.getByRole('button', { name: 'Cancel command preview', exact: true }),
  ).toBeVisible();
  await quick
    .getByLabel('Encode destination', { exact: true })
    .fill('C:\\output\\changed $ % 空.mkv');
  await expect
    .poll(async () => (await calls(page, 'cancel_media_analysis')).length)
    .toBeGreaterThan(0);
  await page.evaluate(() =>
    (globalThis as unknown as { __resolvePlan: () => void }).__resolvePlan(),
  );
  await expect(quick.getByText('Validated plan:', { exact: false })).toHaveCount(0);
  await button.click();
  await expect.poll(async () => (await calls(page, 'preview_encode_plan')).length).toBe(2);
  await page.evaluate(() =>
    (globalThis as unknown as { __resolvePlan: () => void }).__resolvePlan(),
  );
  await expect(
    quick.getByText('Validated plan: 288 frames at 24/1 fps.', { exact: true }),
  ).toBeVisible();
  await quick.getByText('1. Video encoder', { exact: true }).click();
  const argv = quick.getByLabel('Video encoder argument array', { exact: true });
  await expect(argv).toHaveAttribute('readonly', '');
  expect(JSON.parse(await argv.inputValue())).toContain(source.path);
  await quick.getByLabel('Encode destination', { exact: true }).fill('C:\\output\\new.mkv');
  await expect(quick.getByText('Validated plan:', { exact: false })).toHaveCount(0);
});

test('batch parameter changes invalidate review while queued values remain frozen', async ({
  page,
}) => {
  await mock(page);
  await page.getByRole('button', { name: 'Batch encode', exact: true }).click();
  const batch = page.getByRole('region', { name: 'Batch encode workspace', exact: true });
  await batch.getByLabel('Video encoder', { exact: true }).selectOption('x264');
  await batch.getByRole('button', { name: 'Choose output folder', exact: true }).click();
  const preview = batch.getByRole('button', { name: 'Preview batch', exact: true });
  const queue = batch.getByRole('button', { name: 'Queue ready files', exact: true });
  await preview.click();
  await queue.click();
  const original = (await calls(page, 'enqueue_encode_batch'))[0].payload.requests;
  await batch.getByLabel('Select captions.mkv', { exact: true }).check();
  await preview.click();
  await batch.getByText('Advanced encoder parameters', { exact: true }).click();
  await batch.getByLabel('Override Reference frames', { exact: true }).check();
  await batch.getByLabel('Reference frames value', { exact: true }).fill('4');
  await expect(queue).toBeDisabled();
  await preview.click();
  await queue.click();
  const submitted = await calls(page, 'enqueue_encode_batch');
  expect(submitted[0].payload.requests).toEqual(original);
  expect((submitted[1].payload.requests as EncodeRequest[])[0].settings.parameters).toEqual([
    { name: 'ref', value: '4' },
  ]);
});

test('unreadable Rust presets keep saving disabled and expose the preserved-file error', async ({
  page,
}) => {
  await mock(page, true);
  const quick = page.getByRole('region', { name: 'Quick Convert workspace', exact: true });
  await quick.getByLabel('Video encoder', { exact: true }).selectOption('x264');
  await quick.getByText('Advanced encoder parameters', { exact: true }).click();
  await expect(
    quick.getByRole('alert').filter({ hasText: 'Existing files were preserved.' }),
  ).toBeVisible();
  await quick.getByLabel('Parameter preset name', { exact: true }).fill('Cannot overwrite');
  await expect(
    quick.getByRole('button', { name: 'Save parameter preset', exact: true }),
  ).toBeDisabled();
  expect(await calls(page, 'save_parameter_preset')).toHaveLength(0);
  expect(
    await page.evaluate(() => localStorage.getItem('jesses.encoder-parameter-presets.v1')),
  ).toBeNull();
});
