<script lang="ts">
  import { untrack } from 'svelte';
  import { preferences, updatePreferences } from '$lib/preferences.svelte';
  import { chooseOutputFolder, isDesktop } from '$lib/ipc/client';
  import { errorMessage } from '$lib/components/shared/format';
  import SourceDialog from '$lib/components/shared/SourceDialog.svelte';
  import type { GeneralPreferences, ResetOnNewFilePreferences } from '$lib/ipc/generated';
  const isWindows = /Windows/i.test(navigator.userAgent);
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
    resetOpen = $state(false),
    draft = $state<ResetOnNewFilePreferences>({ ...defaultReset });
  let dirty = false;
  let tail = Promise.resolve();
  const saved = $derived(preferences.value.general);
  const reset = $derived(saved.resetOnNewFile ?? defaultReset);
  $effect(() => {
    const value = preferences.value.general.defaultOutputDirectory;
    untrack(() => {
      if (!dirty) folder = value;
    });
  });
  function savePatch(patch: Partial<GeneralPreferences>) {
    tail = tail.then(async () => {
      if (!preferences.loaded) return;
      busy = true;
      error = '';
      try {
        await updatePreferences({
          general: { ...preferences.value.general, ...patch },
          recentPaths: null,
        });
      } catch (cause) {
        error = errorMessage(cause);
      } finally {
        busy = false;
      }
    });
    return tail;
  }
  async function saveFolder() {
    const value = folder.trim();
    await savePatch({ defaultOutputDirectory: value });
    dirty = false;
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
  <div class="form">
    <label
      >Console window <select
        value={saved.consoleWindow ?? 'hidden'}
        disabled={!isDesktop()}
        onchange={(e) =>
          void savePatch({
            consoleWindow: e.currentTarget.value as GeneralPreferences['consoleWindow'],
          })}
        ><option value="hidden">Hidden (Default)</option><option value="visible">Visible</option
        ><option value="visibleStayOpen">Visible, stays open after finishing</option></select
      ></label
    >
    <label
      >Auto-crop samples <input
        type="number"
        min="1"
        max="100"
        value={saved.autoCropSamples ?? 10}
        disabled={!isDesktop()}
        onchange={(e) =>
          void savePatch({
            autoCropSamples: Math.max(1, Math.min(100, e.currentTarget.valueAsNumber || 10)),
          })}
      /></label
    >
    <label
      >Keyframe interval (s) <input
        type="number"
        min="1"
        max="120"
        value={saved.keyframeIntervalSeconds ?? 10}
        disabled={!isDesktop()}
        onchange={(e) =>
          void savePatch({
            keyframeIntervalSeconds: Math.max(
              1,
              Math.min(120, e.currentTarget.valueAsNumber || 10),
            ),
          })}
      /></label
    >
    <label
      >Save encodes to <span class="folder"
        ><input
          value={folder}
          placeholder="Leave empty to save next to the source file"
          disabled={!isDesktop()}
          oninput={(event) => {
            folder = event.currentTarget.value;
            dirty = true;
          }}
          onblur={saveFolder}
        /><button type="button" onclick={browse} disabled={!isDesktop()}>…</button><button
          type="button"
          onclick={() => {
            folder = '';
            dirty = true;
            void saveFolder();
          }}
          disabled={!isDesktop()}>Clear</button
        ></span
      ></label
    >
  </div>
  <label class="check"
    ><input
      type="checkbox"
      checked={saved.zeroIndexedStreams ?? false}
      disabled={!isDesktop()}
      onchange={(e) => void savePatch({ zeroIndexedStreams: e.currentTarget.checked })}
    /> Show stream numbers zero-indexed (like ffmpeg does)</label
  >
  <label class="check"
    ><input
      type="checkbox"
      checked={saved.mp4Faststart ?? true}
      disabled={!isDesktop()}
      onchange={(e) => void savePatch({ mp4Faststart: e.currentTarget.checked })}
    /> Web-optimize MP4 output (+faststart)</label
  >
  <label class="check"
    ><input
      type="checkbox"
      checked={saved.av1anConsoleVisible ?? true}
      disabled={!isDesktop() || !isWindows}
      onchange={(e) => void savePatch({ av1anConsoleVisible: e.currentTarget.checked })}
    />
    {isWindows
      ? 'Show the av1an console window'
      : "Show the av1an console window (Windows only - av1an's output goes to the log here)"}</label
  >
  <h2>RESET ON NEW FILE</h2>
  <div class="reset">
    <span>Settings to reset</span><span
      >{resetLabels
        .filter(([key]) => reset[key])
        .map(([, label]) => label)
        .join(', ') || 'None'}</span
    ><button type="button" onclick={openReset} disabled={!isDesktop()}>Configure…</button><button
      type="button"
      onclick={onreset}>Reset Now</button
    >
  </div>
  {#if error}<p role="alert">{error}</p>{/if}
</section>
<SourceDialog
  title="Settings To Reset On New File"
  open={resetOpen}
  onclose={() => (resetOpen = false)}
>
  <p>Select the settings to reset when a new file is loaded.</p>
  {#each resetLabels as [key, label]}<label class="check"
      ><input type="checkbox" bind:checked={draft[key]} /> {label}</label
    >{/each}
  <div class="reset">
    <button
      onclick={() => {
        void savePatch({ resetOnNewFile: { ...draft } });
        resetOpen = false;
      }}>OK</button
    >
  </div>
</SourceDialog>

<style>
  .settings {
    display: grid;
    align-content: start;
    gap: 0.6rem;
    max-width: 800px;
    padding: 8px;
  }
  h2 {
    margin: 0.5rem 0 0;
    color: var(--muted-foreground);
    font-size: 11px;
    letter-spacing: 0.08em;
  }
  .form {
    display: grid;
    gap: 0.45rem;
  }
  .form > label {
    display: grid;
    grid-template-columns: 200px minmax(200px, 1fr);
    align-items: center;
    gap: 0.5rem;
  }
  select,
  input {
    min-width: 0;
    padding: 0.4rem;
    border: 1px solid var(--border);
    border-radius: var(--radius);
    background: var(--background);
    color: var(--foreground);
  }
  .form input[type='number'] {
    width: 100px;
  }
  .folder,
  .reset {
    display: flex;
    align-items: center;
    gap: 0.4rem;
  }
  .folder input {
    width: 430px;
    max-width: 100%;
  }
  .reset span {
    min-width: 190px;
  }
  button {
    padding: 0.4rem 0.7rem;
    border: 0;
    border-radius: var(--radius);
    background: var(--secondary);
    color: var(--secondary-foreground);
  }
  button:disabled,
  input:disabled,
  select:disabled {
    opacity: 0.6;
  }
  .check {
    display: flex;
    gap: 0.5rem;
    align-items: center;
  }
  p {
    color: var(--muted-foreground);
  }
  p[role='alert'] {
    color: var(--destructive);
  }
  select,
  input,
  button {
    border-radius: 0;
  }
  .form > label select {
    width: min(100%, 225px);
    justify-self: start;
  }
  h2 {
    color: var(--foreground);
    border-bottom: 1px solid var(--rule);
    padding-bottom: 3px;
    letter-spacing: 0.12em;
  }
</style>
