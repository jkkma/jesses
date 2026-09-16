<script lang="ts">
  import {
    chooseUtilityFile,
    chooseUtilityDestination,
    inspectSavedJob,
    exportSavedJob,
    isDesktop,
  } from '$lib/ipc/client';
  import type { JobSnapshot, SavedJobInspection, EncodeRequest } from '$lib/ipc/generated';
  import { errorMessage, fileName } from '$lib/components/shared/format';
  let {
    jobs,
    onqueue,
  }: { jobs: JobSnapshot[]; onqueue: (request: EncodeRequest) => Promise<void> } = $props();
  let selected = $state('');
  let inspection = $state<SavedJobInspection | null>(null);
  let pending = $state(false);
  let error = $state<string | null>(null);
  let message = $state<string | null>(null);
  const encodes = $derived(jobs.filter((j) => j.encodeSettings));
  async function inspect() {
    pending = true;
    error = null;
    message = null;
    try {
      const path = await chooseUtilityFile('Inspect saved job', ['json']);
      if (path) {
        inspection = null;
        inspection = await inspectSavedJob(path);
      }
    } catch (e) {
      error = errorMessage(e);
    } finally {
      pending = false;
    }
  }
  async function save() {
    const job = encodes.find((j) => j.id === selected);
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
</script>

<details class="saved-jobs">
  <summary>Saved requests and resume compatibility</summary>
  <p>
    Resume verified work from job history. A saved request can start a new encode; it does not
    authorize running commands or reusing another application's chunks.
  </p>
  <label
    >Encode from history<select bind:value={selected} disabled={pending}
      ><option value="">Choose an encode</option>{#each encodes as job}<option value={job.id}
          >{fileName(job.request.outputPath)} · {job.state}</option
        >{/each}</select
    ></label
  >
  <button type="button" onclick={save} disabled={pending || !selected || !isDesktop()}
    >Export encode request</button
  >
  <button type="button" onclick={inspect} disabled={pending || !isDesktop()}
    >Inspect saved job</button
  >
  {#if inspection}<p role="status">{inspection.message}</p>
    {#if inspection.request}<dl>
        <dt>Source</dt>
        <dd>{inspection.request.source.inputPath}</dd>
        <dt>Encoder</dt>
        <dd>{inspection.request.settings.encoder}</dd>
      </dl>
      <button type="button" onclick={queue} disabled={pending}
        >Choose destination and queue a new encode</button
      >{/if}{/if}
  {#if message}<p role="status">{message}</p>{/if}{#if error}<p role="alert">{error}</p>{/if}
</details>

<style>
  .saved-jobs {
    border: 1px solid #c0b7aa;
    background: #e3dacc;
    padding: 1rem;
    margin: 1rem 0;
  }
  summary {
    cursor: pointer;
    font-weight: 600;
  }
  label {
    display: flex;
    gap: 0.5rem;
    align-items: center;
    margin: 0.75rem 0;
  }
  select,
  button {
    font: inherit;
    padding: 0.5rem;
    border: 1px solid #9b8c7a;
    background: #f0eee6;
  }
  button {
    margin: 0.25rem 0.5rem 0.25rem 0;
    cursor: pointer;
  }
  dd {
    overflow-wrap: anywhere;
  }
</style>
