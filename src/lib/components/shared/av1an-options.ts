import type {
  Av1anGrainSettings,
  Av1anOptions,
  Av1anTargetMetric,
  Av1anTargetQuality,
  EncoderParameter,
  VideoEncoder,
} from '$lib/ipc/generated';
import { readAv1anPreferences } from './av1an-preferences';
export type Av1anDraft = Omit<Av1anOptions, 'targetQuality'> & {
  targetEnabled: boolean;
  target: Av1anTargetQuality;
};
export function defaultAv1an(usePreferences = true): Av1anDraft {
  const saved = usePreferences ? readAv1anPreferences() : null;
  return {
    chunkMethod: saved?.chunkMethod ?? 'lsmash',
    splitMethod: saved?.splitMethod ?? 'sceneDetection',
    sceneDetection: 'standard',
    maximumChunkFrames: 240,
    minimumSceneFrames: 24,
    sceneDownscaleHeight: 360,
    chunkOrder: saved?.chunkOrder ?? 'longToShort',
    ...(saved?.encoderThreads !== undefined ? { encoderThreads: saved.encoderThreads } : {}),
    ...(saved?.sceneDetectionSlices !== undefined
      ? { sceneDetectionSlices: saved.sceneDetectionSlices }
      : {}),
    maxTries: 3,
    concatMethod: saved?.concatMethod ?? 'ffmpeg',
    attachSettings: false,
    targetEnabled: false,
    target: {
      metric: 'vmaf',
      minimumScoreTenths: 950,
      maximumScoreTenths: 950,
      minimumCrf: 15,
      maximumCrf: 50,
      probes: 4,
      probingRate: 1,
      probeWidth: 1920,
      probeHeight: 1080,
    },
  };
}
export const copyAv1an = (draft: Av1anDraft): Av1anDraft => ({
  ...draft,
  target: { ...draft.target },
});
export function av1anGrainConflict(
  parameters: EncoderParameter[],
  filmGrain: number | undefined,
  grain: Av1anGrainSettings | undefined,
): string | null {
  const noise = parameters.find((parameter) => parameter.name === 'noise');
  if (
    noise &&
    Number(noise.value) > 0 &&
    ((filmGrain ?? 0) > 0 || (grain?.table !== null && grain?.table !== undefined))
  )
    return 'Use one grain source: a grain table, film-grain synthesis, or the advanced noise override.';
  return null;
}
const whole = (value: number, low: number, high: number) =>
  Number.isInteger(value) && value >= low && value <= high;
