<script lang="ts">
  import type { Av1anPixelFormat, VideoEncoder } from '$lib/ipc/generated';
  import { allowedOutputPixelFormats, outputPixelFormats } from './output-pixel-format';

  let {
    id,
    encoder,
    value,
    disabled = false,
    onchange,
  }: {
    id: string;
    encoder: VideoEncoder;
    value: Av1anPixelFormat | undefined;
    disabled?: boolean;
    onchange: (value: Av1anPixelFormat | undefined) => void;
  } = $props();
  const choices = $derived(allowedOutputPixelFormats(encoder));
</script>

<div class="field">
  <label for={id}>Output pixel format</label>
  <select
    {id}
    value={value ?? ''}
    {disabled}
    onchange={(event) =>
      onchange(
        event.currentTarget.value ? (event.currentTarget.value as Av1anPixelFormat) : undefined,
      )}
  >
    <option value="">Source/default</option>
    {#each outputPixelFormats.filter(({ value }) => choices.includes(value)) as choice}
      <option value={choice.value}>{choice.label}</option>
    {/each}
  </select>
  <p>Choose chroma and bit depth explicitly when the installed encoder supports them.</p>
  {#if value === 'yuva420p'}<p>
      Requires an alpha-bearing RGBA or YUVA source. Transparent VP9 sources are decoded with
      libvpx.
    </p>{/if}
</div>
