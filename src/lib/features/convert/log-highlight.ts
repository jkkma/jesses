export type LogTone = 'plain' | 'phase' | 'progress' | 'success' | 'warning' | 'error';
export type LogTokenKind = 'text' | 'label' | 'string' | 'tool' | 'option' | 'number' | 'operator';
export type LogToken = { kind: LogTokenKind; text: string };
export type HighlightedLogLine = { tone: LogTone; tokens: LogToken[] };

function logTone(line: string): LogTone {
  // Paths and command arguments may themselves contain words such as "error".
  if (/^\s*(?:Source|Output|Tool|Command|Mux log|Tool log path):/.test(line)) return 'plain';
  const message = line.replace(/^\s*\[(?:producer|consumer)\]\s*/i, '').trimStart();
  const diagnostic = message.replace(/^(?:\[(?!(?:error|fatal|warn|warning)\])[^\]]+\]\s*)+/i, '');
  if (
    /^(?:[\w.-]+\s*)?\[(?:error|fatal)\]/i.test(diagnostic) ||
    /^(?:Error\b|Fatal\b|Failed to\b|Could not\b|Cannot\b|Conversion failed[!.]?$)/i.test(
      diagnostic,
    )
  )
    return 'error';
  if (
    /^(?:[\w.-]+\s*)?\[(?:warn|warning)\]/i.test(diagnostic) ||
    /^(?:Warning\b|Warn:)/i.test(diagnostic)
  )
    return 'warning';
  if (
    /^(?:Verified output published\.|Output verified and saved\.|Finished and saved\.|encoded \d+ frames\b)/i.test(
      message,
    )
  )
    return 'success';
  if (
    /^(?:frame=\s*\d|Encoding:?\s+\d|Encoding frame\s+\d|\d+\s+frames:|\[?\d+(?:\.\d+)?%\]?\s+\d+\/\d+)/i.test(
      message,
    )
  )
    return 'progress';
  if (
    /^(?:Inspecting |Reading |Planning |Will encode |Encoding .+ with |Muxing |Checking |Output check:|Generated command confirmed)/.test(
      message,
    )
  )
    return 'phase';
  return 'plain';
}

/** Classify plain tool output, preserving every character for display and copying. */
export function highlightLogLine(line: string): HighlightedLogLine {
  const pattern =
    /(?<label>\b(?:Source|Output|Tool|Command|Mux log|Tool log path):)|(?<string>"(?:\\.|[^"\\])*"|'[^']*')|(?<tool>\[[^\]\r\n]+\])|(?<option>(?<![\w/\\])--?[A-Za-z][\w.-]*(?::[\w:]+)?)|(?<number>(?<![\w.\\])[-+]?\d+(?:[:/]\d+)*(?:\.\d+)?(?:%|[ \t]*(?:fps|kb\/s|kbits\/s|kbps|Mbps|KiB|MiB|GiB|kB|MB|GB|ms|s|x)\b)?(?![\w.]))|(?<operator>\|\||&&|[|=])/g;
  const tokens: LogToken[] = [];
  let position = 0;
  for (const match of line.matchAll(pattern)) {
    if (match.index > position)
      tokens.push({ kind: 'text', text: line.slice(position, match.index) });
    const kind = Object.entries(match.groups!).find(
      ([, value]) => value !== undefined,
    )![0] as LogTokenKind;
    tokens.push({ kind, text: match[0] });
    position = match.index + match[0].length;
  }
  if (position < line.length) tokens.push({ kind: 'text', text: line.slice(position) });
  return { tone: logTone(line), tokens };
}
