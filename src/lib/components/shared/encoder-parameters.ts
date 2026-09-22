import type {
  EncoderParameter,
  EncoderParameterCatalog,
  EncoderParameterSpec,
  VideoEncoder,
} from '$lib/ipc/generated';

const x264Names = new Set(
  'tune profile level deblock cqm bframes b-adapt ref weightp rc-lookahead me subme merange direct partitions b-pyramid aq-mode aq-strength psy-rd trellis qcomp chroma-qp-offset nr vbv-maxrate vbv-bufsize qpmin qpmax qpstep ipratio pbratio crf-max'.split(
    ' ',
  ),
);
const svtNames = new Set(
  'tune aq-mode ac-bias tx-bias sharpness sharp-tx max-tx-size alt-ssim-tuning complex-hvs variance-boost-strength variance-octile variance-boost-curve qp-scale-compress-strength luminance-qp-bias chroma-qm-min chroma-qm-max hbd-mds enable-qm qm-min qm-max enable-variance-boost min-qp max-qp startup-qp-offset enable-dlf enable-tf tf-strength kf-tf-strength noise-adaptive-filtering cdef-scaling enable-cdef enable-restoration noise noise-chroma noise-chroma-from-luma noise-size noise-norm-strength adaptive-film-grain scm fast-decode enable-overlays lookahead hierarchical-levels'.split(
    ' ',
  ),
);

function validCatalogValue(spec: EncoderParameterSpec, value: string): boolean {
  const kind = spec.valueKind || 'whole';
  const minimum = Number(spec.minimumValue || spec.minimum);
  const maximum = Number(spec.maximumValue || spec.maximum);
  const whole = (text: string) =>
    /^-?\d{1,10}$/.test(text) && Number(text) >= minimum && Number(text) <= maximum;
  const decimal = (text: string) =>
    /^\d{1,10}(?:\.\d{1,3})?$/.test(text) &&
    Number(text) >= minimum &&
    Number(text) <= maximum &&
    (!(spec.name === 'ipratio' || spec.name === 'pbratio') || Number(text) > 1);
  if (kind === 'choice') return spec.choices.includes(value);
  if (kind === 'choiceList') {
    const parts = value.split(',');
    return (
      parts.length > 0 &&
      parts.length <= spec.choices.length &&
      new Set(parts).size === parts.length &&
      parts.every((part) => spec.choices.includes(part)) &&
      (spec.name !== 'partitions' ||
        parts.length === 1 ||
        (!parts.includes('all') && !parts.includes('none')))
    );
  }
  if (kind === 'pairWhole' || kind === 'pairDecimal') {
    const parts = value.split(':');
    return parts.length === 2 && parts.every(kind === 'pairWhole' ? whole : decimal);
  }
  return kind === 'decimal' ? decimal(value) : whole(value);
}

