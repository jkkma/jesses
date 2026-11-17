import { expect, test } from '@playwright/test';
import { installSourceEncodeFixture, source } from './helpers/source-encode-fixture';

async function loadMedia(page: import('@playwright/test').Page, media: unknown) {
  await page.evaluate((value: any) => {
    const host = globalThis as any;
    host.__setSourceMedia(value);
    host.__mediaByPath[value.path] = value;
  }, media);
  await page.getByRole('button', { name: 'Add Files…' }).click();
  await page.getByRole('button', { name: 'Load Tracks' }).click();
}

test('a styled subtitle brings its fonts along, and the last one takes them away', async ({
  page,
}) => {
  await installSourceEncodeFixture(page);
  const attachment = { ...source.streams[2], kind: 'attachment', codec: null, language: null };
  await loadMedia(page, {
    ...source,
    streams: [
      source.streams[0],
      source.streams[1],
      { ...source.streams[2], codec: 'ass', title: 'Signs' },
      {
        ...attachment,
        index: 3,
        title: null,
        attachmentFilename: 'Main.ttf',
        attachmentMimeType: 'font/ttf',
      },
      {
        ...attachment,
        index: 4,
        title: null,
        attachmentFilename: 'notes.txt',
        attachmentMimeType: 'text/plain',
      },
    ],
  });
  const region = page.getByRole('region', { name: 'Track List' });
  const subtitle = region.getByRole('checkbox', { name: /Include .*Subtitles \(ASS\)/ });
  const font = region.getByRole('checkbox', { name: /Include .*Font \(TTF\) - Main\.ttf/ });
  const other = region.getByRole('checkbox', {
    name: /Include .*Attachment \(File\) - notes\.txt/,
  });
  await expect(subtitle).toBeChecked();
  await expect(font).toBeChecked();
  await expect(other).not.toBeChecked();
  await subtitle.uncheck();
  await expect(font).not.toBeChecked();
  await subtitle.check();
  await expect(font).toBeChecked();
});

test('the encode tabs name their source and show the output extension', async ({ page }) => {
  await installSourceEncodeFixture(page);
  await loadMedia(page, source);
  for (const workflow of ['Quick Convert', 'AV1AN'] as const) {
    await page.getByRole('button', { name: workflow, exact: true }).click();
    const region = page.getByRole('region', { name: workflow, exact: true });
    await expect(region.getByText('Source: input.mkv', { exact: true })).toBeVisible();
    await expect(region.getByText('.mkv', { exact: true })).toBeVisible();
  }
});

test('the trim preview shows a portrait frame whole instead of clipping it', async ({ page }) => {
  await installSourceEncodeFixture(page);
  await page.evaluate(() => {
    const canvas = document.createElement('canvas');
    canvas.width = 90;
    canvas.height = 160;
    const context = canvas.getContext('2d')!;
    context.fillStyle = '#c33';
    context.fillRect(0, 0, 90, 160);
    (globalThis as any).__previewDataUrl = canvas.toDataURL('image/png');
  });
  await loadMedia(page, source);
  await page.getByRole('button', { name: 'Quick Convert', exact: true }).click();
  const quick = page.getByRole('region', { name: 'Quick Convert', exact: true });
  await quick.getByRole('button', { name: 'Configure…' }).first().click();
  const dialog = page.getByRole('dialog', { name: 'Configure Trim' });
  const image = dialog.getByRole('img', { name: 'Video preview' });
  await expect(image).toBeVisible();
  const [frame, box] = await Promise.all([
    dialog.locator('.preview').boundingBox(),
    image.boundingBox(),
  ]);
  expect(box!.height).toBeLessThanOrEqual(frame!.height + 1);
  expect(box!.width).toBeLessThanOrEqual(frame!.width + 1);
});

test('Stop asks the job to keep its progress before releasing the run', async ({ page }) => {
  await installSourceEncodeFixture(page);
  await page.evaluate(() => ((globalThis as any).__holdTerminal = true));
  await loadMedia(page, {
    ...source,
    streams: [{ ...source.streams[0], fieldOrder: 'progressive' }, ...source.streams.slice(1)],
  });
  await page.getByRole('button', { name: 'File List', exact: true }).click();
  await page.getByRole('combobox', { name: 'Mode', exact: true }).selectOption('batch');
  await page.getByRole('button', { name: 'Quick Convert', exact: true }).click();
  await page.getByRole('button', { name: 'Run Batch: Encode', exact: true }).click();
  await expect(page.getByLabel('Current action')).toContainText('File 1/1');
  await page.getByRole('button', { name: 'Stop', exact: true }).click();
  await expect(page.getByLabel('Current action')).toContainText('Batch stopped');
  const commands = await page.evaluate(() =>
    (globalThis as any).__sourceCalls.map((call: any) => call.command),
  );
  expect(commands).toContain('stop_job');
  expect(commands.indexOf('stop_job')).toBeLessThan(
    commands.includes('cancel_job') ? commands.indexOf('cancel_job') : Infinity,
  );
});
