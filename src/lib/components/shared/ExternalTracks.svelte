<script lang="ts">
  import type {
    EncodeBackend,
    MediaFile,
    MediaStream,
    SubtitleTrackSettings,
  } from '$lib/ipc/generated';
  import AudioOptions from './AudioOptions.svelte';
  import SubtitleOptions from './SubtitleOptions.svelte';
  import type { AudioTrackDraft } from './audio-options';
  import {
    externalChoice,
    externalKinds,
    sameExternalTrack,
    type ExternalAudioDraft,
    type ExternalTrackChoice,
  } from './external-tracks';

  let {
    files,
    primary,
    idPrefix,
    backend,
    video,
    toneMapped,
    selected,
    disabled,
    issue,
    onchange,
  }: {
    files: MediaFile[];
    primary: MediaFile | undefined;
    idPrefix: string;
    backend: EncodeBackend;
    video?: MediaStream;
    toneMapped: boolean;
    selected: ExternalTrackChoice[];
    disabled: boolean;
    issue: string | null;
    onchange: (choices: ExternalTrackChoice[]) => void;
  } = $props();
  const sources = $derived(
    files.filter(
      (file) =>
        file.id !== primary?.id &&
        file.path !== primary?.path &&
        !file.id.startsWith('jesses-synthetic') &&
        file.streams.some((stream) => externalKinds.has(stream.kind)),
    ),
  );
  const stale = $derived(
    selected.filter((choice) => {
      const source = files.find(
        (file) => file.id === choice.sourceId && file.path === choice.inputPath,
      );
      const stream = source?.streams.find((entry) => entry.index === choice.streamIndex);
      return (
        !!choice.invalidated ||
        !source ||
        source.id === primary?.id ||
        source.path === primary?.path ||
        !stream ||
        !externalKinds.has(stream.kind) ||
        JSON.stringify(stream) !== choice.streamSnapshot
      );
    }),
  );

  function toggle(source: MediaFile, index: number) {
    const stream = source.streams.find((entry) => entry.index === index);
    if (!stream) return;
    const choice = externalChoice(source, stream);
    const current = selected.find((entry) => sameExternalTrack(entry, choice));
    onchange(
      !current?.invalidated &&
        current?.streamSnapshot === choice.streamSnapshot &&
        current.sourceId === choice.sourceId
        ? selected.filter((entry) => !sameExternalTrack(entry, choice))
        : [...selected.filter((entry) => !sameExternalTrack(entry, choice)), choice],
    );
  }

  function changeAudio(source: MediaFile, index: number, settings: AudioTrackDraft) {
    onchange(
      selected.map((choice) => {
        if (
          choice.sourceId !== source.id ||
          choice.inputPath !== source.path ||
          choice.streamIndex !== index
        )
          return choice;
        if (settings.codec === 'copy') {
          const { audio: _audio, ...copy } = choice;
          return copy;
        }
        const audio: ExternalAudioDraft = {
          codec: settings.codec,
          bitrateKbps: settings.bitrateKbps,
          channels: settings.channels,
          ...(settings.gain ? { gain: { ...settings.gain } } : {}),
        };
        return { ...choice, audio };
      }),
    );
  }

  function changeSubtitle(source: MediaFile, index: number, settings: SubtitleTrackSettings) {
    onchange(
      selected.map((choice) => {
        if (
          choice.sourceId !== source.id ||
          choice.inputPath !== source.path ||
          choice.streamIndex !== index
        )
          return choice;
        if (settings.mode === 'copy') {
          const { subtitleMode: _mode, ...copy } = choice;
          return copy;
        }
        return { ...choice, subtitleMode: settings.mode };
      }),
    );
  }

  function changeOffset(source: MediaFile, index: number, seconds: string) {
    const milliseconds = seconds === '' ? 0 : Math.round(Number(seconds) * 1000);
    onchange(
      selected.map((choice) =>
        choice.sourceId === source.id &&
        choice.inputPath === source.path &&
        choice.streamIndex === index
          ? { ...choice, offsetMilliseconds: milliseconds }
          : choice,
      ),
    );
  }
</script>

