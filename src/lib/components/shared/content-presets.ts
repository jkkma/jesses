import definitions from '$lib/content-presets.json';
import type { EncoderParameter, EncoderParameterCatalog, VideoEncoder } from '$lib/ipc/generated';
import { parameterError } from './encoder-parameters';

export function contentPresets(encoder: VideoEncoder) {
  if (encoder === 'x264') return definitions.x264;
  if (encoder === 'x265' || encoder === 'x265Standalone') return definitions.x265;
  if (encoder.startsWith('svtAv1')) return definitions.svtAv1;
  return [];
}

/** Only apply rows accepted by the installed encoder; show every omitted row. */
export function applicableContentPreset(
  parameters: EncoderParameter[],
  catalog: EncoderParameterCatalog,
  encoder: VideoEncoder,
) {
  const accepted: EncoderParameter[] = [];
  const skipped: string[] = [];
  for (const original of parameters) {
    const spec = catalog.parameters.find((item) => item.name === original.name);
    const value =
      spec && ['pairWhole', 'pairDecimal'].includes(spec.valueKind)
        ? original.value.replace(',', ':')
        : original.value;
    const candidate = { name: original.name, value };
    if (spec && !parameterError([candidate], catalog, encoder)) accepted.push(candidate);
    else skipped.push(original.name);
  }
  return { accepted, skipped };
}
