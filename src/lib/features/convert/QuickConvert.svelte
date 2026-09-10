<script lang="ts">
  import {
    ArrowRight,
    AudioLines,
    Clapperboard,
    FolderOutput,
    Info,
    LockKeyhole,
    Play,
    SlidersHorizontal,
  } from '@lucide/svelte';
  import { Button } from '$lib/components/ui/button';
  import type { MediaFile } from '$lib/ipc/generated';
  let { file, onfiles }: { file: MediaFile | undefined; onfiles: () => void } = $props();
</script>

<section class="convert-workspace" aria-label="Quick Convert configuration preview">
  <div class="view-intro">
    <div>
      <span class="eyebrow">Single-file workflow</span>
      <h1>Quick Convert</h1>
      <p>A straightforward setup for your next encode.</p>
    </div>
    <span class="status-label"
      ><LockKeyhole size={13} aria-hidden="true" />Configuration preview</span
    >
  </div>
  <div class="notice convert-notice">
    <Info size={16} aria-hidden="true" />
    <p>
      Encoding is not connected yet. These are the proposed defaults; controls will become available
      when processing is implemented.
    </p>
  </div>
  <div class="convert-grid">
    <div class="convert-settings">
      <section class="panel settings-panel">
        <div class="section-heading">
          <span class="heading-with-icon"
            ><Clapperboard size={16} aria-hidden="true" /><span class="eyebrow">Video</span></span
          ><span class="small-muted">01</span>
        </div>
        <div class="setting-fields">
          <div class="field full-width">
            <label for="video-encoder">Encoder</label><select id="video-encoder" disabled
              ><option>SVT-AV1 · standalone</option></select
            >
            <p>AV1 software encoding</p>
          </div>
          <div class="field">
            <label for="rate-control">Rate control</label><select id="rate-control" disabled
              ><option>Constant quality (CRF)</option></select
            >
          </div>
          <div class="field">
            <label for="quality">Quality</label>
            <div class="input-unit"><input id="quality" value="30" disabled /><span>CRF</span></div>
          </div>
          <div class="field">
            <label for="encoder-preset">Encoder preset</label><select id="encoder-preset" disabled
              ><option>4</option></select
            >
            <p>Encoding speed / efficiency</p>
          </div>
          <div class="field">
            <label for="video-dimensions">Dimensions</label><select id="video-dimensions" disabled
              ><option>Keep source dimensions</option></select
            >
          </div>
        </div>
      </section>
      <section class="panel settings-panel">
        <div class="section-heading">
          <span class="heading-with-icon"
            ><AudioLines size={16} aria-hidden="true" /><span class="eyebrow">Audio</span></span
          ><span class="small-muted">02</span>
        </div>
        <div class="setting-fields audio-fields">
          <div class="field">
            <label for="audio-codec">Codec</label><select id="audio-codec" disabled
              ><option>Opus</option></select
            >
          </div>
          <div class="field">
            <label for="audio-bitrate">Bitrate</label>
            <div class="input-unit">
              <input id="audio-bitrate" value="128" disabled /><span>kb/s</span>
            </div>
          </div>
          <div class="field">
            <label for="audio-channels">Channels</label><select id="audio-channels" disabled
              ><option>Stereo</option></select
            >
          </div>
        </div>
      </section>
    </div>
    <aside class="panel output-panel">
      <div class="section-heading">
        <span class="heading-with-icon"
          ><FolderOutput size={16} aria-hidden="true" /><span class="eyebrow">Output</span></span
        >
      </div>
      <div class="output-content">
        <div class="output-source">
          <span class="eyebrow">Source</span><strong>{file?.name ?? 'No source selected'}</strong
          ><button type="button" class="text-button" onclick={onfiles}
            >{file ? 'Change source' : 'Choose a source'}<ArrowRight
              size={13}
              aria-hidden="true"
            /></button
          >
        </div>
        <div class="field">
          <label for="output-container">Container</label><select id="output-container" disabled
            ><option>Matroska (.mkv)</option></select
          >
        </div>
        <div class="field">
          <label for="output-directory">Destination</label><input
            id="output-directory"
            value="Same folder as source"
            disabled
          />
        </div>
        <div class="field">
          <label for="output-suffix">Filename suffix</label><input
            id="output-suffix"
            value="_encoded"
            disabled
          />
        </div>
        <div class="output-summary">
          <SlidersHorizontal size={15} aria-hidden="true" />
          <p><strong>AV1 + Opus</strong><span>CRF 30 · Preset 4 · MKV</span></p>
        </div>
        <Button class="start-encode" disabled title="Encoding will be available in a future build"
          ><Play size={14} aria-hidden="true" />Start encode</Button
        >
        <p class="disabled-reason">Available when encoding is implemented.</p>
      </div>
    </aside>
  </div>
</section>
