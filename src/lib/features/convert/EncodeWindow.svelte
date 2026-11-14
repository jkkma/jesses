<script lang="ts">
  import { onMount, tick } from 'svelte';
  import { canKeepProgress, canResumeJob, terminalJob } from '$lib/components/shared/job-state';
  import { errorMessage, fileName, formatBytes } from '$lib/components/shared/format';
  import { liveJobProgress } from '$lib/components/shared/job-progress';
  import {
    ProgressEstimator,
    type ProgressEstimate,
  } from '$lib/components/shared/progress-estimate';
  import { encodeSummary } from '$lib/components/shared/encoder-options';
  import { highlightLogLine } from './log-highlight';
  import {
    subscribeJobs,
    cancelJob,
    stopJob,
    resumeJob,
    setJobPaused,
    openDirectory,
    parentDirectory,
  } from '$lib/ipc/client';
  import type { JobSnapshot } from '$lib/ipc/generated';

  let jobs = $state<JobSnapshot[]>([]);
  let connected = $state(false);
  let connectionError = $state('');
  let connecting = $state(false);
  let disposed = false;
  let unsubscribe: (() => void) | undefined;
  let busy = $state(false);
  let actionError = $state<{ id: string; message: string } | null>(null);
  let logElement: HTMLPreElement | undefined = $state();
  let followLog = true;
  let logJobId: string | undefined;
  const quickJobs = $derived(jobs.filter((job) => job.encodeSettings?.backend === 'standalone'));
  const current = $derived(
    quickJobs.find((job) => !terminalJob(job.state) && job.state !== 'queued') ??
      quickJobs.filter((job) => job.state === 'queued').at(-1) ??
      quickJobs[0],
  );
  const finished = $derived(current ? terminalJob(current.state) : false);
  const stateLabel = $derived(
    current ? current.state.charAt(0).toUpperCase() + current.state.slice(1) : 'Ready',
  );
  const estimator = new ProgressEstimator();
  let estimate = $state<ProgressEstimate | null>(null);
  const estimateKey = $derived(
    current && ['running', 'preparing', 'finalizing'].includes(current.state)
      ? `${current.id}:${current.state}`
      : null,
  );
  $effect(() => {
    estimator.observe(estimateKey, current?.progressSeconds ?? null, performance.now());
    estimate = estimator.estimate(performance.now(), current?.durationSeconds ?? null);
  });
  $effect(() => {
    if (!estimateKey) return;
    const timer = setInterval(() => {
      estimate = estimator.estimate(performance.now(), current?.durationSeconds ?? null);
    }, 1000);
    return () => clearInterval(timer);
  });
  const progress = $derived(current ? liveJobProgress(current, estimate) : null);
  const logText = $derived(
    current
      ? [
          `Source: ${current.request.inputPath}`,
          `Output: ${current.request.outputPath}`,
          current.encodeSettings ? encodeSummary(current.encodeSettings) : '',
          '',
          ...current.logs,
        ].join('\n')
      : 'Waiting for a Quick Convert job…',
  );
  const highlightedLog = $derived(logText.split('\n').map(highlightLogLine));
  $effect(() => {
    const id = current?.id;
    const text = logText;
    if (!logElement || !text) return;
    if (id !== logJobId) {
      logJobId = id;
      followLog = true;
    }
    if (followLog)
      void tick().then(() => {
        if (followLog && logElement) logElement.scrollTop = logElement.scrollHeight;
      });
  });

  async function connect() {
    if (connecting) return;
    connecting = true;
    connectionError = '';
    unsubscribe?.();
    try {
      const stop = await subscribeJobs((snapshot) => {
        if (disposed) return;
        jobs = snapshot;
        connected = true;
      });
      if (disposed) stop();
      else unsubscribe = stop;
    } catch (error) {
      if (!disposed) connectionError = errorMessage(error);
    } finally {
      connecting = false;
    }
  }

  async function act(job: JobSnapshot, action: 'pause' | 'cancel' | 'keep' | 'resume' | 'folder') {
    if (busy) return;
    busy = true;
    actionError = null;
    try {
      if (action === 'folder') {
        const directory = parentDirectory(job.request.outputPath);
        if (!directory) throw new Error('The output has no parent folder.');
        await openDirectory(directory);
      } else {
        const result = await (action === 'pause'
          ? setJobPaused(job.id, job.state !== 'paused')
          : action === 'cancel'
            ? cancelJob(job.id)
            : action === 'keep'
              ? stopJob(job.id)
              : resumeJob(job.id));
        // A subscription may beat the command reply; retain any newer snapshot.
        jobs = jobs.map((entry) => (entry === job ? result : entry));
      }
    } catch (error) {
      actionError = { id: job.id, message: errorMessage(error) };
    } finally {
      busy = false;
    }
  }

  onMount(() => {
    void connect();
    return () => {
      disposed = true;
      unsubscribe?.();
    };
  });
