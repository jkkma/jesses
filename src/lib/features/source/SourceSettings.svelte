<script lang="ts">
  import { untrack } from 'svelte';
  import { loadPreferences, preferences, updatePreferences } from '$lib/preferences.svelte';
  import { chooseOutputFolder, isDesktop } from '$lib/ipc/client';
  import { errorMessage } from '$lib/components/shared/format';
  import SourceDialog from '$lib/components/shared/SourceDialog.svelte';
  import type { GeneralPreferences, ResetOnNewFilePreferences } from '$lib/ipc/generated';
  const isWindows = /Windows/i.test(navigator.userAgent);
  const desktop = isDesktop();
  let { onreset }: { onreset: () => void } = $props();
  const defaultReset: ResetOnNewFilePreferences = {
    trim: true,
    frameRate: false,
    resize: false,
    crop: true,
    borders: false,
    deinterlace: true,
    toneMap: true,
    customFilters: false,
  };
  const resetLabels: [keyof ResetOnNewFilePreferences, string][] = [
    ['trim', 'Trim'],
    ['frameRate', 'Frame Rate'],
    ['resize', 'Resize'],
    ['crop', 'Crop'],
    ['borders', 'Borders'],
    ['deinterlace', 'Deinterlace'],
    ['toneMap', 'Tone Mapping'],
    ['customFilters', 'Custom Filters'],
  ];
  let folder = $state(''),
    busy = $state(false),
    error = $state(''),
    resetNotice = $state(''),
    resetOpen = $state(false),
    draft = $state<ResetOnNewFilePreferences>({ ...defaultReset });
  let dirty = false;
  let tail = Promise.resolve();
  const saved = $derived(preferences.value.general);
  const ready = $derived(desktop && preferences.loaded && !preferences.loading);
  const reset = $derived(saved.resetOnNewFile ?? defaultReset);
  $effect(() => {
    const value = preferences.value.general.defaultOutputDirectory;
    untrack(() => {
      if (!dirty) folder = value;
    });
  });
  function savePatch(patch: Partial<GeneralPreferences>) {
    const result = tail.then(async () => {
      if (!preferences.loaded) return false;
      busy = true;
      error = '';
      try {
        await updatePreferences({
          general: { ...preferences.value.general, ...patch },
          recentPaths: null,
        });
        return true;
      } catch (cause) {
        error = errorMessage(cause);
        return false;
      } finally {
        busy = false;
      }
    });
    tail = result.then(() => {});
    return result;
  }
  async function saveControl<K extends keyof GeneralPreferences>(
    control: HTMLInputElement | HTMLSelectElement,
    key: K,
    value: GeneralPreferences[K],
    defaultValue: GeneralPreferences[K],
  ) {
    const checkbox =
      control instanceof HTMLInputElement && control.type === 'checkbox' ? control : null;
    const changed = checkbox?.checked ?? control.value;
    await savePatch({ [key]: value });
    const authoritative = preferences.value.general[key] ?? defaultValue;
    if (checkbox) {
      if (checkbox.checked === changed) checkbox.checked = Boolean(authoritative);
    } else if (control.value === changed) control.value = String(authoritative);
  }
  async function saveFolder() {
    if (!dirty) return;
    const draftFolder = folder;
    if (
      (await savePatch({ defaultOutputDirectory: draftFolder.trim() })) &&
      folder === draftFolder
    ) {
      dirty = false;
      folder = preferences.value.general.defaultOutputDirectory;
    }
  }
  async function browse() {
    try {
      const path = await chooseOutputFolder();
      if (path) {
        folder = path;
        dirty = true;
        await saveFolder();
      }
    } catch (cause) {
      error = errorMessage(cause);
    }
  }
  function openReset() {
    draft = { ...reset };
    resetOpen = true;
  }
</script>

