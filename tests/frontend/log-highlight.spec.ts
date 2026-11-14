import { expect, test } from '@playwright/test';
import { highlightLogLine } from '../../src/lib/features/convert/log-highlight';

test('recognizes encoder diagnostics, progress and phases without treating filenames as failures', () => {
  const cases = [
    ['[consumer] x264 [error]: could not open file', 'error'],
    ['[consumer] Svt[error]: Invalid parameter', 'error'],
    ['[producer] [h264 @ 00001234] Error while decoding stream', 'error'],
    ['[producer] [h264 @ 00001234] [error] Invalid data found', 'error'],
    ['[producer] [muxer @ 00001234] [warning] Timestamp adjusted', 'warning'],
    ['[consumer] x265 [warning]: Too few rows/columns, --wpp disabled', 'warning'],
    ['[producer] Warning: timestamp adjusted.', 'warning'],
    ['[consumer] 120 frames: 60.00 fps, 3500.00 kb/s', 'progress'],
    ['[consumer] Encoding: 120/450 Frames @ 60.00 fps', 'progress'],
    ['frame= 120 fps=60.0 time=00:00:04.00', 'progress'],
    ['Verified output published. The source was preserved.', 'success'],
    ['Muxing encoded video and applying selected audio settings.', 'phase'],
    ['Source: C:\\Videos\\Error [warning] movie.mkv', 'plain'],
    ['Command: ffmpeg -metadata title="Error: sample" --error-resilient=0', 'plain'],
    ['[consumer] x264 [info]: 0 errors, 2 warnings', 'plain'],
  ] as const;
  for (const [line, tone] of cases) expect(highlightLogLine(line).tone, line).toBe(tone);
});

test('tokenizes command values and measured stats without changing the log text', () => {
  const line =
    'Command: ffmpeg -i "C:\\A folder\\sample.mkv" -map 0:v:0 | x264 --crf 20 --aq-strength -0.5';
  const tokens = highlightLogLine(line).tokens;
  expect(tokens.filter((token) => token.kind === 'option').map((token) => token.text)).toEqual([
    '-i',
    '-map',
    '--crf',
    '--aq-strength',
  ]);
  expect(tokens.filter((token) => token.kind === 'string').map((token) => token.text)).toEqual([
    '"C:\\A folder\\sample.mkv"',
  ]);
  expect(tokens.find((token) => token.text === '-0.5')?.kind).toBe('number');
  expect(tokens.find((token) => token.text === '|')?.kind).toBe('operator');
  for (const text of [
    line,
    '',
    '  \t[consumer] 12/50 Frames @ 3.00 fps | 88.14 kb/s  ',
    'time=00:00:04.200 speed=1.25x',
    '<script>alert("literal")</script> & untouched',
    '"unfinished quote \\ and <tag>',
  ]) {
    expect(
      highlightLogLine(text)
        .tokens.map((token) => token.text)
        .join(''),
    ).toBe(text);
  }
});
