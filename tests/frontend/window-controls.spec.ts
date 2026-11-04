import { expect, test } from '@playwright/test';

test('desktop header controls remain usable and follow native maximize changes', async ({
  page,
}) => {
  await page.addInitScript(() => {
    const host = globalThis as unknown as Record<string, any>;
    let nextId = 0;
    let maximized = false;
    const callbacks = new Map<number, (event: unknown) => void>();
    const listeners = new Map<number, { event: string; handler: number }>();
    host.isTauri = true;
    host.__windowCalls = [];
    host.__setMaximized = (value: boolean) => {
      maximized = value;
      for (const { event, handler } of listeners.values()) {
        if (event === 'tauri://resize') callbacks.get(handler)?.({ event, payload: {} });
      }
    };
    host.__TAURI_EVENT_PLUGIN_INTERNALS__ = { unregisterListener: () => {} };
    host.__TAURI_INTERNALS__ = {
      metadata: { currentWindow: { label: 'main' }, currentWebview: { label: 'main' } },
      transformCallback: (fn: (event: unknown) => void) => {
        const id = ++nextId;
        callbacks.set(id, fn);
        return id;
      },
      unregisterCallback: (id: number) => callbacks.delete(id),
      invoke: async (command: string, payload: any = {}) => {
        if (command.startsWith('plugin:window|')) host.__windowCalls.push(command);
        if (command === 'plugin:window|is_maximized') return maximized;
        if (command === 'plugin:window|is_minimized') return false;
        if (command === 'plugin:window|inner_size') return { width: 1280, height: 850 };
        if (command === 'plugin:window|outer_position') return { x: 0, y: 0 };
        if (command === 'plugin:window|toggle_maximize') host.__setMaximized(!maximized);
        if (command === 'plugin:event|listen') {
          const id = ++nextId;
          listeners.set(id, payload);
          return id;
        }
        if (command === 'plugin:event|unlisten') listeners.delete(payload.eventId);
        if (command === 'get_preferences')
          return {
            general: { defaultOutputDirectory: '', recursiveImport: false },
            recentPaths: [],
            revision: 1,
          };
        if (['get_capabilities', 'list_jobs', 'get_storage_locations'].includes(command)) return [];
      },
    };
  });
  await page.goto('/');
  const controls = page.getByRole('group', { name: 'Window controls' });
  await expect(controls).toBeVisible();
  await controls.getByRole('button', { name: 'Maximize window', exact: true }).click();
  await expect(controls.getByRole('button', { name: 'Restore window' })).toBeVisible();
  await controls.getByRole('button', { name: 'Restore window' }).click();
  await expect(
    controls.getByRole('button', { name: 'Maximize window', exact: true }),
  ).toBeVisible();

  // OS snapping, double-clicking the header, and keyboard shortcuts also resize the window.
  await page.evaluate(() => (globalThis as any).__setMaximized(true));
  await expect(controls.getByRole('button', { name: 'Restore window' })).toBeVisible();
  await controls.getByRole('button', { name: 'Minimize window', exact: true }).click();
  await controls.getByRole('button', { name: 'Close window' }).focus();
  await page.keyboard.press('Enter');
  const calls = await page.evaluate(() => (globalThis as any).__windowCalls as string[]);
  expect(calls.filter((command) => command === 'plugin:window|toggle_maximize')).toHaveLength(2);
  expect(calls).toContain('plugin:window|minimize');
  expect(calls).toContain('plugin:window|close');
  // Closing must use the normal close request so Rust can shut down active jobs.
  expect(calls).not.toContain('plugin:window|destroy');
});

test('browser preview does not expose inoperative desktop controls', async ({ page }) => {
  await page.goto('/');
  await expect(page.locator('.brand')).toBeVisible();
  await expect(page.getByRole('group', { name: 'Window controls' })).toHaveCount(0);
});
