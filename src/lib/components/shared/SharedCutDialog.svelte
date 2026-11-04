<script lang="ts">
  import { untrack } from 'svelte';
  import SourceDialog from './SourceDialog.svelte';
  import { previewFrame, snapKeyframe } from '$lib/ipc/client';
  import type { FramePreviewResult, MediaFile } from '$lib/ipc/generated';
  import { defaultTrim, type TrimDraft } from './trim-options';
  let {
    open,
    file,
    videoStreamIndex,
    value,
    purpose,
    videoCopied = false,
    frameModeZeroBased = true,
    onconfirm,
    onclose,
  }: {
    open: boolean;
    file: MediaFile | undefined;
    videoStreamIndex: number | undefined;
    value: TrimDraft;
    purpose: 'quick' | 'av1an' | 'utility';
    videoCopied?: boolean;
    frameModeZeroBased?: boolean;
    onconfirm: (value: TrimDraft) => void;
    onclose: () => void;
  } = $props();
  let draft = $state<TrimDraft>(defaultTrim()),
    startText = $state(''),
    endText = $state(''),
    position = $state(0),
    preview = $state<FramePreviewResult | null>(null),
    error = $state(''),
    loading = $state(false),
    previewTimer: ReturnType<typeof setTimeout> | undefined,
    previewAbort: AbortController | undefined,
    snapAbort: AbortController | undefined;
  let snapRevision = 0,
    previewIdentity = '';
  const duration = $derived(Math.max(0, file?.durationSeconds ?? 0));
  const copied = $derived(purpose !== 'quick' || videoCopied);
  const stream = $derived(file?.streams.find((s) => s.index === videoStreamIndex));
  const frameRate = $derived.by(() => {
    const [n, d] = (stream?.frameRate ?? '').split('/').map(Number);
    return n > 0 && d > 0 ? n / d : 0;
  });
  const frameDuration = $derived(frameRate > 0 ? 1000 / frameRate : 40);
  function format(ms: number) {
    const rounded = Math.max(0, Math.round(ms)),
      h = Math.floor(rounded / 3600000),
      m = Math.floor(rounded / 60000) % 60,
      s = Math.floor(rounded / 1000) % 60,
      sub = rounded % 1000;
    return `${String(h).padStart(2, '0')}:${String(m).padStart(2, '0')}:${String(s).padStart(2, '0')}${sub ? '.' + String(sub).padStart(3, '0') : ''}`;
  }
  function parse(text: string): number | null {
    const v = text.trim();
    if (/^\d+$/.test(v)) return Number(v) * 1000;
    const match = /^(\d+):(\d{1,2}):(\d{1,2})(?:\.(\d{1,3}))?$/.exec(v);
    if (!match || Number(match[2]) > 59 || Number(match[3]) > 59) return null;
    return (
      ((+match[1] * 60 + +match[2]) * 60 + +match[3]) * 1000 +
      Number((match[4] ?? '').padEnd(3, '0'))
    );
  }
  function textValue(ms: number) {
    return draft.mode === 'frames'
      ? String(Math.max(0, Math.round(ms / frameDuration) + (frameModeZeroBased ? 0 : 1)))
      : format(ms);
  }
  function fromText(v: string) {
    if (draft.mode === 'time') return parse(v);
    if (!/^\d+$/.test(v.trim())) return null;
    const n = Number(v) - (frameModeZeroBased ? 0 : 1);
    return n >= 0 ? n * frameDuration : null;
  }
  function setRange(start: number, end: number) {
    startText = textValue(start);
    endText = textValue(end);
  }
  function invalidateSnap() {
    ++snapRevision;
    snapAbort?.abort();
    snapAbort = undefined;
  }
  function reset() {
    invalidateSnap();
    position = 0;
    setRange(0, duration * 1000);
    error = '';
  }
  $effect(() => {
    if (!open) return;
    const current = value,
      seconds = duration,
      sourcePath = file?.path,
      sourceIndex = videoStreamIndex;
    untrack(() => {
      void sourcePath;
      void sourceIndex;
      draft = current.enabled
        ? { ...current }
        : { ...defaultTrim(), mode: 'time', seekStrategy: 'inputSeek' };
      error = '';
      const start =
        draft.mode === 'frames'
          ? (draft.startFrame ?? 0) * frameDuration
          : (draft.startMilliseconds ?? 0);
      const end =
        draft.mode === 'frames'
          ? (draft.endFrameExclusive ?? Math.round((seconds * 1000) / frameDuration)) *
            frameDuration
          : (draft.endMilliseconds ?? seconds * 1000);
      position = Math.min(seconds, Math.max(0, start / 1000));
      if (copied) draft = { ...draft, mode: 'time', seekStrategy: 'inputSeek' };
      setRange(start, end);
      preview = null;
      loading = false;
    });
    return () => invalidateSnap();
  });
  $effect(() => {
    if (!open || !file || videoStreamIndex === undefined) return;
    const source = file,
      index = videoStreamIndex,
      at = Math.min(position, Math.max(0, duration - 0.001)),
      identity = `${source.path}\0${index}`;
    if (identity !== previewIdentity) {
      previewIdentity = identity;
      preview = null;
    }
    clearTimeout(previewTimer);
    previewAbort?.abort();
    const controller = new AbortController();
    previewAbort = controller;
    previewTimer = setTimeout(() => {
      loading = true;
      void previewFrame(
        {
          inputPath: source.path,
          videoStreamIndex: index,
          positionSeconds: at,
          displayOrientation: true,
        },
        controller.signal,
      )
        .then((result) => {
          if (!controller.signal.aborted) preview = result;
        })
        .catch(() => {
          if (!controller.signal.aborted) preview = null;
        })
        .finally(() => {
          if (!controller.signal.aborted) loading = false;
        });
    }, 120);
    return () => {
      clearTimeout(previewTimer);
      controller.abort();
    };
  });
  function seek(seconds: number) {
    position = Math.min(duration, Math.max(0, seconds));
  }
  function aborted(cause: unknown) {
    return cause instanceof Error && cause.name === 'AbortError';
  }
  async function snap(ms: number) {
    if (!file || videoStreamIndex === undefined) return ms;
    invalidateSnap();
    const revision = snapRevision,
      controller = new AbortController();
    snapAbort = controller;
    try {
      const result = await snapKeyframe(
        { inputPath: file.path, videoStreamIndex, positionSeconds: ms / 1000 },
        controller.signal,
      );
      if (controller.signal.aborted || revision !== snapRevision)
        throw new DOMException('Keyframe lookup canceled.', 'AbortError');
      if (result.positionSeconds === null)
        throw new Error('No keyframe was found at or before the start.');
      return Math.round(result.positionSeconds * 1000);
    } catch (cause) {
      if (controller.signal.aborted || revision !== snapRevision)
        throw new DOMException('Keyframe lookup canceled.', 'AbortError');
      throw cause;
    } finally {
      if (snapAbort === controller) snapAbort = undefined;
    }
  }
  async function markStart() {
    let ms = position * 1000;
    try {
      if (copied) ms = await snap(ms);
      else invalidateSnap();
      startText = textValue(ms);
      error = '';
    } catch (cause) {
      if (!aborted(cause)) error = cause instanceof Error ? cause.message : String(cause);
    }
  }
  async function confirm() {
    let start = fromText(startText),
      end = fromText(endText);
    if (start === null || end === null || end < start) {
      invalidateSnap();
      error = 'Enter a valid start and end, with end after start.';
      return;
    }
    if (end === start) {
      invalidateSnap();
      onconfirm(defaultTrim());
      return;
    }
    try {
      if (copied) start = await snap(start);
      else invalidateSnap();
      const next: TrimDraft =
        draft.mode === 'frames'
          ? {
              ...draft,
              enabled: true,
              startFrame: Math.round(start / frameDuration),
              endFrameExclusive: Math.round(end / frameDuration),
            }
          : {
              ...draft,
              enabled: true,
              startMilliseconds: Math.round(start),
              endMilliseconds: Math.round(end),
              seekStrategy: draft.seekStrategy ?? 'inputSeek',
            };
      onconfirm(next);
    } catch (cause) {
      if (!aborted(cause)) error = cause instanceof Error ? cause.message : String(cause);
    }
  }
  function changeMode(mode: string) {
    invalidateSnap();
    const start = fromText(startText) ?? 0,
      end = fromText(endText) ?? duration * 1000;
    draft = {
      ...draft,
      mode: mode === 'frames' ? 'frames' : 'time',
      seekStrategy: mode === 'slow' ? 'decodeFromStart' : 'inputSeek',
    };
    setRange(start, end);
  }
