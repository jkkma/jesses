import { expect, test } from '@playwright/test';
import { installSourceEncodeFixture } from './helpers/source-encode-fixture';

test('command preview distinguishes estimated and counted frames', async ({ page }) => {
  await installSourceEncodeFixture(page);
  await page.evaluate(async () => {
    const harness = '/tests/frontend/helpers/command-plan-harness.ts';
    await import(/* @vite-ignore */ harness);
  });
  const preview = page.getByRole('region', { name: 'Command plan preview' });
  await preview.getByRole('button', { name: 'Preview command plan' }).click();
  await expect(preview.getByRole('status')).toHaveText(
    'Estimated plan: about 288 frames at 24/1 fps.',
  );
  await page.evaluate(() => ((globalThis as any).__commandPlanEstimated = false));
  await preview.getByRole('button', { name: 'Preview command plan' }).click();
  await expect(preview.getByRole('status')).toHaveText('Plan: 288 frames at 24/1 fps.');
});