const x264Choices: Record<string, string[]> = {
  tune: 'film animation grain stillimage psnr ssim fastdecode zerolatency'.split(' '),
  profile: 'baseline main high high10 high422 high444'.split(' '),
  level: '1 1b 1.1 1.2 1.3 2 2.1 2.2 3 3.1 3.2 4 4.1 4.2 5 5.1 5.2 6 6.1 6.2'.split(' '),
  cqm: ['flat', 'jvt'],
  me: 'dia hex umh esa tesa'.split(' '),
  direct: 'spatial temporal auto none'.split(' '),
  partitions: 'p8x8 b8x8 i8x8 i4x4 p4x4 all none'.split(' '),
  'b-pyramid': 'none strict normal'.split(' '),
};
const x264Ranges: Record<string, [number, number]> = {
  bframes: [0, 16],
  'b-adapt': [0, 2],
  ref: [1, 16],
  weightp: [0, 2],
  'rc-lookahead': [0, 250],
  subme: [0, 11],
  merange: [4, 1024],
  'aq-mode': [0, 3],
  'aq-strength': [0, 3],
  trellis: [0, 2],
  qcomp: [0, 1],
  'chroma-qp-offset': [-12, 12],
  nr: [0, 100000],
  'vbv-maxrate': [0, 1000000],
  'vbv-bufsize': [0, 1000000],
  qpmin: [0, 81],
  qpmax: [0, 81],
  qpstep: [0, 81],
  ipratio: [1, 10],
  pbratio: [1, 10],
  'crf-max': [0, 51],
};
const svtRanges: Record<string, [number, number]> = {
  tune: [0, 5],
  'aq-mode': [0, 2],
  'ac-bias': [0, 8],
  'tx-bias': [0, 3],
  sharpness: [-7, 7],
  'sharp-tx': [0, 1],
  'alt-ssim-tuning': [0, 1],
  'complex-hvs': [0, 1],
  'variance-boost-strength': [1, 4],
  'variance-octile': [1, 8],
  'variance-boost-curve': [0, 3],
  'qp-scale-compress-strength': [0, 8],
  'luminance-qp-bias': [0, 100],
  'chroma-qm-min': [0, 15],
  'chroma-qm-max': [0, 15],
  'hbd-mds': [0, 2],
  'enable-qm': [0, 1],
  'qm-min': [0, 15],
  'qm-max': [0, 15],
  'enable-variance-boost': [0, 1],
  'min-qp': [0, 63],
  'max-qp': [0, 63],
  'startup-qp-offset': [-63, 63],
  'enable-dlf': [0, 2],
  'enable-tf': [0, 2],
  'tf-strength': [0, 4],
  'kf-tf-strength': [0, 4],
  'noise-adaptive-filtering': [0, 4],
  'cdef-scaling': [1, 30],
  'enable-cdef': [0, 1],
  'enable-restoration': [0, 1],
  noise: [0, 200],
  'noise-chroma': [-1, 200],
  'noise-chroma-from-luma': [0, 1],
  'noise-size': [-1, 13],
  'noise-norm-strength': [0, 4],
  'adaptive-film-grain': [0, 1],
  scm: [0, 3],
  'fast-decode': [0, 2],
  'enable-overlays': [0, 1],
  lookahead: [0, 120],
  'hierarchical-levels': [3, 5],
};
function validNativeFallback(encoder: VideoEncoder, name: string, value: string): boolean {
  const x264 = encoder === 'x264';
  const choices = x264 ? x264Choices[name] : name === 'max-tx-size' ? ['32', '64'] : undefined;
  if (choices) {
    const parts = (name === 'tune' && x264) || name === 'partitions' ? value.split(',') : [value];
    return (
      parts.length > 0 &&
      parts.length <= choices.length &&
      new Set(parts).size === parts.length &&
      parts.every((part) => choices.includes(part)) &&
      (name !== 'partitions' ||
        parts.length === 1 ||
        (!parts.includes('all') && !parts.includes('none')))
    );
  }
  const range = x264 ? x264Ranges[name] : svtRanges[name];
  if (name === 'deblock' || name === 'psy-rd') {
    const parts = value.split(':');
    const [min, max] = name === 'deblock' ? [-6, 6] : [0, 10];
    const pattern = name === 'deblock' ? /^-?\d{1,10}$/ : /^\d{1,10}(?:\.\d{1,3})?$/;
    return (
      parts.length === 2 &&
      parts.every((part) => pattern.test(part) && Number(part) >= min && Number(part) <= max)
    );
  }
  if (!range) return false;
  const decimal = x264
    ? ['aq-strength', 'qcomp', 'ipratio', 'pbratio', 'crf-max'].includes(name)
    : ['ac-bias', 'qp-scale-compress-strength'].includes(name);
  const pattern = decimal ? /^\d{1,10}(?:\.\d{1,3})?$/ : /^-?\d{1,10}$/;
  return (
    pattern.test(value) &&
    Number(value) >= range[0] &&
    Number(value) <= range[1] &&
    (!(name === 'ipratio' || name === 'pbratio') || Number(value) > 1)
  );
}

export function parameterError(
  values: EncoderParameter[],
  catalog?: EncoderParameterCatalog | null,
  encoder?: VideoEncoder,
): string | null {
  const native =
    encoder === 'x264' ||
    encoder?.startsWith('svtAv1') ||
    catalog?.encoder === 'x264' ||
    catalog?.encoder.startsWith('svtAv1');
  const maximum = native ? 64 : 16;
  if (values.length > maximum || new Set(values.map((value) => value.name)).size !== values.length)
    return `Choose each parameter once, with at most ${maximum} overrides.`;
  for (const value of values) {
    if (catalog) {
      const spec = catalog.parameters.find((spec) => spec.name === value.name);
      if (!spec) return `${value.name} is unavailable in this installed encoder catalog.`;
      if (!validCatalogValue(spec, value.value))
        return `${spec.label} needs ${spec.choices.length ? spec.choices.join(', ') : `${spec.minimumValue || spec.minimum} through ${spec.maximumValue || spec.maximum}`}.`;
      continue;
    }
    if (encoder === 'x264' || encoder?.startsWith('svtAv1')) {
      const names = encoder === 'x264' ? x264Names : svtNames;
      if (!names.has(value.name))
        return `${value.name} is outside this encoder's qualified catalog.`;
      if (!validNativeFallback(encoder, value.name, value.value))
        return `${value.name} has an invalid value for this encoder.`;
      continue;
    }
    if (!/^\d{1,4}$/.test(value.value)) return `${value.name} needs a whole number.`;
    if (encoder) {
      const ranges: Record<string, number[]> =
        encoder === 'x265'
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
            : {
                'aq-mode': [0, 2],
                'lag-in-frames': [0, 25],
                'auto-alt-ref': [0, 1],
                bf: [0, 4],
                'rc-lookahead': [0, 32],
                'spatial-aq': [0, 1],
                'temporal-aq': [0, 1],
                ref: [1, 6],
                bframes: [0, 16],
                'b-adapt': [0, 2],
                sao: [0, 1],
                cutree: [0, 1],
              };
      const range = ranges[value.name];
      if (!range || Number(value.value) < range[0] || Number(value.value) > range[1])
        return `${value.name} is outside this encoder's qualified range.`;
    }
  }
  return null;
}

export function parameterSummary(values: EncoderParameter[] | undefined): string {
  return values?.length
    ? ` · Overrides: ${values.map((value) => `${value.name}=${value.value}`).join(', ')}`
    : '';
}
