<script lang="ts">
  import { onMount } from 'svelte';
  import {
    ArrowRight,
    AudioLines,
    Check,
    CircleAlert,
    Clapperboard,
    FolderOpen,
    FolderOutput,
    History,
    Info,
    LoaderCircle,
    Play,
    SlidersHorizontal,
    Square,
  } from '@lucide/svelte';
  import { Button } from '$lib/components/ui/button';
  import { errorMessage, fileName } from '$lib/components/shared/format';
  import { cancelEncode, chooseOutputDirectory, listJobs, startEncode } from '$lib/ipc/client';
  import type { EncodeJob, EncodeRequest, MediaFile, ToolInfo } from '$lib/ipc/generated';

  let {
    file,
    desktop,
    tools,
    toolsLoading,
    sample,
    onfiles,
    ontools,
    onlog,
  }: {
    file: MediaFile | undefined;
    desktop: boolean;
    tools: ToolInfo[];
    toolsLoading: boolean;
    sample: boolean;
    onfiles: () => void;
    ontools: () => void;
    onlog: (message: string, level?: 'info' | 'error') => void;
  } = $props();

  const requiredTools = [
    { id: 'ffmpeg', name: 'FFmpeg' },
    { id: 'ffprobe', name: 'FFprobe' },
    { id: 'svt-av1', name: 'SVT-AV1' },
  ];
  let crf = $state<number | undefined>(30);
  let preset = $state(4);
  let audioBitrateKbps = $state<number | undefined>(128);
  let audioChannels = $state<number | null>(2);
  let destination = $state('');
  let suffix = $state('_encoded');
  let choosingDirectory = $state(false);
  let starting = $state(false);
  let cancellingId = $state<string | null>(null);
  let jobs = $state<EncodeJob[]>([]);
  let jobsLoading = $state(true);
  let jobsError = $state<string | null>(null);
  let actionError = $state<string | null>(null);
  let mounted = false;
  let listRevision = 0;

  const sourceDirectory = $derived(
    file?.path.slice(0, Math.max(file.path.lastIndexOf('\\'), file.path.lastIndexOf('/')) + 1) ??
      '',
  );
  const outputDirectory = $derived(destination || sourceDirectory);
  const outputPath = $derived.by(() => {
    if (!file || !outputDirectory) return '';
    const dot = file.name.lastIndexOf('.');
    const stem = dot > 0 ? file.name.slice(0, dot) : file.name;
    const separator = outputDirectory.includes('\\') ? '\\' : '/';
    return `${outputDirectory.replace(/[\\/]+$/, '')}${separator}${stem}${suffix}.mkv`;
  });
  const missingTools = $derived(
    requiredTools.filter(
      (required) => !tools.some((tool) => tool.id === required.id && tool.available),
    ),
  );
  const activeJob = $derived(jobs.find(isActive));
  const locked = $derived(starting || choosingDirectory || !!activeJob);
  const validationError = $derived.by(() => {
    if (!suffix.trim()) return 'Enter a filename suffix to keep the source separate.';
    if (/[<>:"/\\|?*\u0000-\u001f]/.test(suffix) || /[.\s]$/.test(suffix)) {
      return 'Use a filename suffix without path characters or a trailing dot or space.';
    }
    if (crf === undefined || !Number.isInteger(crf) || crf < 0 || crf > 63)
      return 'Quality must be a whole number from 0 to 63.';
    if (!Number.isInteger(preset) || preset < 0 || preset > 13)
      return 'Encoder preset must be from 0 to 13.';
    if (
      audioBitrateKbps === undefined ||
      !Number.isInteger(audioBitrateKbps) ||
      audioBitrateKbps < 32 ||
      audioBitrateKbps > 512
    )
      return 'Audio bitrate must be a whole number from 32 to 512 kb/s.';
    if (
      file &&
      outputPath.replaceAll('/', '\\').toLowerCase() ===
        file.path.replaceAll('/', '\\').toLowerCase()
    )
      return 'Choose an output path different from the source.';
    return null;
  });
  const disabledReason = $derived.by(() => {
    if (!desktop || sample) return 'Open a real source in the desktop app to encode.';
    if (toolsLoading) return 'Checking encoder tools…';
    if (missingTools.length)
      return `Required tools: ${missingTools.map((tool) => tool.name).join(', ')}.`;
    if (jobsLoading) return 'Restoring saved jobs…';
    if (jobsError) return 'Waiting for the saved job list to reconnect.';
    if (activeJob) return 'One encode can run at a time. Its progress is shown below.';
    if (!file) return 'Choose a source file to begin.';
    if (!file.streams.some((stream) => stream.kind === 'video'))
      return 'Choose a source containing a video stream.';
    if (!outputPath) return 'Choose an output destination.';
    return validationError;
  });

  $effect(() => {
    // Each source starts with a safe sibling filename; encoder preferences remain editable.
    void file?.path;
    destination = '';
    suffix = '_encoded';
    actionError = null;
  });

  function isActive(job: EncodeJob): boolean {
    return ['preparing', 'encoding', 'muxing', 'validating', 'publishing'].includes(job.status);
  }

  function statusLabel(job: EncodeJob): string {
    return job.status.charAt(0).toUpperCase() + job.status.slice(1);
  }

  onMount(() => {
    mounted = true;
    let timer: ReturnType<typeof setTimeout> | undefined;
    async function poll() {
      const revision = listRevision;
      try {
        const savedJobs = await listJobs();
        if (!mounted || revision !== listRevision) return;
        jobs = savedJobs;
        jobsError = null;
        jobsLoading = false;
      } catch (error) {
        if (!mounted || revision !== listRevision) return;
        jobsError = errorMessage(error);
        jobsLoading = false;
      } finally {
        if (mounted) timer = setTimeout(() => void poll(), 900);
      }
    }
    if (desktop) void poll();
    return () => {
      mounted = false;
      if (timer) clearTimeout(timer);
    };
  });

  async function pickDestination() {
    if (!desktop || locked) return;
    const sourcePath = file?.path;
    choosingDirectory = true;
    actionError = null;
    try {
      const path = await chooseOutputDirectory(outputDirectory || undefined);
      if (mounted && file?.path === sourcePath && path) destination = path;
    } catch (error) {
      if (mounted) actionError = errorMessage(error);
    } finally {
      if (mounted) choosingDirectory = false;
    }
  }

  async function start() {
    if (disabledReason || locked || !file || crf === undefined || audioBitrateKbps === undefined)
      return;
    const request: EncodeRequest = {
      inputPath: file.path,
      outputPath,
      crf,
      preset,
      audioBitrateKbps,
      audioChannels,
    };
    starting = true;
    actionError = null;
    listRevision += 1;
    try {
      const job = await startEncode(request);
      onlog(`Started encode: ${fileName(request.inputPath)} → ${request.outputPath}`);
      if (!mounted) return;
      listRevision += 1;
      jobs = [job, ...jobs.filter((entry) => entry.id !== job.id)];
    } catch (error) {
      const message = errorMessage(error);
      onlog(`Could not start encode: ${message}`, 'error');
      if (mounted) actionError = message;
    } finally {
      if (mounted) starting = false;
    }
  }

  async function cancel(job: EncodeJob) {
    if (cancellingId) return;
    cancellingId = job.id;
    actionError = null;
    try {
      await cancelEncode(job.id);
      onlog(`Cancellation requested: ${fileName(job.request.inputPath)}`);
    } catch (error) {
      const message = errorMessage(error);
      onlog(`Could not cancel encode: ${message}`, 'error');
      if (mounted) actionError = message;
    } finally {
      if (mounted) cancellingId = null;
    }
  }
</script>

<section class="convert-workspace" aria-label="Quick Convert">
  <div class="view-intro">
    <div>
      <span class="eyebrow">Single-file workflow</span>
      <h1>Quick Convert</h1>
      <p>A straightforward setup for your next encode.</p>
    </div>
    <span class="status-label"
      ><SlidersHorizontal size={13} aria-hidden="true" />AV1 + Opus · MKV</span
    >
  </div>
  <div class="notice convert-notice">
    <Info size={16} aria-hidden="true" />
    <p>
      Encodes the first video and all audio tracks. Subtitles, fonts, and chapters are retained.
      Finished files are checked before saving; existing files are never replaced.
    </p>
  </div>
  {#if actionError}
    <div class="notice error-notice convert-notice" role="alert">
      <CircleAlert size={16} aria-hidden="true" />
      <p>{actionError}</p>
    </div>
  {/if}
  <div class="convert-grid">
    <div class="convert-settings">
      <section class="panel settings-panel">
        <div class="section-heading">
          <span class="heading-with-icon"
            ><Clapperboard size={16} aria-hidden="true" /><span class="eyebrow">Video</span></span
          ><span class="small-muted">01</span>
        </div>
        <div class="setting-fields">
          <div class="field full-width">
            <label for="video-encoder">Encoder</label><select id="video-encoder" disabled
              ><option>SVT-AV1 · standalone</option></select
            >
            <p>AV1 software encoding · source dimensions and frame rate</p>
          </div>
          <div class="field">
            <label for="rate-control">Rate control</label><select id="rate-control" disabled
              ><option>Constant quality (CRF)</option></select
            >
          </div>
          <div class="field">
            <label for="quality">Quality</label>
            <div class="input-unit">
              <input
                id="quality"
                type="number"
                min="0"
                max="63"
                step="1"
                bind:value={crf}
                disabled={locked}
              /><span>CRF</span>
            </div>
            <p>0–63 · lower values retain more detail</p>
          </div>
          <div class="field">
            <label for="encoder-preset">Encoder preset</label><select
              id="encoder-preset"
              bind:value={preset}
              disabled={locked}
              >{#each Array.from({ length: 14 }, (_, index) => index) as option}<option
                  value={option}>{option}</option
                >{/each}</select
            >
            <p>0–13 · higher values encode faster</p>
          </div>
          <div class="field">
            <label for="video-dimensions">Dimensions</label><select id="video-dimensions" disabled
              ><option>Keep source dimensions</option></select
            >
          </div>
        </div>
        <div class="panel-footnote">
          <Info size={14} aria-hidden="true" /><span
            >For standard SDR video with a constant frame rate. HDR, interlaced, variable frame
            rate, rotated, and anamorphic sources are not supported yet.</span
          >
        </div>
      </section>
      <section class="panel settings-panel">
        <div class="section-heading">
          <span class="heading-with-icon"
            ><AudioLines size={16} aria-hidden="true" /><span class="eyebrow">Audio</span></span
          ><span class="small-muted">02</span>
        </div>
        <div class="setting-fields audio-fields">
          <div class="field">
            <label for="audio-codec">Codec</label><select id="audio-codec" disabled
              ><option>Opus</option></select
            >
          </div>
          <div class="field">
            <label for="audio-bitrate">Bitrate</label>
            <div class="input-unit">
              <input
                id="audio-bitrate"
                type="number"
                min="32"
                max="512"
                step="1"
                bind:value={audioBitrateKbps}
                disabled={locked}
              /><span>kb/s</span>
            </div>
            <p>Per audio track</p>
          </div>
          <div class="field">
            <label for="audio-channels">Channels</label><select
              id="audio-channels"
              bind:value={audioChannels}
              disabled={locked}
              ><option value={null}>Keep source channels</option><option value={2}>Stereo</option
              ><option value={1}>Mono</option></select
            >
          </div>
        </div>
      </section>
    </div>
    <aside class="panel output-panel" aria-label="Encode output">
      <div class="section-heading">
        <span class="heading-with-icon"
          ><FolderOutput size={16} aria-hidden="true" /><span class="eyebrow">Output</span></span
        >
      </div>
      <div class="output-content">
        <div class="output-source">
          <span class="eyebrow">Source</span><strong>{file?.name ?? 'No source selected'}</strong
          ><button type="button" class="text-button" onclick={onfiles} disabled={starting}
            >{file ? 'Change source' : 'Choose a source'}<ArrowRight
              size={13}
              aria-hidden="true"
            /></button
          >
        </div>
        <div class="field">
          <label for="output-container">Container</label><select id="output-container" disabled
            ><option>Matroska (.mkv)</option></select
          >
        </div>
        <div class="field">
          <label for="output-directory">Destination</label><input
            id="output-directory"
            value={outputDirectory || 'Same folder as source'}
            readonly
            title={outputDirectory || undefined}
          /><button
            type="button"
            class="text-button destination-picker"
            onclick={pickDestination}
            disabled={!desktop || locked}
            ><FolderOpen size={13} aria-hidden="true" />{choosingDirectory
              ? 'Choosing folder…'
              : 'Choose folder'}</button
          >{#if destination}<button
              type="button"
              class="text-button destination-picker"
              onclick={() => (destination = '')}
              disabled={locked}>Use source folder</button
            >{/if}
        </div>
        <div class="field">
          <label for="output-suffix">Filename suffix</label><input
            id="output-suffix"
            bind:value={suffix}
            disabled={locked}
            spellcheck="false"
            aria-invalid={!!validationError}
          />
        </div>
        {#if outputPath && !sample}<div class="output-path">
            <span class="eyebrow">Save as</span>
            <p class="mono">{outputPath}</p>
          </div>{/if}
        <div class="output-summary">
          <SlidersHorizontal size={15} aria-hidden="true" />
          <p><strong>AV1 + Opus</strong><span>CRF {crf ?? '—'} · Preset {preset} · MKV</span></p>
        </div>
        <Button class="start-encode" onclick={start} disabled={!!disabledReason || locked}
          >{#if starting}<LoaderCircle
              size={14}
              class="spinning"
              aria-hidden="true"
            />Starting…{:else}<Play size={14} aria-hidden="true" />Start encode{/if}</Button
        >
        {#if disabledReason}<p class="disabled-reason">{disabledReason}</p>{/if}
        {#if desktop && !toolsLoading && missingTools.length}<button
            type="button"
            class="text-button check-encode-tools"
            onclick={ontools}
            >View tools & environment<ArrowRight size={13} aria-hidden="true" /></button
          >{/if}
      </div>
    </aside>
  </div>

  {#if desktop}
    <section class="panel encode-jobs" aria-label="Encode jobs">
      <div class="section-heading">
        <span class="heading-with-icon"
          ><History size={16} aria-hidden="true" /><span class="eyebrow">Encode jobs</span></span
        ><span class="small-muted">Saved on this computer</span>
      </div>
      {#if jobsError}<div class="notice error-notice jobs-notice" role="alert">
          <CircleAlert size={16} aria-hidden="true" />
          <p>Could not refresh jobs: {jobsError} Retrying automatically.</p>
        </div>{/if}
      {#if jobsLoading}<p class="jobs-empty" role="status">Restoring saved jobs…</p>
      {:else if !jobs.length}<p class="jobs-empty">
          Your encodes will appear here. Progress and results remain available after restarting the
          app.
        </p>
      {:else}
        <div class="job-list">
          {#each jobs as job (job.id)}
            <article class="encode-job" aria-label={`Encode ${fileName(job.request.inputPath)}`}>
              <div class="job-heading">
                <strong>{fileName(job.request.inputPath)}</strong><span
                  class="job-status"
                  class:job-failed={job.status === 'failed' || job.status === 'interrupted'}
                  class:job-completed={job.status === 'completed'}
                  >{#if isActive(job)}<LoaderCircle
                      size={13}
                      class="spinning"
                      aria-hidden="true"
                    />{:else if job.status === 'completed'}<Check
                      size={13}
                      aria-hidden="true"
                    />{:else if job.status === 'failed' || job.status === 'interrupted'}<CircleAlert
                      size={13}
                      aria-hidden="true"
                    />{/if}{statusLabel(job)}</span
                >
              </div>
              <p class="job-path mono" title={job.request.outputPath}>{job.request.outputPath}</p>
              <div class="job-progress-row">
                <p class="job-message" role="status">{job.message}</p>
                {#if job.progress !== null}<span class="mono job-percent"
                    >{Math.round(job.progress)}%</span
                  >{/if}
              </div>
              {#if isActive(job)}<progress
                  max="100"
                  value={job.progress ?? undefined}
                  aria-label={`Encoding progress for ${fileName(job.request.inputPath)}`}
                ></progress>{/if}
              <div class="job-footer">
                <span class="small-muted"
                  >CRF {job.request.crf} · Preset {job.request.preset} · {job.request
                    .audioBitrateKbps} kb/s per audio track</span
                >{#if isActive(job)}<Button
                    variant="outline"
                    onclick={() => cancel(job)}
                    disabled={cancellingId === job.id}
                    ><Square size={11} aria-hidden="true" />{cancellingId === job.id
                      ? 'Cancelling…'
                      : 'Cancel encode'}</Button
                  >{/if}
              </div>
            </article>
          {/each}
        </div>
      {/if}
    </section>
  {/if}
</section>
