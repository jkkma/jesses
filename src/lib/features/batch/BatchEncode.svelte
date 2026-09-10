<script lang="ts">
  import { untrack } from 'svelte';
  import { FolderOutput, Info, ListChecks, LoaderCircle } from '@lucide/svelte';
  import { Button } from '$lib/components/ui/button';
  import { chooseOutputFolder, isDesktop, previewEncodeBatch } from '$lib/ipc/client';
  import { errorMessage, fileName, formatDuration } from '$lib/components/shared/format';
  import type {
    BatchEncodePreview,
    BatchEncodeRequest,
    EncodeRequest,
    MediaFile,
    ToolInfo,
  } from '$lib/ipc/generated';

  let {
    files,
    tools,
    connected,
    onfiles,
    onqueue,
  }: {
    files: MediaFile[];
    tools: ToolInfo[];
    connected: boolean;
    onfiles: () => void;
    onqueue: (requests: EncodeRequest[]) => Promise<void>;
  } = $props();
  type Draft = {
    identity: string;
    version: number;
    selected: boolean;
    video: number | undefined;
    copies: number[];
  };
  const desktop = isDesktop();
  let drafts = $state<Record<string, Draft>>({});
  let nextVersion = 0;
  let crf = $state<number | undefined>(30);
  let preset = $state(4);
  let outputDirectory = $state('');
  let preview = $state<{ key: string; result: BatchEncodePreview } | null>(null);
  let previewing = $state(false);
  let previewGeneration = 0;
  let submitting = $state(false);
  let choosingOutput = $state(false);
  let error = $state<string | null>(null);
  let submissionNotice = $state<string | null>(null);

  $effect(() => {
    const currentFiles = files;
    const prior = untrack(() => drafts);
    let selectedCount = currentFiles.filter(
      (file) => prior[file.id]?.selected && prior[file.id]?.identity === JSON.stringify(file),
    ).length;
    const next = Object.fromEntries(
      currentFiles.map((file) => {
        const identity = JSON.stringify(file);
        const existing = prior[file.id];
        if (existing?.identity === identity) return [file.id, existing];
        const video = file.streams.find((stream) => stream.kind === 'video')?.index;
        const selected =
          video !== undefined && !file.id.startsWith('jesses-synthetic') && selectedCount < 100;
        if (selected) selectedCount++;
        return [
          file.id,
          {
            identity,
            version: ++nextVersion,
            selected,
            video,
            copies: file.streams
              .filter((stream) => stream.kind !== 'video')
              .map((stream) => stream.index),
          } satisfies Draft,
        ];
      }),
    );
    if (JSON.stringify(next) !== JSON.stringify(prior)) drafts = next;
  });

  const selectedFiles = $derived(files.filter((file) => drafts[file.id]?.selected));
  const toolsReady = $derived(
    ['ffmpeg', 'ffprobe', 'svt-av1'].every((id) =>
      tools.some((tool) => tool.id === id && tool.available),
    ),
  );
  const validSettings = $derived(
    typeof crf === 'number' &&
      Number.isInteger(crf) &&
      crf >= 1 &&
      crf <= 63 &&
      Number.isInteger(preset) &&
      preset >= 0 &&
      preset <= 13,
  );
  const draftKey = $derived(JSON.stringify({ files, drafts, crf, preset, outputDirectory }));
  const currentPreview = $derived(preview?.key === draftKey ? preview.result : null);
  const ready = $derived(currentPreview?.items.filter((item) => item.request && !item.error) ?? []);
  const canPreview = $derived(
    desktop &&
      toolsReady &&
      validSettings &&
      !!outputDirectory.trim() &&
      selectedFiles.length > 0 &&
      selectedFiles.length <= 100 &&
      selectedFiles.every((file) =>
        file.streams.some(
          (stream) => stream.kind === 'video' && stream.index === drafts[file.id]?.video,
        ),
      ) &&
      !submitting &&
      !previewing,
  );
  const canQueue = $derived(
    connected && toolsReady && !!currentPreview && ready.length > 0 && !submitting && !previewing,
  );

  $effect(() => {
    draftKey;
    // Native work may finish later; its result belongs only to its original draft.
    ++previewGeneration;
    preview = null;
    previewing = false;
    error = null;
  });

  function updateDraft(id: string, patch: Partial<Draft>) {
    const draft = drafts[id];
    if (!draft || submitting) return;
    drafts = { ...drafts, [id]: { ...draft, ...patch } };
  }

  function selectFiles(selected: boolean) {
    if (submitting) return;
    let count = 0;
    drafts = Object.fromEntries(
      files.map((file) => {
        const draft = drafts[file.id];
        const eligible =
          selected &&
          draft?.video !== undefined &&
          !file.id.startsWith('jesses-synthetic') &&
          count < 100;
        if (eligible) count++;
        return [file.id, { ...draft, selected: eligible }];
      }),
    );
  }

  function toggleCopy(id: string, index: number) {
    const draft = drafts[id];
    if (draft)
      updateDraft(id, {
        copies: draft.copies.includes(index)
          ? draft.copies.filter((value) => value !== index)
          : [...draft.copies, index],
      });
  }

  async function chooseOutput() {
    if (!desktop || submitting || choosingOutput) return;
    const key = draftKey;
    const generation = previewGeneration;
    choosingOutput = true;
    try {
      const path = await chooseOutputFolder();
      if (path && key === draftKey && generation === previewGeneration) outputDirectory = path;
    } catch (cause) {
      if (key === draftKey && generation === previewGeneration) error = errorMessage(cause);
    } finally {
      choosingOutput = false;
    }
  }

  async function previewBatch() {
    if (!canPreview || crf === undefined) return;
    const key = draftKey;
    const generation = ++previewGeneration;
    const request: BatchEncodeRequest = {
      outputDirectory: outputDirectory.trim(),
      crf,
      preset,
      inputs: selectedFiles.map((file) => {
        const draft = drafts[file.id];
        const copies = file.streams
          .filter((stream) => stream.kind !== 'video' && draft.copies.includes(stream.index))
          .sort((a, b) => Number(a.kind === 'attachment') - Number(b.kind === 'attachment'));
        return {
          inputPath: file.path,
          videoStreamIndex: draft.video!,
          streamIndices: [draft.video!, ...copies.map((stream) => stream.index)],
        };
      }),
    };
    previewing = true;
    preview = null;
    error = null;
    submissionNotice = null;
    try {
      const result = await previewEncodeBatch(request);
      if (generation === previewGeneration && key === draftKey) preview = { key, result };
    } catch (cause) {
      if (generation === previewGeneration && key === draftKey) error = errorMessage(cause);
    } finally {
      if (generation === previewGeneration) previewing = false;
    }
  }

  async function queueReady() {
    if (!canQueue) return;
    const key = draftKey;
    const requests: EncodeRequest[] = JSON.parse(JSON.stringify(ready.map((item) => item.request)));
    const submittedDrafts = new Map(
      files
        .filter((file) => requests.some((request) => request.source.inputPath === file.path))
        .map((file) => [file.id, JSON.stringify(drafts[file.id])]),
    );
    submitting = true;
    error = null;
    try {
      await onqueue(requests);
      // A source may have changed while another tab was open. Clear only the
      // exact submitted selections, leaving new or edited drafts alone.
      drafts = Object.fromEntries(
        Object.entries(drafts).map(([id, draft]) => [
          id,
          submittedDrafts.get(id) === JSON.stringify(draft) ? { ...draft, selected: false } : draft,
        ]),
      );
      preview = null;
      submissionNotice = `${requests.length} ${requests.length === 1 ? 'file' : 'files'} added to the queue. Review any remaining selections before another batch.`;
    } catch (cause) {
      if (key === draftKey) {
        preview = null;
        error = `${errorMessage(cause)} Preview the batch again before retrying.`;
      } else {
        submissionNotice = `The previous batch was not queued: ${errorMessage(cause)} Preview the current selection again.`;
      }
    } finally {
      submitting = false;
    }
  }
