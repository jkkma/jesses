import { expect, test } from '@playwright/test';
import { clock, latestStep, liveJobProgress } from '../../src/lib/components/shared/job-progress';
import { installSourceEncodeFixture } from './helpers/source-encode-fixture';

function job(overrides: Record<string, unknown>) {
  return {
    id: 'job',
    state: 'running',
    request: { inputPath: '/media/in.mp4', outputPath: '/media/out.mkv', streamIndices: [0] },
    encodeSettings: { backend: 'standalone' },
    progressSeconds: null,
    durationSeconds: null,
    logs: [],
    error: null,
    recovery: null,
    logPath: null,
    ...overrides,
  } as any;
}

test('long preparation shows its current step and an indeterminate bar', () => {
  const progress = liveJobProgress(
    job({
      state: 'preparing',
      logs: [
        'Planning source frames from every packet timestamp and the first decoded frames.',
        'Fingerprinting the 18.0 GB source and the tools so an interrupted encode can resume safely.',
        'Tool: C:\\tools\\ffmpeg.exe — ffmpeg version 9.0.1',
        '[producer] [in#0 @ 0x1] UDTA parsing failed retrying raw',
        '    Last message repeated 6 times',
      ],
    }),
    null,
  );
  expect(progress.text).toBe(
    'Preparing encode: Fingerprinting the 18.0 GB source and the tools so an interrupted encode can resume safely.',
  );
  expect(progress.value).toBeNull();
});

test('encoding shows percent, position, speed and time left', () => {
  const progress = liveJobProgress(job({ progressSeconds: 126, durationSeconds: 300 }), {
    kind: 'estimate',
    speed: 0.556,
    remainingSeconds: 313,
  });
  expect(progress.text).toBe('Encoding 42% · 2:06 of 5:00 · 0.56× realtime · about 5:13 left');
  expect(progress).toMatchObject({ value: 126, max: 300 });
  expect(liveJobProgress(job({ progressSeconds: 3, durationSeconds: 12 }), null).text).toBe(
    'Encoding 25% · 0:03 of 0:12',
  );
});

test('encoder telemetry supplies the bar and rate when it has an exact frame total', () => {
  const snapshot = job({
    progressSeconds: 30,
    durationSeconds: 300,
    encoderProgress: {
      frames: 12,
      totalFrames: 50,
      percent: 24,
      fps: 30,
      bitrateKbps: 1234.56,
      sizeBytes: 12340000,
      estimatedSizeBytes: 51420000,
      elapsedSeconds: 3,
      etaSeconds: 9,
    },
  });
  const measured = liveJobProgress(snapshot, { kind: 'waiting' });
  expect(measured).toMatchObject({ value: 12, max: 50 });
  expect(measured.text).toContain('Encoding 24% · 12/50 frames · 30.00 fps');
  expect(measured.text).toContain('0:03 elapsed · 0:09 left');
  expect(measured.text).toContain('1,234.56 kb/s');
  expect(measured.text).toContain('12.34 MB');
  expect(measured.text).toContain('51.42 MB');
  const paused = liveJobProgress({ ...snapshot, state: 'paused' }, { kind: 'waiting' });
  expect(paused).toMatchObject({ value: 12, max: 50 });
  expect(paused.text).toContain('Paused at 24%');
  expect(paused.text).toContain('12/50 frames');
  expect(paused.text).not.toMatch(/fps|left/);
  const finishing = liveJobProgress(
    { ...snapshot, state: 'finalizing', progressSeconds: 0, logs: ['Verifying output.'] },
    null,
  );
  expect(finishing).toMatchObject({ text: 'Finishing encode: Verifying output.', value: null });
});

test('final checks without a frame count stay indeterminate, with them they measure', () => {
  const logs = [
    "Verifying the completed output's exact frame count and timing, then checking copied tracks and metadata.",
  ];
  const waiting = liveJobProgress(
    job({ state: 'finalizing', progressSeconds: 0, durationSeconds: 300, logs }),
    null,
  );
  expect(waiting.text).toBe(
    "Finishing encode: Verifying the completed output's exact frame count and timing, then checking copied tracks and metadata.",
  );
  expect(waiting.value).toBeNull();
  const scanning = liveJobProgress(
    job({ state: 'finalizing', progressSeconds: 150, durationSeconds: 300, logs }),
    null,
  );
  expect(scanning.text.startsWith('Finishing encode 50%: Verifying')).toBe(true);
  expect(scanning.value).toBe(150);
});

test('pausing verification preserves its last step instead of reporting an encoding percentage', () => {
  const progress = liveJobProgress(
    job({
      state: 'paused',
      encodeSettings: { backend: 'av1an' },
      progressSeconds: null,
      durationSeconds: 305,
      logs: [
        'Verifying AV1AN chunks (12/55 decoded).',
        'Paused the media job process trees. Memory and open files are retained.',
      ],
    }),
    null,
  );
  expect(progress.text).toContain('Paused');
  expect(progress.text).toContain('file checks may continue');
  expect(progress.text).toContain('Verifying AV1AN chunks (12/55 decoded).');
  expect(progress.value).toBeNull();
});

