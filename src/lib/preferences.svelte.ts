import { getPreferences, savePreferences, rememberRecentMedia } from '$lib/ipc/client';
import { errorMessage } from '$lib/components/shared/format';
import type { SavePreferencesRequest, UserPreferences } from '$lib/ipc/generated';

export const preferences = $state<{
  value: UserPreferences;
  loaded: boolean;
  error: string | null;
}>({
  value: {
    general: { defaultOutputDirectory: '', recursiveImport: false },
    recentPaths: [],
    revision: 0,
  },
  loaded: false,
  error: null,
});
function receive(value: UserPreferences) {
  if (!preferences.loaded || value.revision >= preferences.value.revision)
    preferences.value = value;
  preferences.loaded = true;
  preferences.error = null;
}
export async function loadPreferences() {
  try {
    receive(await getPreferences());
  } catch (error) {
    preferences.error = errorMessage(error);
  }
}
export async function updatePreferences(request: SavePreferencesRequest) {
  const value = await savePreferences(request);
  receive(value);
}
export async function rememberMedia(paths: string[]) {
  if (!preferences.loaded) return;
  try {
    receive(await rememberRecentMedia(paths));
  } catch (error) {
    preferences.error = errorMessage(error);
  }
}
/** A suggestion only. Existing drafts and queued destinations stay unchanged. */
export function preferredDestination(suggestion: string): string {
  const folder = preferences.value.general.defaultOutputDirectory;
  if (!folder) return suggestion;
  const name = suggestion.split(/[\\/]/).at(-1) ?? '';
  return folder.replace(/[\\/]$/, '') + (folder.includes('\\') ? '\\' : '/') + name;
}
