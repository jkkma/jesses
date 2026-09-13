import type { EncoderParameter, EncoderParameterCatalog, VideoEncoder } from '$lib/ipc/generated';

export function parameterError(
  values: EncoderParameter[],
  catalog?: EncoderParameterCatalog | null,
  encoder?: VideoEncoder,
): string | null {
  if (values.length > 16 || new Set(values.map((value) => value.name)).size !== values.length)
    return 'Choose each parameter once, with at most 16 overrides.';
  for (const value of values) {
    if (!/^\d{1,4}$/.test(value.value)) return `${value.name} needs a whole number.`;
    if (!catalog && encoder) {
      const svt = {
        'aq-mode': [0, 2],
        'enable-tf': [0, 1],
        'enable-overlays': [0, 1],
        'hierarchical-levels': [3, 5],
      };
      const ranges: Record<string, number[]> =
        encoder === 'x264'
          ? {
              ref: [1, 6],
              bframes: [0, 16],
              'b-adapt': [0, 2],
              'aq-mode': [0, 3],
              trellis: [0, 2],
              'rc-lookahead': [0, 100],
            }
          : encoder === 'x265'
            ? {
                ref: [1, 6],
                bframes: [0, 16],
                'b-adapt': [0, 2],
                'aq-mode': [0, 4],
                sao: [0, 1],
                cutree: [0, 1],
              }
            : encoder === 'vp9'
              ? { 'aq-mode': [0, 4], 'lag-in-frames': [0, 25], 'auto-alt-ref': [0, 1] }
              : svt;
      const range = ranges[value.name];
      if (!range || Number(value.value) < range[0] || Number(value.value) > range[1])
        return `${value.name} is outside this encoder's qualified range.`;
    }
    if (catalog) {
      const spec = catalog.parameters.find((spec) => spec.name === value.name);
      if (!spec) return `${value.name} is unavailable in this installed encoder catalog.`;
      const number = Number(value.value);
      if (number < spec.minimum || number > spec.maximum)
        return `${spec.label} must be from ${spec.minimum} through ${spec.maximum}.`;
    }
  }
  return null;
}

export function parameterSummary(values: EncoderParameter[] | undefined): string {
  return values?.length
    ? ` · Overrides: ${values.map((value) => `${value.name}=${value.value}`).join(', ')}`
    : '';
}
