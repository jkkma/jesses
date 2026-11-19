<script lang="ts">
  import { tick, type Snippet } from 'svelte';
  let {
    title,
    open,
    onclose,
    children,
    width = 580,
  }: {
    title: string;
    open: boolean;
    onclose: () => void;
    children: Snippet;
    width?: number;
  } = $props();
  const headingId = $props.id();
  let element = $state<HTMLDialogElement>();
  $effect(() => {
    if (!open) return;
    let alive = true;
    const prior = document.activeElement as HTMLElement | null;
    void tick().then(() => {
      if (alive && element && !element.open) {
        element.showModal();
        (
          element.querySelector<HTMLElement>('[data-dialog-initial-focus]') ??
          element.querySelector<HTMLElement>(
            '.body input:not([type=hidden]):not(:disabled), .body select:not(:disabled), .body textarea:not(:disabled), .body button:not(:disabled)',
          ) ??
          element.querySelector<HTMLElement>('button:not(:disabled)')
        )?.focus();
      }
    });
    return () => {
      alive = false;
      if (element?.open) element.close();
      prior?.focus();
    };
  });
</script>

{#if open}
  <dialog
    bind:this={element}
    aria-labelledby={headingId}
    style={`--dialog-width:${width}px`}
    oncancel={(event) => {
      event.preventDefault();
      onclose();
    }}
  >
    <header>
      <h2 id={headingId}>{title}</h2>
      <button type="button" aria-label="Close" onclick={onclose}>×</button>
    </header>
    <div class="body">{@render children()}</div>
  </dialog>
{/if}

<style>
  dialog {
    position: fixed;
    inset: 0;
    margin: auto;
    color: var(--foreground);
    background: var(--background);
    border: 1px solid var(--border);
    width: min(var(--dialog-width), calc(100vw - 32px));
    max-height: 90vh;
    overflow: auto;
    padding: 0;
    box-shadow: 5px 5px 0 #41463a66;
  }
  dialog:modal {
    overscroll-behavior: contain;
  }
  dialog::backdrop {
    background: #282c2588;
    overflow: hidden;
    overscroll-behavior: contain;
  }
  header {
    display: flex;
    align-items: center;
    justify-content: space-between;
    gap: 8px;
    padding: 10px 14px;
    border-bottom: 1px solid var(--border);
    background: var(--secondary);
  }
  header h2 {
    margin: 0;
    font-size: 1rem;
    font-weight: 600;
    letter-spacing: 0.08em;
  }
  header button {
    display: inline-flex;
    align-items: center;
    justify-content: center;
    min-width: 28px;
    min-height: 28px;
    padding: 0 6px;
    border: 1px solid transparent;
    background: transparent;
    color: inherit;
    font-size: 1.4rem;
    line-height: 1;
    cursor: pointer;
  }
  header button:hover {
    background: var(--accent);
    border-color: var(--border);
  }
  .body {
    padding: 14px;
  }
</style>
