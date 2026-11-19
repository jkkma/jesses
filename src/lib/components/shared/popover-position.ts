export interface PopoverPlacement {
  trigger: () => HTMLElement | undefined;
  placement: 'above' | 'below';
  align?: 'start' | 'end';
  constrainToMain?: boolean;
}

/**
 * Positions a native top-layer popover beside its trigger button and clamps it
 * inside the viewport (or `<main>` when requested), without clipping.
 */
export function placePopover(node: HTMLElement, options: PopoverPlacement) {
  let current = options;

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
      node.style.top = 'auto';
      node.style.bottom = `${Math.max(4, Math.round(window.innerHeight - rect.top + gap))}px`;
    } else {
      const available = Math.max(80, bottomEdge - rect.bottom - gap);
      node.style.maxHeight = `${Math.floor(available)}px`;
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
