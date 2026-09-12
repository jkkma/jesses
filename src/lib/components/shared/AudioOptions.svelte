<script lang="ts">
  import type { AudioChannels, AudioCodec, MediaStream } from '$lib/ipc/generated';
  import { audioBitrateMax, type AudioTrackDraft } from './audio-options';

  let {
    idPrefix,
    stream,
    settings,
    disabled = false,
    onchange,
  }: {
    idPrefix: string;
    stream: MediaStream;
    settings: AudioTrackDraft;
    disabled?: boolean;
    onchange: (settings: AudioTrackDraft) => void;
  } = $props();
  const id = $derived(`${idPrefix}-audio-${stream.index}`);
  const converting = $derived(settings.codec !== 'copy');
  const bitrateMax = $derived(audioBitrateMax(settings, stream));
  function changeFormat(patch: Partial<AudioTrackDraft>) {
    const next = { ...settings, ...patch };
    const max = audioBitrateMax(next, stream);
    if (typeof next.bitrateKbps === 'number' && next.bitrateKbps > max) next.bitrateKbps = max;
    onchange(next);
  }
</script>

<div class="audio-options" role="group" aria-label={`Audio settings for stream #${stream.index}`}>
  <p class="audio-source">
    Source: {stream.codec ?? 'Unknown codec'} · {stream.channels == null
      ? 'Unknown channels'
      : `${stream.channels} ${stream.channels === 1 ? 'channel' : 'channels'}`}{stream.sampleRate
      ? ` · ${stream.sampleRate / 1000} kHz`
      : ''}
  </p>
  <div class="audio-controls">
    <div class="field">
      <label for={`${id}-codec`}>Audio codec</label>
      <select
        id={`${id}-codec`}
        value={settings.codec}
        {disabled}
        onchange={(event) => changeFormat({ codec: event.currentTarget.value as AudioCodec })}
      >
        <option value="copy">Copy source</option>
        <option value="opus">Opus</option>
        <option value="aac">AAC</option>
      </select>
    </div>
    {#if converting}
      <div class="field">
        <label for={`${id}-bitrate`}>Audio bitrate</label>
        <div class="input-unit">
          <input
            id={`${id}-bitrate`}
            type="number"
            min="32"
            max={bitrateMax}
            step="1"
            value={settings.bitrateKbps ?? ''}
            {disabled}
            oninput={(event) =>
              onchange({
                ...settings,
                bitrateKbps:
                  event.currentTarget.value === '' ? undefined : event.currentTarget.valueAsNumber,
              })}
          /><span>kb/s</span>
        </div>
      </div>
      <div class="field">
        <label for={`${id}-channels`}>Audio channels</label>
        <select
          id={`${id}-channels`}
          value={settings.channels}
          {disabled}
          onchange={(event) =>
            changeFormat({ channels: event.currentTarget.value as AudioChannels })}
        >
          <option value="preserve">Preserve source</option>
          <option value="mono">Mono</option>
          <option value="stereo">Stereo</option>
        </select>
      </div>
    {/if}
  </div>
  <p class="audio-help">
    {converting
      ? `32–${bitrateMax} kb/s. ${settings.codec === 'opus' ? 'Opus uses 48 kHz.' : 'AAC retains the source sample rate.'} Track title, language, and default status are retained.`
      : 'Keeps the original audio without quality loss.'}
  </p>
</div>

<style>
  .audio-options {
    padding: 10px 0 14px 28px;
    border-bottom: 1px solid var(--border);
  }
  .audio-controls {
    display: grid;
    grid-template-columns: repeat(3, minmax(0, 1fr));
    gap: 10px;
  }
  .audio-source,
  .audio-help {
    font-size: 10px;
    color: var(--muted-foreground);
    overflow-wrap: anywhere;
  }
  .audio-source {
    margin-bottom: 10px;
  }
  .audio-help {
    margin-top: 8px;
  }
  @media (max-width: 760px) {
    .audio-controls {
      grid-template-columns: 1fr;
    }
  }
</style>
