<script lang="ts">
  import {
    chooseUtilityFile,
    chooseUtilityDestination,
    inspectSavedJob,
    exportSavedJob,
    discardJobProgress,
    resumeJob,
    readJobLog,
    isDesktop,
  } from '$lib/ipc/client';
  import type {
    JobSnapshot,
    SavedJobInspection,
    EncodeRequest,
    VideoEncoder,
  } from '$lib/ipc/generated';
  import { errorMessage, fileName } from '$lib/components/shared/format';
  import DiscardProgressDialog from '$lib/components/shared/DiscardProgressDialog.svelte';
  import SourceDialog from '$lib/components/shared/SourceDialog.svelte';
  import { canResumeJob, savedProgressSummary } from '$lib/components/shared/job-state';
  let {
    jobs,
    onqueue,
    open = $bindable(false),
    selected = $bindable(''),
  }: {
    jobs: JobSnapshot[];
    onqueue: (request: EncodeRequest) => Promise<void>;
    open?: boolean;
    selected?: string;
  } = $props();
  let svtBuild = $state<VideoEncoder | ''>('');
  let inspection = $state<SavedJobInspection | null>(null);
  let pending = $state(false);
  let error = $state<string | null>(null);
  let message = $state<string | null>(null);
  const selectedJob = $derived(jobs.find((job) => job.id === selected));
  let discardTargetId = $state<string>();
  const discardTarget = $derived(
    jobs.find((job) => job.id === discardTargetId && canResumeJob(job)),
  );
  let log = $state<{ name: string; result: Promise<{ text: string; notice?: string }> }>();
  function isRemuxJob(job: JobSnapshot): boolean {
    const settings = job.encodeSettings;
    if (!settings) return true;
    if ((settings.videoMode ?? 'encode') !== 'copy') return false;
    if (
      settings.audio.some(
        (track) => track.codec !== 'copy' || !!track.gain || !!track.normalization,
      )
    )
      return false;
    if ((settings.subtitles ?? []).some((track) => track.mode !== 'copy' || !!track.burnIn))
      return false;
    if (
      (settings.externalTracks ?? []).some(
        (track) =>
          !!track.audio ||
          (track.subtitleMode !== undefined && track.subtitleMode !== 'copy') ||
          !!track.subtitleBurnIn,
      )
    )
      return false;
    return true;
  }
  function jobModeLabel(job: JobSnapshot): string {
    if (job.encodeSettings?.backend === 'av1an') return 'AV1AN';
    return isRemuxJob(job) ? 'Remux' : 'Quick Convert';
  }
  function viewLog(job: JobSnapshot) {
    const summary = job.logs.join('\n');
    log = {
      name: fileName(job.request.outputPath),
      result: job.logPath
        ? readJobLog(job.id).then(
            (text) => ({ text: text || summary }),
            (cause) => ({ text: summary, notice: errorMessage(cause) }),
          )
        : Promise.resolve({ text: summary }),
    };
  }
  async function inspect() {
    pending = true;
    error = null;
    message = null;
    try {
      const path = await chooseUtilityFile('Inspect saved job', ['json']);
      if (path) {
        inspection = null;
        inspection = await inspectSavedJob(path, svtBuild || null);
      }
    } catch (e) {
      error = errorMessage(e);
    } finally {
      pending = false;
    }
  }
  async function save() {
    const job = selectedJob;
    if (!job?.encodeSettings) return;
    pending = true;
    error = null;
    message = null;
    try {
      const path = await chooseUtilityDestination('encode-request.json', ['json']);
      if (path) {
        await exportSavedJob(path, { source: job.request, settings: job.encodeSettings });
        message = 'Saved encode request. Recovery files remain in their original workspace.';
      }
    } catch (e) {
      error = errorMessage(e);
    } finally {
      pending = false;
    }
  }
  async function queue() {
    if (!inspection?.request) return;
    pending = true;
    error = null;
    message = null;
    try {
      const request = structuredClone($state.snapshot(inspection.request));
      const path = await chooseUtilityDestination(fileName(request.source.outputPath), [
        'mkv',
        'mp4',
        'mov',
        'webm',
      ]);
      if (!path) return;
      request.source.outputPath = path;
      await onqueue(request);
      message =
        'Queued a new encode with the saved settings. It starts from the source; existing recovery files remain unchanged.';
    } catch (e) {
      error = errorMessage(e);
    } finally {
      pending = false;
    }
  }
  async function useProgress(job: JobSnapshot, action: 'resume' | 'discard') {
    pending = true;
    error = null;
    message = null;
    try {
      if (action === 'resume') {
        await resumeJob(job.id);
        message = 'Queued the saved encode for resume.';
      } else {
        await discardJobProgress(job.id);
        message = 'Discarded the saved encoder progress.';
      }
    } catch (e) {
      error = errorMessage(e);
    } finally {
      pending = false;
    }
  }
</script>

