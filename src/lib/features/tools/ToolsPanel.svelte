<script lang="ts">
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
  import type { ToolInfo } from '$lib/ipc/generated';
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
</script>

<section class="tools-workspace" aria-label="Tools and settings">
  <div class="view-intro">
    <div>
      <span class="eyebrow">Application settings</span>
      <h1>Tools & environment</h1>
      <p>Check the media tools available to jesses.</p>
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
        ><Wrench size={15} aria-hidden="true" /><span class="eyebrow">Tool detection</span></span
      ><span class="small-muted"
        >{desktop && loading
          ? 'Checking local tools…'
          : desktop && !checked
            ? 'Tools have not been checked'
            : desktop && tools.length
              ? `${available} / ${tools.length} available`
              : desktop
                ? 'Local environment'
                : 'Desktop required'}</span
      >
    </div>
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
        <h2>Checking local tools…</h2>
        <p>Reading executable locations and versions.</p>
      </div>
    {:else if tools.length}
      <div class="tools-table-scroll">
        <table class="tools-table">
          <thead><tr><th>Tool</th><th>Status</th><th>Version & location</th></tr></thead><tbody>
            {#each tools as tool (tool.id)}
              <tr
                ><td><strong>{tool.name}</strong><span class="tool-id mono">{tool.id}</span></td><td
                  ><span class:available={tool.available} class="tool-status"
                    >{#if !checked && loading}<LoaderCircle
                        size={13}
                        class="spinning"
                        aria-hidden="true"
                      />Checking…{:else if !checked}<Info size={13} aria-hidden="true" />Not checked{:else if tool.available}<Check
                        size={13}
                        aria-hidden="true"
                      />Available{:else}<CircleAlert size={13} aria-hidden="true" />Not found{/if}</span
                  ></td
                ><td
                  ><span class="tool-version mono"
                    >{!checked
                      ? 'Awaiting detection'
                      : (tool.version ?? 'Version unavailable')}</span
                  ><span class="tool-path mono" title={tool.path ?? undefined}
                    >{!checked
                      ? 'Executable location and version are not known yet.'
                      : (tool.path ?? tool.detail ?? 'No executable detected')}</span
                  >{#if tool.path && tool.detail}<span class="tool-detail">{tool.detail}</span
                    >{/if}</td
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
        >Detection reports the current environment. Tools are not downloaded or installed
        automatically.</span
      >
    </div>
  </section>
  <section class="panel about-panel">
    <div>
      <div class="brand-wordmark">jesses<span class="version-tag">0.1.0</span></div>
      <p>Desktop media encoding, muxing, and analysis.</p>
      <span class="small-muted">Created by jkkma.</span>
    </div>
    <a href="https://github.com/jkkma/jesses" target="_blank" rel="noreferrer" class="text-button"
      >Project repository<ExternalLink size={13} aria-hidden="true" /></a
    >
  </section>
</section>
