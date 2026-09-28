<script lang="ts">
  import { onMount } from 'svelte';
  import {
    Check,
    CircleAlert,
    ExternalLink,
    FolderSearch,
    Info,
    LoaderCircle,
    RefreshCw,
    Wrench,
  } from '@lucide/svelte';
  import { Button } from '$lib/components/ui/button';
  import PreferencesPanel from './PreferencesPanel.svelte';
  import { appVersion } from '../../app-version';
  import type { ToolInfo } from '$lib/ipc/generated';
  import { getStorageLocations } from '$lib/ipc/client';
  import { errorMessage } from '$lib/components/shared/format';
  let {
    tools,
    desktop,
    loading,
    checked,
    error,
    onrefresh,
  }: {
    tools: ToolInfo[];
    desktop: boolean;
    loading: boolean;
    checked: boolean;
    error: string | null;
    onrefresh: () => void;
  } = $props();
  const available = $derived(tools.filter((tool) => tool.available).length);
  let locations = $state<[string, string][]>([]);
  let storageError = $state<string | null>(null);
  onMount(() => {
    let active = true;
    if (desktop) {
      void getStorageLocations()
        .then((result) => {
          if (active) locations = result;
        })
        .catch((error) => {
          if (active) storageError = errorMessage(error);
        });
    }
    return () => {
      active = false;
    };
  });
</script>

