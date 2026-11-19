<script lang="ts">
  import { onMount } from 'svelte';
  import { Check, CircleAlert, ExternalLink, Info, LoaderCircle, RefreshCw } from '@lucide/svelte';
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
    preferences?: boolean;
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
        .catch((err) => {
          if (active) storageError = errorMessage(err);
        });
    }
    return () => {
      active = false;
    };
  });
</script>

<section class="tools-workspace" aria-label="Installed tools">
  <div class="section-bar">
    <h2>INSTALLED TOOLS</h2>
    <div class="section-meta">
      <span class="small-muted"
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
      <button type="button" onclick={onrefresh} disabled={!desktop || loading}>
        <RefreshCw size={13} class={loading ? 'spinning' : ''} aria-hidden="true" />Check again
      </button>
    </div>
  </div>
  <p class="tool-guidance">
    Check which tools jesses found and why one is missing. A missing tool affects only the features
    that depend on it.
  </p>
  {#if error}
    <div class="error-banner" role="alert">
      <CircleAlert size={15} aria-hidden="true" />
      <p>{error}</p>
    </div>
  {/if}
  <div class="tool-box">
    {#if !desktop}
      <div class="tools-empty">
        <h3>Connect to your desktop</h3>
        <p>
          Open the jesses desktop app to locate media tools and inspect local files. Tool
          availability cannot be checked in this browser preview.
        </p>
      </div>
    {:else if loading && !tools.length}
      <div class="tools-empty" role="status">
        <LoaderCircle size={20} class="spinning" aria-hidden="true" />
        <h3>Checking tools…</h3>
        <p>Checking the required media tools and additional capabilities.</p>
      </div>
    {:else if tools.length}
      <div class="tools-table-scroll">
        <table class="tools-table">
          <thead>
            <tr>
              <th scope="col">Tool</th>
              <th scope="col">Status</th>
              <th scope="col">Diagnostics</th>
            </tr>
          </thead>
          <tbody>
            {#each tools as tool (tool.id)}
              <tr>
                <td><strong>{tool.name}</strong><span class="tool-id mono">{tool.id}</span></td>
                <td>
                  <span class:available={tool.available} class="tool-status">
                    {#if loading}
                      <LoaderCircle size={13} class="spinning" aria-hidden="true" />{checked
                        ? 'Refreshing…'
                        : 'Checking…'}
                    {:else if !checked}
                      <Info size={13} aria-hidden="true" />Not checked
                    {:else if tool.available}
                      <Check size={13} aria-hidden="true" />Available
                    {:else}
                      <CircleAlert size={13} aria-hidden="true" />{tool.path
                        ? 'Check failed'
                        : 'Not found'}
                    {/if}
                  </span>
                </td>
                <td>
                  <details class="tool-diagnostics">
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
                      {#if tool.detail}
                        <div>
                          <dt>Detection</dt>
                          <dd>{tool.detail}</dd>
                        </div>
                      {/if}
                    </dl>
                  </details>
                </td>
              </tr>
            {/each}
          </tbody>
        </table>
      </div>
    {:else}
      <div class="tools-empty">
        <h3>No detection results</h3>
        <p>Choose “Check again” to inspect this environment.</p>
      </div>
    {/if}
    <div class="footnote">
      <Info size={13} aria-hidden="true" />
      <span>
        Open a row’s details first. If a Windows package is missing a bundled file, repair or
        reinstall Jesses, then choose Check again. If a configured override fails, correct that path
        and recheck.
      </span>
    </div>
  </div>

  {#if desktop}
    <div class="sub-section">
      <h2>QUALITY SCORING</h2>
      <p class="scorer-summary">
        Windows packages include Vship and CPU quality scorers. Jesses checks GPU support for
        SSIMULACRA2 and Butteraugli jobs and uses CPU scoring when the GPU check fails. No extra
        setup is needed for the Windows package.
      </p>
    </div>

    <details class="storage-disclosure" open={storageError !== null}>
      <summary>
        <span>APPLICATION STORAGE</span>
        <span class="small-muted">Paths &amp; portability</span>
      </summary>
      <div class="storage-body">
        {#if storageError}
          <p class="storage-error" role="alert">{storageError}</p>
        {:else if !locations.length}
          <p class="small-muted">Reading application locations…</p>
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
        <p class="footnote-plain">
          Portable packages keep preferences, history and logs beside the app. Installed copies use
          your profile folders.
        </p>
      </div>
    </details>
  {/if}

  <div class="sub-section">
    <h2>ABOUT</h2>
    <div class="about-row">
      <div>
        <div class="about-title">
          <strong>jesses</strong><span class="version-tag mono">v{appVersion}</span>
        </div>
        <p class="small-muted">Desktop media encoding, muxing, and analysis. Created by jkkma.</p>
      </div>
      <a href="https://github.com/jkkma/jesses" target="_blank" rel="noreferrer" class="repo-link">
        Project repository<ExternalLink size={13} aria-hidden="true" />
      </a>
    </div>
  </div>
</section>

<style>
  .tools-workspace {
    display: grid;
    align-content: start;
    gap: 10px;
    max-width: 920px;
    padding: 0 8px 14px;
  }
  .section-bar {
    display: flex;
    flex-wrap: wrap;
    align-items: center;
    justify-content: space-between;
    gap: 8px;
    border-bottom: 1px solid var(--rule);
    padding-bottom: 4px;
  }
  h2 {
    margin: 0;
    color: var(--foreground);
    font-size: 0.85rem;
    letter-spacing: 0.12em;
  }
  .sub-section {
    display: grid;
    gap: 6px;
    margin-top: 4px;
  }
  .sub-section h2 {
    border-bottom: 1px solid var(--rule);
    padding-bottom: 3px;
  }
  .section-meta {
    display: flex;
    align-items: center;
    gap: 10px;
  }
  button {
    display: inline-flex;
    align-items: center;
    gap: 6px;
    padding: 0.35rem 0.7rem;
    border: 1px solid var(--border);
    background: var(--secondary);
    color: var(--secondary-foreground);
    font-size: 0.92rem;
  }
  button:hover:not(:disabled) {
    background: var(--accent);
    color: var(--accent-foreground);
  }
  .tool-guidance,
  .scorer-summary {
    color: var(--muted-foreground);
    font-size: 0.92rem;
    line-height: 1.5;
    max-width: 78ch;
  }
  .error-banner {
    display: flex;
    align-items: flex-start;
    gap: 8px;
    padding: 8px 10px;
    border: 1px solid var(--destructive);
    background: var(--background);
    color: var(--destructive);
    font-weight: 600;
  }
  .tool-box {
    border: 1px solid var(--border);
    background: var(--background);
  }
  .tools-empty {
    display: grid;
    gap: 4px;
    padding: 18px 14px;
    color: var(--muted-foreground);
  }
  .tools-empty h3 {
    color: var(--foreground);
    font-size: 1rem;
  }
  .tools-empty p {
    max-width: 66ch;
    font-size: 0.92rem;
  }
  .tools-table-scroll {
    overflow-x: auto;
  }
  .tools-table {
    width: 100%;
    border-collapse: collapse;
    text-align: left;
    table-layout: fixed;
  }
  .tools-table thead {
    background: var(--secondary);
  }
  .tools-table th {
    padding: 8px 12px;
    font-size: 0.77rem;
    text-transform: uppercase;
    letter-spacing: 0.08em;
    color: var(--foreground);
    font-weight: 600;
    border-bottom: 1px solid var(--border);
  }
  .tools-table th:first-child {
    width: 24%;
  }
  .tools-table th:nth-child(2) {
    width: 20%;
  }
  .tools-table td {
    padding: 9px 12px;
    border-bottom: 1px solid var(--rule);
    vertical-align: top;
    background: var(--card);
  }
  .tools-table tr:last-child td {
    border-bottom: 0;
  }
  .tool-id {
    display: block;
    color: var(--muted-foreground);
    font-size: 0.85rem;
    margin-top: 2px;
  }
  .tool-status {
    display: inline-flex;
    align-items: center;
    gap: 5px;
    font-size: 0.92rem;
    color: var(--destructive);
    white-space: nowrap;
  }
  .tool-status.available {
    color: var(--success);
  }
  .tool-diagnostics summary {
    cursor: pointer;
    color: var(--muted-foreground);
    font-size: 0.85rem;
  }
  .tool-diagnostics summary:hover {
    color: var(--foreground);
  }
  .tool-diagnostics dl {
    margin: 6px 0 0;
    display: grid;
    gap: 6px;
  }
  .tool-diagnostics dl > div {
    display: grid;
    gap: 2px;
  }
  .tool-diagnostics dt {
    color: var(--muted-foreground);
    font-size: 0.77rem;
    text-transform: uppercase;
    letter-spacing: 0.06em;
  }
  .tool-diagnostics dd {
    margin: 0;
    overflow-wrap: anywhere;
    font-size: 0.85rem;
    line-height: 1.45;
  }
  .footnote {
    display: flex;
    align-items: flex-start;
    gap: 8px;
    padding: 8px 12px;
    border-top: 1px solid var(--rule);
    background: var(--card);
    color: var(--muted-foreground);
    font-size: 0.85rem;
    line-height: 1.45;
  }
  .footnote > :global(svg) {
    margin-top: 2px;
  }
  .storage-disclosure {
    border: 1px solid var(--border);
    background: var(--background);
  }
  .storage-disclosure summary {
    display: flex;
    align-items: center;
    justify-content: space-between;
    gap: 12px;
    padding: 8px 12px;
    font-weight: 600;
    font-size: 0.85rem;
    letter-spacing: 0.08em;
    cursor: pointer;
  }
  .storage-disclosure summary:hover {
    background: var(--secondary);
  }
  .storage-body {
    display: grid;
    gap: 10px;
    padding: 12px;
    border-top: 1px solid var(--rule);
    background: var(--card);
  }
  .storage-locations {
    margin: 0;
    display: grid;
    gap: 8px;
  }
  .storage-locations div {
    display: grid;
    grid-template-columns: minmax(8rem, 1fr) minmax(0, 3fr);
    gap: 12px;
  }
  .storage-locations dt {
    font-size: 0.92rem;
    font-weight: 600;
  }
  .storage-locations dd {
    margin: 0;
    overflow-wrap: anywhere;
    font-size: 0.85rem;
  }
  .storage-error {
    color: var(--destructive);
    font-weight: 600;
  }
  .footnote-plain {
    color: var(--muted-foreground);
    font-size: 0.85rem;
  }
  .about-row {
    display: flex;
    flex-wrap: wrap;
    align-items: center;
    justify-content: space-between;
    gap: 12px;
    padding: 6px 0;
  }
  .about-title {
    display: flex;
    align-items: center;
    gap: 8px;
    margin-bottom: 2px;
  }
  .about-title strong {
    font-family: 'Century Gothic', 'Avenir Next', 'DejaVu Sans', sans-serif;
    font-size: 1.08rem;
    letter-spacing: 0.1em;
    text-transform: uppercase;
  }
  .version-tag {
    border: 1px solid var(--border);
    padding: 1px 6px;
    font-size: 0.85rem;
    color: var(--muted-foreground);
    background: var(--background);
  }
  .repo-link {
    display: inline-flex;
    align-items: center;
    gap: 5px;
    font-weight: 600;
    font-size: 0.92rem;
  }
  @media (max-width: 800px) {
    .storage-locations div {
      grid-template-columns: 1fr;
      gap: 0.2rem;
    }
  }
</style>
