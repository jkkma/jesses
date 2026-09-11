<script lang="ts">
  import type { EncodeBackend, VideoEncoder } from '$lib/ipc/generated';
  import { encoderOptions, isSvtEncoder, type HdrTune } from './encoder-options';

  let {
    idPrefix,
    disabled = false,
    allowBackendSelection = true,
    backend = $bindable<EncodeBackend>('standalone'),
    encoder = 'svtAv1',
    workers = $bindable<number | undefined>(2),
    filmGrain = $bindable<number | undefined>(0),
    hdr10Fallback = $bindable(false),
    lineartPsyBias = $bindable<number | undefined>(0),
    texturePsyBias = $bindable<number | undefined>(0),
    hdrTune = $bindable<HdrTune>('visualQuality'),
  }: {
    idPrefix: string;
    disabled?: boolean;
    allowBackendSelection?: boolean;
    backend?: EncodeBackend;
    encoder?: VideoEncoder;
    workers?: number;
    filmGrain?: number;
    hdr10Fallback?: boolean;
    lineartPsyBias?: number;
    texturePsyBias?: number;
    hdrTune?: HdrTune;
  } = $props();
</script>

{#if allowBackendSelection}
  <div class="field full-width">
    <label for={`${idPrefix}-backend`}>Encode backend</label>
    <select id={`${idPrefix}-backend`} bind:value={backend} {disabled}>
      <option value="standalone">Standalone encoder</option>
      <option value="av1an">av1an · SVT-AV1 chunks</option>
    </select>
    <p>Standalone uses the selected encoder. av1an runs the selected SVT-AV1 build in parallel.</p>
  </div>
{/if}
{#if backend === 'av1an'}
  <div class="field full-width av1an-details">
    <p>
      av1an supports the first video track only. It detects scenes and uses chunks of at most 240
      frames. Requires FFmpeg, FFprobe, standalone {encoderOptions(encoder).name}, av1an, and
      VapourSynth with the L-SMASH Works source plugin. Tool availability does not confirm the
      source plugin; the runtime checks it before encoding.
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
{#if encoder === 'svtAv1FiveFish'}
  <div class="field">
    <label for={`${idPrefix}-lineart-bias`}>Lineart psy bias</label>
    <input
      id={`${idPrefix}-lineart-bias`}
      type="number"
      min="0"
      max="7"
      step="1"
      bind:value={lineartPsyBias}
      {disabled}
    />
    <p>0–7 · 5fish lineart emphasis for anime.</p>
  </div>
  <div class="field">
    <label for={`${idPrefix}-texture-bias`}>Texture psy bias</label>
    <input
      id={`${idPrefix}-texture-bias`}
      type="number"
      min="0"
      max="7"
      step="1"
      bind:value={texturePsyBias}
      {disabled}
    />
    <p>0–7 · 5fish texture emphasis.</p>
  </div>
{/if}
{#if encoder === 'svtAv1Hdr'}
  <div class="field full-width">
    <label for={`${idPrefix}-hdr-tune`}>HDR tune</label>
    <select id={`${idPrefix}-hdr-tune`} bind:value={hdrTune} {disabled}>
      <option value="filmGrain">Film grain</option>
      <option value="visualQuality">Visual quality</option>
    </select>
    <p>
      Choose the HDR build's tuning for the source. Film grain tuning does not enable grain
      synthesis or HDR10 fallback.
    </p>
  </div>
{/if}
{#if isSvtEncoder(encoder)}<div class="field full-width">
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
{/if}

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
