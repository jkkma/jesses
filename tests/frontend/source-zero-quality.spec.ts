import { expect, test } from '@playwright/test';
import { installSourceEncodeFixture, source } from './helpers/source-encode-fixture';

for (const workflow of ['Quick Convert', 'AV1AN'] as const) {
  test(`${workflow} keeps quality zero distinct from the explicit native lossless setting`, async ({
    page,
  }) => {
    await installSourceEncodeFixture(page);
    await page.evaluate((media) => (globalThis as any).__setSourceMedia(media), {
      ...source,
      streams: source.streams.map((stream) =>
        stream.kind === 'video' ? { ...stream, fieldOrder: 'progressive' } : stream,
      ),
    });
    await page.getByRole('button', { name: 'Add Files…' }).click();
    await page.getByRole('button', { name: 'Load Tracks' }).click();
    await page.getByRole('button', { name: workflow, exact: true }).click();
    const region = page.getByRole('region', { name: workflow, exact: true });
    if (workflow === 'AV1AN') {
      await region.getByRole('tab', { name: 'Av1an Options', exact: true }).click();
      await region.getByLabel('Concat Method', { exact: true }).selectOption('ffmpeg');
      await region.getByRole('tab', { name: 'Video', exact: true }).click();
    }
    const codecs =
      workflow === 'AV1AN'
        ? ['svtAv1', 'aomAv1', 'vpxStandalone', 'x264', 'x265Standalone']
        : ['svtAv1', 'aomAv1', 'vpxStandalone', 'x264', 'x265Standalone'];
    for (const [index, codec] of codecs.entries()) {
      await region
        .getByLabel(workflow === 'AV1AN' ? 'Encoder' : 'Codec', { exact: true })
        .selectOption(codec);
      await region.getByLabel('Quality', { exact: true }).fill('0');
      await page
        .getByRole('button', {
          name: workflow === 'AV1AN' ? 'Run: AV1AN encode' : 'Run: Encode',
          exact: true,
        })
        .click();
      await expect
        .poll(() =>
          page.evaluate(
            () =>
              (globalThis as any).__sourceCalls.filter(
                (c: any) => c.command === 'mock_start_encode',
              ).length,
          ),
        )
        .toBe(index + 1);
    }
    const settings = await page.evaluate(() =>
      (globalThis as any).__sourceCalls
        .filter((c: any) => c.command === 'mock_start_encode')
        .map((c: any) => c.payload.request.settings),
    );
    expect(settings.map((item: any) => item.crf)).toEqual(codecs.map(() => 0));
    expect(settings.map((item: any) => item.lossless)).toEqual(codecs.map(() => false));
    expect(settings.map((item: any) => item.sourceNvencQuality ?? false)).toEqual(
      codecs.map(() => false),
    );
    if (workflow === 'AV1AN') {
      expect(settings.every((item: any) => item.av1anOptions.sourceWorkflow === true)).toBe(true);
      expect(settings.map((item: any) => item.av1anOptions.concatMethod)).toEqual(
        codecs.map((codec) => (codec === 'x265Standalone' ? 'mkvmerge' : 'ffmpeg')),
      );
    }
  });
}

test('AV1AN source quality targets submit each selected metric and score', async ({ page }) => {
  await installSourceEncodeFixture(page);
  await page.evaluate((media) => (globalThis as any).__setSourceMedia(media), {
    ...source,
    streams: source.streams.map((stream) =>
      stream.kind === 'video' ? { ...stream, fieldOrder: 'progressive' } : stream,
    ),
  });
  await page.getByRole('button', { name: 'Add Files…' }).click();
  await page.getByRole('button', { name: 'Load Tracks' }).click();
  await page.getByRole('button', { name: 'AV1AN', exact: true }).click();
  const av = page.getByRole('region', { name: 'AV1AN', exact: true });
  const cases = [
    ['vmaf', '95', 950],
    ['ssimulacra2', '80', 800],
    ['butteraugli', '4', 40],
    ['xpsnrWeighted', '40', 400],
  ] as const;
  for (const [index, [metric, score]] of cases.entries()) {
    await av.getByLabel('Quality Mode').selectOption(metric);
    await av.getByLabel('Quality', { exact: true }).fill(score);
    await page.getByRole('button', { name: 'Run: AV1AN encode' }).click();
    await expect
      .poll(() =>
        page.evaluate(
          () =>
            (globalThis as any).__sourceCalls.filter((c: any) => c.command === 'mock_start_encode')
              .length,
        ),
      )
      .toBe(index + 1);
  }
  const options = await page.evaluate(() =>
    (globalThis as any).__sourceCalls
      .filter((c: any) => c.command === 'mock_start_encode')
      .map((c: any) => c.payload.request.settings.av1anOptions),
  );
  expect(options.every((item: any) => item.sourceTargetDefaults === true)).toBe(true);
  const targets = options.map((item: any) => item.targetQuality);
  expect(
    targets.map((target: any) => [
      target.metric,
      target.minimumScoreTenths,
      target.maximumScoreTenths,
    ]),
  ).toEqual(cases.map(([metric, , score]) => [metric, score, score]));
});
