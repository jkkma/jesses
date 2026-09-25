import type { EncodeBackend, MediaStream, ToneMapSettings } from '$lib/ipc/generated';

export type ToneMapDraft = {
  enabled: boolean;
  algorithm: 'hable' | 'mobius' | 'reinhard' | 'spline';
  backend: 'auto' | 'cpu' | 'gpu';
  peakMode: 'manual' | 'measured';
  sourcePeakNits: number | undefined;
  hdr10BaseLayer: boolean;
};
export const defaultToneMap = (): ToneMapDraft => ({
  enabled: false,
  algorithm: 'hable',
  backend: 'auto',
  peakMode: 'measured',
  sourcePeakNits: 1000,
  hdr10BaseLayer: false,
});
export function selectedToneMap(draft: ToneMapDraft): ToneMapSettings | undefined {
  return draft.enabled
    ? {
        ...(draft.algorithm !== 'hable' ? { algorithm: draft.algorithm } : {}),
        ...(draft.backend !== 'cpu' ? { backend: draft.backend } : {}),
        ...(draft.peakMode !== 'manual' ? { peakMode: draft.peakMode } : {}),
        sourcePeakNits: draft.sourcePeakNits!,
        hdr10BaseLayer: draft.hdr10BaseLayer,
      }
    : undefined;
}
export function toneMapError(
  draft: ToneMapDraft,
  backend: EncodeBackend,
  video: MediaStream | undefined,
  hdr10Fallback = false,
): string | null {
  if (!draft.enabled) return null;
  if (!['hable', 'mobius', 'reinhard', 'spline'].includes(draft.algorithm))
    return 'Choose a supported tone mapping curve.';
  if (!['auto', 'cpu', 'gpu'].includes(draft.backend))
    return 'Choose Auto, CPU or GPU tone mapping.';
  if (!['manual', 'measured'].includes(draft.peakMode))
    return 'Choose manual or measured signal peak.';
  if (backend === 'av1an' && draft.backend === 'gpu')
    return 'av1an uses CPU tone mapping. Choose Auto or CPU.';
  if (draft.backend === 'gpu' && draft.peakMode !== 'measured')
    return 'GPU tone mapping requires measured peak detection.';
  if (draft.algorithm === 'spline' && (draft.backend === 'cpu' || backend === 'av1an'))
    return 'Spline requires standalone Auto or GPU tone mapping.';
  if (draft.algorithm === 'spline' && draft.peakMode !== 'measured')
    return 'Spline requires measured peak detection on Auto or GPU.';
  if (
    typeof draft.sourcePeakNits !== 'number' ||
    !Number.isInteger(draft.sourcePeakNits) ||
    draft.sourcePeakNits < 100 ||
    draft.sourcePeakNits > 10000
  )
    return 'Enter a signal peak from 100 to 10000 nits.';
  if (hdr10Fallback)
    return 'Turn off HDR10 output fallback and use the tone mapping base-layer option when needed.';
  const dolbyVisionProfile = video?.dolbyVisionProfile;
  if (dolbyVisionProfile === 5 && draft.hdr10BaseLayer)
    return 'Dolby Vision profile 5 has no HDR10 base layer. Turn off the base-layer option.';
  if (dolbyVisionProfile === 5 && (backend === 'av1an' || draft.backend === 'cpu'))
    return 'Dolby Vision profile 5 requires standalone Auto or GPU rendering with a capable Vulkan/libplacebo route.';
  if (dolbyVisionProfile === 5 && draft.peakMode !== 'measured')
    return 'Dolby Vision profile 5 requires measured peak detection.';
  if (dolbyVisionProfile === 5) {
    if (!['hevc', 'h265'].includes(video?.codec ?? '') || video?.pixelFormat !== 'yuv420p10le')
      return 'Dolby Vision profile 5 rendering requires a 10-bit 4:2:0 HEVC source.';
    if (video?.colorRange && video.colorRange !== 'pc' && video.colorRange !== 'unknown')
      return 'Dolby Vision profile 5 rendering requires full-range input.';
    return null;
  }
  if (
    !video ||
    video.colorPrimaries !== 'bt2020' ||
    video.colorSpace !== 'bt2020nc' ||
    video.colorRange !== 'tv' ||
    video.pixelFormat !== 'yuv420p10le' ||
    !['smpte2084', 'arib-std-b67'].includes(video.colorTransfer ?? '')
  )
    return 'Tone mapping requires tagged limited-range 10-bit 4:2:0 BT.2020 PQ/HDR10 or HLG video.';
  if (video.colorTransfer === 'arib-std-b67' && draft.hdr10BaseLayer)
    return 'HDR10 base-layer fallback does not apply to HLG.';
  return null;
}
export function toneMapSummary(tone: ToneMapSettings | undefined | null): string {
  if (!tone) return '';
  const algorithm = { hable: 'Hable', mobius: 'Mobius', reinhard: 'Reinhard', spline: 'Spline' }[
    tone.algorithm ?? 'hable'
  ];
  const route = { auto: 'Auto', cpu: 'CPU', gpu: 'GPU' }[tone.backend ?? 'cpu'];
  return ` · SDR BT.709 · ${algorithm} · ${route} · ${tone.peakMode === 'measured' ? `measured peak (fallback ${tone.sourcePeakNits} nits)` : `${tone.sourcePeakNits} → 100 nits`}${tone.hdr10BaseLayer ? ' · HDR10 base layer' : ''}`;
}
