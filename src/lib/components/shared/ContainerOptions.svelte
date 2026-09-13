<script lang="ts">
  import type { ContainerFormat } from '$lib/ipc/generated';
  let {
    value,
    onchange,
    disabled = false,
  }: {
    value: ContainerFormat;
    onchange: (value: ContainerFormat) => void;
    disabled?: boolean;
  } = $props();
</script>

<label class="field"
  >Output container
  <select
    aria-label="Output container"
    {disabled}
    {value}
    onchange={(event) => onchange(event.currentTarget.value as ContainerFormat)}
  >
    <option value="matroska">Matroska (.mkv)</option>
    <option value="mp4">MP4 (.mp4)</option>
    <option value="mov">QuickTime (.mov)</option>
    <option value="webm">WebM (.webm)</option>
  </select>
</label>
<p class="small-muted">
  {#if value === 'matroska'}Matroska preserves compatible tracks, attachments and metadata.
  {:else if value === 'webm'}WebM requires VP8, VP9 or AV1 video and Opus or Vorbis audio. Text
    subtitles become WebVTT; fonts, styles and positioning may change. Attachments are unsupported.
  {:else}MP4/MOV text subtitles become MP4 text; fonts, styles and positioning may change. Track
    titles and chapter timing are preserved. If a track type has no default, its first track becomes
    default. Attachments and unsupported metadata require Matroska.
  {/if}
  Existing files are never replaced. Incompatible selections are rejected before encoding.
</p>
