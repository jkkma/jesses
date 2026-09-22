<script lang="ts">
  let {
    value,
    disabled = false,
    onchange,
  }: { value: string[]; disabled?: boolean; onchange: (value: string[]) => void } = $props();
  const id = $props.id();
  let text = $state('');
  const rows = (input: string) => input.split('\n').filter((line) => line.trim().length > 0);
  $effect(() => {
    if (JSON.stringify(value) !== JSON.stringify(rows(text))) text = value.join('\n');
  });
</script>

<details class="filters">
  <summary
    >Custom video filters <small>{value.length ? `${value.length} filter rows` : 'Off'}</small
    ></summary
  >
  <div>
    <label for={id}>Pixel filters · one chain per line</label>
    <textarea
      {id}
      rows="4"
      {disabled}
      value={text}
      oninput={(e) => {
        text = e.currentTarget.value;
        onchange(rows(text));
      }}
      placeholder="eq=contrast=1.05:saturation=0.95&#10;unsharp=5:5:0.3"></textarea>
    {#if value.length > 16}<p role="alert">Use at most 16 filter rows.</p>{/if}
    <p>
      Rows run in order after the other picture adjustments. Supported filters include eq, unsharp,
      hqdn3d, nlmeans, deband, deblock, cas, noise, vibrance, colorbalance, colorlevels, curves,
      hue, lut, limiter, negate, normalize, smartblur, gblur and boxblur.
    </p>
    <p>
      Use the crop, resize, trim and frame-rate controls for geometry or timing. Custom filters
      cannot read or write files. A verified lossless intermediate keeps scene detection and quality
      scoring aligned with the filtered encode.
    </p>
  </div>
</details>

<style>
  .filters {
    grid-column: 1/-1;
    border: 1px solid var(--border);
    border-radius: 0.5rem;
    min-width: 0;
  }
  summary {
    padding: 0.75rem;
    font-size: 0.82rem;
    font-weight: 600;
    cursor: pointer;
  }
  small {
    font-weight: 400;
    color: var(--muted-foreground);
    margin-left: 0.6rem;
  }
  div {
    display: grid;
    gap: 0.5rem;
    padding: 0 0.85rem 0.85rem;
  }
  label {
    font-size: 0.82rem;
  }
  textarea {
    width: 100%;
    padding: 0.5rem;
    font-family: monospace;
    font-size: 0.8rem;
    border: 1px solid var(--border);
    border-radius: 0.3rem;
    resize: vertical;
  }
  p {
    margin: 0;
    color: var(--muted-foreground);
    font-size: 0.77rem;
  }
</style>