<section class="tools-workspace" aria-label="Tools and settings">
  <div class="view-intro">
    <div>
      <span class="eyebrow">Installed tools & app settings</span>
      <h1>Tools & environment</h1>
      <p>Check what jesses can use, then set defaults for future jobs.</p>
    </div>
    <Button variant="outline" onclick={onrefresh} disabled={!desktop || loading}
      ><RefreshCw size={14} class={loading ? 'spinning' : ''} aria-hidden="true" />Check again</Button
    >
  </div>
  {#if error}<div class="notice error-notice" role="alert">
      <CircleAlert size={16} aria-hidden="true" />
      <p>{error}</p>
    </div>{/if}
  <section class="panel tool-detection">
    <div class="section-heading">
      <span class="heading-with-icon"
        ><Wrench size={15} aria-hidden="true" /><span class="eyebrow">Tool readiness</span></span
      ><span class="small-muted"
        >{desktop && loading
          ? 'Checking local tools…'
          : desktop && !checked
            ? 'Tools not checked'
            : desktop && tools.length
              ? `${available} / ${tools.length} available`
              : desktop
                ? 'No tool results'
                : 'Desktop required'}</span
      >
    </div>
    <p class="tool-guidance">
      Tool availability varies by workflow. A missing tool affects only the features that depend on
      it.
    </p>
    {#if !desktop}
      <div class="tools-empty">
        <FolderSearch size={34} strokeWidth={1.2} aria-hidden="true" />
        <h2>Connect to your desktop.</h2>
        <p>
          Open the jesses desktop app to locate media tools and inspect local files. Tool
          availability cannot be checked in this browser preview.
        </p>
      </div>
    {:else if loading && !tools.length}
      <div class="tools-empty" role="status">
        <LoaderCircle size={25} class="spinning" aria-hidden="true" />
        <h2>Checking tools…</h2>
        <p>Checking the required media tools and additional capabilities.</p>
      </div>
    {:else if tools.length}
      <div class="tools-table-scroll">
        <table class="tools-table">
          <thead><tr><th>Tool</th><th>Status</th><th>Diagnostics</th></tr></thead><tbody>
            {#each tools as tool (tool.id)}
              <tr
                ><td><strong>{tool.name}</strong><span class="tool-id mono">{tool.id}</span></td><td
                  ><span class:available={tool.available} class="tool-status"
                    >{#if loading}<LoaderCircle
                        size={13}
                        class="spinning"
                        aria-hidden="true"
                      />{checked ? 'Refreshing…' : 'Checking…'}{:else if !checked}<Info
                        size={13}
                        aria-hidden="true"
                      />Not checked{:else if tool.available}<Check
                        size={13}
                        aria-hidden="true"
                      />Available{:else}<CircleAlert size={13} aria-hidden="true" />{tool.path
                        ? 'Check failed'
                        : 'Not found'}{/if}</span
                  ></td
                ><td
                  ><details class="tool-diagnostics">
                    <summary>Show details</summary>
                    <dl>
                      <div>
                        <dt>Version</dt>
                        <dd class="mono">{tool.version ?? 'Unavailable'}</dd>
                      </div>
                      <div>
                        <dt>Location</dt>
                        <dd class="mono">{tool.path ?? 'Not detected'}</dd>
                      </div>
                      {#if tool.detail}<div>
                          <dt>Detection</dt>
                          <dd>{tool.detail}</dd>
                        </div>{/if}
                    </dl>
                  </details></td
                ></tr
              >
            {/each}
          </tbody>
        </table>
      </div>
    {:else}
      <div class="tools-empty">
        <FolderSearch size={30} strokeWidth={1.2} aria-hidden="true" />
        <h2>No detection results.</h2>
        <p>Choose “Check again” to inspect this environment.</p>
      </div>
    {/if}
    <div class="panel-footnote">
      <Info size={14} aria-hidden="true" /><span
        >Open a row’s details first. If a Windows package is missing a bundled file, repair or
        reinstall Jesses, then choose Check again. If a configured override fails, correct that path
        and recheck.</span
      >
    </div>
  </section>
  {#if desktop}
    <PreferencesPanel />
    <section class="panel scorer-panel">
      <div class="section-heading">
        <span class="heading-with-icon"
          ><Wrench size={15} aria-hidden="true" /><span class="eyebrow">Quality scoring</span></span
        >
      </div>
      <p class="scorer-summary">
        Windows packages include Vship and CPU quality scorers. Jesses checks GPU support for
        SSIMULACRA2 and Butteraugli jobs and uses CPU scoring when the GPU check fails. No extra
        setup is needed for the Windows package.
      </p>
    </section>
    <details class="panel optional-panel storage-panel" open={storageError !== null}>
      <summary class="section-heading"
        ><span class="eyebrow">Application storage</span><span class="small-muted"
          >Paths & portability</span
        ></summary
      >
      {#if storageError}<p class="storage-error" role="alert">{storageError}</p>
      {:else if !locations.length}<p class="small-muted storage-error">
          Reading application locations…
        </p>
      {:else}
        <dl class="storage-locations">
          {#each locations as [label, value] (label)}
            <div>
              <dt>{label}</dt>
              <dd class="mono">{value}</dd>
            </div>
          {/each}
        </dl>
      {/if}
      <div class="panel-footnote">
        Portable packages keep preferences, history and logs beside the app. Installed copies use
        your profile folders.
      </div>
    </details>
  {/if}
  <section class="panel about-panel">
    <div>
      <div class="brand-wordmark">jesses<span class="version-tag">{appVersion}</span></div>
      <p>Desktop media encoding, muxing, and analysis.</p>
      <span class="small-muted">Created by jkkma.</span>
    </div>
    <a href="https://github.com/jkkma/jesses" target="_blank" rel="noreferrer" class="text-button"
      >Project repository<ExternalLink size={13} aria-hidden="true" /></a
    >
  </section>
</section>

<style>
  .storage-locations {
    margin: 0;
    padding: 14px;
    display: grid;
    gap: 10px;
  }
  .storage-locations div {
    display: grid;
    grid-template-columns: minmax(8rem, 1fr) minmax(0, 3fr);
    gap: 12px;
  }
  .storage-locations dt {
    font-size: 0.8rem;
  }
  .storage-locations dd {
    margin: 0;
    overflow-wrap: anywhere;
    font-size: 0.75rem;
  }
  .storage-error {
    padding: 14px;
  }
  .tool-guidance,
  .scorer-summary {
    margin: 0;
    padding: 0 17px 13px;
    color: var(--muted-foreground);
    font-size: 11px;
    line-height: 1.55;
  }
  .tool-detection .section-heading > :global(.small-muted) {
    max-width: 55%;
    text-align: right;
  }
  .tool-diagnostics summary {
    cursor: pointer;
    color: var(--muted-foreground);
    font-size: 10px;
  }
  .tool-diagnostics dl {
    margin: 8px 0 0;
    display: grid;
    gap: 7px;
  }
  .tool-diagnostics dl > div {
    display: grid;
    gap: 2px;
  }
  .tool-diagnostics dt {
    color: var(--muted-foreground);
    font-size: 9px;
  }
  .tool-diagnostics dd {
    margin: 0;
    overflow-wrap: anywhere;
    font-size: 10px;
    line-height: 1.45;
  }
  .optional-panel {
    margin-top: 12px;
  }
  .optional-panel > summary {
    cursor: pointer;
    list-style-position: inside;
  }
  .optional-panel > summary::after {
    content: '›';
    margin-left: auto;
    transition: transform 100ms ease;
  }
  .optional-panel[open] > summary::after {
    transform: rotate(90deg);
  }
  .optional-panel[open] > summary {
    border-bottom: 1px solid var(--rule);
  }
  .optional-panel:not([open]) > summary {
    border-bottom: 0;
  }
  .tools-table td {
    padding-block: 10px;
  }
  .about-panel {
    margin-top: 12px;
    padding: 14px 17px;
  }
  .about-panel p {
    margin-top: 6px;
  }
  .scorer-panel {
    margin-top: 12px;
  }
  @media (max-width: 800px) {
    .storage-locations div {
      grid-template-columns: 1fr;
      gap: 0.3rem;
    }
    .optional-panel > summary {
      height: auto;
      min-height: 43px;
      padding-block: 10px;
    }
  }
</style>
