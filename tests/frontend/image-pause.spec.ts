import { expect, test } from '@playwright/test';
import { installSourceEncodeFixture, source } from './helpers/source-encode-fixture';

for (const mode of ['ordinary', 'edited', 'implicit'] as const) {
  test(`${mode} image Pause and Resume target its held session across tabs; Stop works while paused`, async ({
    page,
  }) => {
    await installSourceEncodeFixture(page);
    await page.getByRole('button', { name: 'Add Files…' }).click();
    await page.getByRole('button', { name: 'Load Tracks' }).click();
    if (mode === 'implicit')
      await page.getByRole('checkbox', { name: /Include .*Video/ }).uncheck();
    await page.getByRole('button', { name: 'Quick Convert', exact: true }).click();
    await page
      .getByRole('region', { name: 'Quick Convert', exact: true })
      .getByLabel('Codec', { exact: true })
      .selectOption('png');
    await page.evaluate(() => {
      const h = globalThis as any;
      h.__holdImage = new Promise((resolve) => (h.__releaseImage = resolve));
    });
    if (mode === 'edited') await page.keyboard.down('Shift');
    await page.getByRole('button', { name: 'Run: Encode', exact: true }).click();
    if (mode === 'edited') {
      const dialog = page.getByRole('dialog', { name: 'Edit ffmpeg Arguments' });
      await expect(dialog).toBeVisible();
      await page.keyboard.up('Shift');
      await dialog.getByRole('textbox').fill('-i edited input');
      await dialog.getByRole('button', { name: 'Run', exact: true }).click();
    }
    await expect
      .poll(() =>
        page.evaluate(() =>
          (globalThis as any).__sourceCalls.some((c: any) => c.command === 'mock_image_execution'),
        ),
      )
      .toBe(true);
    const id = await page.evaluate(
      () =>
        (globalThis as any).__sourceCalls.find((c: any) => c.command === 'confirm_command_edit')
          .payload.request.id,
    );
    await expect(page.getByRole('button', { name: 'Pause', exact: true })).toBeEnabled();
    await page.getByRole('button', { name: 'Pause', exact: true }).click();
    await expect(page.getByRole('button', { name: 'Resume', exact: true })).toBeEnabled();
    await page.getByRole('button', { name: 'Settings', exact: true }).click();
    await page.getByRole('button', { name: 'Resume', exact: true }).click();
    await expect(page.getByRole('button', { name: 'Pause', exact: true })).toBeEnabled();
    await page.getByRole('button', { name: 'Pause', exact: true }).click();
    await expect(page.getByRole('button', { name: 'Resume', exact: true })).toBeEnabled();
    await page.getByRole('button', { name: 'Stop', exact: true }).click();
    await page.evaluate(() => (globalThis as any).__releaseImage());
    await expect(page.getByLabel('Current action')).toHaveText('Canceled.');
    expect(
      await page.evaluate(() =>
        (globalThis as any).__sourceCalls
          .filter((c: any) => c.command === 'set_command_edit_paused')
          .map((c: any) => c.payload),
      ),
    ).toEqual([
      { id, paused: true },
      { id, paused: false },
      { id, paused: true },
    ]);
    expect(
      await page.evaluate(() =>
        (globalThis as any).__sourceCalls.some((c: any) => c.command === 'cancel_command_edit'),
      ),
    ).toBe(true);
  });
}

