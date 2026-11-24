/** Shared file/track state follows the fork's File List and Track List. */
export type FileListMode = 'mux' | 'batch';
export type BatchRowStatus = { glyph: string; text: string; note?: string };

export type TrackSelection = {
  sourceId: string;
  streamIndex: number;
  included: boolean;
  title: string;
  language: string;
  defaultFlag: boolean | null;
  forcedFlag: boolean | null;
};

/**
 * The Track List's selection kind. Cover art is its own kind, so a cover listed
 * before the movie is never picked as the video. Encodes keep a checked cover.
 */
export function trackKind(
  stream: { kind: string; attachedPic?: boolean } | undefined,
): string | undefined {
  return stream?.kind === 'video' && stream.attachedPic ? 'cover' : stream?.kind;
}

/** Apply the Track List's first video, audio, subtitle and cover selection rule. */
export function firstTrackSelection(
  rows: TrackSelection[],
  kindOf: (row: TrackSelection) => string | undefined,
  seen = new Set<string>(),
): TrackSelection[] {
  return rows.map((row) => {
    const kind = kindOf(row) ?? '';
    const included = ['video', 'audio', 'subtitle', 'cover'].includes(kind) && !seen.has(kind);
    seen.add(kind);
    return { ...row, included };
  });
}

type StreamFacts = {
  kind: string;
  codec?: string | null;
  attachmentMimeType?: string | null;
  attachmentFilename?: string | null;
};

/** A font attached to a Matroska file, as styled (ASS/SSA) subtitles use. */
export function isFontAttachment(stream: StreamFacts | undefined): boolean {
  if (stream?.kind !== 'attachment') return false;
  const mime = stream.attachmentMimeType?.toLowerCase() ?? '';
  const name = stream.attachmentFilename?.toLowerCase() ?? '';
  // FFmpeg names recognised font attachments by codec (ttf, otf); others only
  // by MIME type or file name.
  return (
    ['ttf', 'otf'].includes(stream.codec?.toLowerCase() ?? '') ||
    mime.startsWith('font/') ||
    /(?:truetype|opentype|font-ttf|font-otf|x-font)/.test(mime) ||
    /\.(?:ttf|otf|ttc|woff2?)$/.test(name)
  );
}

export function isStyledSubtitle(stream: StreamFacts | undefined): boolean {
  return stream?.kind === 'subtitle' && ['ass', 'ssa'].includes(stream.codec ?? '');
}

/**
 * Styled subtitles render with the fonts packed beside them. Include a file's
 * fonts when one of its ASS/SSA tracks becomes included, and leave them out
 * again with the last one. Between those changes the user's own font choices
 * stand.
 */
export function syncSubtitleFonts(
  rows: TrackSelection[],
  streamOf: (row: TrackSelection) => StreamFacts | undefined,
  previous: TrackSelection[] = [],
): TrackSelection[] {
  const styled = (list: TrackSelection[], sourceId: string) =>
    list.some(
      (row) => row.sourceId === sourceId && row.included && isStyledSubtitle(streamOf(row)),
    );
  const changes = new Map<string, boolean>();
  for (const sourceId of new Set(rows.map((row) => row.sourceId))) {
    const now = styled(rows, sourceId);
    const before = previous.length ? styled(previous, sourceId) : false;
    if (now !== before) changes.set(sourceId, now);
  }
  if (!changes.size) return rows;
  return rows.map((row) =>
    changes.has(row.sourceId) && isFontAttachment(streamOf(row))
      ? { ...row, included: changes.get(row.sourceId)! }
      : row,
  );
}

/** Active tabs register the source-style global Run action with the shell. */
export type WorkspaceAction = {
  run: (options?: { editCommand?: boolean; isShiftHeld?: () => boolean }) => Promise<void>;
  reset?: () => void;
  active?: boolean;
  paused?: boolean;
  progress?: number;
  progressDescription?: string;
  /** The batch's current job, or null while the batch prepares its next job. */
  progressJobId?: string | null;
  waitingBatch?: { id: string; count: number; cancel: () => void };
  batchNaming?: { codec: string; quality: string; preset: string; extension: string };
  pause?: () => Promise<void>;
  stop?: () => void;
  stopReview?: () => void;
  canRun: boolean;
  /** Why Run is unavailable, shown beside the Run button as an error. */
  blockedReason?: string;
  blockedByMissingTools?: boolean;
  /** Open the setting responsible for the visible blocker, when available. */
  openBlockedSetting?: () => void;
  label: string;
};

export type TaskView = 'convert' | 'av1an' | 'utilities';
/** Each mounted tab owns its key and unregisters it on destruction. */
export type RegisterWorkspaceAction = (view: TaskView, action: WorkspaceAction | null) => void;

let batchRunSerial = 0;
/** Unique for the lifetime of this UI instance; lets the shell discard late status events. */
export function batchRunId(owner: string): string {
  return `${owner}:${Date.now()}:${++batchRunSerial}`;
}