<details class="external-tracks">
  <summary>Tracks from other files{selected.length ? ` · ${selected.length} selected` : ''}</summary
  >
  <div class="external-content">
    <p>
      Add tracks from imported files. Audio can be copied, converted or gain-adjusted. Subtitles can
      be copied{backend === 'standalone' ? ', converted or burned into video' : ''}; attachments are
      copied. Timing offsets delay a track when positive. The video stays with the encoding source;
      metadata, chapters and output order can be chosen below.
    </p>
    {#if stale.length}
      <div class="stale-tracks">
        {#each stale as choice}
          <div class="stale-row">
            <span
              >{choice.inputPath} · stream #{choice.streamIndex} needs to be selected again.</span
            >
            <button
              type="button"
              {disabled}
              onclick={() => onchange(selected.filter((entry) => entry !== choice))}
              aria-label={`Remove unavailable track ${choice.inputPath} stream #${choice.streamIndex}`}
              >Remove</button
            >
          </div>
        {/each}
      </div>
    {/if}
    {#each sources as source (source.id)}
      <fieldset {disabled}>
        <legend>{source.name}</legend>
        {#each source.streams.filter( (stream) => externalKinds.has(stream.kind) ) as stream (stream.index)}
          {@const choice = externalChoice(source, stream)}
          {@const current = selected.find(
            (entry) =>
              sameExternalTrack(entry, choice) &&
              entry.sourceId === choice.sourceId &&
              entry.streamSnapshot === choice.streamSnapshot &&
              !entry.invalidated,
          )}
          <label class="external-choice">
            <input
              type="checkbox"
              checked={!!current}
              onchange={() => toggle(source, stream.index)}
              aria-label={`Add ${source.name} ${stream.kind} stream #${stream.index}`}
            />
            <span
              ><strong>#{stream.index} · {stream.kind} · {stream.codec ?? 'Unknown codec'}</strong
              ><small
                >{[stream.language, stream.title].filter(Boolean).join(' · ') ||
                  'No language or title'}</small
              ></span
            >
          </label>
          {#if current && stream.kind === 'audio'}
            <AudioOptions
              idPrefix={`${idPrefix}-external-${source.id}`}
              inputPath={source.path}
              {stream}
              settings={{
                streamIndex: stream.index,
                codec: current.audio?.codec ?? 'copy',
                bitrateKbps: current.audio ? current.audio.bitrateKbps : 128,
                channels: current.audio?.channels ?? 'preserve',
                ...(current.audio?.gain ? { gain: { ...current.audio.gain } } : {}),
              }}
              {disabled}
              compact
              onchange={(next) => changeAudio(source, stream.index, next)}
            />
          {/if}
          {#if current && stream.kind === 'subtitle'}
            <SubtitleOptions
              idPrefix={`${idPrefix}-external-${source.id}`}
              {stream}
              settings={{ streamIndex: stream.index, mode: current.subtitleMode ?? 'copy' }}
              {backend}
              {video}
              {toneMapped}
              {disabled}
              onchange={(next) => changeSubtitle(source, stream.index, next)}
            />
          {/if}
          {#if current && (stream.kind === 'audio' || stream.kind === 'subtitle')}
            <label class="timing-offset">
              Timing offset (seconds)
              <input
                type="number"
                min="-86400"
                max="86400"
                step="0.001"
                value={(current.offsetMilliseconds ?? 0) / 1000}
                {disabled}
                aria-label={`Timing offset in seconds for ${source.name} stream #${stream.index}`}
                oninput={(event) => changeOffset(source, stream.index, event.currentTarget.value)}
              />
              <small>Positive delays this track; negative starts it earlier.</small>
            </label>
          {/if}
        {/each}
      </fieldset>
    {:else}
      <p>Import another file with audio, subtitle, or attachment tracks to add them here.</p>
    {/each}
    {#if issue}<p class="external-issue" role="alert">{issue}</p>{/if}
  </div>
</details>

<style>
  .external-tracks {
    margin: 2px 14px 14px;
    border: 1px solid var(--border);
    border-radius: 8px;
    min-width: 0;
  }
  summary {
    cursor: pointer;
    padding: 10px 12px;
    font-size: 12px;
    font-weight: 700;
  }
  .external-content {
    display: grid;
    gap: 10px;
    padding: 0 12px 12px;
    max-height: 340px;
    overflow-y: auto;
  }
  p {
    margin: 0;
    color: var(--muted-foreground);
    font-size: 11px;
    line-height: 1.45;
  }
  fieldset {
    margin: 0;
    padding: 0 0 2px;
    border: 0;
    min-width: 0;
  }
  legend {
    padding: 0 0 3px;
    font-size: 11px;
    font-weight: 700;
    overflow-wrap: anywhere;
  }
  .external-choice {
    display: flex;
    align-items: center;
    gap: 8px;
    padding: 5px 0;
    font-size: 11px;
  }
  .external-choice input {
    width: 16px;
    height: 16px;
    accent-color: #ad5326;
    flex: 0 0 auto;
  }
  .external-choice span {
    min-width: 0;
    overflow-wrap: anywhere;
  }
  .external-choice strong,
  .external-choice small {
    display: block;
  }
  .timing-offset {
    display: grid;
    gap: 4px;
    max-width: 230px;
    margin: 3px 0 10px 24px;
    font-size: 11px;
    font-weight: 600;
  }
  .timing-offset input {
    width: 100%;
    min-width: 0;
    padding: 6px 8px;
    border: 1px solid var(--border);
    border-radius: 4px;
    background: var(--background);
    color: var(--foreground);
    font: inherit;
  }
  .timing-offset small {
    color: var(--muted-foreground);
    font-weight: 400;
  }
  .external-choice small {
    margin-top: 2px;
    color: var(--muted-foreground);
  }
  .stale-tracks {
    display: grid;
    gap: 6px;
  }
  .stale-row {
    display: flex;
    align-items: center;
    justify-content: space-between;
    gap: 8px;
    font-size: 11px;
    color: #a3362f;
    overflow-wrap: anywhere;
  }
  .stale-row button {
    padding: 4px 7px;
    border: 1px solid var(--border);
    border-radius: 4px;
    background: var(--background);
    cursor: pointer;
  }
  .external-issue {
    color: #a3362f;
  }
</style>
