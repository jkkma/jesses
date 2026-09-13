<script lang="ts">
  import { untrack } from 'svelte';
  import type { EncodeCommandPlan, EncodeRequest } from '$lib/ipc/generated';
  import { previewEncodePlan } from '$lib/ipc/client';
  let { request, disabled = false }: { request: EncodeRequest | undefined; disabled?: boolean } =
    $props();
  let plan = $state<EncodeCommandPlan | null>(null);
  let busy = $state(false);
  let error = $state<string | null>(null);
  let controller: AbortController | undefined;
  let generation = 0;
  $effect(() => {
    JSON.stringify(request);
    untrack(() => {
      controller?.abort();
      generation++;
      plan = null;
      error = null;
      busy = false;
    });
    return () => {
      controller?.abort();
      generation++;
    };
  });
  async function preview() {
    if (!request || disabled) return;
    controller?.abort();
    const current = ++generation;
    const signal = (controller = new AbortController()).signal;
    const snapshot = structuredClone($state.snapshot(request));
    busy = true;
    error = null;
    plan = null;
    try {
      const result = await previewEncodePlan(snapshot, signal);
      if (!signal.aborted && current === generation) plan = result;
    } catch (cause) {
      if (!signal.aborted && current === generation)
        error =
          cause instanceof Error
            ? cause.message
            : String((cause as { message?: string })?.message ?? cause);
    } finally {
      if (current === generation) busy = false;
    }
  }
  function cancel() {
    controller?.abort();
    generation++;
    busy = false;
    plan = null;
  }
</script>

<section class="command-preview" aria-label="Command plan preview">
  <button type="button" onclick={preview} disabled={disabled || !request || busy}
    >Preview command plan</button
  >
  {#if busy}<button type="button" onclick={cancel}>Cancel command preview</button>
    <p role="status">Validating the complete source and preparing the actual encoder plan…</p>{/if}
  <p class="small-muted">
    Preview validates the whole source and may prepare temporary subtitles or measure target-size
    audio. It creates no final output. Argument edits are restricted to the validated advanced
    catalog.
  </p>
  {#if error}<p role="alert">{error}</p>{/if}
  {#if plan}
    <p role="status">
      Validated plan: {plan.outputFrameCount} frames at {plan.outputFrameRate} fps.
    </p>
    {#each plan.notes as note}<p class="small-muted">{note}</p>{/each}
    {#each plan.stages as stage, i}
      <details>
        <summary>{i + 1}. {stage.label}</summary>
        <p><strong>Executable:</strong> <code>{stage.executable}</code></p>
        {#if stage.workingDirectory}<p>
            <strong>Working directory:</strong> <code>{stage.workingDirectory}</code>
          </p>{/if}
        <p class="small-muted">
          Each JSON string below is one native argument, including its literal spaces and
          punctuation.
        </p>
        <textarea
          readonly
          rows="10"
          aria-label={`${stage.label} argument array`}
          value={JSON.stringify(stage.arguments, null, 2)}></textarea>
        {#each stage.notes as note}<p class="small-muted">{note}</p>{/each}
      </details>
    {/each}
  {/if}
</section>

<style>
  .command-preview {
    display: grid;
    gap: 10px;
    min-width: 0;
    border: 1px solid var(--border);
    padding: 12px;
    border-radius: 8px;
  }
  button {
    border: 1px solid var(--border);
    padding: 8px;
    border-radius: 6px;
    background: var(--background);
    color: var(--foreground);
    cursor: pointer;
    font-size: 12px;
  }
  button:disabled {
    opacity: 0.5;
    cursor: default;
  }
  summary {
    cursor: pointer;
    font-size: 12px;
    font-weight: 600;
  }
  p {
    font-size: 12px;
    overflow-wrap: anywhere;
  }
  textarea {
    width: 100%;
    resize: vertical;
    font-size: 11px;
    max-height: 320px;
    overflow: auto;
    white-space: pre-wrap;
    overflow-wrap: anywhere;
    border: 1px solid var(--border);
    padding: 10px;
  }
  details {
    min-width: 0;
  }
</style>
