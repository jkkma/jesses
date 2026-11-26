<script lang="ts">
  import { onDestroy } from 'svelte';
  import SourceDialog from './SourceDialog.svelte';
  import CommandPlanPreview from './CommandPlanPreview.svelte';
  import { copyTextWithFeedback } from './clipboard';
  import type { EncodeRequest } from '$lib/ipc/generated';

  let {
    open,
    program,
    value = $bindable(),
    planRequest,
    onconfirm,
    oncancel,
  }: {
    open: boolean;
    program: 'ffmpeg' | 'av1an';
    value: string;
    planRequest?: EncodeRequest;
    onconfirm: () => void;
    oncancel: () => void;
  } = $props();

  let copied = $state(false);
  let copyTimer: ReturnType<typeof setTimeout> | undefined;

  async function copyCommand() {
    if (await copyTextWithFeedback(value)) {
      copied = true;
      clearTimeout(copyTimer);
      copyTimer = setTimeout(() => {
        copied = false;
      }, 2000);
    }
  }

  onDestroy(() => {
    clearTimeout(copyTimer);
  });
</script>

<SourceDialog
  title={`Edit ${program} Arguments`}
  {open}
  portal={true}
  onclose={oncancel}
  width={900}
>
  <form
    onsubmit={(event) => {
      event.preventDefault();
      onconfirm();
    }}
  >
    <p>Review or edit the command line before it runs. Clearing the box cancels the task.</p>
    <!-- svelte-ignore a11y_autofocus -->
    <textarea
      aria-label="Command line"
      bind:value
      autofocus
      spellcheck="false"
      onkeydown={(event) => {
        if ((event.ctrlKey || event.metaKey) && event.key === 'Enter' && !event.isComposing) {
          event.preventDefault();
          onconfirm();
        }
      }}></textarea>
    {#if planRequest}
      <details class="stage-plan">
        <summary>Per-stage native arguments</summary>
        <p class="plan-note">
          Shows the native per-stage arguments for the prepared settings before edits above.
        </p>
        <CommandPlanPreview request={planRequest} />
      </details>
    {/if}
    <div class="actions">
      <button type="button" class="cancel" onclick={() => void copyCommand()}>
        {copied ? 'Copied' : 'Copy command'}
      </button>
      <button type="button" class="cancel" onclick={oncancel}>Cancel</button>
      <button type="submit">Run</button>
    </div>
  </form>
</SourceDialog>

<style>
  form {
    display: flex;
    flex-direction: column;
    min-height: min(312px, calc(90vh - 80px));
    max-height: calc(90vh - 80px);
    overflow-y: auto;
    gap: 8px;
  }
  p {
    margin: 0;
    color: var(--muted-foreground);
  }
  textarea {
    flex: 1 0 180px;
    min-height: 80px;
    width: 100%;
    box-sizing: border-box;
    resize: vertical;
    padding: 7px;
    border: 1px solid var(--border);
    background: var(--background);
    color: var(--foreground);
    font-family: ui-monospace, SFMono-Regular, Consolas, monospace;
    white-space: pre-wrap;
  }
  .stage-plan {
    font-size: 0.7475rem;
  }
  .stage-plan summary {
    cursor: pointer;
    color: var(--muted-foreground);
    margin-bottom: 6px;
  }
  .actions {
    display: flex;
    justify-content: flex-end;
    gap: 8px;
    padding-top: 4px;
  }
  button {
    min-width: 100px;
    padding: 6px 12px;
    border: 1px solid var(--border);
    background: var(--primary);
    color: var(--primary-foreground);
    cursor: pointer;
  }
  button.cancel {
    background: var(--secondary);
    color: var(--secondary-foreground);
  }
  button.cancel:hover:not(:disabled) {
    background: var(--accent);
    color: var(--accent-foreground);
  }
</style>
