<script lang="ts">
  import type { MediaFile, MediaStream } from '$lib/ipc/generated';
  import type { FileListMode, TrackSelection } from '$lib/source-workspace';
  import SourceThumbnail from './SourceThumbnail.svelte';
  import { preferences } from '$lib/preferences.svelte';
  import { formatBytes, formatDuration, formatFrameRate } from '$lib/components/shared/format';
  let {
    files,
    tracks,
    mainFile,
    mode,
    onchange,
    onextract,
  }: {
    files: MediaFile[];
    tracks: TrackSelection[];
    mainFile?: MediaFile;
    mode: FileListMode;
    onchange: (tracks: TrackSelection[]) => void;
    onextract: (track: TrackSelection) => Promise<void>;
  } = $props();
  let selected = $state('');
  let extracting = $state(false);
  let extractError = $state('');
  function fitMenu(menu: HTMLDivElement) {
    const details = menu.closest('details')!;
    const workspace = menu.closest('main')!;
    const update = () => {
      if (!details.open) return;
      const top = Math.max(0, workspace.getBoundingClientRect().top);
      menu.style.maxHeight = `${Math.max(0, details.getBoundingClientRect().top - top - 4)}px`;
    };
    const observer = new ResizeObserver(update);
    observer.observe(workspace);
    const rows = menu.closest('section')?.querySelector('.track-body');
    if (rows) observer.observe(rows);
    details.addEventListener('toggle', update);
    window.addEventListener('resize', update);
    return {
      destroy() {
        observer.disconnect();
        details.removeEventListener('toggle', update);
        window.removeEventListener('resize', update);
      },
    };
  }
  async function extract(row: TrackSelection) {
    extracting = true;
    extractError = '';
    try {
      await onextract(row);
    } catch (error) {
      extractError =
        error instanceof Error
          ? error.message
          : String((error as { message?: string })?.message ?? error);
    } finally {
      extracting = false;
    }
  }
  const key = (row: TrackSelection) => JSON.stringify([row.sourceId, row.streamIndex]);
  const stream = (row: TrackSelection) =>
    files
      .find((file) => file.id === row.sourceId)
      ?.streams.find((item) => item.index === row.streamIndex);
  const checked = $derived(tracks.filter((row) => row.included));
  const audio = $derived(checked.filter((row) => stream(row)?.kind === 'audio'));
  const subtitles = $derived(checked.filter((row) => stream(row)?.kind === 'subtitle'));
  const selectedRow = $derived(tracks.find((row) => key(row) === selected));
  const selectedStream = $derived(selectedRow ? stream(selectedRow) : undefined);
  function details(item: MediaStream, row: TrackSelection) {
    const source = files.find((file) => file.id === row.sourceId);
    const lines = [
      `Source File: ${source?.name ?? ''}`,
      `Codec: ${item.codecLongName ?? item.codec ?? 'Unknown'} (${item.codec ?? 'unknown'})`,
    ];
    if (item.kind === 'video') {
      const [n, d] = (item.frameRate ?? '').split('/').map(Number);
      const fps = d ? (n / d).toFixed(3).replace(/\.?0+$/, '') : 'Unknown';
      const scan =
        item.fieldOrder === 'progressive'
          ? 'Progressive'
          : ['tt', 'tb'].includes(item.fieldOrder ?? '')
            ? 'Interlaced, Top Field First'
            : ['bb', 'bt'].includes(item.fieldOrder ?? '')
              ? 'Interlaced, Bottom Field First'
              : 'Unknown';
      lines.push(
        `Title: ${item.title?.trim() || 'None'}`,
        `Resolution and Aspect Ratio: ${item.width ?? 'Unknown'}x${item.height ?? 'Unknown'} - SAR ${item.sampleAspectRatio ?? 'Unknown'} - DAR ${item.displayAspectRatio ?? 'Unknown'}`,
        `Color Format: ${item.pixelFormat ?? 'Unknown'}${item.bitDepth ? ` (${item.bitDepth}-bit)` : ''}`,
        `Frame Rate: ${item.frameRate ?? 'Unknown'} (~${fps} FPS)`,
        `Scan Type: ${scan}`,
      );
    } else if (item.kind === 'audio') {
      lines.push(
        `Title: ${item.title?.trim() || 'None'}`,
        `Sample Rate: ${item.sampleRate && item.sampleRate > 1 ? `${item.sampleRate / 1000} kHz` : 'None'}`,
        `Channels: ${item.channels || 'Unknown'}${item.channelLayout ? ` as ${item.channelLayout}` : ''}`,
        `Language: ${item.language || 'Unknown'}`,
      );
    } else if (item.kind === 'subtitle') {
      const bitmap = ['hdmv_pgs_subtitle', 'dvd_subtitle', 'dvb_subtitle', 'xsub'].includes(
        item.codec ?? '',
      );
      lines.push(
        `Title: ${item.title?.trim() || 'None'}`,
        `Language: ${item.language || 'Unknown'}`,
        `Type: ${bitmap ? 'Bitmap-based' : 'Text-based'}`,
      );
    } else if (item.kind === 'attachment')
      lines.push(
        `Filename: ${item.attachmentFilename || 'None'}`,
        `MIME Type: ${item.attachmentMimeType || 'Unknown'}`,
      );
    return lines.join('\n');
  }
  const blocked = $derived(mode === 'batch');
  function codecName(codec: string | null | undefined) {
    const value = codec ?? 'unknown',
      lower = value.toLowerCase();
    if (lower.startsWith('hdmv_pgs')) return 'PGS';
    if (lower.startsWith('subrip')) return 'SRT';
    if (lower.startsWith('dvd_subtitle')) return 'DVD Subtitles';
    if (lower.startsWith('pcm') && lower !== 'pcm_bluray')
      return value.replaceAll('_', ' ').toUpperCase();
    if (lower.startsWith('binkaudio'))
      return `Bink Audio ${lower.split('_').at(-1)?.toUpperCase()}`;
    return (
      (
        {
          webvtt: 'WebVTT',
          truehd: 'TrueHD',
          opus: 'Opus',
          pcm_bluray: 'Blu-ray PCM',
          vc1: 'VC-1',
          mjpeg: 'MJPEG',
          mpeg4: 'MPEG-4',
          mpeg2video: 'MPEG-2',
          msmpeg4v3: 'MS MPEG-4 V3',
          prores: 'ProRes',
          dnxhd: 'DNxHD',
          binkvideo: 'Bink Video',
          timed_id3: 'Timed ID3',
          text: 'Text',
          rawvideo: 'Raw Video',
          msrle: 'MS RLE',
          wmav2: 'WMAV2',
          wmapro: 'WMA Pro',
          dvb_teletext: 'DVB Teletext',
          dvb_subtitle: 'DVB Subtitles',
        } as Record<string, string>
      )[lower] ?? (value.length <= 5 ? value.toUpperCase() : value)
    );
  }
  const display = (row: TrackSelection) => {
    const item = stream(row);
    const zeroBased =
      (
        preferences.value.general as typeof preferences.value.general & {
          zeroIndexedStreams?: boolean;
        }
      ).zeroIndexedStreams ?? false;
    const number = String(row.streamIndex + (zeroBased ? 0 : 1)).padStart(2, '0');
    const kind =
      item?.kind === 'subtitle'
        ? 'Subtitles'
        : item?.kind
          ? item.kind[0].toUpperCase() + item.kind.slice(1)
          : 'Track';
    const parts = [`(${codecName(item?.codec)})`];
    if (item?.kind === 'video') {
      if (row.language && row.language.toLowerCase() !== 'und')
        parts.push(row.language.toUpperCase().slice(0, 7));
      if (item.width && item.height) parts.push(`${item.width}x${item.height}`);
      if (item.frameRate) parts.push(`${formatFrameRate(item.frameRate)} FPS`);
      if (row.title) parts.push(`'${row.title.slice(0, 40)}'`);
    } else if (item?.kind === 'audio' || item?.kind === 'subtitle') {
      if (row.language) parts.push(row.language.toUpperCase().slice(0, 7));
      if (item.kind === 'audio' && item.channelLayout)
        parts.push(item.channelLayout.split('(')[0].trim());
      if (row.title) parts.push(`'${row.title.slice(0, 50)}'`);
    } else if (item?.kind === 'attachment' && item.attachmentFilename)
      parts.push(item.attachmentFilename.slice(0, 50));
    return `[#${number}]: ${kind} ${parts.join(' - ')}`;
  };
  function defaultDisplay(row: TrackSelection, index: number, subtitle: boolean) {
    const number = index + (preferences.value.general.zeroIndexedStreams ? 0 : 1);
    const parts = [
      `#${String(number).padStart(2, '0')}`,
      row.language.toUpperCase().slice(0, 6),
      row.title.slice(0, subtitle ? 18 : 22),
    ].filter(Boolean);
    return `${parts.join(' - ')}${subtitle ? ` (${codecName(stream(row)?.codec).slice(0, 10)})` : ''}`;
  }
  function toggle(row: TrackSelection) {
    updateChecked(
      tracks.map((item) => (key(item) === key(row) ? { ...item, included: !item.included } : item)),
    );
  }
  function updateChecked(next: TrackSelection[]) {
    for (const kind of ['audio', 'subtitle']) {
      const candidates = next.filter((row) => row.included && stream(row)?.kind === kind);
      if (candidates.length && !candidates.some((row) => row.defaultFlag === true)) {
        const first = key(candidates[0]);
        next = next.map((row) => (key(row) === first ? { ...row, defaultFlag: true } : row));
      }
    }
    onchange(next);
  }
  function selectAll(option: string) {
    const seen = new Set<string>();
    updateChecked(
      tracks.map((row) => {
        const item = stream(row);
        let included = row.included;
        if (option === 'all') included = true;
        else if (option === 'none') included = false;
        else if (option === 'invert') included = !included;
        else if (['video', 'audio', 'subtitle'].includes(option)) included = item?.kind === option;
        else {
          const group =
            option === 'firstLanguage'
              ? `${item?.kind}:${row.language.toLowerCase()}`
              : `${item?.kind}`;
          included =
            (option !== 'first' || ['video', 'audio', 'subtitle'].includes(item?.kind ?? '')) &&
            !seen.has(group);
          seen.add(group);
        }
        return { ...row, included };
      }),
    );
  }
  function sortBy(option: string) {
    const [field, direction] = option.split(':');
    const kinds = [...new Set(tracks.map((row) => stream(row)?.kind ?? ''))];
    const next = [...tracks].sort((a, b) => {
      const group = kinds.indexOf(stream(a)?.kind ?? '') - kinds.indexOf(stream(b)?.kind ?? '');
      if (group) return group;
      const get = (row: TrackSelection) =>
        field === 'codec'
          ? (stream(row)?.codec ?? '')
          : field === 'language'
            ? row.language
            : row.title;
      return get(a).localeCompare(get(b)) * (direction === 'desc' ? -1 : 1);
    });
    onchange(next);
  }
  function move(offset: number) {
    const i = tracks.findIndex((row) => key(row) === selected);
    if (i < 0 || i + offset < 0 || i + offset >= tracks.length) return;
    const next = [...tracks];
    [next[i], next[i + offset]] = [next[i + offset], next[i]];
    onchange(next);
  }
  function setDefault(kind: 'audio' | 'subtitle', value: string) {
    onchange(
      tracks.map((row) =>
        stream(row)?.kind === kind ? { ...row, defaultFlag: key(row) === value } : row,
      ),
    );
  }