test('image batch pause state belongs to the current file and resets for the next', async ({
  page,
}) => {
  await installSourceEncodeFixture(page);
  const second = { ...source, id: 'second', path: '/other/second.mkv', name: 'second.mkv' };
  await page.evaluate(
    ({ first, second }) => {
      const h = globalThis as any;
      h.__pickerPaths = [first.path, second.path];
      h.__mediaByPath = { [first.path]: first, [second.path]: second };
      h.__holdImage = new Promise((resolve) => (h.__releaseFirst = resolve));
    },
    { first: source, second },
  );
  await page.getByRole('button', { name: 'Add Files…' }).click();
  await page.getByRole('combobox', { name: 'Mode', exact: true }).selectOption('batch');
  await page.getByRole('button', { name: 'Quick Convert', exact: true }).click();
  await page
    .getByRole('region', { name: 'Quick Convert', exact: true })
    .getByLabel('Codec', { exact: true })
    .selectOption('png');
  await page.getByRole('button', { name: 'Run Batch: Encode', exact: true }).click();
  await expect
    .poll(() =>
      page.evaluate(
        () =>
          (globalThis as any).__sourceCalls.filter((c: any) => c.command === 'mock_image_execution')
            .length,
      ),
    )
    .toBe(1);
  await page.getByRole('button', { name: 'Pause', exact: true }).click();
  await expect(page.getByRole('button', { name: 'Resume', exact: true })).toBeEnabled();
  await page.getByRole('button', { name: 'Resume', exact: true }).click();
  await page.evaluate(() => {
    const h = globalThis as any;
    h.__holdImage = new Promise((resolve) => (h.__releaseSecond = resolve));
    h.__releaseFirst();
  });
  await expect
    .poll(() =>
      page.evaluate(
        () =>
          (globalThis as any).__sourceCalls.filter((c: any) => c.command === 'mock_image_execution')
            .length,
      ),
    )
    .toBe(2);
  await expect(page.getByRole('button', { name: 'Pause', exact: true })).toBeEnabled();
  await page.getByRole('button', { name: 'Pause', exact: true }).click();
  const calls = await page.evaluate(() =>
    (globalThis as any).__sourceCalls
      .filter((c: any) => c.command === 'set_command_edit_paused')
      .map((c: any) => c.payload),
  );
  expect(calls).toHaveLength(3);
  expect(calls[0]).toEqual({ id: calls[1].id, paused: true });
  expect(calls[1]).toEqual({ id: calls[0].id, paused: false });
  expect(calls[2]).toEqual({ id: expect.not.stringMatching(`^${calls[0].id}$`), paused: true });
  await page.getByRole('button', { name: 'Stop', exact: true }).click();
  await page.evaluate(() => (globalThis as any).__releaseSecond());
  await page.getByRole('button', { name: 'File List', exact: true }).click();
  await expect(page.getByRole('option', { name: /input.mkv/ })).toContainText('Done');
  await expect(page.getByRole('option', { name: /second.mkv/ })).toContainText('Canceled');
});

test('pause failure keeps the action usable; Stop wins over an in-flight pause reply', async ({
  page,
}) => {
  await installSourceEncodeFixture(page);
  await page.getByRole('button', { name: 'Add Files…' }).click();
  await page.getByRole('button', { name: 'Load Tracks' }).click();
  await page.getByRole('button', { name: 'Quick Convert', exact: true }).click();
  const quick = page.getByRole('region', { name: 'Quick Convert', exact: true });
  await quick.getByLabel('Codec', { exact: true }).selectOption('png');
  await page.evaluate(() => {
    const h = globalThis as any;
    h.__pauseFailure = 'Pause refused';
    h.__holdImage = new Promise((resolve) => (h.__releaseImage = resolve));
  });
  await page.getByRole('button', { name: 'Run: Encode', exact: true }).click();
  await expect
    .poll(() =>
      page.evaluate(() =>
        (globalThis as any).__sourceCalls.some((c: any) => c.command === 'mock_image_execution'),
      ),
    )
    .toBe(true);
  await page.getByRole('button', { name: 'Pause', exact: true }).click();
  await expect(quick.getByRole('alert')).toContainText('Pause refused');
  await expect(page.getByRole('button', { name: 'Pause', exact: true })).toBeEnabled();
  await page.evaluate(() => {
    (globalThis as any).__pauseFailure = '';
  });
  await page.getByRole('button', { name: 'Pause', exact: true }).click();
  await expect(page.getByRole('button', { name: 'Resume', exact: true })).toBeEnabled();
  await expect(quick.getByRole('alert')).toHaveCount(0);
  await page.getByRole('button', { name: 'Resume', exact: true }).click();
  await page.evaluate(() => {
    const h = globalThis as any;
    h.__holdPause = new Promise((resolve) => (h.__releasePause = resolve));
  });
  await page.getByRole('button', { name: 'Pause', exact: true }).click();
  await expect(page.getByRole('button', { name: 'Pause', exact: true })).toBeDisabled();
  await page.getByRole('button', { name: 'Stop', exact: true }).click();
  await page.evaluate(() => {
    const h = globalThis as any;
    h.__releasePause();
    h.__releaseImage();
  });
  await expect(page.getByLabel('Current action')).toHaveText('Canceled.');
  await expect(page.getByRole('button', { name: 'Resume', exact: true })).toHaveCount(0);
});