<section class="settings" aria-label="Settings">
  <h2>GENERAL</h2>
  {#if preferences.loadError}
    <p role="alert">Could not load preferences. {preferences.loadError}</p>
    <div><button type="button" onclick={loadPreferences}>Retry loading preferences</button></div>
  {:else if desktop && !ready}
    <p role="status">Loading preferences…</p>
  {:else if preferences.notice}
    <p role="status">{preferences.notice}</p>
  {/if}
  <div class="form">
    <label>
      Console window <select
        value={saved.consoleWindow ?? 'hidden'}
        disabled={!ready}
        onchange={(e) =>
          void saveControl(
            e.currentTarget,
            'consoleWindow',
            e.currentTarget.value as GeneralPreferences['consoleWindow'],
            'hidden',
          )}
      >
        <option value="hidden">Hidden (Default)</option><option value="visible">
          Visible
        </option><option value="visibleStayOpen">Visible, stays open after finishing</option>
      </select>
    </label>
    <label>
      Auto-crop samples <input
        type="number"
        min="1"
        max="100"
        value={saved.autoCropSamples ?? 10}
        disabled={!ready}
        onchange={(e) =>
          void saveControl(
            e.currentTarget,
            'autoCropSamples',
            Math.max(1, Math.min(100, e.currentTarget.valueAsNumber || 10)),
            10,
          )}
      />
    </label>
    <label>
      Keyframe interval (s) <input
        type="number"
        min="1"
        max="120"
        value={saved.keyframeIntervalSeconds ?? 10}
        disabled={!ready}
        onchange={(e) =>
          void saveControl(
            e.currentTarget,
            'keyframeIntervalSeconds',
            Math.max(1, Math.min(120, e.currentTarget.valueAsNumber || 10)),
            10,
          )}
      />
    </label>
    <div class="form-row">
      <label for="default-output-folder">Save encodes to</label>
      <span class="folder">
        <input
          id="default-output-folder"
          value={folder}
          placeholder="Leave empty to save next to the source file"
          disabled={!ready}
          oninput={(event) => {
            folder = event.currentTarget.value;
            dirty = true;
          }}
          onblur={saveFolder}
          onkeydown={(event) => {
            if (event.key === 'Enter' && !event.isComposing && event.keyCode !== 229) {
              event.preventDefault();
              void saveFolder();
            }
          }}
        /><button
          type="button"
          aria-label="Browse default output folder"
          title="Browse default output folder"
          onclick={browse}
          disabled={!ready}
        >
          …
        </button><button
          type="button"
          onclick={() => {
            folder = '';
            dirty = true;
            void saveFolder();
          }}
          disabled={!ready || !folder}
        >
          Clear
        </button>
      </span>
    </div>
  </div>
  <label class="check">
    <input
      type="checkbox"
      checked={saved.zeroIndexedStreams ?? false}
      disabled={!ready}
      onchange={(e) =>
        void saveControl(e.currentTarget, 'zeroIndexedStreams', e.currentTarget.checked, false)}
    /> Show stream numbers zero-indexed (like ffmpeg does)
  </label>
  <label class="check">
    <input
      type="checkbox"
      checked={saved.mp4Faststart ?? true}
      disabled={!ready}
      onchange={(e) =>
        void saveControl(e.currentTarget, 'mp4Faststart', e.currentTarget.checked, true)}
    /> Web-optimize MP4 output (+faststart)
  </label>
  <label class="check">
    <input
      type="checkbox"
      checked={saved.av1anConsoleVisible ?? true}
      disabled={!ready || !isWindows}
      onchange={(e) =>
        void saveControl(e.currentTarget, 'av1anConsoleVisible', e.currentTarget.checked, true)}
    />
    {isWindows
      ? 'Show the av1an console window'
      : "Show the av1an console window (Windows only - av1an's output goes to the log here)"}
  </label>
  <h2>RESET ON NEW FILE</h2>
  <div class="reset">
    <span>Settings to reset</span><span>
      {resetLabels
        .filter(([key]) => reset[key])
        .map(([, label]) => label)
        .join(', ') || 'None'}
    </span><button type="button" onclick={openReset} disabled={!ready}> Configure… </button><button
      type="button"
      onclick={() => {
        onreset();
        resetNotice = 'Reset selected settings in Quick Convert and AV1AN.';
      }}
    >
      Reset Now
    </button>
  </div>
  {#if resetNotice}<p role="status">{resetNotice}</p>{/if}
  {#if error}<p role="alert">{error}</p>{/if}
</section>
<SourceDialog
  title="Settings To Reset On New File"
  open={resetOpen}
  onclose={() => (resetOpen = false)}
>
  <p>Select the settings to reset when a new file is loaded.</p>
  {#each resetLabels as [key, label]}<label class="check">
      <input type="checkbox" bind:checked={draft[key]} />
      {label}
    </label>{/each}
  {#if error}<p role="alert">{error}</p>{/if}
  <div class="dialog-actions">
    <button type="button" onclick={() => (resetOpen = false)}>Cancel</button>
    <button
      type="button"
      disabled={busy}
      onclick={async () => {
        if (await savePatch({ resetOnNewFile: { ...draft } })) resetOpen = false;
      }}
    >
      OK
    </button>
  </div>
</SourceDialog>

<style>
  .settings {
    display: grid;
    align-content: start;
    gap: 0.4875rem;
    max-width: 920px;
    padding: 8px;
  }
  h2 {
    margin: 0.4063rem 0 0;
    color: var(--foreground);
    border-bottom: 1px solid var(--rule);
    padding-bottom: 3px;
    font-size: 0.6906rem;
    letter-spacing: 0.12em;
  }
  .form {
    display: grid;
    gap: 0.3656rem;
  }
  .form > label,
  .form > .form-row {
    display: grid;
    grid-template-columns: 200px minmax(200px, 1fr);
    align-items: center;
    gap: 0.4063rem;
  }
  .form > label select {
    width: min(100%, 225px);
    justify-self: start;
  }
  .dialog-actions {
    display: flex;
    justify-content: flex-end;
    gap: 0.325rem;
    margin-top: 0.7313rem;
  }
  select,
  input {
    min-width: 0;
    padding: 0.325rem;
    border: 1px solid var(--border);
    background: var(--background);
    color: var(--foreground);
  }
  .form input[type='number'] {
    width: 100px;
  }
  .folder,
  .reset {
    display: flex;
    flex-wrap: wrap;
    align-items: center;
    gap: 0.325rem;
  }
  .folder input {
    width: 430px;
    max-width: 100%;
  }
  .reset span {
    min-width: 190px;
  }
  button {
    padding: 0.325rem 0.5688rem;
    border: 1px solid var(--border);
    background: var(--secondary);
    color: var(--secondary-foreground);
  }
  button:hover:not(:disabled) {
    background: var(--accent);
    color: var(--accent-foreground);
  }
  button:disabled,
  input:disabled,
  select:disabled {
    opacity: 0.6;
  }
  .check {
    display: flex;
    gap: 0.4063rem;
    align-items: center;
  }
  p {
    color: var(--muted-foreground);
  }
  p[role='alert'] {
    color: var(--destructive);
    font-weight: 600;
  }
</style>
