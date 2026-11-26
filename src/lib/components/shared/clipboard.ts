let statusRegion: HTMLSpanElement | undefined;
let statusTimer: ReturnType<typeof setTimeout> | undefined;
let lastAnnouncement = '';

/**
 * Copy text using the Async Clipboard API with a synchronous execCommand fallback
 * that preserves read-only semantics and restores focus without scrolling.
 */
export async function copyText(text: string): Promise<boolean> {
  if (!text) return false;
  try {
    if (navigator.clipboard) {
      await navigator.clipboard.writeText(text);
      return true;
    }
  } catch {
    /* Try the synchronous selection fallback below. */
  }
  const previousFocus = document.activeElement as HTMLElement | null;
  const field = document.createElement('textarea');
  field.value = text;
  field.setAttribute('readonly', '');
  field.style.cssText = 'position:fixed;inset-block-start:0;opacity:0';
  document.body.append(field);
  field.select();
  try {
    return document.execCommand('copy');
  } finally {
    field.remove();
    previousFocus?.focus({ preventScroll: true });
  }
}

function getStatusRegion(): HTMLSpanElement {
  if (statusRegion?.isConnected) return statusRegion;
  const node = document.createElement('span');
  node.className = 'visually-hidden';
  node.setAttribute('role', 'status');
  node.setAttribute('data-copy-status', '');
  document.body.append(node);
  statusRegion = node;
  return node;
}

/**
 * Announce copy outcome via an external status live region (never inside a
 * button whose children are presentational), avoiding duplicate-text skips.
 */
export function announceCopyFeedback(
  ok: boolean,
  successMessage = 'Copied',
  failMessage = 'Copy failed',
): void {
  const region = getStatusRegion();
  clearTimeout(statusTimer);
  let message = ok ? successMessage : failMessage;
  if (message === lastAnnouncement) message += '\u00A0';
  lastAnnouncement = message;
  region.textContent = '';
  requestAnimationFrame(() => {
    region.textContent = message;
  });
  statusTimer = setTimeout(() => {
    region.textContent = '';
  }, 2000);
}

export async function copyTextWithFeedback(
  text: string,
  messages?: { success?: string; fail?: string },
): Promise<boolean> {
  const ok = await copyText(text);
  announceCopyFeedback(ok, messages?.success, messages?.fail);
  return ok;
}
