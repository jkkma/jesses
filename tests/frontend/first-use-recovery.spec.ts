import { expect, test } from '@playwright/test';
import { installSourceEncodeFixture } from './helpers/source-encode-fixture';

test.use({ viewport: { width: 1152, height: 720 } });

test('Quick Convert can add its first source without leaving the settings', async ({ page }) => {
  await installSourceEncodeFixture(page);
  await page.getByRole('button', { name: 'Quick Convert', exact: true }).click();
  const quick = page.getByRole('region', { name: 'Quick Convert', exact: true });
  await quick.getByLabel('Quality', { exact: true }).fill('23');
  const addFiles = quick.getByRole('button', { name: 'Add Files…', exact: true });
  await addFiles.focus();
  await addFiles.press('Enter');
  await expect(quick.getByText('Source: input.mkv')).toBeVisible();
  await expect(quick.getByLabel('Quality', { exact: true })).toHaveValue('23');
  await expect(addFiles).toHaveCount(0);
});

test('missing tool explanation is fully visible and opens tool settings', async ({ page }) => {
  await page.addInitScript(() => {
    const state = globalThis as any;
    state.__unavailableTools = ['svt-av1-hdr'];
    Object.defineProperty(state, '__TAURI_INTERNALS__', {
      configurable: true,
      set(value) {
        const invoke = value.invoke;
        value.invoke = async (command: string, payload: unknown) => {
          const result = await invoke(command, payload);
          if (command === 'get_capabilities') {
            return result.map((tool: any) =>
              tool.id === 'svt-av1-hdr'
                ? {
                    ...tool,
                    detail:
                      'Not found in its configured override, managed install, package, or distinct PATH alias. Install this fork and refresh Tools.',
                  }
                : tool,
            );
          }
          return result;
        };
        Object.defineProperty(state, '__TAURI_INTERNALS__', { value, configurable: true });
      },
    });
  });
  await installSourceEncodeFixture(page);
  await page.getByRole('button', { name: 'Quick Convert', exact: true }).click();
  await page
    .getByRole('region', { name: 'Quick Convert', exact: true })
    .getByRole('button', { name: 'Add Files…' })
    .click();
  const explanation = page.locator('#run-blocked-reason');
  await expect(explanation).toContainText('Install this fork and refresh Tools.');
  await expect(explanation).toBeInViewport();
  expect(
    await explanation.evaluate((element) => element.scrollHeight <= element.clientHeight),
  ).toBe(true);
  const openTools = page.getByRole('button', { name: 'Open Tools' });
  await openTools.focus();
  await openTools.press('Enter');
  const tools = page.getByRole('region', { name: 'Installed tools' });
  await expect(tools).toBeInViewport();
  const toolsTop = await tools.evaluate((element) => element.getBoundingClientRect().top);
  const mainTop = await page
    .locator('main')
    .evaluate((element) => element.getBoundingClientRect().top);
  expect(toolsTop).toBeLessThan(mainTop + 30);
  await expect(page.getByRole('button', { name: 'Check again' })).toBeVisible();
});
