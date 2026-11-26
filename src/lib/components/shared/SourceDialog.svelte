<script lang="ts">
  import { tick, type Snippet } from 'svelte';
  let {
    title,
    open,
    onclose,
    children,
    width = 580,
    portal = false,
    closedby = 'closerequest',
  }: {
    title: string;
    open: boolean;
    onclose: () => void;
    children: Snippet;
    width?: number;
    portal?: boolean;
    closedby?: 'any' | 'closerequest' | 'none';
  } = $props();
  const headingId = $props.id();
  let element = $state<HTMLDialogElement>();
  let pressedOutside = false;

  function portalHost(node: HTMLDialogElement, enabled: boolean) {
    if (!enabled) return {};
    document.body.appendChild(node);
    return {
      destroy() {
        if (node.open) node.close();
        node.remove();
      },
    };
  }

  function isOutsideDialog(event: PointerEvent, dialog: HTMLDialogElement) {
    const r = dialog.getBoundingClientRect();
    return (
      event.clientX < r.left ||
      event.clientX > r.right ||
      event.clientY < r.top ||
      event.clientY > r.bottom
    );
  }

  $effect(() => {
    if (!open) return;
    let alive = true;
    let modalNode: HTMLDialogElement | undefined;
    const prior = document.activeElement as HTMLElement | null;
    void tick().then(() => {
      if (alive && element && !element.open) {
        modalNode = element;
        modalNode.showModal();
        // Native `autofocus` is handled by `showModal()`; only fall back when absent.
        if (!modalNode.querySelector('[autofocus]')) {
          (
            modalNode.querySelector<HTMLElement>(
              '.body input:not([type=hidden]):not(:disabled), .body select:not(:disabled), .body textarea:not(:disabled), .body button:not(:disabled)',
            ) ?? modalNode.querySelector<HTMLElement>('button:not(:disabled)')
          )?.focus();
        }
      }
    });
    return () => {
      alive = false;
      const closing = element ?? modalNode;
      if (closing?.open) closing.close();
      const target =
        prior && prior.isConnected && !prior.closest('[hidden]')
          ? prior
          : document.querySelector<HTMLElement>(
              'nav button[aria-current="page"], main button:not(:disabled), main',
            );
      target?.focus();
    };
  });
</script>

{#if open}
  <dialog
    use:portalHost={portal}
    bind:this={element}
    aria-labelledby={headingId}
    style={`--dialog-width:${width}px`}
    {...{ closedby } as Record<string, string>}
    onpointerdown={(event) => {
      if (closedby !== 'any' || 'closedBy' in HTMLDialogElement.prototype || !element) return;
      pressedOutside = isOutsideDialog(event, element);
    }}
    onpointerup={(event) => {
      if (closedby !== 'any' || 'closedBy' in HTMLDialogElement.prototype || !element) return;
      if (pressedOutside && isOutsideDialog(event, element)) {
        pressedOutside = false;
        onclose();
      }
    }}
    oncancel={(event) => {
      event.preventDefault();
      if (closedby !== 'none') onclose();
    }}
  >
    <header>
      <h1 id={headingId}>{title}</h1>
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
    width: min(var(--dialog-width), calc(100dvw - 32px));
    max-height: 90dvh;
    overflow: auto;
    padding: 0;
    box-shadow: 5px 5px 0 #41463a66;
  }
  dialog:modal {
    overscroll-behavior: contain;
  }
  @media (prefers-reduced-motion: no-preference) {
    dialog[open] {
      transition: opacity 0.14s ease-out;
      @starting-style {
        opacity: 0;
      }
    }
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
  header h1 {
    margin: 0;
    font-size: 0.8125rem;
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
    font-size: 1.1375rem;
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
