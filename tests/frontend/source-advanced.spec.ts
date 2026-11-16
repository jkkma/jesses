import { expect, test } from '@playwright/test';
import { parseSourceHdr } from '../../src/lib/components/shared/source-advanced';
import {
  av1anError,
  defaultAv1an,
  selectedAv1an,
  grainTableIssue,
} from '../../src/lib/components/shared/av1an-options';
import { supportsDynamicHdr } from '../../src/lib/components/shared/hdr-metadata-options';
import { requiredEncoderTools } from '../../src/lib/components/shared/encoder-options';

test('source HDR grid values map to typed metadata and reject malformed input', () => {
  expect(
    parseSourceHdr({
      'master-display': 'G(13250,34500)B(7500,3000)R(34000,16000)WP(15635,16450)L(10000000,1)',
      'max-cll': '1000,400',
      'dolby-vision-rpu': '/media/rpu.bin',
    }),
  ).toEqual({
    value: {
      masteringDisplay: {
        green: { x: 13250, y: 34500 },
        blue: { x: 7500, y: 3000 },
        red: { x: 34000, y: 16000 },
        whitePoint: { x: 15635, y: 16450 },
        maxLuminanceTenThousandths: 10000000,
        minLuminanceTenThousandths: 1,
      },
      contentLight: { maxContent: 1000, maxAverage: 400 },
      dolbyVisionRpuPath: '/media/rpu.bin',
    },
  });
  expect(parseSourceHdr({ 'max-cll': '1000,bad' }).error).toBeTruthy();
  expect(
    parseSourceHdr({ 'master-display': 'G(99999,1)B(1,1)R(1,1)WP(1,1)L(100,1)' }).error,
  ).toBeTruthy();
});

test('source quality targets validate visible scores while engine defaults own hidden probe settings', () => {
  const draft = defaultAv1an(false);
  draft.targetEnabled = true;
  draft.target = {
    ...draft.target,
    minimumCrf: 99,
    maximumCrf: 99,
    probes: 0,
    probingRate: 4,
    probeWidth: 1,
    probeHeight: 1,
  };
  expect(av1anError(draft, false, 'svtAv1', true, true)).toBeNull();
  expect(av1anError(draft, false, 'svtAv1', true)).toBeTruthy();
  draft.target = {
    ...draft.target,
    metric: 'vmaf',
    minimumScoreTenths: 995,
    maximumScoreTenths: 995,
  };
  expect(av1anError(draft, false, 'svtAv1', true, true)).toBeTruthy();
  draft.target = {
    ...draft.target,
    metric: 'ssimulacra2',
    minimumScoreTenths: 800,
    maximumScoreTenths: 800,
  };
  draft.chunkMethod = 'select';
  expect(av1anError(draft, false, 'svtAv1', true, true)).toContain('L-SMASH');
  draft.target = {
    ...draft.target,
    metric: 'xpsnrWeighted',
    minimumScoreTenths: 400,
    maximumScoreTenths: 400,
  };
  expect(av1anError(draft, false, 'svtAv1', true, true)).toBeNull();
});

test('the canonical source SVT choice accepts qualified dynamic HDR metadata', () => {
  expect(supportsDynamicHdr('svtAv1')).toBe(true);
  expect(supportsDynamicHdr('x264')).toBe(false);
});

test('source chunk defaults ignore hidden frame and scene controls', () => {
  const draft = defaultAv1an(false);
  draft.maximumChunkFrames = 100001;
  draft.minimumSceneFrames = 0;
  draft.sceneDownscaleHeight = 1;
  expect(av1anError(draft, false, 'svtAv1', true, true)).toBeNull();
  expect(av1anError(draft, false, 'svtAv1', true, false)).toBeTruthy();
});

test('source AV1AN honors valid FFmpeg concat choices and retains the HEVC correction', () => {
  const draft = defaultAv1an(false);
  draft.concatMethod = 'ffmpeg';
  expect(selectedAv1an(draft, 'x264', true).concatMethod).toBe('ffmpeg');
  expect(selectedAv1an(draft, 'vpxStandalone', true).concatMethod).toBe('ffmpeg');
  expect(selectedAv1an(draft, 'x265Standalone', true).concatMethod).toBe('mkvmerge');
  expect(selectedAv1an(draft, 'x264', false).concatMethod).toBe('mkvmerge');
  expect(requiredEncoderTools('av1an', 'x264', 'ffmpeg', true)).not.toContain('mkvmerge');
  expect(requiredEncoderTools('av1an', 'vpxStandalone', 'ffmpeg', true)).not.toContain('mkvmerge');
  expect(requiredEncoderTools('av1an', 'x264', 'ffmpeg', false)).toContain('mkvmerge');
});

