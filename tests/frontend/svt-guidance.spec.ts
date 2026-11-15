import { expect, test } from '@playwright/test';
import { encodeSummary } from '../../src/lib/components/shared/encoder-options';
import type { EncodeSettings } from '../../src/lib/ipc/generated';
import { installSourceEncodeFixture, source } from './helpers/source-encode-fixture';

test('SVT job summaries reflect selected output depth and the historical default', () => {
  const settings = {
    backend: 'standalone',
    encoder: 'svtAv1Hdr',
    crf: 30,
    preset: 4,
    hdrTune: 'visualQuality',
    audio: [],
    framing: {
      crop: { top: 0, right: 0, bottom: 0, left: 0 },
      resizeWidth: null,
      borders: { top: 0, right: 0, bottom: 0, left: 0 },
    },
    workers: 2,
    videoStreamIndex: 0,
    lossless: false,
    filmGrain: 0,
    lineartPsyBias: 0,
    texturePsyBias: 0,
    hdr10Fallback: false,
  } satisfies EncodeSettings;
  expect(encodeSummary({ ...settings, outputPixelFormat: 'yuv420p' })).toContain(' · 8-bit · ');
  expect(encodeSummary({ ...settings, outputPixelFormat: 'yuv420p10le' })).toContain(
    ' · 10-bit · ',
  );
  expect(encodeSummary(settings)).toContain(' · 10-bit · ');
  expect(
    encodeSummary({
      ...settings,
      backend: 'av1an',
      av1anOptions: { pixelFormat: 'yuv420p' } as EncodeSettings['av1anOptions'],
    }),
  ).toContain(' · 8-bit · ');
});

for (const workflow of ['Quick Convert', 'AV1AN'] as const) {
  test(`${workflow} explains SVT quality zero and fast presets at their boundaries`, async ({
    page,
  }) => {
    await installSourceEncodeFixture(page);
    await page.evaluate((media) => (globalThis as any).__setSourceMedia(media), source);
    await page.getByRole('button', { name: 'Add Files…' }).click();
    await page.getByRole('button', { name: 'Load Tracks' }).click();
    await page.getByRole('button', { name: workflow, exact: true }).click();
    const region = page.getByRole('region', { name: workflow, exact: true });
    const codec = region.getByLabel(workflow === 'AV1AN' ? 'Encoder' : 'Codec', { exact: true });
    const quality = region.getByLabel('Quality', { exact: true });
    const preset = region.getByLabel('Preset / Effort', { exact: true });
    const zeroHint = region.getByText(/documented.*range.*1.*not lossless/i);
    const presetHint = region.getByText(/preset.*10.*automated tooling.*visual artifacts/i);
    for (const value of ['svtAv1', 'svtAv1Hdr']) {
      await codec.selectOption(value);
      await quality.fill('0');
      await expect(zeroHint).toBeVisible();
      await expect(quality).toHaveAttribute(
        'aria-describedby',
        (await zeroHint.getAttribute('id')) as string,
      );
      await quality.fill('1');
      await expect(zeroHint).toHaveCount(0);
      await preset.selectOption('9');
      await expect(presetHint).toHaveCount(0);
      await preset.selectOption('10');
      await expect(presetHint).toBeVisible();
      await expect(preset).toHaveAttribute(
        'aria-describedby',
        (await presetHint.getAttribute('id')) as string,
      );
    }
    await page.setViewportSize({ width: 800, height: 700 });
    await quality.fill('0');
    for (const hint of [zeroHint, presetHint]) {
      const fits = await hint.evaluate((element) => {
        const bounds = element.getBoundingClientRect();
        const form = element.parentElement!.getBoundingClientRect();
        return (
          bounds.left >= form.left &&
          bounds.right <= form.right &&
          element.scrollWidth <= element.clientWidth
        );
      });
      expect(fits).toBe(true);
    }
    await region
      .getByLabel('Quality Mode', { exact: true })
      .selectOption(workflow === 'AV1AN' ? 'vmaf' : 'bitrate');
    await expect(zeroHint).toHaveCount(0);
    await expect(quality).not.toHaveAttribute('aria-describedby', /svt-zero-hint/);
    await region
      .getByLabel('Quality Mode', { exact: true })
      .selectOption(workflow === 'AV1AN' ? 'crf' : 'quality');
    await codec.selectOption('aomAv1');
    await quality.fill('0');
    await expect(zeroHint).toHaveCount(0);
    await expect(presetHint).toHaveCount(0);
  });
}

test('SVT content preset descriptions explain possible single-lane encoding', async ({ page }) => {
  await installSourceEncodeFixture(page);
  await page.evaluate((media) => (globalThis as any).__setSourceMedia(media), source);
  await page.getByRole('button', { name: 'Add Files…' }).click();
  await page.getByRole('button', { name: 'Load Tracks' }).click();
  await page.getByRole('button', { name: 'Quick Convert', exact: true }).click();
  const quick = page.getByRole('region', { name: 'Quick Convert', exact: true });
  await quick.getByLabel('Codec', { exact: true }).selectOption('svtAv1Hdr');
  await quick.getByRole('tab', { name: 'Advanced', exact: true }).click();
  for (const name of ['Anime / Cel Animation', 'Game Capture / Gameplay']) {
    await expect(quick.getByRole('button', { name, exact: true })).toHaveAttribute(
      'title',
      /some SVT builds or AV1AN.*one processing lane.*slow encoding/i,
    );
  }
});