</script>

<svelte:head
  ><title
    >{current ? `Processing: ${fileName(current.request.outputPath)} — ` : ''}Quick Convert — jesses</title
  ></svelte:head
>

<main class="encode-window">
  <header>
    <h1>Quick Convert</h1>
    <span class="filename" title={current?.request.outputPath}
      >{current ? fileName(current.request.outputPath) : 'Processing'}</span
    >
    <span class="queue-count"
      >{quickJobs.filter((job) => job.state === 'queued').length} queued</span
    >
  </header>
  <!-- svelte-ignore a11y_no_noninteractive_tabindex (The scrollable log needs keyboard access.) -->
  <pre
    class="tool-output"
    role="log"
    aria-live="off"
    aria-label="Job log"
    tabindex="0"
    bind:this={logElement}
    onscroll={() => {
      if (logElement)
        followLog = logElement.scrollHeight - logElement.scrollTop - logElement.clientHeight < 24;
    }}>{#each highlightedLog as line, index}<span class="log-line" data-tone={line.tone}
        >{#each line.tokens as token}<span class="log-token" data-kind={token.kind}
            >{token.text}</span
          >{/each}</span
      >{index < highlightedLog.length - 1 ? '\n' : ''}{/each}</pre>
  <section class="progress-area" aria-label="Encoding status">
    {#if connectionError}
      <p role="alert">Could not connect to job progress: {connectionError}</p>
      <button type="button" onclick={connect} disabled={connecting}>Reconnect</button>
    {:else if !connected}<p role="status">Connecting to the encode…</p>
    {:else if !current}<p role="status">No Quick Convert jobs yet.</p>{/if}
    {#if current && progress}
      <div class="status-line">
        <strong role="status">{stateLabel}</strong>
        {#if current.state === 'succeeded'}
          <span>Output verified and saved.</span>
          {#if current.outputSizeBytes}<span>{formatBytes(current.outputSizeBytes)}</span>{/if}
        {:else if !finished}<span aria-label="Encoder statistics">{progress.text}</span>{/if}
      </div>
      {#if current.state === 'succeeded'}<progress aria-label="Encode progress" value="1" max="1"
        ></progress>
      {:else if !finished}<progress
          aria-label="Encode progress"
          value={progress.value ?? undefined}
          max={progress.max}
        ></progress>{/if}
      {#if current.error}<p class="error" role="alert">{current.error.message}</p>{/if}
      {#if actionError?.id === current.id}<p class="error" role="alert">
          {actionError.message}
        </p>{/if}
    {/if}
  </section>
  <footer>
    <div class="controls">
      {#if current}
        <button type="button" class="folder" onclick={() => act(current!, 'folder')} disabled={busy}
          >Open output folder</button
        >
        {#if ['preparing', 'running', 'paused', 'finalizing'].includes(current.state)}
          <button
            type="button"
            aria-label={current.state === 'paused' ? 'Continue encoding' : 'Pause encoding'}
            onclick={() => act(current!, 'pause')}
            disabled={busy}>{current.state === 'paused' ? 'Resume' : 'Pause'}</button
          >
        {/if}
        {#if canKeepProgress(current)}
          <button
            type="button"
            title="Save completed phases. The interrupted phase restarts when resumed."
            onclick={() => act(current!, 'keep')}
            disabled={busy}>Stop and keep progress</button
          >
        {:else if canResumeJob(current)}
          <button type="button" onclick={() => act(current!, 'resume')} disabled={busy}
            >Resume saved job</button
          >
        {/if}
        {#if !finished}<button
            type="button"
            class="abort"
            aria-label="Cancel job"
            onclick={() => act(current!, 'cancel')}
            disabled={busy || ['canceling', 'stopping'].includes(current.state)}>Abort</button
          >{/if}
      {/if}
    </div>
    <p class="close-note">
      Closing this window leaves the encode running. Closing jesses stops all jobs.
    </p>
  </footer>
</main>

<style>
  :global(body:has(.encode-window)) {
    min-width: 320px;
  }
  .encode-window {
    display: grid;
    grid-template-rows: auto minmax(100px, 1fr) auto auto;
    gap: 8px;
    height: 100dvh;
    min-height: 360px;
    padding: 10px;
    font:
      12px/1.45 'Segoe UI',
      sans-serif;
  }
  header {
    display: flex;
    align-items: center;
    gap: 12px;
    min-width: 0;
  }
  h1 {
    flex-shrink: 0;
    margin: 0;
    font-size: 13px;
    font-weight: 600;
  }
  .filename {
    min-width: 0;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
    color: var(--muted-foreground);
  }
  .queue-count {
    margin-left: auto;
    flex-shrink: 0;
    color: var(--muted-foreground);
  }
  .tool-output {
    min-width: 0;
    min-height: 0;
    margin: 0;
    padding: 12px;
    overflow: auto;
    white-space: pre-wrap;
    overflow-wrap: anywhere;
    border: 1px solid var(--border);
    background: var(--card);
    color: var(--foreground);
    font:
      12px/1.5 Consolas,
      'Cascadia Mono',
      monospace;
    tab-size: 4;
  }
  .progress-area {
    min-width: 0;
    display: grid;
    gap: 6px;
  }
  .log-token[data-kind='label'],
  .log-token[data-kind='option'] {
    color: var(--primary);
    font-weight: 600;
  }
  .log-token[data-kind='tool'],
  .log-token[data-kind='operator'] {
    color: var(--muted-foreground);
  }
  .log-token[data-kind='string'] {
    color: var(--jesses-rust-text);
  }
  .log-token[data-kind='number'],
  .log-line[data-tone='success'] {
    color: var(--success);
  }
  .log-line[data-tone='progress'] {
    color: var(--jesses-rust-text);
  }
  .log-line[data-tone='phase'] {
    color: var(--primary);
    font-weight: 600;
  }
  .log-line[data-tone='warning'] {
    color: color-mix(in srgb, var(--destructive) 75%, var(--foreground));
  }
  .log-line[data-tone='error'] {
    color: var(--destructive);
    background: color-mix(in srgb, var(--destructive) 8%, transparent);
    font-weight: 600;
  }
  .log-line[data-tone='success'] {
    font-weight: 600;
  }
  .log-line[data-tone='error'] .log-token,
  .log-line[data-tone='warning'] .log-token,
  .log-line[data-tone='success'] .log-token {
    color: inherit;
  }
  .status-line {
    display: flex;
    align-items: baseline;
    flex-wrap: wrap;
    gap: 4px 12px;
  }
  .status-line strong {
    font-weight: 600;
  }
  .status-line span,
  .error {
    overflow-wrap: anywhere;
    min-width: 0;
  }
  progress {
    appearance: none;
    width: 100%;
    height: 16px;
    border: 1px solid var(--border);
    border-radius: 0;
    background: var(--muted);
    accent-color: var(--primary);
  }
  progress::-webkit-progress-bar {
    background: var(--muted);
  }
  progress::-webkit-progress-value {
    background: var(--primary);
  }
  progress:indeterminate {
    background: linear-gradient(90deg, var(--muted) 25%, var(--accent) 50%, var(--muted) 75%);
    background-size: 200% 100%;
    animation: working 1.6s linear infinite;
  }
  @keyframes working {
    from {
      background-position: 100% 0;
    }
    to {
      background-position: -100% 0;
    }
  }
  footer {
    min-width: 0;
  }
  .controls {
    display: flex;
    flex-wrap: wrap;
    justify-content: flex-end;
    gap: 6px;
  }
  button {
    min-height: 30px;
    min-width: 76px;
    max-width: 100%;
    padding: 4px 12px;
    border: 1px solid var(--border);
    border-radius: var(--radius);
    background: var(--background);
    color: var(--foreground);
    font: inherit;
    white-space: normal;
  }
  button:hover:not(:disabled) {
    border-color: var(--primary);
    background: var(--primary);
    color: var(--primary-foreground);
  }
  button:disabled {
    opacity: 0.55;
  }
  button:focus-visible,
  .tool-output:focus-visible {
    outline: 2px solid var(--ring);
    outline-offset: 1px;
  }
  .folder {
    margin-right: auto;
  }
  .abort {
    color: var(--destructive);
  }
  .close-note {
    margin: 6px 0 0;
    color: var(--muted-foreground);
    font-size: 11px;
  }
  .error {
    color: var(--destructive);
  }
  @media (max-width: 540px) {
    .encode-window {
      padding: 8px;
      gap: 6px;
    }
    header {
      flex-wrap: wrap;
      gap: 3px 10px;
    }
    .filename {
      order: 2;
      flex-basis: 100%;
    }
    .controls {
      display: grid;
      grid-template-columns: repeat(2, minmax(0, 1fr));
    }
    .folder {
      margin: 0;
    }
    .tool-output {
      padding: 8px;
    }
  }
  @media (prefers-reduced-motion: reduce) {
    progress:indeterminate {
      animation: none;
    }
  }
</style>