test('grain table restrictions follow the encoder and effective chunking', () => {
  const constant = 'filmgrn1\nE 0 100 1 2 1\n';
  const late = 'filmgrn1\nE 10 100 1 2 1\n';
  const timed = 'filmgrn1\nE 0 50 1 2 1\nE 50 100 1 2 1\n';
  const draft = defaultAv1an(false);
  for (const backend of ['standalone', 'av1an'] as const) {
    expect(grainTableIssue('svtAv1Hdr', backend, constant, draft, true)).toBeNull();
    expect(grainTableIssue('svtAv1Hdr', backend, timed, draft, true)).toContain(
      'one segment starting at zero',
    );
    expect(grainTableIssue('svtAv1', backend, late, draft, true)).toContain(
      'one segment starting at zero',
    );
  }
  expect(
    grainTableIssue('aomAv1', 'standalone', constant.replaceAll('\n', '\r\n'), draft, false),
  ).toBe(
    'AOM requires LF line endings in grain tables. Re-select the file to reload it, or use an LF copy.',
  );
  const tabbed = 'filmgrn1\nE 0 50 1 2 1\nE\t50 100 1 2 1\n';
  const nonAsciiPrefix = 'filmgrn1\nE 0 100 1 2 1\n\u00a0E 100 200 1 2 1\n';
  expect(grainTableIssue('svtAv1Hdr', 'standalone', tabbed, draft, false)).toContain(
    'one segment starting at zero',
  );
  expect(grainTableIssue('svtAv1Hdr', 'standalone', nonAsciiPrefix, draft, false)).toBeNull();
  expect(grainTableIssue('aomAv1', 'standalone', timed, draft, false)).toBeNull();
  expect(grainTableIssue('aomAv1', 'av1an', timed, draft, true)).toContain('standalone AOM');
  expect(grainTableIssue('aomAv1', 'av1an', timed, draft, false)).toContain('standalone AOM');
  draft.splitMethod = 'fixedChunks';
  draft.maximumChunkFrames = 0;
  expect(grainTableIssue('aomAv1', 'av1an', timed, draft, false)).toBeNull();
  expect(grainTableIssue('aomAv1', 'av1an', timed, draft, true)).toContain('standalone AOM');
});

test('a grain table that clearly ends before the video is refused in the form', () => {
  const draft = defaultAv1an(false);
  const table = (end: string) => `filmgrn1\nE 0 ${end} 1 2 1\n`;
  // 100 s of video: the margin is 2 %, so a table ending at 98 s is left to native validation.
  expect(grainTableIssue('svtAv1Hdr', 'av1an', table('980000000'), draft, true, 100)).toBeNull();
  const short = grainTableIssue('svtAv1Hdr', 'av1an', table('979999999'), draft, true, 100);
  expect(short).toContain('ends at 98.000 s but the video needs about 100.000 s');
  expect(short).toContain('Thorough verification');
  // Short clips keep a margin of one second.
  expect(grainTableIssue('svtAv1', 'standalone', table('10000000'), draft, true, 2)).toBeNull();
  expect(grainTableIssue('svtAv1', 'standalone', table('10000000'), draft, true, 2.1)).toContain(
    'needs about 2.100 s',
  );
  // Open-ended stamps and an unknown video length are never refused here.
  for (const end of ['9223372036854775807', '18446744073709551615'])
    expect(grainTableIssue('svtAv1Hdr', 'av1an', table(end), draft, true, 7200)).toBeNull();
  for (const length of [undefined, null, 0, Number.NaN])
    expect(grainTableIssue('svtAv1Hdr', 'av1an', table('10'), draft, true, length)).toBeNull();
  // Standalone AOM honors timed tables, so it needs no coverage; chunked AOM does.
  expect(grainTableIssue('aomAv1', 'standalone', table('10'), draft, false, 7200)).toBeNull();
  expect(grainTableIssue('aomAv1', 'av1an', table('10'), draft, false, 7200)).toContain(
    'needs about 7200.000 s',
  );
});