test('queued, paused, AV1AN and plain tasks keep their own labels', () => {
  expect(liveJobProgress(job({ state: 'queued' }), null).text).toBe('Queued: encode...');
  expect(
    liveJobProgress(job({ state: 'paused', progressSeconds: 60, durationSeconds: 300 }), null).text,
  ).toBe('Paused at 20%: encode');
  expect(
    liveJobProgress(
      job({
        encodeSettings: { backend: 'av1an' },
        progressSeconds: 30,
        durationSeconds: 120,
      }),
      { kind: 'waiting' },
    ).text,
  ).toBe('Encoding (AV1AN) 25% · 0:30 of 2:00 · waiting for progress');
  expect(liveJobProgress(job({ encodeSettings: null, logs: ['Copying tracks.'] }), null).text).toBe(
    'Running: Copying tracks.',
  );
});

test('steps skip tool output and file bookkeeping; clock stays compact', () => {
  expect(latestStep(['Muxing encoded video.', 'Mux log: C:\\logs\\job.mux.log'])).toBe(
    'Muxing encoded video.',
  );
  expect(latestStep(['[consumer] Svt[info]: SVT [config]'])).toBeNull();
  expect(clock(59.6)).toBe('1:00');
  expect(clock(3725)).toBe('1:02:05');
});

test('a single encode in preparation shows the live job step, not a blank bar', async ({
  page,
}) => {
  await installSourceEncodeFixture(page);
  await page.getByRole('button', { name: 'Add Files…' }).click();
  await page.getByRole('button', { name: 'Load Tracks' }).click();
  await page.getByRole('button', { name: 'Quick Convert', exact: true }).click();
  await page
    .getByRole('region', { name: 'Quick Convert', exact: true })
    .getByLabel('Output Path', { exact: true })
    .fill('/media/held');
  await page.evaluate(() => {
    (globalThis as any).__holdResolution = new Promise(() => {});
  });
  await page.getByRole('button', { name: 'Run: Encode', exact: true }).click();
  const status = page.getByLabel('Current action');
  const bar = page.getByLabel('Task progress');
  await expect(status).toHaveText('Running: Encode...');
  await expect(bar).not.toHaveAttribute('value');
  await page.evaluate(() => {
    const host = globalThis as any;
    host.__testJobs = [
      {
        id: 'prepared',
        state: 'preparing',
        request: {
          inputPath: '/media/input.mkv',
          outputPath: '/media/held.mkv',
          streamIndices: [0],
        },
        encodeSettings: { backend: 'standalone' },
        progressSeconds: null,
        durationSeconds: null,
        logs: ['Planning source frames from every packet timestamp and the first decoded frames.'],
        error: null,
        recovery: null,
        logPath: null,
      },
      ...host.__testJobs,
    ];
    host.__emitJobs();
  });
  await expect(status).toHaveText(
    'Preparing encode: Planning source frames from every packet timestamp and the first decoded frames.',
  );
  await expect(bar).not.toHaveAttribute('value');
});

test('finalizing updates stay visible through pause and continue without encoding estimates', async ({
  page,
}) => {
  await installSourceEncodeFixture(page);
  await page.evaluate(() => {
    const host = globalThis as any;
    host.__testJobs = [
      {
        id: 'finalizing',
        state: 'finalizing',
        request: {
          inputPath: '/media/in.mkv',
          outputPath: '/media/out.webm',
          streamIndices: [0, 1],
        },
        encodeSettings: { backend: 'av1an', encoder: 'svtAv1' },
        progressSeconds: null,
        durationSeconds: 305,
        logs: ['Verifying AV1AN chunks (12/55 decoded).'],
        error: null,
        recovery: null,
        logPath: null,
      },
    ];
    host.__emitJobs();
  });
  const status = page.getByLabel('Current action');
  const bar = page.getByLabel('Task progress');
  await expect(status).toContainText('Verifying AV1AN chunks (12/55 decoded).');
  await expect(bar).not.toHaveAttribute('value');
  await page.getByRole('button', { name: 'Pause', exact: true }).click();
  await expect(status).toContainText('Paused');
  await expect(status).toContainText('file checks may continue');
  await expect(status).toContainText('Verifying AV1AN chunks (12/55 decoded).');
  await expect(bar).not.toHaveAttribute('value');
  await page.getByRole('button', { name: 'Resume', exact: true }).click();
  await expect(status).toContainText('Finishing');
  await expect(status).toContainText('Verifying AV1AN chunks (12/55 decoded).');
  await expect(status).not.toContainText('realtime');
  await expect(status).not.toContainText('left');
  await page.evaluate(() => {
    const host = globalThis as any;
    host.__testJobs[0].logs.push('Checking the joined AV1AN video against the sealed chunk queue.');
    host.__emitJobs();
  });
  await expect(status).toContainText('Checking the joined AV1AN video');
});
