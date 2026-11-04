<script lang="ts">
  import { onMount } from 'svelte';
  import { getCurrentWindow } from '@tauri-apps/api/window';
  import { Copy, Minus, Square, X } from '@lucide/svelte';
  import { errorMessage } from '$lib/components/shared/format';

  let { onerror }: { onerror: (message: string) => void } = $props();
  const appWindow = getCurrentWindow();
  let maximized = $state(false);
  let disposed = false;

  async function syncMaximized() {
    const value = await appWindow.isMaximized();
    if (!disposed) maximized = value;
  }

  onMount(() => {
    let stopResize: (() => void) | undefined;
    const refresh = () => {
      void syncMaximized().catch((error) => {
        if (!disposed) onerror(`Could not read window state: ${errorMessage(error)}`);
      });
    };
    refresh();
    void appWindow.onResized(refresh).then(
      (stop) => {
        if (disposed) stop();
        else stopResize = stop;
      },
      (error) => {
        if (!disposed) onerror(`Could not track window state: ${errorMessage(error)}`);
      },
    );
    return () => {
      disposed = true;
      stopResize?.();
    };
  });

  async function control(action: 'minimize' | 'toggleMaximize' | 'close') {
    try {
      // A normal close request preserves the desktop's job shutdown handling.
      await appWindow[action]();
      if (action === 'toggleMaximize') await syncMaximized();
    } catch (error) {
      onerror(
        `Could not ${action === 'toggleMaximize' ? 'resize' : action} window: ${errorMessage(error)}`,
      );
    }
  }
</script>

<div class="window-controls" role="group" aria-label="Window controls">
  <button
    type="button"
    aria-label="Minimize window"
    title="Minimize window"
    onclick={() => control('minimize')}><Minus size={14} aria-hidden="true" /></button
  >
  <button
    type="button"
    aria-label={maximized ? 'Restore window' : 'Maximize window'}
    title={maximized ? 'Restore window' : 'Maximize window'}
    onclick={() => control('toggleMaximize')}
  >
    {#if maximized}
      <Copy size={13} aria-hidden="true" />
    {:else}
      <Square size={13} aria-hidden="true" />
    {/if}
  </button>
  <button
    class="close"
    type="button"
    aria-label="Close window"
    title="Close window"
    onclick={() => control('close')}><X size={16} aria-hidden="true" /></button
  >
</div>

<style>
  .window-controls {
    display: flex;
    align-self: stretch;
    flex-shrink: 0;
    -webkit-app-region: no-drag;
  }
  button {
    display: inline-flex;
    align-items: center;
    justify-content: center;
    width: 46px;
    min-height: 32px;
    padding: 0;
    border: 1px solid transparent;
    border-radius: 0;
    background: transparent;
    color: var(--foreground);
  }
  button:hover {
    border-color: var(--border);
    background: var(--foreground);
    color: var(--primary-foreground);
  }
  button:active {
    background: var(--jesses-primary-hover);
    color: var(--primary-foreground);
  }
  button:focus-visible {
    outline-offset: -3px;
  }
  .close:hover,
  .close:active {
    border-color: var(--destructive);
    background: var(--destructive);
    color: var(--primary-foreground);
  }
</style>
