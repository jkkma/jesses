export interface PopoverPlacement {
  trigger: () => HTMLElement | undefined;
  placement: 'above' | 'below';
  align?: 'start' | 'end';
  constrainToMain?: boolean;
}

let toggletipFallbackInstalled = false;

function ensureToggletipFallback() {
  if (toggletipFallbackInstalled || typeof document === 'undefined') return;
  toggletipFallbackInstalled = true;
  const probe = document.createElement('div');
  probe.setAttribute('popover', 'hint');
  if (probe.popover !== 'hint') {
    document.addEventListener(
      'beforetoggle',
      (event) => {
        const target = event.target as HTMLElement | null;
        if (!target || target.getAttribute('popover') !== 'hint') return;
        const toggle = event as ToggleEvent;
        if (
          toggle.newState === 'open' &&
          target.id &&
          document.querySelector(`[popovertarget="${CSS.escape(target.id)}"]`)
        ) {
          target.popover = 'auto';
        }
      },
      true,
    );
  }
}

/**
 * Keeps native top-layer popovers bounded within `<main>` when CSS Anchor Positioning
 * handles placement, and falls back to explicit coordinates where unsupported.
 */
export function placePopover(node: HTMLElement, options: PopoverPlacement) {
  let current = options;
  ensureToggletipFallback();
  const supportsAnchor =
    typeof CSS !== 'undefined' &&
    typeof CSS.supports === 'function' &&
    CSS.supports('position-area', 'block-end');

  function position() {
    const trigger = current.trigger();
    if (!trigger) return;
    const rect = trigger.getBoundingClientRect();
    const bounds =
      current.constrainToMain && trigger.closest('main')
        ? trigger.closest('main')!.getBoundingClientRect()
        : { top: 8, bottom: window.innerHeight - 8, left: 8, right: window.innerWidth - 8 };
    const topEdge = Math.max(4, bounds.top + 4);
    const bottomEdge = Math.min(window.innerHeight - 4, bounds.bottom - 4);
    const gap = 4;

    if (current.placement === 'above') {
      const available = Math.max(80, rect.top - topEdge - gap);
      node.style.maxHeight = `${Math.floor(available)}px`;
    } else {
      const availableBelow = Math.max(80, bottomEdge - rect.bottom - gap);
      node.style.maxHeight = `${Math.floor(availableBelow)}px`;
    }

    if (supportsAnchor) return;

    if (current.placement === 'above') {
      node.style.top = 'auto';
      node.style.bottom = `${Math.max(4, Math.round(window.innerHeight - rect.top + gap))}px`;
    } else {
      node.style.bottom = 'auto';
      node.style.top = `${Math.max(4, Math.round(rect.bottom + gap))}px`;
    }

    if ((current.align ?? 'end') === 'start') {
      node.style.right = 'auto';
      node.style.left = `${Math.max(8, Math.round(rect.left))}px`;
    } else {
      node.style.left = 'auto';
      node.style.right = `${Math.max(8, Math.round(window.innerWidth - rect.right))}px`;
    }
  }

  function onBeforeToggle(event: Event) {
    const toggle = event as ToggleEvent;
    if (toggle.newState === 'open') position();
  }

  function onResize() {
    if (node.matches(':popover-open')) position();
  }

  node.addEventListener('beforetoggle', onBeforeToggle);
  window.addEventListener('resize', onResize);

  return {
    update(next: PopoverPlacement) {
      current = next;
      if (node.matches(':popover-open')) position();
    },
    destroy() {
      node.removeEventListener('beforetoggle', onBeforeToggle);
      window.removeEventListener('resize', onResize);
    },
  };
}