export function av1anError(
  draft: Av1anDraft,
  hdr = false,
  encoder: VideoEncoder = 'svtAv1Hdr',
  attachmentSupported = true,
): string | null {
  if (
    !whole(draft.maximumChunkFrames, 0, 100000) ||
    !whole(draft.minimumSceneFrames, 1, 100000) ||
    (draft.maximumChunkFrames > 0 && draft.minimumSceneFrames > draft.maximumChunkFrames) ||
    (draft.sceneDownscaleHeight !== null &&
      (!whole(draft.sceneDownscaleHeight, 64, 4320) || draft.sceneDownscaleHeight % 2 !== 0))
  )
    return 'Use valid whole frame counts and an even scene height. Minimum scene length cannot exceed an enabled chunk limit.';
  if (
    (draft.encoderThreads !== undefined && !whole(draft.encoderThreads, 0, 64)) ||
    !whole(draft.maxTries ?? 3, 1, 10) ||
    (draft.sceneDetectionSlices !== undefined && !whole(draft.sceneDetectionSlices, 1, 16)) ||
    (draft.concatMethod !== undefined && !['ffmpeg', 'mkvmerge'].includes(draft.concatMethod)) ||
    (draft.attachSettings !== undefined && typeof draft.attachSettings !== 'boolean')
  )
    return 'Use 0–64 encoder threads (0 for automatic), 1–10 attempts, and a listed concat method.';
  if (
    draft.pixelFormat !== undefined &&
    !(
      encoder === 'x264'
        ? ['yuv420p', 'yuv420p10le', 'yuv422p', 'yuv422p10le', 'yuv444p', 'yuv444p10le']
        : ['yuv420p', 'yuv420p10le']
    ).includes(draft.pixelFormat)
  )
    return 'Choose a supported output pixel format for the selected encoder.';
  if (draft.attachSettings && !attachmentSupported)
    return 'Encoding settings can be attached to Matroska output only. Choose MKV or turn off the attachment.';
  if (!draft.targetEnabled) return null;
  if (hdr)
    return 'Quality targeting currently requires SDR. These metric pipelines are not qualified for HDR sources.';
  const q = draft.target;
  const maximumCrf = encoder === 'x264' ? 51 : 63;
  if (
    !whole(q.minimumScoreTenths, 0, 1000) ||
    !whole(q.maximumScoreTenths, q.minimumScoreTenths, 1000) ||
    !whole(q.minimumCrf, 0, maximumCrf) ||
    !whole(q.maximumCrf, q.minimumCrf, maximumCrf) ||
    !whole(q.probes, 1, 10) ||
    !whole(q.probingRate, 1, 4) ||
    ![q.probeWidth, q.probeHeight].every((size) => whole(size, 128, 8192) && size % 2 === 0)
  )
    return `Use ordered target scores from 0–100, CRF bounds from 0–${maximumCrf}, 1–10 probes, sampling from 1–4, and even evaluation dimensions from 128–8192.`;
  if (q.metric === 'butteraugli' && (q.minimumScoreTenths < 5 || q.maximumScoreTenths > 100))
    return 'Butteraugli INF targets use 0.5–10 in tenth-point steps.';
  if (
    q.metric === 'xpsnrWeighted' &&
    (q.minimumScoreTenths < 200 ||
      q.maximumScoreTenths > 600 ||
      q.minimumScoreTenths % 5 !== 0 ||
      q.maximumScoreTenths % 5 !== 0)
  )
    return 'Weighted XPSNR targets use 20–60 dB in half-point steps.';
  if (
    (q.metric === 'ssimulacra2' ||
      q.metric === 'butteraugli' ||
      ((q.metric === 'xpsnr' || q.metric === 'xpsnrWeighted') && q.probingRate > 1)) &&
    ['select', 'hybrid'].includes(draft.chunkMethod)
  )
    return 'This scorer needs L-SMASH, FFMS2, or BestSource. XPSNR with every frame can use FFmpeg select or hybrid.';
  return null;
}
export function selectedAv1an(
  draft: Av1anDraft,
  encoder: VideoEncoder = 'svtAv1Hdr',
): Av1anOptions {
  const { targetEnabled, target, ...options } = draft;
  return {
    ...options,
    ...(encoder === 'x264' ? { concatMethod: 'mkvmerge' as const } : {}),
    ...(targetEnabled ? { targetQuality: { ...target } } : {}),
  };
}
export function av1anSummary(options?: Av1anOptions): string {
  if (!options) return '';
  const q = options.targetQuality;
  return ` · ${options.chunkMethod} · ${options.splitMethod === 'fixedChunks' ? 'fixed chunks' : `${options.sceneDetection} scenes`} · ${options.maximumChunkFrames ? `${options.maximumChunkFrames}f maximum` : 'unlimited chunks'}${options.encoderThreads === undefined ? '' : ` · ${options.encoderThreads === 0 ? 'Auto' : options.encoderThreads} encoder threads`}${q ? ` · ${metricName(q.metric)} ${(q.minimumScoreTenths / 10).toFixed(1)}–${(q.maximumScoreTenths / 10).toFixed(1)} probes` : ''}`;
}

export function metricName(metric: Av1anTargetMetric = 'vmaf'): string {
  return {
    vmaf: 'VMAF',
    ssimulacra2: 'SSIMULACRA2',
    butteraugli: 'Butteraugli INF',
    xpsnr: 'XPSNR (dB)',
    xpsnrWeighted: 'Weighted XPSNR (dB)',
  }[metric];
}
export function metricDefaults(
  metric: Av1anTargetMetric,
): Pick<Av1anTargetQuality, 'metric' | 'minimumScoreTenths' | 'maximumScoreTenths'> {
  const ranges = {
    vmaf: [950, 950],
    ssimulacra2: [800, 800],
    butteraugli: [40, 40],
    xpsnr: [300, 350],
    xpsnrWeighted: [400, 400],
  };
  const [minimumScoreTenths, maximumScoreTenths] = ranges[metric];
  return { metric, minimumScoreTenths, maximumScoreTenths };
}
