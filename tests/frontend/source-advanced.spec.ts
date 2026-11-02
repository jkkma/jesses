import { expect, test } from '@playwright/test';
import { parseSourceHdr } from '../../src/lib/components/shared/source-advanced';
import {
  av1anError,
  defaultAv1an,
  selectedAv1an,
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
