<script lang="ts">
  import { exportAnalysis } from '$lib/ipc/client';
  import type { AnalysisExportFormat, AnalysisReport } from '$lib/ipc/generated';
  import { errorMessage } from './format';
  let { report }: { report: AnalysisReport } = $props();
  let pending = $state(false);
  let error = $state<string | null>(null);
  let saved = $state<string | null>(null);
  let generation = 0;
  $effect(() => {
    void report;
    error = null;
    saved = null;
    return () => {
      generation++;
    };
  });
  async function save(format: AnalysisExportFormat) {
    if (pending) return;
    const snapshot = $state.snapshot(report);
    const run = generation;
    pending = true;
    error = null;
    saved = null;
    try {
      const path = await exportAnalysis(snapshot, format);
      if (run === generation) saved = path;
    } catch (cause) {
      if (run === generation) error = errorMessage(cause);
    } finally {
      pending = false;
    }
  }
</script>

<div class="export" aria-label="Export analysis">
  <div class="actions">
    <button type="button" disabled={pending} onclick={() => save('csv')}>Export CSV</button>
    <button type="button" disabled={pending} onclick={() => save('svg')}>Export SVG</button>
  </div>
  <p>
    CSV keeps every measured point. SVG includes chart units and source details. Choose a new
    filename.
  </p>
  {#if pending}<p role="status">Saving analysis…</p>{/if}
  {#if saved}<p role="status">Saved: {saved}</p>{/if}
  {#if error}<p role="alert">{error}</p>{/if}
</div>

<style>
  .export {
    margin-top: 12px;
  }
  .actions {
    display: flex;
    flex-wrap: wrap;
    gap: 8px;
  }
  button {
    font: inherit;
    font-size: 11px;
    color: var(--foreground);
    background: var(--background);
    border: 1px solid var(--border);
    border-radius: 4px;
    padding: 6px 9px;
    cursor: pointer;
  }
  button:focus-visible {
    outline: 2px solid var(--primary);
    outline-offset: 2px;
  }
  button:disabled {
    opacity: 0.5;
    cursor: default;
  }
  p {
    font-size: 11px;
    line-height: 1.5;
    color: var(--muted-foreground);
    margin: 8px 0;
    overflow-wrap: anywhere;
  }
  [role='alert'] {
    color: var(--destructive);
  }
</style>
