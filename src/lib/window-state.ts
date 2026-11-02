import {
  availableMonitors,
  getCurrentWindow,
  PhysicalPosition,
  PhysicalSize,
} from '@tauri-apps/api/window';

type SavedWindow = { x: number; y: number; width: number; height: number; maximized: boolean };
const key = 'jesses.window-state';

function read(): SavedWindow | null {
  try {
    const value = JSON.parse(localStorage.getItem(key) ?? 'null') as SavedWindow | null;
    if (
      !value ||
      ![value.x, value.y, value.width, value.height].every(Number.isFinite) ||
      value.width < 320 ||
      value.height < 240 ||
      value.width > 16384 ||
      value.height > 16384 ||
      typeof value.maximized !== 'boolean'
    )
      return null;
    return value;
  } catch {
    return null;
  }
}

/** Restore only visible window positions and persist changes after the user moves or resizes. */
export async function restoreAndTrackWindow(): Promise<() => void> {
  const window = getCurrentWindow();
  const saved = read();
  if (saved) {
    try {
      const monitors = await availableMonitors();
      const visible = monitors.some(
        ({ position, size }) =>
          Math.min(saved.x + saved.width, position.x + size.width) -
            Math.max(saved.x, position.x) >=
            80 &&
          Math.min(saved.y + saved.height, position.y + size.height) -
            Math.max(saved.y, position.y) >=
            80,
      );
      if (visible) {
        await window.setSize(new PhysicalSize(saved.width, saved.height));
        await window.setPosition(new PhysicalPosition(saved.x, saved.y));
      }
      if (saved.maximized) await window.maximize();
    } catch {
      /* Keep the platform's safe default if display configuration changed. */
    }
  }
  let timer: ReturnType<typeof setTimeout> | undefined;
  let disposed = false;
  async function save() {
    try {
      const [maximized, minimized] = await Promise.all([
        window.isMaximized(),
        window.isMinimized(),
      ]);
      if (disposed || minimized) return;
      const previous = read();
      if (maximized && previous) {
        localStorage.setItem(key, JSON.stringify({ ...previous, maximized }));
      } else if (maximized) {
        const [size, position] = await Promise.all([window.innerSize(), window.outerPosition()]);
        if (!disposed)
          localStorage.setItem(
            key,
            JSON.stringify({
              x: position.x,
              y: position.y,
              width: size.width,
              height: size.height,
              maximized,
            }),
          );
      } else if (!maximized) {
        const [size, position] = await Promise.all([window.innerSize(), window.outerPosition()]);
        if (!disposed)
          localStorage.setItem(
            key,
            JSON.stringify({
              x: position.x,
              y: position.y,
              width: size.width,
              height: size.height,
              maximized,
            }),
          );
      }
    } catch {
      /* Window state is optional; encoding must remain usable. */
    }
  }
  const schedule = () => {
    clearTimeout(timer);
    timer = setTimeout(() => void save(), 300);
  };
  const [stopResize, stopMove] = await Promise.all([
    window.onResized(schedule),
    window.onMoved(schedule),
  ]);
  void save();
  return () => {
    disposed = true;
    clearTimeout(timer);
    stopResize();
    stopMove();
  };
}
