<script lang="ts">
  import { Square } from '@lucide/svelte';
  import { Button } from '$lib/components/ui/button';
  import { errorMessage, fileName, formatDuration } from './format';
  import { ProgressEstimator, type ProgressEstimate } from './progress-estimate';
  import type { JobSnapshot, MuxRequest } from '$lib/ipc/generated';
  import { encodeSummary, encoderOptions } from './encoder-options';
  import { canKeepProgress, canResumeJob, savedProgressSummary, terminalJob } from './job-state';

  let {
    job,
    jobs,
    oncancel,
    onstop,
    onkeep,
    onresume,
    ondiscard,
    onpause,
  }: {
    job: JobSnapshot | undefined;
    jobs: JobSnapshot[];
    oncancel: (id: string) => Promise<void>;
    onstop: () => Promise<void>;
    onkeep: (id: string) => Promise<void>;
    onresume: (id: string) => Promise<void>;
    ondiscard: (id: string) => Promise<void>;
    onpause: (id: string, paused: boolean) => Promise<void>;
  } = $props();
  let errors = $state<Record<string, string>>({});
  let queueError = $state<string | null>(null);
  let actions = $state<
    Record<string, 'cancel' | 'keep' | 'resume' | 'discard' | 'pause' | undefined>
  >({});
  let confirmDiscard = $state<string | null>(null);
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
  const canDiscard = (entry: JobSnapshot) =>
    entry.encodeSettings?.backend === 'av1an' &&
    !!entry.recovery &&
    ['stopped', 'failed', 'interrupted'].includes(entry.state);
  async function runAction(id: string, action: 'cancel' | 'keep' | 'resume' | 'discard') {
    if (actions[id] || stopping) return;
    const entry = jobs.find((candidate) => candidate.id === id);
    if (
      !entry ||
      (action === 'keep' && !canKeepProgress(entry)) ||
      (action === 'resume' && !canResumeJob(entry)) ||
      (action === 'discard' && (!canDiscard(entry) || confirmDiscard !== id))
    )
      return;
    actions = { ...actions, [id]: action };
    const nextErrors = { ...errors };
    delete nextErrors[id];
    errors = nextErrors;
    try {
      await (
        action === 'cancel'
          ? oncancel
          : action === 'keep'
            ? onkeep
            : action === 'resume'
              ? onresume
              : ondiscard
      )(id);
      if (action === 'discard') confirmDiscard = null;
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
  async function pause(entry: JobSnapshot) {
    if (actions[entry.id] || stopping) return;
    actions = { ...actions, [entry.id]: 'pause' };
    const nextErrors = { ...errors };
    delete nextErrors[entry.id];
    errors = nextErrors;
    try {
      await onpause(entry.id, entry.state !== 'paused');
    } catch (cause) {
      errors = { ...errors, [entry.id]: errorMessage(cause) };
    } finally {
      actions = { ...actions, [entry.id]: undefined };
    }
  }
</script>

{#snippet recoveryControls(entry: JobSnapshot)}
  {#if entry.encodeSettings?.backend === 'av1an' && ['running', 'paused'].includes(entry.state)}
    <Button
      variant="outline"
      disabled={!!actions[entry.id] || stopping}
      onclick={() => pause(entry)}
      >{actions[entry.id] === 'pause'
        ? 'Updating workers…'
        : entry.state === 'paused'
          ? 'Continue encoding'
          : 'Pause encoding'}</Button
    >
    {#if entry.state === 'paused'}<p class="small-muted" role="status">
        Workers are paused. Memory and open files remain in use. Continue, cancel, or stop and keep
        progress.
      </p>{/if}
  {/if}
  {#if entry.state === 'stopping'}
    <p class="small-muted" role="status">Stopping and saving progress…</p>
  {:else if entry.state === 'stopped' && !entry.recovery && !entry.standaloneRecovery}
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
    <p class="small-muted">
      {entry.encodeSettings?.backend === 'standalone'
        ? 'Keeps only a fully verified pass, video, timing, or final-mux boundary. An interrupted phase reruns from its start.'
        : 'Keep completed chunks and resume this job later.'}
    </p>
  {:else if canResumeJob(entry)}
    <Button
      variant="outline"
      disabled={!!actions[entry.id] || stopping}
      onclick={() => runAction(entry.id, 'resume')}
      >{actions[entry.id] === 'resume' ? 'Resuming…' : 'Resume'}</Button
    >
  {/if}
  {#if canDiscard(entry)}
    {#if confirmDiscard === entry.id}
      <div
        class="saved-progress"
        role="group"
        aria-label={`Discard saved progress for job ${entry.id}`}
      >
        <p class="small-muted">
          Delete this job's saved AV1AN work? You will need to start a new encode. The source file
          is untouched.
        </p>
        <div class="discard-actions">
          <Button
            variant="outline"
            disabled={!!actions[entry.id] || stopping}
            onclick={() => runAction(entry.id, 'discard')}
            >{actions[entry.id] === 'discard' ? 'Discarding…' : 'Confirm discard'}</Button
          >
          <Button
            variant="ghost"
            disabled={!!actions[entry.id] || stopping}
            onclick={() => (confirmDiscard = null)}>Keep progress</Button
          >
        </div>
      </div>
    {:else}
      <Button
        variant="ghost"
        disabled={!!actions[entry.id] || stopping}
        onclick={() => (confirmDiscard = entry.id)}>Discard saved progress</Button
      >
    {/if}
  {/if}
{/snippet}

{#snippet muxSettings(request: MuxRequest)}
  <p class="small-muted">
    {request.sources.length} sources · {request.tracks.length} selected tracks
  </p>
  {#each request.sources as source (source.id)}
    <p class="small-muted job-path">Source: {source.inputPath}</p>
  {/each}
  {#each request.tracks as track, position}
    <p class="small-muted job-path">
      Track {position + 1}: {fileName(
        request.sources.find((source) => source.id === track.sourceId)?.inputPath ?? track.sourceId,
      )} · #{track.streamIndex}{track.title != null
        ? ` · Title: ${track.title || '(cleared)'}`
        : ''}{track.language != null
        ? ` · Language: ${track.language || '(cleared)'}`
        : ''}{track.default != null
        ? ` · Default: ${track.default ? 'yes' : 'no'}`
        : ''}{track.forced != null ? ` · Forced: ${track.forced ? 'yes' : 'no'}` : ''}
    </p>
  {/each}
  <p class="small-muted job-path">
    Metadata: {fileName(
      request.sources.find((source) => source.id === request.metadataSourceId)?.inputPath ??
        request.metadataSourceId,
    )} · Chapters: {request.chaptersSourceId
      ? fileName(
          request.sources.find((source) => source.id === request.chaptersSourceId)?.inputPath ??
            request.chaptersSourceId,
        )
      : 'none'}
  </p>
{/snippet}

{#if job}
  <section
    class="panel job-panel"
    aria-label={job.encodeSettings ? 'Current encode job' : 'Current remux job'}
  >
    <div class="section-heading">
      <span class="eyebrow"
        >{job.encodeSettings
          ? `${encoderOptions(job.encodeSettings.encoder).codec} encode`
          : 'Remux'} · Job status</span
      >
      <strong role="status">{job.state.charAt(0).toUpperCase() + job.state.slice(1)}</strong>
    </div>
    <div class="job-body">
      <p class="job-path">{job.request.outputPath}</p>
      {#if job.muxRequest}<details>
          <summary>Saved track settings</summary>{@render muxSettings(job.muxRequest)}
        </details>{/if}
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
      {#if job.state === 'running' || job.state === 'paused' || validationPhase}
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
      {#if job.error?.code === 'OUTPUT_CLEANUP_FAILED' && job.error.path}
        <p class="job-path">Temporary file retained: {job.error.path}</p>
      {/if}
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
                ? `${encoderOptions(entry.encodeSettings.encoder).codec} encode`
                : 'Remux'} · {entry.state.charAt(0).toUpperCase() + entry.state.slice(1)}</strong
            >
            <p class="job-path">{entry.request.outputPath}</p>
            {#if entry.error}<p class="job-error">{entry.error.message}</p>{/if}
            {#if entry.error?.code === 'OUTPUT_CLEANUP_FAILED' && entry.error.path}
              <p class="job-path">Temporary file retained: {entry.error.path}</p>
            {/if}
            {#if entry.state === 'interrupted' && !canResumeJob(entry)}<p class="small-muted">
                Review the previous output before starting a new job. This job will not resume.
              </p>{/if}
            {#if errors[entry.id]}<p class="job-error" role="alert">{errors[entry.id]}</p>{/if}
            <div class="history-recovery">{@render recoveryControls(entry)}</div>
            <details>
              <summary>Saved settings and log</summary>
              {#if entry.muxRequest}
                {@render muxSettings(entry.muxRequest)}
              {:else}
                <p class="small-muted job-path">Source: {entry.request.inputPath}</p>
                <p class="small-muted">
                  Selected streams: {entry.request.streamIndices.join(', ')}
                </p>
              {/if}
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
  .discard-actions {
    display: flex;
    flex-wrap: wrap;
    gap: 8px;
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