</script>

<section class="track-list" aria-label="Track List">
  {#if mainFile}<p class="format-info">
      {mainFile.title && mainFile.title.trim().length > 2
        ? `Title: ${mainFile.title.slice(0, 30)} - `
        : ''}Format: {mainFile.format ?? 'Unknown'} - Duration: {formatDuration(
        mainFile.durationSeconds ?? 0,
      )}{mainFile.bitRate ? ` - Bitrate: ${(Number(mainFile.bitRate) / 1000).toFixed(0)} kbps` : ''} -
      Size: {formatBytes(mainFile.sizeBytes)}
    </p>{/if}
  {#if blocked}<p>Manual track selection is not available in Batch Processing Mode.</p>{/if}
  <div class="track-body">
    <div class="rows" role="listbox" aria-label="Tracks" aria-disabled={blocked}>
      {#each tracks as row (key(row))}
        <div
          role="option"
          aria-selected={selected === key(row)}
          class:selected={selected === key(row)}
          onclick={() => (selected = key(row))}
          onkeydown={(event) => {
            if (event.key === 'Enter' || event.key === ' ') {
              event.preventDefault();
              selected = key(row);
            }
          }}
          tabindex="0"
        >
          <input
            type="checkbox"
            aria-label={`Include ${display(row)}`}
            checked={row.included}
            disabled={blocked}
            onchange={() => toggle(row)}
          />
          <span>{display(row)}</span>
        </div>
      {:else}<p class="empty">Load tracks from the File List.</p>{/each}
    </div>
    <SourceThumbnail file={mainFile} />
  </div>
  <div class="actions">
    <details>
      <summary>Select…</summary>
      <div class="menu" use:fitMenu>
        {#each [['all', 'Check All'], ['none', 'Check None'], ['invert', 'Invert Selection'], ['video', 'All Video Tracks'], ['audio', 'All Audio Tracks'], ['subtitle', 'All Subtitle Tracks'], ['first', 'First Track Of Each Type'], ['firstLanguage', 'First Track Of Each Language Per Type']] as [value, label]}<button
            type="button"
            disabled={blocked}
            onclick={(event) => {
              selectAll(value);
              (event.currentTarget.closest('details') as HTMLDetailsElement).open = false;
            }}>{label}</button
          >{/each}
      </div>
    </details>
    <details>
      <summary>Sort…</summary>
      <div class="menu" use:fitMenu>
        {#each [['language:asc', 'Language (A-Z)'], ['language:desc', 'Language (Z-A)'], ['title:asc', 'Title (A-Z)'], ['title:desc', 'Title (Z-A)'], ['codec:asc', 'Codec (A-Z)'], ['codec:desc', 'Codec (Z-A)']] as [value, label]}<button
            type="button"
            disabled={blocked}
            onclick={(event) => {
              sortBy(value);
              (event.currentTarget.closest('details') as HTMLDetailsElement).open = false;
            }}>{label}</button
          >{/each}
      </div>
    </details>
    {#if selectedRow && selectedStream?.kind === 'attachment'}<button
        type="button"
        disabled={blocked || extracting}
        onclick={() => extract(selectedRow)}>Extract</button
      >{/if}
    {#if selectedRow}<button
        type="button"
        disabled={blocked}
        aria-label="Move track up"
        onclick={() => move(-1)}>▲</button
      ><button type="button" disabled={blocked} aria-label="Move track down" onclick={() => move(1)}
        >▼</button
      >{/if}
  </div>
  {#if extractError}<p role="alert">{extractError}</p>{/if}
  {#if selectedStream && selectedRow}<pre>{details(selectedStream, selectedRow)}</pre>{/if}
  <div class="defaults">
    <label
      >Default Audio <select
        disabled={blocked || !audio.length}
        value={key(
          audio.find((row) => row.defaultFlag === true) ??
            audio[0] ?? {
              sourceId: '',
              streamIndex: -1,
              included: false,
              title: '',
              language: '',
              defaultFlag: null,
              forcedFlag: null,
            },
        )}
        onchange={(event) => setDefault('audio', event.currentTarget.value)}
        >{#each audio as row, index}<option value={key(row)}
            >{defaultDisplay(row, index, false)}</option
          >{/each}</select
      ></label
    ><label
      >Default Subtitles <select
        disabled={blocked || !subtitles.length}
        value={subtitles.find((row) => row.defaultFlag === true)
          ? key(subtitles.find((row) => row.defaultFlag === true)!)
          : ''}
        onchange={(event) => setDefault('subtitle', event.currentTarget.value)}
        ><option value="">None</option>{#each subtitles as row, index}<option value={key(row)}
            >{defaultDisplay(row, index, true)}</option
          >{/each}</select
      ></label
    >
  </div>
</section>

<style>
  /* The track rows take the space left by the other controls and scroll
     inside it. A fixed minimum height pushed the list's last rows below the
     visible workspace, where the wheel could not reach them. */
  .track-list {
    display: flex;
    flex-direction: column;
    gap: 0.6rem;
    flex: 1;
    min-width: 0;
    min-height: 0;
  }
  .track-list > :not(.track-body) {
    flex: none;
  }
  .format-info {
    margin: 0;
    color: var(--muted-foreground);
    font-size: 12px;
  }
  .track-body {
    flex: 1 1 0;
    display: grid;
    grid-template-columns: minmax(0, 1fr) minmax(200px, 384px);
    grid-template-rows: minmax(0, 1fr);
    gap: 10px;
    min-height: 7rem;
  }
  .rows {
    min-height: 0;
    background: var(--background);
    border-radius: var(--radius);
    overflow: auto;
  }
  .rows > div {
    display: flex;
    gap: 0.65rem;
    align-items: center;
    padding: 0.45rem 0.65rem;
    border-left: 4px solid var(--primary);
  }
  .rows > div.selected {
    background: var(--accent);
    color: var(--foreground);
  }
  .empty,
  p {
    padding: 0.6rem;
    color: var(--muted-foreground);
  }
  .actions {
    display: flex;
    justify-content: flex-end;
    gap: 0.4rem;
  }
  button,
  summary {
    padding: 0.38rem 0.75rem;
    border: 0;
    border-radius: var(--radius);
    background: var(--secondary);
    color: var(--secondary-foreground);
    cursor: pointer;
  }
  details {
    position: relative;
  }
  summary {
    list-style: none;
  }
  .menu {
    position: absolute;
    bottom: 100%;
    box-sizing: border-box;
    overflow-y: auto;
    right: 0;
    z-index: 4;
    width: 250px;
    display: grid;
    padding: 0.3rem;
    background: var(--background);
    border: 1px solid var(--border);
  }
  .menu button {
    text-align: left;
    background: none;
  }
  .menu button:hover {
    background: var(--accent);
  }
  pre {
    margin: 0;
    padding: 0.5rem;
    max-height: 110px;
    overflow: auto;
    background: var(--background);
  }
  .defaults {
    display: flex;
    flex-wrap: wrap;
    gap: 2rem;
  }
  .defaults label {
    display: flex;
    align-items: center;
    gap: 0.7rem;
  }
  select {
    min-width: 210px;
    padding: 0.4rem;
    background: var(--background);
    border: 1px solid var(--border);
  }
  .rows,
  button,
  summary {
    border-radius: 0;
  }
  .rows {
    border: 1px solid var(--border);
  }
  .rows > div {
    border-left: 4px solid transparent;
    border-bottom: 1px solid var(--rule);
  }
  .rows > div.selected {
    border-left-color: var(--primary);
  }
  .menu {
    background: var(--popover);
    box-shadow: 3px 3px 0 #454b3c38;
  }
</style>