</script>

<SourceDialog
  title={purpose === 'utility' ? 'Cut Video' : 'Configure Trim'}
  {open}
  onclose={() => {
    invalidateSnap();
    onclose();
  }}
  width={720}
>
  <div class="preview">
    {#if preview}<img src={preview.imageDataUrl} alt="Video preview" />{:else}<span
        >{loading ? 'Reading preview…' : file ? 'No preview available' : 'No file loaded'}</span
      >{/if}
  </div>
  <input
    aria-label="Position"
    class="slider"
    type="range"
    min="0"
    max={duration}
    step="0.001"
    value={position}
    disabled={duration <= 0}
    oninput={(event) => seek(event.currentTarget.valueAsNumber)}
  />
  <div class="range">
    <div
      style={`left:${duration ? Math.max(0, ((fromText(startText) ?? 0) / 1000 / duration) * 100) : 0}%;right:${duration ? Math.max(0, 100 - ((fromText(endText) ?? 0) / 1000 / duration) * 100) : 0}%`}
    ></div>
    <i style={`left:${duration ? (position / duration) * 100 : 0}%`}></i>
  </div>
  <div class="position">
    <span>{format(position * 1000)}</span>
    <div>
      <button
        title="Jump to the start point"
        onclick={() => seek((fromText(startText) ?? 0) / 1000)}>⏮</button
      >{#each [{ l: '-10s', n: -10 }, { l: '-1s', n: -1 }, { l: '-1f', n: -frameDuration / 1000 }, { l: '+1f', n: frameDuration / 1000 }, { l: '+1s', n: 1 }, { l: '+10s', n: 10 }] as item}<button
          onclick={() => seek(position + item.n)}>{item.l}</button
        >{/each}<button
        title="Jump to the end point"
        onclick={() => seek((fromText(endText) ?? 0) / 1000)}>⏭</button
      >
    </div>
  </div>
  <div class="form">
    {#if !copied}<label for="cut-mode">Mode</label><select
        id="cut-mode"
        value={draft.mode === 'frames'
          ? 'frames'
          : draft.seekStrategy === 'decodeFromStart'
            ? 'slow'
            : 'fast'}
        onchange={(event) => changeMode(event.currentTarget.value)}
        ><option value="fast">Time (exact - seeks straight to the point)</option><option
          value="slow">Time (exact - decodes from the start, much slower)</option
        ><option value="frames">Frame numbers (exact - decodes from the start, much slower)</option
        ></select
      ><span></span>{/if}
    <label for="cut-start">Start</label><input
      id="cut-start"
      bind:value={startText}
      oninput={invalidateSnap}
      onblur={() => {
        if (copied) {
          const ms = fromText(startText);
          if (ms !== null)
            void snap(ms)
              .then((result) => (startText = textValue(result)))
              .catch((cause) => {
                if (!aborted(cause)) error = String(cause);
              });
        }
      }}
    /><button onclick={() => void markStart()}>Set to playhead</button>
    <label for="cut-end">End</label><input
      id="cut-end"
      bind:value={endText}
      oninput={invalidateSnap}
    /><button
      onclick={() => {
        invalidateSnap();
        endText = textValue(position * 1000);
      }}>Set to playhead</button
    >
    <label for="cut-duration">Duration</label><input
      id="cut-duration"
      readonly
      value={draft.mode === 'frames'
        ? String(
            Math.round(
              Math.max(0, (fromText(endText) ?? 0) - (fromText(startText) ?? 0)) / frameDuration,
            ),
          )
        : format(Math.max(0, (fromText(endText) ?? 0) - (fromText(startText) ?? 0)))}
    />{#if !copied}<button
        onclick={() => {
          const ms = fromText(startText);
          if (ms !== null)
            void snap(ms)
              .then((result) => (startText = textValue(result)))
              .catch((cause) => {
                if (!aborted(cause)) error = String(cause);
              });
        }}>Snap start to keyframe</button
      >{:else}<span></span>{/if}
  </div>
  {#if copied}<p>The start is moved to the preceding keyframe.</p>{/if}{#if error}<p role="alert">
      {error}
    </p>{/if}
  <div class="actions">
    <button onclick={reset}>Reset</button><button onclick={() => void confirm()}>OK</button>
  </div>
</SourceDialog>

<style>
  .preview {
    height: 300px;
    display: grid;
    place-items: center;
    background: var(--muted);
    overflow: hidden;
  }
  .preview img {
    width: 100%;
    height: 100%;
    object-fit: contain;
  }
  .slider {
    width: 100%;
    margin: 8px 0;
  }
  .range {
    position: relative;
    height: 8px;
    background: var(--muted);
    border-radius: var(--radius);
  }
  .range div {
    position: absolute;
    height: 100%;
    background: var(--primary);
  }
  .range i {
    position: absolute;
    height: 14px;
    width: 2px;
    top: -3px;
    background: var(--foreground);
  }
  .position,
  .actions {
    display: flex;
    justify-content: space-between;
    align-items: center;
    gap: 8px;
    margin: 10px 0;
  }
  .position div {
    display: flex;
    gap: 4px;
    flex-wrap: wrap;
  }
  .form {
    display: grid;
    grid-template-columns: 110px 1fr auto;
    gap: 6px;
    align-items: center;
  }
  .actions {
    justify-content: flex-end;
    margin-bottom: 0;
  }
  button,
  select,
  input {
    font: inherit;
    color: inherit;
    background: var(--background);
    border: 1px solid var(--border);
    padding: 5px 8px;
  }
  button {
    cursor: pointer;
  }
  p {
    font-size: 12px;
  }
</style>
