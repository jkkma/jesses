<script lang="ts">
  import { untrack } from 'svelte';
  import { Button } from '$lib/components/ui/button';
  import { preferences, updatePreferences, loadPreferences } from '$lib/preferences.svelte';
  import {
    chooseOutputFolder,
    choosePreferenceImport,
    previewPreferenceImport,
  } from '$lib/ipc/client';
  import { errorMessage } from '$lib/components/shared/format';
  import type { PreferenceImportPreview } from '$lib/ipc/generated';
  let folder = $state('');
  let recursive = $state(false);
  let folderDirty = false;
  let recursiveDirty = false;
  let busy = $state(false);
  let error = $state<string | null>(null);
  let message = $state('');
  let preview = $state<PreferenceImportPreview | null>(null);
  let disposed = false;
  $effect(() => {
    const general = preferences.value.general;
    untrack(() => {
      if (!folderDirty) folder = general.defaultOutputDirectory;
      if (!recursiveDirty) recursive = general.recursiveImport;
    });
  });
  $effect(() => () => {
    disposed = true;
  });
  function acceptSavedGeneral() {
    folderDirty = false;
    recursiveDirty = false;
    folder = preferences.value.general.defaultOutputDirectory;
    recursive = preferences.value.general.recursiveImport;
  }
  async function action(work: () => Promise<void>) {
    busy = true;
    error = null;
    message = '';
    try {
      await work();
    } catch (e) {
      if (!disposed) error = errorMessage(e);
    } finally {
      if (!disposed) busy = false;
    }
  }
  async function save() {
    await action(async () => {
      await updatePreferences({
        general: { defaultOutputDirectory: folder.trim(), recursiveImport: recursive },
        recentPaths: null,
      });
      if (!disposed) {
        acceptSavedGeneral();
        message = 'Preferences saved. New drafts use the selected folder.';
      }
    });
  }
  async function browse() {
    await action(async () => {
      const path = await chooseOutputFolder();
      if (path && !disposed) {
        folderDirty = true;
        folder = path;
      }
    });
  }
  async function review() {
    preview = null;
    await action(async () => {
      const path = await choosePreferenceImport();
      if (!path || disposed) return;
      const value = await previewPreferenceImport(path);
      if (!disposed) preview = value;
    });
  }
  async function applyImport() {
    const selected = preview;
    if (!selected) return;
    await action(async () => {
      await updatePreferences(selected.request);
      if (!disposed) {
        acceptSavedGeneral();
        preview = null;
        message = 'Imported the reviewed general preferences.';
      }
    });
  }
  async function clearRecent() {
    await action(async () => {
      await updatePreferences({ general: preferences.value.general, recentPaths: [] });
      if (!disposed) message = 'Recent-media history cleared.';
    });
  }
</script>

<section class="panel preferences-panel" aria-label="General preferences">
  <div class="section-heading"><span class="eyebrow">General preferences</span></div>
  <div class="preferences-body">
    {#if preferences.error}<p role="alert">{preferences.error}</p>
      <Button variant="outline" onclick={() => loadPreferences()} disabled={busy}
        >Retry preferences</Button
      >{/if}
    <label for="preference-folder">Default output folder</label>
    <div class="folder-row">
      <input
        id="preference-folder"
        type="text"
        bind:value={folder}
        oninput={() => {
          folderDirty = true;
        }}
        disabled={busy || !preferences.loaded}
        placeholder="Use the source folder"
      /><Button variant="outline" onclick={browse} disabled={busy || !preferences.loaded}
        >Choose folder</Button
      >
    </div>
    <p class="small-muted">
      Used for new encode destinations. Existing drafts and queued jobs keep their destinations.
      Leave empty to use each source folder.
    </p>
    <label class="checkbox-row"
      ><input
        type="checkbox"
        bind:checked={recursive}
        onchange={() => {
          recursiveDirty = true;
        }}
        disabled={busy || !preferences.loaded}
      />Include subfolders when adding a folder</label
    >
    <div class="actions">
      <Button onclick={save} disabled={busy || !preferences.loaded}>Save preferences</Button><Button
        variant="outline"
        onclick={clearRecent}
        disabled={busy || !preferences.loaded || !preferences.value.recentPaths.length}
        >Clear recent media</Button
      ><Button variant="outline" onclick={review} disabled={busy || !preferences.loaded}
        >Import saved preferences</Button
      >
    </div>
    <p class="small-muted">
      Import reviews the saved output folder and recent-media list. Encoder arguments, media drafts,
      tool paths, and automatic resume settings are not imported.
    </p>
    {#if preview}<div class="import-preview" aria-label="Preference import review">
        <h2>Review preferences</h2>
        <p>
          Output folder: <span class="mono"
            >{preview.request.general.defaultOutputDirectory || 'Use source folder'}</span
          >
        </p>
        <p>
          {preview.request.recentPaths?.length ?? 0} recent entries · {preview.ignoredKeyCount} unsupported
          keys skipped
        </p>
        {#each preview.warnings as warning}<p>{warning}</p>{/each}<Button
          onclick={applyImport}
          disabled={busy}>Apply imported preferences</Button
        ><Button variant="outline" onclick={() => (preview = null)} disabled={busy}
          >Discard import</Button
        >
      </div>{/if}
    {#if error}<p role="alert">{error}</p>{/if}{#if message}<p role="status">{message}</p>{/if}
  </div>
</section>

<style>
  .preferences-body {
    padding: 14px;
    display: grid;
    gap: 10px;
    min-width: 0;
  }
  .folder-row,
  .actions {
    display: flex;
    gap: 10px;
    flex-wrap: wrap;
  }
  .folder-row input {
    flex: 1;
    min-width: min(100%, 16rem);
  }
  .checkbox-row {
    display: flex;
    align-items: center;
    gap: 8px;
    width: fit-content;
  }
  .import-preview {
    padding: 12px;
    border: 1px solid var(--border);
    overflow-wrap: anywhere;
  }
  .import-preview h2 {
    font-size: 16px;
    margin-bottom: 10px;
  }
  .import-preview p {
    margin-bottom: 10px;
  }
  input[type='text'] {
    border: 1px solid var(--border);
    background: var(--background);
    border-radius: 5px;
    padding: 8px 10px;
  }
  input:focus-visible {
    outline: 2px solid var(--primary);
    outline-offset: 2px;
  }
  @media (max-width: 800px) {
    .folder-row {
      align-items: stretch;
    }
    .folder-row input {
      flex-basis: 100%;
    }
  }
</style>
