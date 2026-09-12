<script lang="ts">
  import { Square } from '@lucide/svelte';
  import { Button } from '$lib/components/ui/button';
  import { errorMessage, formatDuration } from './format';
  import { ProgressEstimator, type ProgressEstimate } from './progress-estimate';
  import type { JobSnapshot } from '$lib/ipc/generated';
  import { encodeSummary } from './encoder-options';
  import { canKeepProgress, canResumeJob, savedProgressSummary, terminalJob } from './job-state';

  let {
    job,
    jobs,
    oncancel,
    onstop,
    onkeep,
    onresume,
  }: {
    job: JobSnapshot | undefined;
    jobs: JobSnapshot[];
    oncancel: (id: string) => Promise<void>;
    onstop: () => Promise<void>;
    onkeep: (id: string) => Promise<void>;
    onresume: (id: string) => Promise<void>;
  } = $props();
  let errors = $state<Record<string, string>>({});
  let queueError = $state<string | null>(null);
  let actions = $state<Record<string, 'cancel' | 'keep' | 'resume' | undefined>>({});
  let stopping = $state(false);
  const pending = $derived(jobs.filter((entry) => !terminalJob(entry.state)));
  const history = $derived(
    [
      ...jobs.filter((entry) => !terminalJob(entry.state) && entry.state !== 'queued'),
      ...jobs.filter((entry) => entry.state === 'queued').reverse(),
      ...jobs.filter((entry) => terminalJob(entry.state)),
    ].filter((entry) => entry.id !== job?.id),
  );
  const validationPhase = $derived(
    job?.encodeSettings && (job.state === 'preparing' || job.state === 'finalizing')
      ? job.state
      : null,
  );
  const progressLabel = $derived(
    validationPhase === 'preparing'
      ? 'Source validation progress'
      : validationPhase === 'finalizing'
        ? 'Output validation progress'
        : job?.encodeSettings
          ? 'Encode progress'
          : 'Remux progress',
  );
  const duration = $derived(
    job?.durationSeconds != null && Number.isFinite(job.durationSeconds) && job.durationSeconds > 0
      ? job.durationSeconds
      : null,
  );
  const progress = $derived(
    job?.progressSeconds != null && Number.isFinite(job.progressSeconds)
      ? Math.max(0, Math.min(job.progressSeconds, duration ?? Infinity))
      : null,
  );
  const estimateKey = $derived(
    job?.encodeSettings && (validationPhase || job.state === 'running')
      ? `${job.id}:${job.state}`
      : null,
  );
  const estimator = new ProgressEstimator();
  let estimate = $state<ProgressEstimate | null>(null);
  $effect(() => {
    estimator.observe(estimateKey, progress, performance.now());
    estimate = estimator.estimate(performance.now(), duration);
  });
  $effect(() => {
    if (estimateKey === null) return;
    const timer = setInterval(() => {
      estimate = estimator.estimate(performance.now(), duration);
    }, 1_000);
    return () => clearInterval(timer);
  });
  async function runAction(id: string, action: 'cancel' | 'keep' | 'resume') {
    if (actions[id] || stopping) return;
    const entry = jobs.find((candidate) => candidate.id === id);
    if (
      !entry ||
      (action === 'keep' && !canKeepProgress(entry)) ||
      (action === 'resume' && !canResumeJob(entry))
    )
      return;
    actions = { ...actions, [id]: action };
    const nextErrors = { ...errors };
    delete nextErrors[id];
    errors = nextErrors;
    try {
      await (action === 'cancel' ? oncancel : action === 'keep' ? onkeep : onresume)(id);
    } catch (cause) {
      errors = { ...errors, [id]: errorMessage(cause) };
    } finally {
      actions = { ...actions, [id]: undefined };
    }
  }
  async function stop() {
    if (stopping) return;
    stopping = true;
    queueError = null;
    try {
      await onstop();
    } catch (cause) {
      queueError = errorMessage(cause);
    } finally {
      stopping = false;
    }
  }
</script>

