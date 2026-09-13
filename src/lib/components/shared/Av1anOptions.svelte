<script lang="ts">
  import { av1anError, metricName, metricDefaults, type Av1anDraft } from './av1an-options';
  let {
    idPrefix,
    draft,
    disabled,
    hdr = false,
    framed = false,
    onchange,
  }: {
    idPrefix: string;
    draft: Av1anDraft;
    disabled: boolean;
    hdr?: boolean;
    framed?: boolean;
    onchange: (value: Av1anDraft) => void;
  } = $props();
  function number(event: Event) {
    return (event.currentTarget as HTMLInputElement).valueAsNumber;
  }
  const frameFields = [
    { key: 'maximumChunkFrames', label: 'Maximum chunk frames (0 disables)', min: 0, max: 100000 },
    { key: 'minimumSceneFrames', label: 'Minimum scene frames', min: 1, max: 100000 },
  ] as const;
  const probeFields = [
    { key: 'minimumCrf', label: 'Minimum probe CRF', min: 1, max: 63 },
    { key: 'maximumCrf', label: 'Maximum probe CRF', min: 1, max: 63 },
    { key: 'probes', label: 'Probes per chunk', min: 1, max: 10 },
    { key: 'probingRate', label: 'Probe every N frames', min: 1, max: 4 },
    { key: 'probeWidth', label: 'Metric evaluation width', min: 128, max: 8192 },
    { key: 'probeHeight', label: 'Metric evaluation height', min: 128, max: 8192 },
  ] as const;
</script>

