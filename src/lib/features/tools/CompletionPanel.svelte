<script lang="ts">
  import { onMount } from 'svelte';
  import {
    getCompletionStatus,
    setCompletionOptions,
    cancelFinishAction,
    isDesktop,
  } from '$lib/ipc/client';
  import type { CompletionStatus, FinishAction } from '$lib/ipc/generated';
  import { errorMessage } from '$lib/components/shared/format';
  let { settingsVisible = true }: { settingsVisible?: boolean } = $props();
  let status = $state<CompletionStatus>({
    options: { notify: false, finishAction: 'none' },
    armedJobs: 0,
    secondsRemaining: null,
    error: null,
  });
  let action = $state<FinishAction>('none');
  let notify = $state(false);
  let error = $state<string | null>(null);
  let pending = $state(false);
  let initialized = false;
  let actionDirty = false;
  let notifyDirty = false;
  let generation = 0;
  onMount(() => {
    if (!isDesktop()) return;
    let disposed = false;
    let reading = false;
    async function refresh() {
      if (reading || pending) return;
      const current = generation;
      reading = true;
      try {
        const value = await getCompletionStatus();
        if (!disposed && current === generation) {
          const disarmed =
            status.options.finishAction !== 'none' && value.options.finishAction === 'none';
          if (!initialized || !notifyDirty) {
            notify = value.options.notify;
          }
          if (!initialized || !actionDirty || disarmed) {
            action = value.options.finishAction;
            actionDirty = false;
          }
          status = value;
          initialized = true;
          error = null;
        }
      } catch (e) {
        if (!disposed && current === generation) error = errorMessage(e);
      } finally {
        reading = false;
      }
    }
    void refresh();
    const timer = setInterval(refresh, 1000);
    return () => {
      disposed = true;
      clearInterval(timer);
    };
  });
  async function apply() {
    ++generation;
    pending = true;
    error = null;
    try {
      status = await setCompletionOptions({ notify, finishAction: action });
      action = status.options.finishAction;
      notify = status.options.notify;
      actionDirty = notifyDirty = false;
    } catch (e) {
      error = errorMessage(e);
    } finally {
      pending = false;
    }
  }
  async function cancel() {
    ++generation;
    pending = true;
    error = null;
    try {
      status = await cancelFinishAction();
      action = 'none';
      actionDirty = false;
    } catch (e) {
      error = errorMessage(e);
    } finally {
      pending = false;
    }
  }
</script>

{#if status.options.finishAction !== 'none'}
  <aside class="finish-banner" aria-label="Armed finish action">
    <p role="status">
      {status.options.finishAction === 'shutdown' ? 'Computer shutdown' : 'Close jesses'} armed for {status.armedJobs}
      job{status.armedJobs === 1 ? '' : 's'}{status.secondsRemaining === null
        ? ''
        : ` — ${status.secondsRemaining} seconds remaining`}.
    </p>
    <button type="button" onclick={cancel} disabled={pending}>Cancel finish action</button>
  </aside>
{/if}
{#if error || status.error}
  <p class="completion-error" role="alert">{error ?? status.error}</p>
{/if}
<details class="completion-panel" hidden={!settingsVisible}>
  <summary>When the queue finishes</summary>
  <div class="completion-settings">
    <p class="small-muted">
      These choices last for this session. Finish actions require every queued job to succeed and
      wait 60 seconds. Canceling or stopping a job disarms the action.
    </p>
    <div class="completion-fields">
      <label class="notify-field"
        ><input
          type="checkbox"
          bind:checked={notify}
          onchange={() => {
            notifyDirty = true;
          }}
          disabled={pending}
        /> Notify when jobs complete or fail</label
      >
      <label class="action-field"
        >After successful completion <select
          bind:value={action}
          onchange={() => {
            actionDirty = true;
          }}
          disabled={pending}
          aria-label="Finish action"
          ><option value="none">Keep jesses open</option><option value="closeApp"
            >Close jesses</option
          ><option value="shutdown">Shut down the computer</option></select
        ></label
      >
      <button type="button" onclick={apply} disabled={pending || !isDesktop()}
        >Apply to this queue</button
      >
    </div>
  </div>
</details>

<style>
  .finish-banner {
    position: fixed;
    bottom: 56px;
    left: 20px;
    right: 20px;
    z-index: 50;
    display: flex;
    flex-wrap: wrap;
    align-items: center;
    justify-content: space-between;
    gap: 12px;
    padding: 16px;
    background: var(--background, #f0eee6);
    border: 2px solid var(--primary, #ad5326);
    box-shadow: 0 4px 20px #0003;
  }
  .completion-panel {
    margin: 12px 0;
    border: 1px solid var(--border, #c0b7aa);
    background: var(--panel, #e3dacc);
  }
  .completion-error {
    margin: 1rem 0;
    border: 1px solid var(--border, #c0b7aa);
    padding: 1rem;
  }
  summary {
    cursor: pointer;
    font-weight: 600;
    padding: 12px 14px;
  }
  .completion-settings {
    display: grid;
    gap: 12px;
    padding: 14px;
    border-top: 1px solid var(--border, #c0b7aa);
  }
  .completion-fields {
    display: flex;
    flex-wrap: wrap;
    align-items: end;
    gap: 10px 16px;
  }
  .completion-fields label {
    display: flex;
    align-items: center;
    gap: 0.5rem;
  }
  .action-field {
    flex-direction: column;
    align-items: flex-start !important;
  }
  .notify-field {
    padding-block: 0.5rem;
  }
  select,
  button {
    padding: 0.5rem;
    border: 1px solid #9b8c7a;
    background: #f0eee6;
  }
  button {
    cursor: pointer;
  }
  p {
    max-width: 85ch;
  }
  @media (max-width: 800px) {
    .action-field {
      flex: 1 1 16rem;
    }
    .action-field select {
      max-width: 100%;
      width: 100%;
    }
  }
</style>