</script>

<section class="batch-workspace" aria-label="Batch encode workspace">
  <div class="view-intro">
    <div>
      <span class="eyebrow">Folder workflow</span>
      <h1>Batch encode</h1>
      <p>Review your episodes, choose common settings, then queue the ready files together.</p>
    </div>
    <span class="status-label">SVT-AV1 · 10-bit</span>
  </div>
  <div class="notice">
    <Info size={16} aria-hidden="true" />
    <p>
      Progressive SDR, constant frame rate, square pixels, and 4:2:0 color only. Selected audio,
      subtitles, and attachments are copied. Full source compatibility is checked before each
      encode.
    </p>
  </div>
  {#if error}<div class="notice error-notice" role="alert"><p>{error}</p></div>{/if}
  {#if submissionNotice}<div class="notice" role="status"><p>{submissionNotice}</p></div>{/if}
  <div class="batch-grid">
    <section class="panel batch-sources" aria-label="Batch source selection">
      <div class="section-heading">
        <span class="heading-with-icon"
          ><ListChecks size={16} aria-hidden="true" /><span class="eyebrow"
            >Episodes · {selectedFiles.length} selected</span
          ></span
        ><button class="text-button" type="button" onclick={onfiles}>Add or remove files</button>
      </div>
      <div class="selection-actions">
        <button
          class="text-button"
          type="button"
          disabled={!files.length || submitting}
          onclick={() => selectFiles(true)}>Select up to 100</button
        ><button
          class="text-button"
          type="button"
          disabled={!selectedFiles.length || submitting}
          onclick={() => selectFiles(false)}>Clear selection</button
        ><span class="small-muted">Maximum 100 files per batch.</span>
      </div>
      <div class="episode-list">
        {#each files as file (file.id)}
          {@const draft = drafts[file.id]}
          {@const videos = file.streams.filter((stream) => stream.kind === 'video')}
          {#if draft}
            <article class="episode" class:selected={draft.selected}>
              <label class="episode-choice"
                ><input
                  type="checkbox"
                  aria-label={`Select ${file.name}`}
                  checked={draft.selected}
                  disabled={submitting ||
                    !desktop ||
                    !videos.length ||
                    file.id.startsWith('jesses-synthetic') ||
                    (!draft.selected && selectedFiles.length >= 100)}
                  onchange={(event) =>
                    updateDraft(file.id, { selected: event.currentTarget.checked })}
                /><span
                  ><strong>{file.name}</strong><small
                    >{formatDuration(file.durationSeconds)} · {file.path}</small
                  ></span
                ></label
              >
              {#if !videos.length}<p class="disabled-reason">
                  No video stream. This source cannot be encoded.
                </p>{/if}
              {#if file.id.startsWith('jesses-synthetic')}<p class="disabled-reason">
                  Synthetic preview only. Import a local file to encode.
                </p>{/if}
              {#if draft.selected}
                <details class="episode-tracks">
                  <summary>Video and copied tracks · {draft.copies.length} copies</summary>
                  <div class="field">
                    <label for={`batch-video-${draft.version}`}>Video stream for {file.name}</label
                    ><select
                      id={`batch-video-${draft.version}`}
                      value={draft.video}
                      disabled={submitting}
                      onchange={(event) =>
                        updateDraft(file.id, { video: Number(event.currentTarget.value) })}
                      >{#each videos as stream (stream.index)}<option value={stream.index}
                          >#{stream.index} · {stream.codec ?? 'Unknown'}{stream.title
                            ? ` · ${stream.title}`
                            : ''}</option
                        >{/each}</select
                    >
                  </div>
                  <div class="track-choices">
                    {#each file.streams.filter((stream) => stream.kind !== 'video') as stream (stream.index)}<label
                        ><input
                          type="checkbox"
                          aria-label={`Copy stream #${stream.index} from ${file.name}`}
                          checked={draft.copies.includes(stream.index)}
                          disabled={submitting}
                          onchange={() => toggleCopy(file.id, stream.index)}
                        /><span
                          >#{stream.index} · {stream.kind} · {stream.codec ??
                            'Unknown'}{stream.language ? ` · ${stream.language}` : ''}{stream.title
                            ? ` · ${stream.title}`
                            : ''}</span
                        ></label
                      >{/each}
                  </div>
                </details>
              {/if}
            </article>
          {/if}
        {:else}<div class="batch-empty">
            <p>Import files or a folder to prepare a batch.</p>
            <Button variant="outline" onclick={onfiles}>Choose source files</Button>
          </div>{/each}
      </div>
    </section>
    <aside class="panel batch-settings">
      <div class="section-heading">
        <span class="heading-with-icon"
          ><FolderOutput size={16} aria-hidden="true" /><span class="eyebrow">Common settings</span
          ></span
        >
      </div>
      <div class="batch-settings-content">
        <div class="setting-fields">
          <div class="field">
            <label for="batch-crf">CRF</label><input
              id="batch-crf"
              type="number"
              min="1"
              max="63"
              step="1"
              bind:value={crf}
              disabled={submitting}
            />
            <p>1–63 · Lower keeps more detail</p>
          </div>
          <div class="field">
            <label for="batch-preset">Preset</label><select
              id="batch-preset"
              bind:value={preset}
              disabled={submitting}
              >{#each Array.from({ length: 14 }, (_, index) => index) as value}<option {value}
                  >{value}</option
                >{/each}</select
            >
            <p>0–13 · Higher encodes faster</p>
          </div>
        </div>
        <button
          class="text-button"
          type="button"
          disabled={submitting}
          onclick={() => {
            crf = 30;
            preset = 4;
          }}>Reset batch settings</button
        >
        <div class="field">
          <label for="batch-output">Output folder</label><input
            id="batch-output"
            bind:value={outputDirectory}
            disabled={!desktop || submitting}
            placeholder="Choose an existing output folder"
          />
        </div>
        <Button
          variant="outline"
          onclick={chooseOutput}
          disabled={!desktop || submitting || choosingOutput}>Choose output folder</Button
        >
        <p class="small-muted">
          Each source gets a new Matroska filename. The preview avoids existing names; files are
          never replaced. Names are checked again when queued.
        </p>
        <Button onclick={previewBatch} disabled={!canPreview}
          >{#if previewing}<LoaderCircle size={14} class="spinning" aria-hidden="true" />Preparing
            preview…{:else}Preview batch{/if}</Button
        >
        {#if !desktop}<p class="disabled-reason">
            Batch encoding requires the desktop app.
          </p>{:else if !toolsReady}<p class="disabled-reason">
            Install FFmpeg, FFprobe, and standalone SVT-AV1, then refresh Tools & settings.
          </p>{:else if !validSettings}<p class="disabled-reason">
            Use a whole-number CRF from 1 to 63 and preset from 0 to 13.
          </p>{:else if !selectedFiles.length}<p class="disabled-reason">
            Select at least one episode.
          </p>{:else if !outputDirectory.trim()}<p class="disabled-reason">
            Choose an output folder before previewing.
          </p>{/if}
      </div>
    </aside>
  </div>
  <section class="panel batch-review" aria-label="Batch output preview">
    <div class="section-heading">
      <span class="eyebrow">Output preview</span><span class="small-muted"
        >{currentPreview
          ? `${ready.length} ready / ${currentPreview.items.length} reviewed`
          : 'Review required before queueing'}</span
      >
    </div>
    {#if currentPreview}<div class="batch-preview-scroll">
        <table>
          <thead><tr><th>Source</th><th>New output</th><th>Result</th></tr></thead><tbody
            >{#each currentPreview.items as item, index (`${item.inputPath}-${index}`)}<tr
                ><td title={item.inputPath}>{fileName(item.inputPath)}</td><td
                  >{item.outputPath ?? '—'}</td
                ><td
                  >{#if item.error}<span class="preview-error">{item.error.message}</span
                    >{:else if item.request}Ready{:else}Unavailable{/if}</td
                ></tr
              >{/each}</tbody
          >
        </table>
      </div>{:else}<p class="preview-placeholder">
        Preview the current selection to check filenames and per-file errors. Changes to files,
        tracks, settings, or the output folder require a new preview.
      </p>{/if}
    <div class="batch-submit">
      <Button onclick={queueReady} disabled={!canQueue}
        >{submitting ? 'Queueing…' : 'Queue ready files'}</Button
      >
      <p class="small-muted">
        {connected
          ? 'Only ready rows are submitted. Each job keeps its reviewed settings and runs in order.'
          : 'Connecting to the job runtime…'}
      </p>
    </div>
  </section>
</section>

<style>
  .batch-grid {
    display: grid;
    grid-template-columns: minmax(0, 1.6fr) minmax(260px, 1fr);
    gap: 20px;
    margin-top: 20px;
  }
  .batch-sources,
  .batch-settings {
    min-width: 0;
  }
  .selection-actions,
  .batch-submit {
    display: flex;
    flex-wrap: wrap;
    gap: 12px;
    align-items: center;
    padding: 16px 20px;
  }
  .episode-list {
    max-height: 580px;
    overflow-y: auto;
  }
  .episode {
    padding: 16px 20px;
    border-top: 1px solid var(--border);
  }
  .episode.selected {
    background: color-mix(in srgb, var(--background) 40%, transparent);
  }
  .episode-choice,
  .track-choices label {
    display: flex;
    gap: 10px;
    align-items: flex-start;
  }
  .episode-choice input,
  .track-choices input {
    width: 16px;
    height: 16px;
    flex: 0 0 auto;
    margin-top: 2px;
    accent-color: #ad5326;
  }
  .episode-choice span {
    min-width: 0;
  }
  .episode-choice strong,
  .episode-choice small {
    display: block;
    overflow-wrap: anywhere;
  }
  .episode-choice strong {
    font-size: 13px;
  }
  .episode-choice small {
    margin-top: 5px;
    font-size: 11px;
    color: var(--muted-foreground);
  }
  .episode-tracks {
    margin: 12px 0 0 26px;
  }
  .episode-tracks summary {
    cursor: pointer;
    font-size: 12px;
  }
  .episode-tracks .field {
    margin-top: 14px;
  }
  .track-choices {
    display: grid;
    gap: 10px;
    margin-top: 12px;
    max-height: 240px;
    overflow-y: auto;
    font-size: 12px;
  }
  .track-choices span {
    overflow-wrap: anywhere;
  }
  .batch-settings-content {
    display: grid;
    gap: 16px;
    padding: 20px;
  }
  .batch-settings-content .setting-fields {
    padding: 0;
  }
  .batch-review {
    margin-top: 20px;
  }
  .batch-preview-scroll {
    overflow-x: auto;
    max-height: 420px;
  }
  table {
    width: 100%;
    border-collapse: collapse;
    font-size: 12px;
    table-layout: fixed;
  }
  th,
  td {
    padding: 12px 16px;
    text-align: left;
    border-bottom: 1px solid var(--border);
    overflow-wrap: anywhere;
    vertical-align: top;
  }
  th {
    font-size: 10px;
    text-transform: uppercase;
    color: var(--muted-foreground);
  }
  .preview-error {
    color: #8b3328;
  }
  .preview-placeholder,
  .batch-empty {
    padding: 24px 20px;
    font-size: 12px;
  }
  .batch-empty p {
    margin-bottom: 14px;
  }
  .batch-submit p {
    flex: 1;
    min-width: 180px;
  }
  .text-button:disabled {
    opacity: 0.5;
    cursor: default;
  }
  @media (max-width: 950px) {
    .batch-grid {
      grid-template-columns: 1fr;
    }
    .episode-list {
      max-height: 430px;
    }
  }
</style>
