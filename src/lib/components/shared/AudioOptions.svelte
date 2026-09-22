<script lang="ts">
  import type { AudioChannels, AudioCodec, MediaStream } from '$lib/ipc/generated';
  import LoudnessControl from './LoudnessControl.svelte';
  import {
    audioBitrateMax,
    audioBitrateChoices,
    audioCompatibilityError,
    type AudioTrackDraft,
  } from './audio-options';

  let {
    idPrefix,
    inputPath,
    stream,
    settings,
    disabled = false,
    onchange,
  }: {
    idPrefix: string;
    inputPath: string;
    stream: MediaStream;
    settings: AudioTrackDraft;
    disabled?: boolean;
    onchange: (settings: AudioTrackDraft) => void;
  } = $props();
  const id = $derived(`${idPrefix}-audio-${stream.index}`);
  const converting = $derived(settings.codec !== 'copy');
  const bitrateMax = $derived(audioBitrateMax(settings, stream));
  const bitrateChoices = $derived(audioBitrateChoices(settings, stream));
  const compatibilityError = $derived(audioCompatibilityError(settings, stream));
  function changeFormat(patch: Partial<AudioTrackDraft>) {
    const next = { ...settings, ...patch };
    if (patch.codec === 'copy' || (patch.channels && patch.channels !== settings.channels))
      next.gain = undefined;
    const max = audioBitrateMax(next, stream);
    if (typeof next.bitrateKbps === 'number' && next.bitrateKbps > max) next.bitrateKbps = max;
    onchange(next);
  }
</script>

<div class="audio-options" role="group" aria-label={`Audio settings for stream #${stream.index}`}>
  <p class="audio-source">
    Source: {stream.codec ?? 'Unknown codec'} · {stream.channels == null
      ? 'Unknown channels'
      : `${stream.channels} ${stream.channels === 1 ? 'channel' : 'channels'}`}{stream.channelLayout
      ? ` · ${stream.channelLayout}`
      : ''}{stream.sampleRate ? ` · ${stream.sampleRate / 1000} kHz` : ''}
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
        <option value="flac">FLAC (24-bit)</option>
        <option value="mp3">MP3</option>
        <option value="vorbis">Vorbis</option>
        <option value="eac3">E-AC-3</option>
      </select>
    </div>
    {#if converting}
      {#if settings.codec !== 'flac'}<div class="field">
          <label for={`${id}-bitrate`}>Audio bitrate</label>
          <div class="input-unit">
            {#if bitrateChoices}<select
                id={`${id}-bitrate`}
                value={settings.bitrateKbps ?? ''}
                {disabled}
                onchange={(event) =>
                  onchange({ ...settings, bitrateKbps: Number(event.currentTarget.value) })}
              >
                <option value="" disabled>Choose bitrate</option>
                {#each bitrateChoices as value}<option {value}>{value}</option>{/each}
              </select>{:else}<input
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
                      event.currentTarget.value === ''
                        ? undefined
                        : event.currentTarget.valueAsNumber,
                  })}
              />{/if}<span>kb/s</span>
          </div>
        </div>{/if}
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
          <option value="surround51">5.1 surround</option>
          <option value="surround71">7.1 surround</option>
        </select>
      </div>
    {/if}
  </div>
  <p class="audio-help">
    {converting
      ? `${settings.codec === 'flac' ? 'Encodes 24-bit integer PCM without a bitrate target. Floating-point and higher-depth sources are converted to 24-bit.' : `${settings.codec === 'mp3' ? 'Choose a standard bitrate.' : `32–${bitrateMax} kb/s target.`} ${settings.codec === 'opus' ? 'Opus uses 48 kHz.' : 'Retains the source sample rate.'}`} Track title, language, and default status are retained.`
      : 'Keeps the original audio without quality loss.'}
  </p>
  {#if converting && (settings.channels === 'surround51' || settings.channels === 'surround71')}
    <p class="audio-help">
      Converts the speaker layout. Sources with fewer channels do not gain original surround detail.
    </p>
  {/if}
  {#if compatibilityError}<p class="audio-help" role="alert">{compatibilityError}</p>{/if}
  <LoudnessControl {inputPath} {settings} {disabled} {onchange} />
</div>

<style>
  .audio-options {
    padding: 10px 0 12px 20px;
    border-bottom: 1px solid var(--border);
  }
  .audio-controls {
    display: grid;
    grid-template-columns: repeat(auto-fit, minmax(min(100%, 12rem), 1fr));
    gap: 10px;
  }
  .audio-controls input[type='number'] {
    width: min(100%, 9rem);
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
  @media (max-width: 560px) {
    .audio-options {
      padding-left: 12px;
    }
  }
</style>
