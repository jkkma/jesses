import type { Av1anOptions, Av1anTargetMetric, Av1anTargetQuality } from '$lib/ipc/generated';
export type Av1anDraft = Omit<Av1anOptions, 'targetQuality'> & {
  targetEnabled: boolean;
  target: Av1anTargetQuality;
};
export function defaultAv1an(): Av1anDraft {
  return {
    chunkMethod: 'lsmash',
    splitMethod: 'sceneDetection',
    sceneDetection: 'standard',
    maximumChunkFrames: 240,
    minimumSceneFrames: 24,
    sceneDownscaleHeight: 360,
    chunkOrder: 'longToShort',
    targetEnabled: false,
    target: {
      metric: 'vmaf',
      minimumScoreTenths: 940,
      maximumScoreTenths: 960,
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
const whole = (value: number, low: number, high: number) =>
  Number.isInteger(value) && value >= low && value <= high;
export function av1anError(draft: Av1anDraft, hdr = false): string | null {
  if (
    !whole(draft.maximumChunkFrames, 0, 100000) ||
    !whole(draft.minimumSceneFrames, 1, 100000) ||
    (draft.maximumChunkFrames > 0 && draft.minimumSceneFrames > draft.maximumChunkFrames) ||
    (draft.sceneDownscaleHeight !== null &&
      (!whole(draft.sceneDownscaleHeight, 64, 4320) || draft.sceneDownscaleHeight % 2 !== 0))
  )
    return 'Use valid whole frame counts and an even scene height. Minimum scene length cannot exceed an enabled chunk limit.';
  if (!draft.targetEnabled) return null;
  if (hdr)
    return 'Quality targeting currently requires SDR. These metric pipelines are not qualified for HDR sources.';
  const q = draft.target;
  if (
    !whole(q.minimumScoreTenths, 0, 1000) ||
    !whole(q.maximumScoreTenths, q.minimumScoreTenths, 1000) ||
    !whole(q.minimumCrf, 1, 63) ||
    !whole(q.maximumCrf, q.minimumCrf, 63) ||
    !whole(q.probes, 1, 10) ||
    !whole(q.probingRate, 1, 4) ||
    ![q.probeWidth, q.probeHeight].every((size) => whole(size, 128, 8192) && size % 2 === 0)
  )
    return 'Use ordered target scores from 0–100, CRF bounds from 1–63, 1–10 probes, sampling from 1–4, and even evaluation dimensions from 128–8192.';
  if (
    (q.metric === 'ssimulacra2' ||
      q.metric === 'butteraugli' ||
      (q.metric === 'xpsnr' && q.probingRate > 1)) &&
    ['select', 'hybrid'].includes(draft.chunkMethod)
  )
    return 'This scorer needs L-SMASH, FFMS2, or BestSource. XPSNR with every frame can use FFmpeg select or hybrid.';
  return null;
}
export function selectedAv1an(draft: Av1anDraft): Av1anOptions {
  const { targetEnabled, target, ...options } = draft;
  return { ...options, ...(targetEnabled ? { targetQuality: { ...target } } : {}) };
}
export function av1anSummary(options?: Av1anOptions): string {
  if (!options) return '';
  const q = options.targetQuality;
  return ` · ${options.chunkMethod} · ${options.splitMethod === 'fixedChunks' ? 'fixed chunks' : `${options.sceneDetection} scenes`} · ${options.maximumChunkFrames ? `${options.maximumChunkFrames}f maximum` : 'unlimited chunks'}${q ? ` · ${metricName(q.metric)} ${(q.minimumScoreTenths / 10).toFixed(1)}–${(q.maximumScoreTenths / 10).toFixed(1)} probes` : ''}`;
}

export function metricName(metric: Av1anTargetMetric = 'vmaf'): string {
  return {
    vmaf: 'VMAF',
    ssimulacra2: 'SSIMULACRA2',
    butteraugli: 'Butteraugli INF',
    xpsnr: 'XPSNR (dB)',
  }[metric];
}
export function metricDefaults(
  metric: Av1anTargetMetric,
): Pick<Av1anTargetQuality, 'metric' | 'minimumScoreTenths' | 'maximumScoreTenths'> {
  const ranges = {
    vmaf: [940, 960],
    ssimulacra2: [700, 800],
    butteraugli: [8, 12],
    xpsnr: [300, 350],
  };
  const [minimumScoreTenths, maximumScoreTenths] = ranges[metric];
  return { metric, minimumScoreTenths, maximumScoreTenths };
}
