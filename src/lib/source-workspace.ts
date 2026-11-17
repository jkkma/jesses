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
  batchNaming?: { codec: string; quality: string; preset: string; extension: string };
  pause?: () => Promise<void>;
  stop?: () => void;
  canRun: boolean;
  /** Why Run is unavailable, shown beside the Run button as an error. */
  blockedReason?: string;
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