<fieldset {disabled} class="av1an-options">
  <legend>Scenes and quality targeting</legend>
  <label for={`${idPrefix}-chunk-method`}>Source reader</label>
  <select
    id={`${idPrefix}-chunk-method`}
    value={draft.chunkMethod}
    onchange={(event) =>
      onchange({ ...draft, chunkMethod: event.currentTarget.value as Av1anDraft['chunkMethod'] })}
  >
    <option value="lsmash">L-SMASH Works</option><option value="ffms2">FFMS2</option><option
      value="bestsource">BestSource</option
    ><option value="select">FFmpeg select</option><option value="hybrid">Hybrid segments</option>
  </select>
  <p>
    The selected VapourSynth reader needs its plugin. FFmpeg select and hybrid require the corrected
    av1an build. Hybrid independently verifies its decoded segment sequence against the original
    source before reuse or publication.
  </p>
  <label for={`${idPrefix}-split-method`}>Split method</label>
  <select
    id={`${idPrefix}-split-method`}
    value={draft.splitMethod}
    onchange={(event) =>
      onchange({ ...draft, splitMethod: event.currentTarget.value as Av1anDraft['splitMethod'] })}
    ><option value="sceneDetection">Scene detection</option><option value="fixedChunks"
      >Fixed chunks</option
    ></select
  >
  {#if draft.splitMethod === 'sceneDetection'}
    <label for={`${idPrefix}-scene-detection`}>Scene detector</label>
    <select
      id={`${idPrefix}-scene-detection`}
      value={draft.sceneDetection}
      onchange={(event) =>
        onchange({
          ...draft,
          sceneDetection: event.currentTarget.value as Av1anDraft['sceneDetection'],
        })}><option value="standard">Standard</option><option value="fast">Fast</option></select
    >
    <label for={`${idPrefix}-scene-height`}>Scene detection height (blank uses source)</label>
    <input
      id={`${idPrefix}-scene-height`}
      type="number"
      min="64"
      max="4320"
      step="2"
      value={draft.sceneDownscaleHeight ?? ''}
      oninput={(event) =>
        onchange({
          ...draft,
          sceneDownscaleHeight: event.currentTarget.value === '' ? null : number(event),
        })}
    />
  {/if}
  {#each frameFields as field}<label for={`${idPrefix}-${field.key}`}>{field.label}</label><input
      id={`${idPrefix}-${field.key}`}
      type="number"
      min={field.min}
      max={field.max}
      step="1"
      value={draft[field.key]}
      oninput={(event) => onchange({ ...draft, [field.key]: number(event) })}
    />{/each}
  <label for={`${idPrefix}-chunk-order`}>Chunk order</label>
  <select
    id={`${idPrefix}-chunk-order`}
    value={draft.chunkOrder}
    onchange={(event) =>
      onchange({ ...draft, chunkOrder: event.currentTarget.value as Av1anDraft['chunkOrder'] })}
    ><option value="longToShort">Longest first</option><option value="shortToLong"
      >Shortest first</option
    ><option value="sequential">Source order</option><option value="random">Random</option></select
  >
  <label class="check"
    ><input
      type="checkbox"
      checked={draft.targetEnabled}
      onchange={(event) => onchange({ ...draft, targetEnabled: event.currentTarget.checked })}
    />Target perceptual quality</label
  >
  {#if draft.targetEnabled}
    <label for={`${idPrefix}-target-metric`}>Target metric</label>
    <select
      id={`${idPrefix}-target-metric`}
      value={draft.target.metric ?? 'vmaf'}
      onchange={(event) =>
        onchange({
          ...draft,
          target: {
            ...draft.target,
            ...metricDefaults(event.currentTarget.value as Av1anDraft['target']['metric']),
          },
        })}
    >
      <option value="vmaf">VMAF v0.6.1</option>
      <option value="ssimulacra2">SSIMULACRA2</option>
      <option value="butteraugli">Butteraugli INF</option>
      <option value="xpsnr">XPSNR minimum Y/U/V (dB)</option>
    </select>
    <p>
      {draft.target.metric === 'butteraugli'
        ? 'Lower scores mean fewer visible differences. The range remains ordered from its smaller to larger value.'
        : 'Higher scores mean closer agreement with the source.'} Changing the metric resets its score
      range.
    </p>
    {#each [{ key: 'minimumScoreTenths', label: `Minimum ${metricName(draft.target.metric)} score` }, { key: 'maximumScoreTenths', label: `Maximum ${metricName(draft.target.metric)} score` }] as field}<label
        for={`${idPrefix}-${field.key}`}>{field.label}</label
      ><input
        id={`${idPrefix}-${field.key}`}
        type="number"
        min="0"
        max="100"
        step="0.1"
        value={draft.target[field.key as 'minimumScoreTenths' | 'maximumScoreTenths'] / 10}
        oninput={(event) =>
          onchange({
            ...draft,
            target: { ...draft.target, [field.key]: Math.round(number(event) * 10) },
          })}
      />{/each}
    {#each probeFields as field}<label for={`${idPrefix}-${field.key}`}>{field.label}</label><input
        id={`${idPrefix}-${field.key}`}
        type="number"
        min={field.min}
        max={field.max}
        step={field.key === 'probeWidth' || field.key === 'probeHeight' ? 2 : 1}
        value={draft.target[field.key]}
        oninput={(event) =>
          onchange({ ...draft, target: { ...draft.target, [field.key]: number(event) } })}
      />{/each}
    <p>
      av1an chooses a CRF per chunk using mean {metricName(draft.target.metric)} and the selected encoder
      preset. The score can finish outside the range when the probe or CRF limits are reached. Probe scores
      do not measure the completed output.
    </p>
    <p>
      {#if draft.target.metric === 'ssimulacra2'}Requires a working vszip or Vship plugin and a
        VapourSynth source reader. Evaluation dimensions resize the scoring pair, while probes
        encode at source resolution.
      {:else if draft.target.metric === 'butteraugli'}Requires Julek with the corrected av1an build,
        or Vship, and a VapourSynth source reader. Scoring uses intensity 203 nits and the infinity
        norm.
      {:else if draft.target.metric === 'xpsnr'}Every-frame scoring uses the selected FFmpeg XPSNR
        filter. Sampled scoring requires vszip R7 or newer and a VapourSynth source reader. Each
        frame uses the minimum Y/U/V score.
      {:else}Requires the selected FFmpeg with a working libvmaf v0.6.1 model.{/if}
    </p>
    {#if framed}<p>
        Probes use the source before crop, resize, and borders. These framing changes are excluded
        from the quality decision.
      </p>{/if}
    {#if draft.target.probingRate > 1}<p>
        Sampling scores only every {draft.target.probingRate} frames.
      </p>{/if}
  {/if}
  {#if av1anError(draft, hdr)}<p role="alert">{av1anError(draft, hdr)}</p>{/if}
</fieldset>

<style>
  .av1an-options {
    display: grid;
    gap: 0.55rem;
    grid-column: 1 / -1;
    padding: 0.85rem;
    border: 1px solid var(--border);
    border-radius: 0.5rem;
  }
  legend,
  label {
    font-size: 0.82rem;
    font-weight: 600;
  }
  select,
  input[type='number'] {
    min-height: 2.35rem;
    padding: 0.45rem 0.6rem;
    color: inherit;
    background: var(--background);
    border: 1px solid var(--border);
    border-radius: 0.35rem;
  }
  .check {
    display: flex;
    align-items: center;
    gap: 0.5rem;
  }
  p {
    margin: 0;
    color: var(--muted-foreground);
    font-size: 0.77rem;
    line-height: 1.5;
  }
  [role='alert'] {
    color: var(--destructive);
  }
</style>
