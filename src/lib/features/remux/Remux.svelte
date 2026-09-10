<script lang="ts">
  import { ArrowDown, ArrowUp, ArrowRight, FolderOutput, Play, CircleAlert } from '@lucide/svelte';
  import { Button } from '$lib/components/ui/button';
  import { chooseRemuxDestination, isDesktop } from '$lib/ipc/client';
  import { errorMessage } from '$lib/components/shared/format';
  import type { JobSnapshot, MediaFile, RemuxRequest, ToolInfo } from '$lib/ipc/generated';
  let {
    file,
    tools,
    jobs,
    connected,
    onfiles,
    onstart,
  }: {
    file: MediaFile | undefined;
    tools: ToolInfo[];
    jobs: JobSnapshot[];
    connected: boolean;
    onfiles: () => void;
    onstart: (request: RemuxRequest) => Promise<void>;
  } = $props();
  let destination = $state('');
  let order = $state<number[]>([]);
  let included = $state<number[]>([]);
  let error = $state<string | null>(null);
  let submitting = $state(false);
  const desktop = isDesktop();
  const terminal = (state: string) =>
    ['succeeded', 'failed', 'canceled', 'interrupted'].includes(state);
  const active = $derived(jobs.find((job) => !terminal(job.state)));
  const toolsReady = $derived(
    ['ffmpeg', 'ffprobe'].every((id) => tools.some((tool) => tool.id === id && tool.available)),
  );
  const usableSource = $derived(!!file && !file.id.startsWith('jesses-synthetic'));
  const canStart = $derived(
    desktop &&
      connected &&
      usableSource &&
      toolsReady &&
      included.length > 0 &&
      !!destination.trim() &&
      !active &&
      !submitting,
  );
  $effect(() => {
    const source = file;
    const indices = source?.streams.map((stream) => stream.index) ?? [];
    order = indices;
    included = [...indices];
    destination =
      source && !source.id.startsWith('jesses-synthetic')
        ? source.path.replace(/\.[^./\\]+$/, '') + '_remux.mkv'
        : '';
    error = null;
  });
  function toggle(index: number) {
    included = included.includes(index)
      ? included.filter((item) => item !== index)
      : [...included, index];
  }
  function move(position: number, offset: number) {
    const next = [...order];
    [next[position], next[position + offset]] = [next[position + offset], next[position]];
    order = next;
  }
  async function chooseOutput() {
    try {
      const selected = await chooseRemuxDestination(destination);
      if (selected) destination = selected;
    } catch (cause) {
      error = errorMessage(cause);
    }
  }
  async function start() {
    if (!canStart || !file) return;
    submitting = true;
    error = null;
    try {
      await onstart({
        inputPath: file.path,
        outputPath: destination.trim(),
        streamIndices: order.filter((index) => included.includes(index)),
      });
    } catch (cause) {
      error = errorMessage(cause);
    } finally {
      submitting = false;
    }
  }
</script>

