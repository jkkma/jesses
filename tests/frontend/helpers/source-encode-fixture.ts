import type { Page } from '@playwright/test';

export const source = {
  id: 'source-1',
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
      width: 1920,
      height: 1080,
      frameRate: '24/1',
      fieldOrder: 'tt',
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
      title: 'Audio',
    },
    {
      index: 2,
      kind: 'subtitle',
      codec: 'subrip',
      width: null,
      height: null,
      frameRate: null,
      sampleRate: null,
      channels: null,
      language: 'eng',
      title: 'Subtitles',
    },
  ],
};

export async function installSourceEncodeFixture(page: Page) {
  await page.addInitScript((media) => {
    const state = globalThis as unknown as Record<string, any>;
    let callback = 0;
    let selectedMedia = media;
    state.__setSourceMedia = (next: any) => (selectedMedia = next);
    state.__recoveries = JSON.parse(sessionStorage.getItem('test-recoveries') ?? '[]');
    state.__mediaByPath = {};
    state.__pickerPaths = null;
    state.__imageFailurePath = null;
    state.__holdImage = null;
    state.__preferences = {
      general: { defaultOutputDirectory: '', recursiveImport: false },
      recentPaths: [],
      revision: 1,
    };
    state.__testJobs = JSON.parse(sessionStorage.getItem('test-initial-jobs') ?? '[]');
    state.__reviews = {};
    const calls: any[] = [];
    state.__sourceCalls = calls;
    state.isTauri = true;
    const emitJobs = () => state.__jobChannel?.onmessage(structuredClone(state.__testJobs));
    state.__emitJobs = emitJobs;
    state.__TAURI_INTERNALS__ = {
      metadata: { currentWindow: { label: 'main' }, currentWebview: { label: 'main' } },
      transformCallback: () => ++callback,
      unregisterCallback: () => {},
      invoke: async (command: string, payload: any = {}) => {
        calls.push({ command, payload });
        if (command === 'plugin:dialog|open') return state.__pickerPaths ?? [selectedMedia.path];
        if (command === 'plugin:dialog|save') {
          if (state.__holdSave) await state.__holdSave;
          if (state.__saveFailure) throw new Error(state.__saveFailure);
          return state.__savePath === undefined ? '/media/output.mkv' : state.__savePath;
        }
        if (command === 'probe_media') {
          if (state.__probeFailures?.includes(payload.path))
            throw new Error('Invalid media fixture.');
          return structuredClone(state.__mediaByPath[payload.path] ?? selectedMedia);
        }
        if (command === 'get_capabilities')
          return [
            'ffmpeg',
            'ffprobe',
            'svt-av1',
            'av1an',
            'mkvmerge',
            'x264',
            'x265',
            'vpxenc',
            'aomenc',
          ].map((id) => ({
            id,
            name: id,
            available: !state.__unavailableTools?.includes(id),
            path: `/bin/${id}`,
            version: 'test',
          }));
        if (command === 'list_jobs') return [];
        if (command === 'get_preferences' || command === 'remember_recent_media')
          return structuredClone(state.__preferences);
        if (command === 'save_preferences') {
          state.__preferences = {
            ...state.__preferences,
            general: payload.request.general,
            revision: state.__preferences.revision + 1,
          };
          return structuredClone(state.__preferences);
        }
        if (command === 'list_av1an_recoveries') return structuredClone(state.__recoveries);
        if (command === 'av1an_recovery_action') {
          const req = payload.request;
          const selected = state.__recoveries.filter((entry: any) =>
            req.action === 'clearAll' ? req.ids.includes(entry.job.id) : entry.job.id === req.id,
          );
          if (state.__recoveryFailure) throw new Error(state.__recoveryFailure);
          if (
            req.action === 'deleteEncode' ||
            req.action === 'clearAll' ||
            req.action === 'resumeSaved' ||
            req.action === 'resumeNew'
          )
            state.__recoveries = state.__recoveries.filter(
              (entry: any) => !selected.includes(entry),
            );
          return {
            jobs: selected.map((entry: any) => ({
              ...entry.job,
              state: req.action.startsWith('resume') ? 'queued' : entry.job.state,
              recovery:
                req.action === 'deleteEncode' || req.action === 'clearAll'
                  ? null
                  : entry.job.recovery,
              encodeSettings: req.settings ?? entry.job.encodeSettings,
            })),
          };
        }
        if (command === 'subscribe_jobs') {
          state.__jobChannel = payload.channel;
          payload.channel?.onmessage(structuredClone(state.__testJobs));
          return;
        }
        if (command === 'get_completion_status')
          return {
            options: { notify: false, finishAction: 'none' },
            armedJobs: 0,
            secondsRemaining: null,
            error: null,
          };
        if (command.startsWith('plugin:event|')) return ++callback;
        if (command === 'start_encode')
          return {
            id: 'job',
            state: 'running',
            request: payload.request.source,
            encodeSettings: payload.request.settings,
            progressSeconds: 0,
            durationSeconds: 12,
            logs: [],
            error: null,
            recovery: null,
            logPath: null,
          };
        if (command === 'prepare_command_edit') {
          if (state.__holdPrepare) await state.__holdPrepare;
          if (state.__prepareFailure) throw new Error(state.__prepareFailure);
          const id = `review-${++callback}`,
            request = payload.request,
            recovery =
              request.kind === 'resumeSaved' || request.kind === 'resumeNew'
                ? state.__recoveries.find((entry: any) => entry.job.id === request.id)
                : undefined;
          if (
            (request.kind === 'resumeSaved' || request.kind === 'resumeNew') &&
            state.__recoveryFailure
          )
            throw new Error(state.__recoveryFailure);
          const job =
            request.kind === 'encode'
              ? {
                  id,
                  state: 'preparing',
                  request: request.request.source,
                  encodeSettings: request.request.settings,
                  progressSeconds: 0,
                  durationSeconds: 12,
                  logs: [],
                  error: null,
                  recovery: null,
                  logPath: null,
                }
              : recovery
                ? {
                    ...recovery.job,
                    state: 'preparing',
                    encodeSettings: request.settings ?? recovery.job.encodeSettings,
                  }
                : undefined;
          state.__reviews[id] = { request, job };
          if (job) {
            state.__testJobs = [job, ...state.__testJobs];
            emitJobs();
          }
          return {
            id,
            executableLabel:
              request.kind.startsWith('resume') ||
              (request.kind === 'encode' && request.request.settings.backend === 'av1an')
                ? 'av1an'
                : 'ffmpeg',
            arguments: '-i "input file" | encoder --value 4 && mux',
            outputPath:
              request.request?.source?.outputPath ??
              request.request?.outputPath ??
              job?.request.outputPath ??
              '/media/out',
            job,
          };
        }
        if (command === 'confirm_command_edit') {
          const held = state.__reviews[payload.request.id];
          if (!held) throw new Error('Expired review');
          if (state.__holdConfirm) await state.__holdConfirm;
          if (held.request.kind === 'encode') {
            const request = held.request.request;
            calls.push({ command: 'mock_start_encode', payload: { request } });
            const job = { ...held.job, state: state.__holdTerminal ? 'running' : 'succeeded' };
            state.__testJobs = state.__testJobs.map((entry: any) =>
              entry.id === job.id ? job : entry,
            );
            emitJobs();
            return { kind: 'encode', job };
          }
          if (held.request.kind === 'resumeSaved' || held.request.kind === 'resumeNew') {
            state.__recoveries = state.__recoveries.filter(
              (entry: any) => entry.job.id !== held.request.id,
            );
            const job = { ...held.job, state: 'running' };
            state.__testJobs = state.__testJobs.map((entry: any) =>
              entry.id === job.id ? job : entry,
            );
            emitJobs();
            return { kind: 'encode', job };
          }
          if (held.request.kind === 'image') {
            const request = held.request.request;
            calls.push({ command: 'mock_image_execution', payload: { request } });
            if (state.__holdImage) await state.__holdImage;
            if (
              request.inputPath === state.__imageFailurePath ||
              (request.operation === 'implicitExport' && state.__implicitFailure)
            )
              throw new Error('image export failed');
            return {
              kind: 'image',
              result: {
                outputPath: request.outputPath,
                frameCount: 288,
                width: 1920,
                height: 1080,
                notes: [],
              },
            };
          }
          throw new Error('Unexpected review kind');
        }
        if (command === 'cancel_command_edit') {
          const held = state.__reviews[payload.id];
          if (held?.job) {
            state.__testJobs = state.__testJobs.map((entry: any) =>
              entry.id === held.job.id ? { ...entry, state: 'canceled' } : entry,
            );
            emitJobs();
          }
          return;
        }
        if (command === 'set_command_edit_paused') {
          if (state.__pauseFailure) throw new Error(state.__pauseFailure);
          if (state.__holdPause) await state.__holdPause;
          if (!state.__reviews[payload.id]) throw new Error('Expired review');
          state.__reviews[payload.id].paused = payload.paused;
          return;
        }
        if (command === 'cancel_job') {
          state.__testJobs = state.__testJobs.map((entry: any) =>
            entry.id === payload.id ? { ...entry, state: 'canceled' } : entry,
          );
          emitJobs();
          return state.__testJobs.find((entry: any) => entry.id === payload.id);
        }
        if (command === 'cancel_all_jobs') {
          state.__testJobs = state.__testJobs.map((entry: any) =>
            ['succeeded', 'failed', 'canceled', 'interrupted'].includes(entry.state)
              ? entry
              : { ...entry, state: 'canceled' },
          );
          emitJobs();
          return structuredClone(state.__testJobs);
        }
        if (command === 'set_job_paused') {
          state.__testJobs = state.__testJobs.map((entry: any) =>
            entry.id === payload.id
              ? { ...entry, state: payload.paused ? 'paused' : 'running' }
              : entry,
          );
          emitJobs();
          return state.__testJobs.find((entry: any) => entry.id === payload.id);
        }
        if (command === 'begin_media_analysis') {
          const id = `analysis-${++callback}`;
          if (state.__holdAnalysis) await state.__holdAnalysis;
          return id;
        }
        if (command === 'cancel_media_analysis') return;
        if (command === 'snap_keyframe') {
          if (state.__holdSnaps)
            return await new Promise((resolve, reject) => {
              (state.__pendingSnaps ??= []).push({ request: payload.request, resolve, reject });
            });
          return {
            positionSeconds: Math.floor(payload.request.positionSeconds / 2) * 2,
            sourceFingerprint: 'test',
          };
        }
        if (command === 'preview_frame') {
          if (state.__holdPreviews)
            return await new Promise((resolve, reject) => {
              (state.__pendingPreviews ??= []).push({ request: payload.request, resolve, reject });
            });
          return {
            imageDataUrl:
              'data:image/png;base64,iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAQAAAC1HAwCAAAAC0lEQVR42mNk+A8AAQUBAScY42YAAAAASUVORK5CYII=',
            width: 1,
            height: 1,
            sourceWidth: 1920,
            sourceHeight: 1080,
            positionSeconds: payload.request.positionSeconds,
            sourceFingerprint: 'test',
            toneMapped: false,
          };
        }
        if (command === 'resolve_deinterlace')
          return { request: payload.request, message: 'resolved' };
        if (command === 'detect_crop') {
          if (payload.request.inputPath === state.__cropFailurePath)
            throw new Error('Crop sample failed');
          return { crop: { top: 2, right: 0, bottom: 2, left: 0 }, sourceFingerprint: 'test' };
        }
        if (command === 'get_encoder_parameters') {
          if (state.__holdCatalogs)
            return await new Promise((resolve) =>
              (state.__pendingCatalogs ??= []).push({ request: payload.request, resolve }),
            );
          const nvenc = ['h264Nvenc', 'hevcNvenc'].includes(payload.request.encoder);
          return {
            encoder: payload.request.encoder,
            backend: payload.request.backend,
            route: 'test',
            toolPath: '/bin/test',
            toolVersion: 'test',
            notes: [],
            parameters: [
              {
                name: 'rc',
                label: 'rc',
                argument: '-rc',
                minimum: 0,
                maximum: 0,
                valueKind: 'choice',
                minimumValue: '',
                maximumValue: '',
                choices: ['constqp', 'vbr', 'cbr'],
                group: 'Quality & Tuning',
                description: 'Rate control',
                example: 'constqp',
              },
              {
                name: 'tune',
                label: 'tune',
                argument: '--tune',
                minimum: 0,
                maximum: 5,
                valueKind: nvenc ? 'choice' : 'integer',
                minimumValue: nvenc ? '' : '0',
                maximumValue: nvenc ? '' : '5',
                choices: nvenc ? ['hq', 'll', 'ull', 'lossless'] : [],
                group: 'Quality & Tuning',
                description: 'Tune',
                example: nvenc ? 'hq' : '0',
              },
            ],
          };
        }
        if (command === 'read_av1an_grain_table') return 'filmgrn1\nE 0 100 1 2 1\n';
        if (command === 'run_image_job') {
          if (state.__holdImage) await state.__holdImage;
          if (payload.request.inputPath === state.__imageFailurePath)
            throw new Error('image export failed');
          return {
            outputPath: payload.request.outputPath,
            frameCount: 288,
            width: 1920,
            height: 1080,
            notes: [],
          };
        }
        if (command === 'resolve_batch_output_paths') {
          if (state.__holdBatchResolution) await state.__holdBatchResolution;
          if (state.__batchResolutionFailure) throw new Error(state.__batchResolutionFailure);
          return {
            items: payload.request.inputs.map((input: any, index: number) => {
              if (index === state.__batchItemError)
                return {
                  inputPath: input.inputPath,
                  error: { code: 'OUTPUT_PATH_INVALID', message: 'Cannot use output folder' },
                };
              const path =
                state.__batchOutputPaths?.[index] ??
                input.inputPath.replace(/\.[^.]+$/, '') +
                  (payload.request.target.format === 'gif' ? '.gif' : '');
              return {
                inputPath: input.inputPath,
                resolution: {
                  outputStem: path,
                  outputPath: path,
                  isDirectory: payload.request.target.format !== 'gif',
                  numbered: false,
                },
              };
            }),
          };
        }
        if (command === 'preview_encode_batch') {
          if (state.__holdBatchPreview) await state.__holdBatchPreview;
          return {
            items: payload.request.inputs.map((input: any, index: number) => {
              const path =
                state.__batchOutputPaths?.[index] ??
                input.inputPath.replace(/\.[^.]+$/, '') + '.mkv';
              if (index === state.__batchPreviewItemError)
                return {
                  inputPath: input.inputPath,
                  outputPath: path,
                  error: { code: 'TEST_FAILURE', message: 'Preparation failed for this input' },
                  request: null,
                };
              return {
                inputPath: input.inputPath,
                outputPath: path,
                error: null,
                request: {
                  source: {
                    inputPath: input.inputPath,
                    outputPath: path,
                    streamIndices: input.streamIndices,
                  },
                  settings: { ...payload.request, ...input },
                },
              };
            }),
          };
        }
        if (command === 'enqueue_encode_batch') return [];
        if (command === 'resolve_output_path') {
          const request = payload.request;
          if (state.__holdResolution) await state.__holdResolution;
          if (state.__resolutionFailure) throw new Error(state.__resolutionFailure);
          const media =
            state.__mediaByPath[request.inputPath] ??
            (selectedMedia.path === request.inputPath ? selectedMedia : null);
          const sourcePath =
            media?.source?.kind === 'imageSequence'
              ? media.source.directoryPath
              : request.inputPath;
          const slash = Math.max(sourcePath.lastIndexOf('/'), sourcePath.lastIndexOf('\\'));
          const sourceName = sourcePath.slice(slash + 1);
          const name =
            media?.source?.kind === 'imageSequence'
              ? sourceName
              : sourceName.replace(/\.[^.]+$/, '');
          const directory =
            request.defaultOutputDirectory && request.defaultOutputDirectory !== '/missing'
              ? request.defaultOutputDirectory
              : sourcePath.slice(0, slash);
          let stem = request.outputStem?.trim() ?? `${directory}/${name}`;
          const target = request.target;
          const ext =
            target.kind === 'container'
              ? target.format === 'matroska'
                ? 'mkv'
                : target.format
              : target.format === 'gif'
                ? 'gif'
                : target.format === 'png'
                  ? 'png'
                  : target.format === 'jpeg'
                    ? 'jpg'
                    : '';
          const output = (value: string) => value + (ext ? `.${ext}` : '');
          const original = stem;
          let n = 0;
          while ((state.__occupied ?? []).includes(output(stem)))
            stem = `${original.replace(/\s*\(\d+\)$/, '')} (${++n})`;
          return { outputStem: stem, outputPath: output(stem), isDirectory: !ext, numbered: n > 0 };
        }
        throw new Error(`Unexpected command ${command}`);
      },
    };
  }, source);
  await page.goto('/');
}
