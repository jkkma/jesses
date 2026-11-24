<script lang="ts">
  import SourceDialog from '$lib/components/shared/SourceDialog.svelte';
  import { fileName } from '$lib/components/shared/format';
  import { savedProgressSummary } from '$lib/components/shared/job-state';
  import type { JobSnapshot } from '$lib/ipc/generated';

  let {
    job,
    onclose,
    onconfirm,
  }: {
    job?: JobSnapshot;
    onclose: () => void;
    onconfirm: (job: JobSnapshot) => void;
  } = $props();
</script>

<SourceDialog title="Discard saved progress?" open={!!job} {onclose}>
  {#if job}
    <p><strong>{fileName(job.request.outputPath)}</strong></p>
    <p>{savedProgressSummary(job)}</p>
    <p>Discarding the checkpoint permanently removes this saved work.</p>
    <div class="actions">
      <button type="button" data-dialog-initial-focus onclick={onclose}>Keep saved progress</button>
      <button type="button" onclick={() => onconfirm(job!)}>Discard saved progress</button>
    </div>
  {/if}
</SourceDialog>

<style>
  .actions {
    display: flex;
    flex-wrap: wrap;
    justify-content: flex-end;
    gap: 0.5rem;
    margin-top: 1rem;
  }
  p + p {
    margin-top: 0.5rem;
  }
</style>
