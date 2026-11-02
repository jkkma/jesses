<script lang="ts">
  import SourceDialog from './SourceDialog.svelte';

  let {
    open,
    program,
    value = $bindable(),
    onconfirm,
    oncancel,
  }: {
    open: boolean;
    program: 'ffmpeg' | 'av1an';
    value: string;
    onconfirm: () => void;
    oncancel: () => void;
  } = $props();
</script>

<SourceDialog title={`Edit ${program} Arguments`} {open} onclose={oncancel} width={900}>
  <form
    onsubmit={(event) => {
      event.preventDefault();
      onconfirm();
    }}
  >
    <p>Review or edit the command line before it runs. Clearing the box cancels the task.</p>
    <textarea aria-label="Command line" bind:value data-dialog-initial-focus spellcheck="false"
    ></textarea>
    <div class="actions"><button type="submit">Run</button></div>
  </form>
</SourceDialog>

<style>
  form {
    display: flex;
    flex-direction: column;
    height: min(312px, calc(90vh - 80px));
    gap: 8px;
  }
  p {
    margin: 0;
    color: var(--muted-foreground);
  }
  textarea {
    flex: 1;
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
  .actions {
    display: flex;
    justify-content: flex-end;
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
</style>
