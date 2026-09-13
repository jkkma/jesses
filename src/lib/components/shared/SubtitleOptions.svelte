<script lang="ts">
  import type {
    EncodeBackend,
    MediaStream,
    SubtitleMode,
    SubtitleTrackSettings,
  } from '$lib/ipc/generated';
  import { knownHdr } from './media-color';
  import { subtitleLabels } from './subtitle-options';

  let {
    idPrefix,
    stream,
    settings,
    backend,
    video,
    toneMapped = false,
    disabled = false,
    onchange,
  }: {
    idPrefix: string;
    stream: MediaStream;
    settings: SubtitleTrackSettings;
    backend: EncodeBackend;
    video?: MediaStream;
    toneMapped?: boolean;
    disabled?: boolean;
    onchange: (settings: SubtitleTrackSettings) => void;
  } = $props();
  const id = $derived(`${idPrefix}-subtitle-${stream.index}`);
  const text = $derived(['ass', 'subrip', 'webvtt', 'mov_text'].includes(stream.codec ?? ''));
  const bitmap = $derived(
    ['hdmv_pgs_subtitle', 'dvd_subtitle', 'dvb_subtitle', 'xsub'].includes(stream.codec ?? ''),
  );
  const standalone = $derived(backend === 'standalone');
</script>

<div
  class="subtitle-options"
  role="group"
  aria-label={`Subtitle settings for stream #${stream.index}`}
>
  <label for={id}>Subtitle action</label>
  <select
    {id}
    value={settings.mode}
    {disabled}
    aria-describedby={`${id}-help`}
    onchange={(event) => onchange({ ...settings, mode: event.currentTarget.value as SubtitleMode })}
  >
    <option value="copy">Copy source</option>
    {#each ['subRip', 'ass', 'webVtt'] as mode}
      <option value={mode} disabled={!standalone || !text}
        >{subtitleLabels[mode as SubtitleMode]}</option
      >
    {/each}
    <option
      value="burnIn"
      disabled={!standalone || (!text && !bitmap) || (knownHdr(video) && !toneMapped)}
      >Burn into video</option
    >
  </select>
  <p id={`${id}-help`}>
    {#if !standalone}Conversion and burn-in require standalone encoding.
    {:else if settings.mode === 'burnIn'}The subtitles become video pixels and this subtitle track
      is removed from the output. {text
        ? 'Embedded fonts are used even when font attachments are not copied. Text is rendered after crop and resize, before borders.'
        : 'Bitmap graphics are rendered before crop and resize.'}
    {:else if settings.mode !== 'copy'}Changing text format can change fonts, styling and positions.
      Readable text, cue order and timing are verified; ASS timing has 10 ms precision.
    {:else}Keep the source subtitle track and its formatting. {knownHdr(video) && !toneMapped
        ? 'Burn-in is unavailable for HDR video.'
        : !text && !bitmap
          ? 'This format supports copying only.'
          : ''}{/if}
  </p>
</div>

<style>
  .subtitle-options {
    display: grid;
    gap: 0.45rem;
    margin: 0.4rem 0 0.8rem 1.7rem;
  }
  label {
    font-size: 0.8rem;
    font-weight: 600;
  }
  select {
    max-width: 19rem;
    min-height: 2.35rem;
    padding: 0.4rem 0.6rem;
    border: 1px solid var(--border);
    border-radius: 0.4rem;
    background: var(--background);
    color: var(--foreground);
  }
  p {
    color: var(--muted-foreground);
    font-size: 0.77rem;
    line-height: 1.5;
    margin: 0;
    max-width: 46rem;
  }
</style>
