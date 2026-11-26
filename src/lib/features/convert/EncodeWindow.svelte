<script lang="ts">
  import { onMount, tick } from 'svelte';
  import {
    canKeepProgressOnStop,
    canResumeJob,
    canStopJob,
    hasSavedProgress,
    terminalJob,
  } from '$lib/components/shared/job-state';
  import DiscardProgressDialog from '$lib/components/shared/DiscardProgressDialog.svelte';
  import { displayPaths, errorMessage, fileName, formatBytes } from '$lib/components/shared/format';
  import {
    isEncoderProgressLine,
    liveJobProgress,
    withoutAttachmentProbeNoise,
  } from '$lib/components/shared/job-progress';
  import {
    ProgressEstimator,
    type ProgressEstimate,
  } from '$lib/components/shared/progress-estimate';
  import { encodeSummary } from '$lib/components/shared/encoder-options';
  import { copyTextWithFeedback } from '$lib/components/shared/clipboard';
  import { highlightLogLine, type HighlightedLogLine } from './log-highlight';
  import {
    subscribeJobs,
    cancelJob,
    cancelQueuedJobs,
    discardJobProgress,
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
  let discardTargetId = $state<string>();
  const discardTarget = $derived(
    jobs.find((job) => job.id === discardTargetId && canResumeJob(job)),
  );
  let actionError = $state<{ id: string; message: string } | null>(null);
  let logCopied = $state(false);
  let copyTimer: ReturnType<typeof setTimeout> | undefined;
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
      ? displayPaths(
          [
            `Source: ${current.request.inputPath}`,
            `Output: ${current.request.outputPath}`,
            current.encodeSettings ? encodeSummary(current.encodeSettings) : '',
            '',
            ...withoutAttachmentProbeNoise(current.logs).filter(
              (line) => !isEncoderProgressLine(line),
            ),
          ].join('\n'),
        )
      : 'Waiting for a Quick Convert job…',
  );
  let highlightedCache = new Map<string, HighlightedLogLine>();
  function highlightLogLines(text: string) {
    const nextCache = new Map<string, HighlightedLogLine>();
    const lines = text.split('\n').map((line) => {
      let parsed = highlightedCache.get(line) ?? nextCache.get(line);
      if (!parsed) parsed = highlightLogLine(line);
      nextCache.set(line, parsed);
      return parsed;
    });
    highlightedCache = nextCache;
    return lines;
  }
  const highlightedLog = $derived(highlightLogLines(logText));
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

  async function copyLog() {
    if (!logText) return;
    const copied = await copyTextWithFeedback(logText, {
      success: 'Job log copied.',
      fail: 'Could not copy job log.',
    });
    if (!copied) {
      if (current) actionError = { id: current.id, message: 'Could not copy job log.' };
      return;
    }
    logCopied = true;
    clearTimeout(copyTimer);
    copyTimer = setTimeout(() => {
      logCopied = false;
    }, 2000);
  }

  async function act(
    job: JobSnapshot,
    action: 'pause' | 'cancel' | 'cancelQueued' | 'stop' | 'resume' | 'discard' | 'folder',
  ) {
    if (busy) return;
    busy = true;
    actionError = null;
    try {
      if (action === 'folder') {
        const directory = parentDirectory(job.request.outputPath);
        if (!directory) throw new Error('The output has no parent folder.');
        await openDirectory(directory);
      } else if (action === 'cancelQueued') {
        const canceled = await cancelQueuedJobs([job.id]);
        const updated = canceled.find((entry) => entry.id === job.id);
        if (!updated) {
          actionError = { id: job.id, message: 'Job is no longer queued.' };
          return;
        }
        jobs = jobs.map((entry) => (entry === job ? updated : entry));
      } else {
        const result = await (action === 'pause'
          ? setJobPaused(job.id, job.state !== 'paused')
          : action === 'cancel'
            ? cancelJob(job.id)
            : action === 'stop'
              ? stopJob(job.id)
              : action === 'discard'
                ? discardJobProgress(job.id)
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
      clearTimeout(copyTimer);
      unsubscribe?.();
    };
  });
</script>

<svelte:head>
  <title
    >{current ? `Processing: ${fileName(current.request.outputPath)} — ` : ''}Quick Convert — jesses</title
  >
</svelte:head>

<main class="encode-window">
  <header>
    <h1>Quick Convert</h1>
    <span class="filename" title={current?.request.outputPath}>
      {current ? fileName(current.request.outputPath) : 'Processing'}
    </span>
    <span class="queue-count">
      {quickJobs.filter((job) => job.state === 'queued').length} queued
    </span>
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
      <p class="error" role="alert">Could not connect to job progress: {connectionError}</p>
      <button type="button" onclick={connect} disabled={connecting}>Reconnect</button>
    {:else if !connected}<p role="status">Connecting to the encode…</p>
    {:else if !current}<p role="status">No Quick Convert jobs yet.</p>{/if}
    {#if current && progress}
      <div class="status-line">
        <strong role="status">{stateLabel}</strong>
        {#if current.state === 'succeeded'}
          <span>Output verified and saved.</span>
          {#if current.outputSizeBytes}<span>{formatBytes(current.outputSizeBytes)}</span>{/if}
        {:else if !finished}<output aria-label="Encoder statistics" aria-live="off">
            {progress.text}
          </output>
        {:else if current.state === 'stopped' && !hasSavedProgress(current)}
          <span>Start a new job to encode this source.</span>
        {/if}
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
      <button type="button" onclick={() => void copyLog()} disabled={!logText}>
        {logCopied ? 'Copied' : 'Copy log'}
      </button>
      {#if current}
        <button
          type="button"
          class="folder"
          onclick={() => act(current!, 'folder')}
          disabled={busy}
        >
          Open output folder
        </button>
        {#if canStopJob(current.state)}
          <button
            type="button"
            aria-label={current.state === 'paused' ? 'Continue encoding' : 'Pause encoding'}
            onclick={() => act(current!, 'pause')}
            disabled={busy}
          >
            {current.state === 'paused' ? 'Resume' : 'Pause'}
          </button>
          <button
            type="button"
            title={hasSavedProgress(current)
              ? 'Keep the saved checkpoint. Resume restarts the interrupted phase.'
              : 'Stop this job. A new run starts from the source.'}
            onclick={() => act(current!, canKeepProgressOnStop(current!) ? 'stop' : 'cancel')}
            disabled={busy}
          >
            {hasSavedProgress(current) ? 'Stop and keep progress' : 'Stop'}
          </button>
        {:else if canResumeJob(current)}
          <button type="button" onclick={() => act(current!, 'resume')} disabled={busy}>
            Resume saved job
          </button>
          <button
            type="button"
            class="discard"
            onclick={() => (discardTargetId = current!.id)}
            disabled={busy}
          >
            Discard saved progress
          </button>
        {:else if current.state === 'queued'}<button
            type="button"
            onclick={() => act(current!, 'cancelQueued')}
            disabled={busy}
          >
            Cancel job
          </button>{/if}
      {/if}
    </div>
    <p class="close-note">
      Closing this window leaves the encode running. Closing jesses stops all jobs.
    </p>
  </footer>
</main>

<DiscardProgressDialog
  job={discardTarget}
  onclose={() => (discardTargetId = undefined)}
  onconfirm={(job) => {
    discardTargetId = undefined;
    void act(job, 'discard');
  }}
/>

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
    container-type: inline-size;
    font:
      0.7475rem/1.45 'Segoe UI',
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
    font-size: 0.8125rem;
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
    scrollbar-gutter: stable;
    white-space: pre-wrap;
    overflow-wrap: anywhere;
    border: 1px solid var(--border);
    background: var(--card);
    color: var(--foreground);
    font:
      0.7475rem/1.5 Consolas,
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
  .status-line output,
  .error {
    overflow-wrap: anywhere;
    min-width: 0;
  }
  progress {
    appearance: none;
    container-type: inline-size;
    width: 100%;
    height: 16px;
    border: 1px solid var(--border);
    border-radius: 0;
    overflow: clip;
    background: var(--muted);
    accent-color: var(--primary);
    --progress-bar-indeterminate-size: 35%;
    --progress-bar-indeterminate-animation-duration: 1.5s;
  }
  @media (prefers-reduced-motion: reduce) {
    progress {
      --progress-bar-indeterminate-animation-duration: 10s;
    }
  }
  progress:indeterminate {
    --progress-bar-animation: working var(--progress-bar-indeterminate-animation-duration) infinite
      linear;
  }
  progress::-webkit-progress-bar {
    background: transparent;
  }
  progress::-webkit-progress-value {
    background: var(--primary);
    transition: inline-size 0.25s;
    animation: var(--progress-bar-animation, revert-rule);
  }
  progress::-moz-progress-bar {
    background: var(--primary);
    transition: inline-size 0.25s;
    animation: var(--progress-bar-animation, revert-rule);
  }
  @keyframes working {
    from,
    to {
      inline-size: var(--progress-bar-indeterminate-size);
      margin-right: auto;
    }
    from {
      translate: -100%;
    }
    to {
      translate: 100cqi;
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
  .discard {
    border-color: var(--destructive);
    color: var(--destructive);
  }
  .discard:hover:not(:disabled) {
    border-color: var(--destructive);
    background: var(--destructive);
    color: var(--primary-foreground);
  }
  .close-note {
    margin: 6px 0 0;
    color: var(--muted-foreground);
    font-size: 0.6906rem;
  }
  .error {
    color: var(--destructive);
  }
  @media (max-width: 540px) {
    .encode-window {
      padding: 8px;
      gap: 6px;
    }
  }
  @container (inline-size <= 520px) {
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
</style>
