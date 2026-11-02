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
          element.querySelector<HTMLElement>('input:not([type=hidden]),select,button')
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
    aria-label={title}
    style={`--dialog-width:${width}px`}
    oncancel={(event) => {
      event.preventDefault();
      onclose();
    }}
  >
    <header><strong>{title}</strong><button aria-label="Close" onclick={onclose}>×</button></header>
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
  dialog::backdrop {
    background: #282c2588;
  }
  header {
    display: flex;
    align-items: center;
    justify-content: space-between;
    padding: 10px 14px;
    border-bottom: 1px solid var(--border);
    background: var(--secondary);
    letter-spacing: 0.08em;
    font-weight: 600;
  }
  header button {
    border: 0;
    background: transparent;
    color: inherit;
    font-size: 20px;
    cursor: pointer;
  }
  .body {
    padding: 14px;
  }
</style>