<section class="remux-workspace" aria-label="Remux workspace">
  <div class="view-intro">
    <div>
      <span class="eyebrow">Copy selected streams</span>
      <h1>Remux</h1>
      <p>Save your video, audio, and subtitles in a new Matroska file without re-encoding.</p>
    </div>
    <span class="status-label">Matroska · .mkv</span>
  </div>
  {#if error}<div class="notice error-notice" role="alert">
      <CircleAlert size={16} />
      <p>{error}</p>
    </div>{/if}
  <div class="remux-grid">
    <section class="panel remux-streams" aria-label="Streams to copy">
      <div class="section-heading">
        <span class="eyebrow">Source streams</span><button
          type="button"
          class="text-button"
          onclick={onfiles}>Change source<ArrowRight size={13} /></button
        >
      </div>
      <div class="remux-source">
        <strong>{file?.name ?? 'No source selected'}</strong>
        <p>Select streams and arrange their output order.</p>
      </div>
      {#if file}<div class="remux-stream-list">
          {#each order as index, position (index)}
            {@const stream = file.streams.find((item) => item.index === index)}
            {#if stream}
              <div class="remux-stream-row">
                <label
                  ><input
                    type="checkbox"
                    aria-label={`Include stream #${index}`}
                    checked={included.includes(index)}
                    disabled={!!active || submitting}
                    onchange={() => toggle(index)}
                  /><span
                    ><strong
                      >#{index} · {stream.kind}
                      <span class="small-muted">{stream.codec ?? 'Unknown codec'}</span></strong
                    ><small
                      >{[stream.title, stream.language].filter(Boolean).join(' · ') ||
                        'No track title'}</small
                    ></span
                  ></label
                >
                <div class="stream-order">
                  <button
                    type="button"
                    class="icon-button"
                    aria-label={`Move stream #${index} up`}
                    disabled={position === 0 || !!active || submitting}
                    onclick={() => move(position, -1)}><ArrowUp size={14} /></button
                  ><button
                    type="button"
                    class="icon-button"
                    aria-label={`Move stream #${index} down`}
                    disabled={position === order.length - 1 || !!active || submitting}
                    onclick={() => move(position, 1)}><ArrowDown size={14} /></button
                  >
                </div>
              </div>
            {/if}
          {/each}
        </div>{:else}<p class="remux-empty">Add a media file from the Files tab to begin.</p>{/if}
      <p class="panel-footnote">
        Selected attachments, track metadata, and chapters are carried forward. Keep attachments
        after media tracks. Unsupported streams produce an error.
      </p>
    </section>
    <aside class="panel remux-output">
      <div class="section-heading">
        <span class="heading-with-icon"
          ><FolderOutput size={16} /><span class="eyebrow">Destination</span></span
        >
      </div>
      <div class="remux-output-content">
        <div class="field">
          <label for="remux-destination">Destination</label><input
            id="remux-destination"
            bind:value={destination}
            disabled={!desktop || !!active || submitting}
            placeholder="Choose a new .mkv file"
          />
        </div>
        <Button
          variant="outline"
          onclick={chooseOutput}
          disabled={!desktop || !!active || submitting}>Choose destination</Button
        >
        <p class="small-muted">Choose a new filename. Existing files are never replaced.</p>
        <Button onclick={start} disabled={!canStart}
          ><Play size={14} />{submitting ? 'Starting…' : 'Start remux'}</Button
        >
        {#if !desktop}<p class="disabled-reason">Remux requires the desktop app.</p>
        {:else if !connected}<p class="disabled-reason">Connecting to the job runtime…</p>
        {:else if !toolsReady}<p class="disabled-reason">
            Install FFmpeg and FFprobe, then refresh Tools & settings.
          </p>
        {:else if !usableSource}<p class="disabled-reason">Choose a local source file.</p>
        {:else if !included.length}<p class="disabled-reason">Select at least one stream.</p>{/if}
      </div>
    </aside>
  </div>
</section>

<style>
  .remux-workspace {
    min-width: 0;
  }
  .remux-grid {
    display: grid;
    grid-template-columns: minmax(0, 1.5fr) minmax(0, 1fr);
    gap: 20px;
  }
  .remux-streams,
  .remux-output {
    min-width: 0;
  }
  .section-heading {
    padding: 16px 20px;
  }
  .remux-source,
  .remux-empty {
    padding: 16px 20px;
    overflow-wrap: anywhere;
  }
  .remux-source p {
    margin-top: 5px;
    font-size: 12px;
  }
  .remux-stream-list {
    padding: 0 20px 12px;
    max-height: 400px;
    overflow-y: auto;
  }
  .remux-stream-row {
    display: flex;
    align-items: center;
    gap: 12px;
    padding: 14px 0;
    border-top: 1px solid var(--border);
  }
  .remux-stream-row label {
    display: flex;
    align-items: center;
    gap: 12px;
    flex: 1;
    min-width: 0;
  }
  .remux-stream-row input {
    accent-color: #ad5326;
    flex: 0 0 auto;
    width: 16px;
    height: 16px;
  }
  .remux-stream-row label span {
    min-width: 0;
    overflow-wrap: anywhere;
  }
  .remux-stream-row strong,
  .remux-stream-row small {
    display: block;
  }
  .remux-stream-row strong {
    font-size: 13px;
  }
  .remux-stream-row small {
    margin-top: 4px;
    font-size: 11px;
  }
  .stream-order {
    display: flex;
    flex: 0 0 auto;
  }
  .remux-output-content {
    padding: 20px;
    display: flex;
    flex-direction: column;
    gap: 14px;
  }
  @media (max-width: 850px) {
    .remux-grid {
      grid-template-columns: minmax(0, 1fr);
    }
  }
</style>