<details class="saved-jobs" bind:open>
  <summary>Saved requests and resume compatibility</summary>
  <div class="saved-jobs-body">
    <p>
      Inspect a job's result or resume saved encoder progress. Export an encode request to run it
      again later.
    </p>
    <div class="saved-job-controls">
      <label>
        Job from history<select bind:value={selected} disabled={pending}>
          <option value="">Choose a job</option>{#each jobs as job}<option value={job.id}>
              {fileName(job.request.outputPath)} · {job.state} · {jobModeLabel(job)} · {fileName(
                job.request.inputPath,
              )}
            </option>{/each}
        </select>
      </label>
      <div class="actions">
        {#if selectedJob && (selectedJob.logPath || selectedJob.logs.length)}
          <button type="button" onclick={() => viewLog(selectedJob!)}>View job log</button>
        {/if}
        <button
          type="button"
          onclick={save}
          disabled={pending || !selectedJob?.encodeSettings || !isDesktop()}
        >
          Export encode request
        </button>
        <button type="button" onclick={inspect} disabled={pending || !isDesktop()}>
          Inspect saved job
        </button>
      </div>
      <label>
        Original SVT build for historical imports
        <select
          bind:value={svtBuild}
          disabled={pending}
          onchange={() => {
            inspection = null;
          }}
        >
          <option value="">Not specified</option>
          <option value="svtAv1">SVT-AV1 mainline</option>
          <option value="svtAv1FiveFish">SVT-AV1 5fish</option>
          <option value="svtAv1Hdr">SVT-AV1 HDR</option>
        </select>
      </label>
    </div>
    {#if selectedJob}
      <dl class="job-details">
        <dt>Source</dt>
        <dd>{selectedJob.request.inputPath}</dd>
        <dt>Output</dt>
        <dd>{selectedJob.request.outputPath}</dd>
        {#if selectedJob.error}
          <dt>Error</dt>
          <dd>{selectedJob.error.message}</dd>
        {/if}
      </dl>
    {/if}
    {#if selectedJob?.encodeSettings?.backend === 'standalone' && canResumeJob(selectedJob)}
      <p>{savedProgressSummary(selectedJob)}</p>
      <div class="actions">
        <button
          type="button"
          onclick={() => useProgress(selectedJob!, 'resume')}
          disabled={pending}
        >
          Resume saved job
        </button>
        <button
          type="button"
          onclick={() => (discardTargetId = selectedJob!.id)}
          disabled={pending}
        >
          Discard saved progress
        </button>
      </div>
    {/if}
    {#if inspection}<div class="inspection" role="status">
        <p>{inspection.message}</p>
        {#if inspection.request}<dl>
            <dt>Source</dt>
            <dd>{inspection.request.source.inputPath}</dd>
            <dt>Encoder</dt>
            <dd>{inspection.request.settings.encoder}</dd>
          </dl>
          <button type="button" onclick={queue} disabled={pending}>
            Choose destination and queue a new encode
          </button>{/if}
      </div>{/if}
    {#if message}<p role="status">{message}</p>{/if}{#if error}<p role="alert">{error}</p>{/if}
  </div>
</details>

<SourceDialog
  title={log ? `Job log for ${log.name}` : 'Job log'}
  open={!!log}
  closedby="any"
  onclose={() => (log = undefined)}
  width={820}
>
  {#if log}
    {#await log.result}
      <p role="status">Reading job log…</p>
    {:then result}
      {#if result.notice}
        <p role="status">{result.notice}{result.text ? ' Showing saved diagnostic lines.' : ''}</p>
      {/if}
      {#if result.text}
        <!-- svelte-ignore a11y_no_noninteractive_tabindex (The scrollable job log needs keyboard access.) -->
        <pre class="job-log" role="region" tabindex="0" aria-label="Job log contents"><code
            >{result.text}</code
          ></pre>
      {:else if !result.notice}<p>No log entries were saved for this job.</p>{/if}
    {/await}
  {/if}
</SourceDialog>

<DiscardProgressDialog
  job={discardTarget}
  onclose={() => (discardTargetId = undefined)}
  onconfirm={(job) => {
    discardTargetId = undefined;
    void useProgress(job, 'discard');
  }}
/>

<style>
  .job-details {
    display: grid;
    grid-template-columns: max-content minmax(0, 1fr);
    gap: 0.2031rem 0.6094rem;
  }
  .job-details dt {
    font-weight: 600;
  }
  .job-details dd {
    overflow-wrap: anywhere;
  }
  .job-log {
    max-height: 55dvh;
    overflow: auto;
    white-space: pre-wrap;
    overflow-wrap: anywhere;
  }
  .saved-jobs {
    max-width: 920px;
    margin: 4px 8px 12px;
    border: 1px solid var(--border);
    background: var(--background);
  }
  summary {
    cursor: pointer;
    font-weight: 600;
    font-size: 0.6906rem;
    letter-spacing: 0.08em;
    text-transform: uppercase;
    padding: 8px 12px;
    color: var(--foreground);
  }
  summary:hover {
    background: var(--secondary);
  }
  .saved-jobs-body {
    display: grid;
    gap: 10px;
    padding: 12px;
    border-top: 1px solid var(--rule);
    background: var(--card);
  }
  .saved-jobs-body > p {
    margin: 0;
    max-width: 80ch;
    color: var(--muted-foreground);
  }
  .saved-job-controls {
    display: flex;
    align-items: end;
    flex-wrap: wrap;
    gap: 10px 14px;
  }
  label {
    display: flex;
    flex: 1 1 16.25rem;
    min-width: 0;
    flex-direction: column;
    gap: 0.2844rem;
  }
  .actions {
    display: flex;
    flex-wrap: wrap;
    gap: 8px;
  }
  select,
  button {
    font: inherit;
    padding: 0.325rem 0.5688rem;
    border: 1px solid var(--border);
    background: var(--background);
    color: var(--foreground);
    max-width: 100%;
  }
  button {
    background: var(--secondary);
    color: var(--secondary-foreground);
    cursor: pointer;
  }
  button:hover:not(:disabled) {
    background: var(--accent);
    color: var(--accent-foreground);
  }
  .inspection {
    padding: 10px 12px;
    border: 1px solid var(--rule);
    background: var(--background);
  }
  .inspection p,
  .inspection dl {
    margin-top: 0;
  }
  dl {
    display: grid;
    grid-template-columns: auto minmax(0, 1fr);
    gap: 6px 12px;
  }
  dd {
    margin: 0;
    overflow-wrap: anywhere;
  }
  p[role='alert'] {
    color: var(--destructive);
    font-weight: 600;
  }
</style>