{#snippet recoveryControls(entry: JobSnapshot)}
  {#if entry.state === 'stopping'}
    <p class="small-muted" role="status">Stopping and saving progress…</p>
  {:else if entry.state === 'stopped' && !entry.recovery}
    <p class="small-muted">Stopped before progress was saved. Start a new encode to try again.</p>
  {:else if canResumeJob(entry)}
    <div class="saved-progress">
      <p><strong>Progress saved · Ready to resume</strong></p>
      <p class="small-muted">{savedProgressSummary(entry)}</p>
    </div>
  {/if}
  {#if canKeepProgress(entry)}
    <Button
      variant="outline"
      disabled={!!actions[entry.id] || stopping}
      onclick={() => runAction(entry.id, 'keep')}
      >{actions[entry.id] === 'keep' ? 'Saving progress…' : 'Stop and keep progress'}</Button
    >
    <p class="small-muted">Keep completed chunks and resume this job later.</p>
  {:else if canResumeJob(entry)}
    <Button
      variant="outline"
      disabled={!!actions[entry.id] || stopping}
      onclick={() => runAction(entry.id, 'resume')}
      >{actions[entry.id] === 'resume' ? 'Resuming…' : 'Resume'}</Button
    >
  {/if}
{/snippet}

{#if job}
  <section
    class="panel job-panel"
    aria-label={job.encodeSettings ? 'Current encode job' : 'Current remux job'}
  >
    <div class="section-heading">
      <span class="eyebrow"
        >{job.encodeSettings
          ? job.encodeSettings.encoder === 'x264'
            ? 'H.264 encode'
            : 'AV1 encode'
          : 'Remux'} · Job status</span
      >
      <strong role="status">{job.state.charAt(0).toUpperCase() + job.state.slice(1)}</strong>
    </div>
    <div class="job-body">
      <p class="job-path">{job.request.outputPath}</p>
      {#if job.encodeSettings}<p class="small-muted">
          {encodeSummary(job.encodeSettings)}
        </p>{/if}
      {#if validationPhase === 'preparing'}
        <p>
          Checking source metadata and frames before encoding. Long videos can take several minutes.
        </p>
      {:else if validationPhase === 'finalizing'}
        <p>
          Combining tracks and checking the output before saving it. Long videos can take several
          minutes.
        </p>
      {/if}
      {#if job.state === 'running' || validationPhase}
        {#if duration !== null && progress !== null}
          <progress aria-label={progressLabel} max={duration} value={progress}></progress>
        {:else}
          <progress aria-label={progressLabel}></progress>
        {/if}
        <p class="small-muted">
          {#if validationPhase === 'preparing'}Source validation ·
          {:else if validationPhase === 'finalizing'}Output validation ·
          {:else if job.encodeSettings}Encoding ·
          {:else}Remuxing ·{/if}
          {#if progress !== null}
            {formatDuration(progress)}{duration !== null ? ` / ${formatDuration(duration)}` : ''}
            of video {validationPhase ? 'scanned' : 'processed'}
            {#if duration === null}
              · Duration unavailable{/if}
          {:else if validationPhase}Waiting for scan progress…
          {:else}Waiting for progress…{/if}
        </p>
        {#if estimate?.kind === 'estimate'}
          <p class="small-muted" aria-label="Current phase estimate">
            Estimated speed ~{estimate.speed.toLocaleString(undefined, {
              minimumSignificantDigits: 2,
              maximumSignificantDigits: 3,
            })}× realtime
            {#if estimate.remainingSeconds !== null}
              · ~{formatDuration(estimate.remainingSeconds)} remaining in
              {validationPhase === 'preparing'
                ? 'source validation'
                : validationPhase === 'finalizing'
                  ? 'output validation'
                  : 'encoding'}
            {/if}
          </p>
        {:else if estimate?.kind === 'waiting'}
          <p class="small-muted" aria-label="Current phase estimate">
            Waiting for new progress; estimate unavailable.
          </p>
        {/if}
      {/if}
      {#if job.state === 'finalizing' && !job.encodeSettings}<p>
          Checking the output before saving it.
        </p>{/if}
      {#if job.state === 'succeeded'}<p>Output verified and saved.</p>{/if}
      {#if job.state === 'interrupted' && !canResumeJob(job)}<p>
          The previous session was interrupted. Review the destination and any temporary files
          before starting a new job. Nothing resumes automatically.
        </p>{/if}
      {#if job.error}<p class="job-error" role="alert">{job.error.message}</p>{/if}
      {#if errors[job.id]}<p class="job-error" role="alert">{errors[job.id]}</p>{/if}
      {@render recoveryControls(job)}
      {#if !terminalJob(job.state)}
        <Button
          variant="outline"
          disabled={job.state === 'canceling' ||
            job.state === 'stopping' ||
            !!actions[job.id] ||
            stopping}
          onclick={() => runAction(job.id, 'cancel')}
        >
          <Square size={12} aria-hidden="true" />Cancel job
        </Button>
        <p class="small-muted">
          Closing jesses cancels active and queued jobs. Saved jobs resume only when you choose
          Resume.
        </p>
      {/if}
      <details>
        <summary>Job log</summary>
        <pre aria-label="Job log">{job.logs.join('\n') || 'Waiting for tool output…'}</pre>
        {#if job.logPath}<p class="small-muted job-path">Tool log path: {job.logPath}</p>{/if}
      </details>
    </div>
  </section>
{/if}

{#if history.length || pending.length}
  <section class="panel job-panel" aria-label="Job queue and history">
    <div class="section-heading">
      <span class="eyebrow">Queue & history</span>
      {#if pending.length}<Button variant="outline" onclick={stop} disabled={stopping}
          >Stop queue</Button
        >{/if}
    </div>
    <div class="queue-body">
      {#if queueError}<p class="job-error" role="alert">{queueError}</p>{/if}
      <p class="small-muted">
        {pending.length} pending · One job runs at a time. Queued jobs appear in processing order.
      </p>
      {#each history as entry (entry.id)}
        <article class="queue-entry" aria-label={`Job ${entry.id}`}>
          <div class="queue-description">
            <strong
              >{entry.encodeSettings
                ? entry.encodeSettings.encoder === 'x264'
                  ? 'H.264 encode'
                  : 'AV1 encode'
                : 'Remux'} · {entry.state.charAt(0).toUpperCase() + entry.state.slice(1)}</strong
            >
            <p class="job-path">{entry.request.outputPath}</p>
            {#if entry.error}<p class="job-error">{entry.error.message}</p>{/if}
            {#if entry.state === 'interrupted' && !canResumeJob(entry)}<p class="small-muted">
                Review the previous output before starting a new job. This job will not resume.
              </p>{/if}
            {#if errors[entry.id]}<p class="job-error" role="alert">{errors[entry.id]}</p>{/if}
            <div class="history-recovery">{@render recoveryControls(entry)}</div>
            <details>
              <summary>Saved settings and log</summary>
              <p class="small-muted job-path">Source: {entry.request.inputPath}</p>
              <p class="small-muted">Selected streams: {entry.request.streamIndices.join(', ')}</p>
              {#if entry.encodeSettings}<p class="small-muted">
                  {encodeSummary(entry.encodeSettings)}
                </p>{/if}
              <pre>{entry.logs.join('\n') || 'No tool output was recorded.'}</pre>
              {#if entry.logPath}<p class="small-muted job-path">
                  Tool log path: {entry.logPath}
                </p>{/if}
            </details>
          </div>
          {#if !terminalJob(entry.state)}<Button
              variant="outline"
              aria-label={`Cancel queued job ${entry.id}`}
              disabled={entry.state === 'canceling' ||
                entry.state === 'stopping' ||
                !!actions[entry.id] ||
                stopping}
              onclick={() => runAction(entry.id, 'cancel')}>Cancel</Button
            >{/if}
        </article>
      {/each}
    </div>
  </section>
{/if}

<style>
  .job-panel {
    min-width: 0;
    margin-top: 20px;
  }
  .section-heading {
    padding: 16px 20px;
  }
  .job-body {
    padding: 20px;
    display: flex;
    flex-direction: column;
    gap: 14px;
  }
  .job-path,
  .job-error {
    overflow-wrap: anywhere;
  }
  .job-error {
    color: #8c2c22;
  }
  progress {
    width: 100%;
    height: 8px;
    accent-color: #ad5326;
  }
  summary {
    cursor: pointer;
    font-size: 12px;
  }
  pre {
    max-height: 240px;
    overflow: auto;
    white-space: pre-wrap;
    overflow-wrap: anywhere;
    font-size: 11px;
    margin-top: 10px;
  }
  .queue-body {
    padding: 16px 20px;
    max-height: 440px;
    overflow-y: auto;
  }
  .queue-entry {
    display: flex;
    align-items: center;
    gap: 16px;
    padding: 14px 0;
    border-top: 1px solid var(--border);
    margin-top: 12px;
  }
  .queue-description {
    min-width: 0;
    flex: 1;
  }
  .queue-description strong {
    font-size: 13px;
  }
  .queue-description p {
    font-size: 12px;
    margin-top: 5px;
  }
  .history-recovery {
    display: grid;
    justify-items: start;
    gap: 8px;
    margin: 8px 0;
  }
  .history-recovery:empty {
    display: none;
  }
  .saved-progress p + p {
    margin-top: 6px;
  }
  @media (max-width: 520px) {
    .queue-entry {
      flex-wrap: wrap;
    }
    .queue-description {
      flex-basis: 100%;
    }
  }
</style>
