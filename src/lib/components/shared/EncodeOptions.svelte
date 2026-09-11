<script lang="ts">
  import type { EncodeBackend } from '$lib/ipc/generated';

  let {
    idPrefix,
    disabled = false,
    backend = $bindable<EncodeBackend>('svtAv1'),
    workers = $bindable<number | undefined>(2),
    filmGrain = $bindable<number | undefined>(0),
    hdr10Fallback = $bindable(false),
  }: {
    idPrefix: string;
    disabled?: boolean;
    backend?: EncodeBackend;
    workers?: number;
    filmGrain?: number;
    hdr10Fallback?: boolean;
  } = $props();
</script>

<div class="field full-width">
  <label for={`${idPrefix}-backend`}>Encode backend</label>
  <select id={`${idPrefix}-backend`} bind:value={backend} {disabled}>
    <option value="svtAv1">Standalone SVT-AV1</option>
    <option value="av1an">av1an · SVT-AV1 chunks</option>
  </select>
  <p>Both use SVT-AV1. av1an splits the video into chunks that can encode in parallel.</p>
</div>
{#if backend === 'av1an'}
  <div class="field full-width av1an-details">
    <p>
      av1an supports the first video track only. It detects scenes and uses chunks of at most 240
      frames. Requires FFmpeg, FFprobe, standalone SVT-AV1, av1an, and VapourSynth with the L-SMASH
      Works source plugin. Tool availability does not confirm the source plugin; the runtime checks
      it before encoding.
    </p>
    <p>
      Work files and caches stay in a job workspace inside the output folder. If av1an is canceled
      or fails, remaining work files are retained and their location is recorded in the job log.
      Jobs never resume automatically.
    </p>
  </div>
  <div class="field full-width">
    <label for={`${idPrefix}-workers`}>Parallel chunks</label>
    <input
      id={`${idPrefix}-workers`}
      type="number"
      min="1"
      max="32"
      step="1"
      bind:value={workers}
      {disabled}
    />
    <p>1–32 workers · More parallel chunks use more CPU and memory.</p>
  </div>
{/if}
<div class="field full-width">
  <label for={`${idPrefix}-grain`}>Film grain synthesis</label>
  <input
    id={`${idPrefix}-grain`}
    type="number"
    min="0"
    max="50"
    step="1"
    bind:value={filmGrain}
    {disabled}
  />
  <p>
    0 keeps synthesis off and encodes source texture. 1–50 adds synthesized grain with encoder
    denoising disabled. This does not exactly restore the original grain.
  </p>
</div>
<div class="field full-width">
  <label class="fallback-choice" for={`${idPrefix}-hdr-fallback`}>
    <input
      id={`${idPrefix}-hdr-fallback`}
      type="checkbox"
      bind:checked={hdr10Fallback}
      {disabled}
    />
    <span>Allow HDR10 fallback</span>
  </label>
  <p>
    Preserve static HDR in HDR10 output. Checking this explicitly allows Dolby Vision and HDR10+
    dynamic metadata to be discarded when a compatible HDR10 base is available. Unsupported HDR
    sources are rejected.
  </p>
</div>

<style>
  .av1an-details p + p {
    margin-top: 10px;
  }
  .fallback-choice {
    display: flex;
    align-items: flex-start;
    gap: 10px;
  }
  .fallback-choice input {
    width: 16px;
    height: 16px;
    flex: 0 0 auto;
    accent-color: #ad5326;
    margin-top: 1px;
  }
</style>
