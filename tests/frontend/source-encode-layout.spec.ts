import { expect, test } from '@playwright/test';

for (const { width, height } of [
  { width: 800, height: 750 },
  { width: 800, height: 768 },
  { width: 1360, height: 850 },
]) {
  test(`video controls fit and remain compact at ${width}x${height}`, async ({ page }) => {
    await page.setViewportSize({ width, height });
    await page.addInitScript(() => {
      const host = globalThis as unknown as Record<string, any>;
      host.isTauri = true;
      let id = 0;
      host.__TAURI_EVENT_PLUGIN_INTERNALS__ = { unregisterListener: () => {} };
      host.__TAURI_INTERNALS__ = {
        metadata: { currentWindow: { label: 'main' }, currentWebview: { label: 'main' } },
        transformCallback: () => ++id,
        unregisterCallback: () => {},
        invoke: async (command: string, payload: any = {}) => {
          if (command.startsWith('plugin:event|')) return ++id;
          if (command === 'plugin:dialog|open') return ['/media/input.mkv'];
          if (command === 'get_capabilities')
            return ['ffmpeg', 'ffprobe', 'svt-av1', 'av1an'].map((name) => ({
              id: name,
              name,
              available: true,
              path: `/bin/${name}`,
              version: 'test',
            }));
          if (
            command === 'list_jobs' ||
            command === 'get_storage_locations' ||
            command === 'list_av1an_recoveries'
          )
            return [];
          if (command === 'subscribe_jobs') {
            payload.channel.onmessage([]);
            return;
          }
          if (command === 'get_preferences' || command === 'remember_recent_media')
            return {
              general: { defaultOutputDirectory: '', recursiveImport: false },
              recentPaths: [],
              revision: 1,
            };
          if (command === 'get_completion_status')
            return {
              options: { notify: false, finishAction: 'none' },
              armedJobs: 0,
              secondsRemaining: null,
              error: null,
            };
          if (command === 'probe_media')
            return {
              id: 'one',
              path: '/media/input.mkv',
              name: 'input.mkv',
              sizeBytes: '1000',
              durationSeconds: 12,
              format: 'matroska',
              streams: [
                {
                  index: 0,
                  kind: 'video',
                  codec: 'h264',
                  width: 320,
                  height: 180,
                  frameRate: '24/1',
                  fieldOrder: 'tt',
                  pixelFormat: 'yuv420p10le',
                  bitDepth: 10,
                  colorPrimaries: 'bt2020',
                  colorTransfer: 'smpte2084',
                  colorSpace: 'bt2020nc',
                  colorRange: 'tv',
                  hdrFormat: 'HDR10',
                  sampleRate: null,
                  channels: null,
                  language: null,
                  title: null,
                },
                {
                  index: 1,
                  kind: 'audio',
                  codec: 'aac',
                  width: null,
                  height: null,
                  frameRate: null,
                  sampleRate: 48000,
                  channels: 2,
                  language: 'eng',
                  title: null,
                },
              ],
            };
          if (command === 'begin_media_analysis') return `analysis-${++id}`;
          if (command === 'resolve_output_path') {
            const request = payload.request;
            const stem = request.outputStem ?? '/media/input';
            const extension =
              request.target.kind === 'container'
                ? request.target.format === 'matroska'
                  ? 'mkv'
                  : request.target.format
                : request.target.format === 'gif'
                  ? 'gif'
                  : '';
            return {
              outputStem: stem,
              outputPath: stem + (extension ? `.${extension}` : ''),
              isDirectory: !extension,
              numbered: false,
            };
          }
          if (command === 'cancel_media_analysis') return;
          if (command === 'detect_fields')
            return {
              order: 'undetermined',
              telecined: false,
              sampledFrames: 0,
              topFirstFrames: 0,
              bottomFirstFrames: 0,
              progressiveFrames: 0,
              undeterminedFrames: 0,
              fieldGapRatio: null,
              sourceFingerprint: 'test',
              evidence: 'Undetermined',
            };
          throw new Error(`Unexpected ${command}`);
        },
      };
    });
    await page.goto('/');
    await page.getByRole('button', { name: 'Add Files…' }).click();
    await expect(page.getByRole('option', { name: /input.mkv/ })).toBeVisible();
    await page.getByRole('button', { name: 'Quick Convert', exact: true }).click();
    const quick = page.getByRole('region', { name: 'Quick Convert', exact: true });
    const labels = [
      'Codec',
      'Container',
      'Quality Mode',
      'Quality',
      'Preset / Effort',
      'Color Format',
      'Frame Rate',
      'Crop',
      'Resize',
      'Borders',
      'Grain Synthesis',
      'Deinterlace',
      'Tone Mapping',
      'Output Path',
    ];
    for (const name of labels) await expect(quick.getByLabel(name, { exact: true })).toBeVisible();
    await expect(
      quick.getByRole('button', { name: 'Configure…', exact: true }).first(),
    ).toBeVisible();
    const box = await quick.locator('.tab-content').boundingBox();
    expect(box).not.toBeNull();
    for (const name of labels.filter((name) => name !== 'Output Path')) {
      const rect = await quick.getByLabel(name, { exact: true }).boundingBox();
      expect(rect, name).not.toBeNull();
      expect(rect!.y, name).toBeGreaterThanOrEqual(box!.y - 1);
      expect(rect!.y + rect!.height, name).toBeLessThanOrEqual(box!.y + box!.height + 1);
      expect(rect!.x, name).toBeGreaterThanOrEqual(box!.x - 1);
      expect(rect!.x + rect!.width, name).toBeLessThanOrEqual(box!.x + box!.width + 1);
    }
    const scroll = await quick
      .locator('.tab-content')
      .evaluate((el) => ({ scroll: el.scrollHeight, height: el.clientHeight }));
    expect(scroll.scroll).toBeLessThanOrEqual(scroll.height + 1);
    const preset = await quick.getByLabel('Preset / Effort', { exact: true }).boundingBox();
    expect(preset!.width).toBeLessThanOrEqual(105);
    const deinterlace = await quick.getByLabel('Deinterlace', { exact: true }).boundingBox();
    expect(deinterlace!.width).toBeGreaterThanOrEqual(190);
    const codec = await quick.getByLabel('Codec', { exact: true }).boundingBox();
    const crop = await quick.getByLabel('Crop', { exact: true }).boundingBox();
    const grain = await quick.getByLabel('Grain Synthesis', { exact: true }).boundingBox();
    expect(crop!.x).toBeGreaterThan(codec!.x + 100);
    if (width === 1360) expect(grain!.x).toBeGreaterThan(crop!.x + 100);
    else expect(grain!.y).toBeGreaterThan(codec!.y);
    const output = await quick.getByLabel('Output Path', { exact: true }).boundingBox();
    expect(output!.y + output!.height).toBeLessThanOrEqual(height - 1);
    await expect(page.getByRole('button', { name: /Run/ }).last()).toBeVisible();
    console.log(JSON.stringify({ width, form: box, codec, crop, grain, preset, output, scroll }));
    await page.screenshot({
      path: `${process.env.JESSES_MIGRATION_SCREENSHOTS ?? '/tmp'}/jesses-convert-${width}-${height}-revised.png`,
    });
    const cropChoice = quick.getByLabel('Crop', { exact: true });
    await cropChoice.selectOption('automatic');
    await cropChoice.hover();
    await expect(cropChoice).toHaveAttribute('title', 'Automatic (detect black bars)');
    if (width === 800) {
      await cropChoice.selectOption('manual');
      await quick.getByLabel('Resize', { exact: true }).selectOption('custom');
      await quick.getByLabel('Grain Synthesis', { exact: true }).selectOption('encoder');
      const compact = await quick
        .locator('.tab-content')
        .evaluate((el) => ({ scroll: el.scrollHeight, height: el.clientHeight }));
      expect(compact.scroll).toBeLessThanOrEqual(compact.height + 1);
      for (const name of ['Output dimensions', 'Padding outcome', 'Tone Mapping']) {
        const rect = await quick.getByLabel(name, { exact: true }).boundingBox();
        expect(rect!.y + rect!.height, name).toBeLessThanOrEqual(box!.y + box!.height + 1);
      }
      await page.screenshot({
        path: `${process.env.JESSES_MIGRATION_SCREENSHOTS ?? '/tmp'}/jesses-convert-${width}-${height}-readouts.png`,
      });
    }
    await expect(quick.getByLabel('Preset / Effort', { exact: true })).toHaveCSS(
      'width',
      `${preset!.width}px`,
    );
    await page.getByRole('button', { name: 'AV1AN', exact: true }).click();
    const av1an = page.getByRole('region', { name: 'AV1AN', exact: true });
    await av1an.getByRole('tab', { name: 'Av1an Options', exact: true }).click();
    for (const name of ['Split Method', 'Chunk Order']) {
      const select = av1an.getByLabel(name, { exact: true });
      const fit = await select.evaluate((element: HTMLSelectElement) => {
        const style = getComputedStyle(element);
        const canvas = document.createElement('canvas');
        const context = canvas.getContext('2d')!;
        context.font = style.font;
        const labels = [...element.options].map((option) => option.text);
        const longest = Math.max(...labels.map((label) => context.measureText(label).width));
        const available =
          element.clientWidth - parseFloat(style.paddingLeft) - parseFloat(style.paddingRight) - 18;
        return { labels, longest, available };
      });
      expect(fit.available, `${name}: ${fit.labels.join(' | ')}`).toBeGreaterThanOrEqual(
        fit.longest,
      );
    }
    const order = await av1an.getByLabel('Chunk Order', { exact: true }).boundingBox();
    const workers = await av1an.getByLabel('Workers', { exact: true }).boundingBox();
    expect(order!.x + order!.width).toBeLessThan(workers!.x);
    const optionsScroll = await av1an.locator('.tab-content').evaluate((el) => ({
      scroll: el.scrollHeight,
      height: el.clientHeight,
    }));
    expect(optionsScroll.scroll).toBeLessThanOrEqual(optionsScroll.height + 1);
    await av1an.getByLabel('Chunk Order', { exact: true }).selectOption('random');
    await page.screenshot({
      path: `${process.env.JESSES_MIGRATION_SCREENSHOTS ?? '/tmp'}/jesses-av1an-options-${width}-${height}.png`,
    });
  });
}
