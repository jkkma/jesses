<script lang="ts">
  import { AudioLines, Captions, CircleHelp, FileSearch, Film, Info } from '@lucide/svelte';
  import type { MediaFile } from '$lib/ipc/generated';
  import MediaAnalysis from './MediaAnalysis.svelte';
  import MediaThumbnail from './MediaThumbnail.svelte';
  import QualityAnalysis from './QualityAnalysis.svelte';
  import {
    displayCodec,
    formatBytes,
    formatDuration,
    formatFrameRate,
  } from '$lib/components/shared/format';

  let { file, sample = false }: { file: MediaFile | undefined; sample?: boolean } = $props();
  const video = $derived(file?.streams.find((stream) => stream.kind === 'video'));
</script>

<aside class="inspector panel" aria-label="Media inspector">
  <div class="section-heading inspector-heading">
    <span class="eyebrow">Media inspector</span>
    <FileSearch size={15} strokeWidth={1.6} aria-hidden="true" />
  </div>

  {#if file}
    <div class="inspector-scroll">
      <div class="inspector-source">
        <div class="source-icon"><Film size={23} strokeWidth={1.4} aria-hidden="true" /></div>
        <span class="eyebrow">{sample ? 'Synthetic sample' : 'Selected source'}</span>
        <h2 title={file.name}>{file.name}</h2>
        <p class="source-path" title={file.path}>{file.path}</p>
      </div>
      <dl class="metadata-grid">
        <div>
          <dt>Duration</dt>
          <dd class="mono">{formatDuration(file.durationSeconds)}</dd>
        </div>
        <div>
          <dt>File size</dt>
          <dd class="mono">{formatBytes(file.sizeBytes)}</dd>
        </div>
        <div>
          <dt>Dimensions</dt>
          <dd class="mono">
            {video?.width && video.height ? `${video.width} × ${video.height}` : '—'}
          </dd>
        </div>
        <div>
          <dt>Frame rate</dt>
          <dd class="mono">{video?.frameRate ? `${formatFrameRate(video.frameRate)} fps` : '—'}</dd>
        </div>
      </dl>
      <div class="container-readout">
        <span>Container</span><strong title={file.format ?? 'Unknown'}
          >{file.format ?? 'Unknown'}</strong
        >
      </div>
      <MediaThumbnail {file} {sample} />
      <MediaAnalysis {file} {sample} />
      <QualityAnalysis {file} {sample} />
      <div class="section-heading track-heading">
        <span class="eyebrow">Streams</span><span class="count-label"
          >{file.streams.length.toString().padStart(2, '0')}</span
        >
      </div>
      <div class="stream-list">
        {#each file.streams as stream (stream.index)}
          <div class="stream-card">
            <div class="stream-card-heading">
              {#if stream.kind === 'video'}<Film size={15} aria-hidden="true" />
              {:else if stream.kind === 'audio'}<AudioLines size={15} aria-hidden="true" />
              {:else if stream.kind === 'subtitle'}<Captions size={15} aria-hidden="true" />
              {:else}<CircleHelp size={15} aria-hidden="true" />{/if}
              <strong class="stream-kind">{stream.kind}</strong>
              <span class="mono stream-index">#{stream.index}</span>
              <span class="codec-tag">{displayCodec(stream.codec)}</span>
            </div>
            <p class="stream-description">
              {#if stream.kind === 'video'}
                {stream.width && stream.height
                  ? `${stream.width} × ${stream.height}`
                  : 'Dimensions unavailable'}
                {#if stream.frameRate}<span class="detail-separator">·</span>{formatFrameRate(
                    stream.frameRate,
                  )} fps{/if}
              {:else if stream.kind === 'audio'}
                {stream.channels === 1
                  ? 'Mono'
                  : stream.channels === 2
                    ? 'Stereo'
                    : stream.channels
                      ? `${stream.channels} channels`
                      : 'Channels unavailable'}
                {#if stream.sampleRate}<span class="detail-separator">·</span>{(
                    stream.sampleRate / 1000
                  ).toLocaleString()} kHz{/if}
              {:else}
                {stream.title ?? 'Embedded stream'}
              {/if}
              {#if stream.language}<span class="detail-separator">·</span
                >{stream.language.toUpperCase()}{/if}
            </p>
            {#if stream.title && (stream.kind === 'video' || stream.kind === 'audio')}<p
                class="stream-title"
              >
                {stream.title}
              </p>{/if}
            {#if stream.kind === 'video'}
              <dl class="video-color" aria-label={`Video metadata for stream ${stream.index}`}>
                <div>
                  <dt>Pixel aspect</dt>
                  <dd>{stream.sampleAspectRatio ?? 'Not reported'}</dd>
                </div>
                <div>
                  <dt>Display aspect</dt>
                  <dd>{stream.displayAspectRatio ?? 'Not reported'}</dd>
                </div>
                <div>
                  <dt>Rotation</dt>
                  <dd>
                    {stream.rotationDegrees === undefined
                      ? 'Not reported'
                      : `${stream.rotationDegrees}°`}
                  </dd>
                </div>
                <div>
                  <dt>Frame rate</dt>
                  <dd>{stream.frameRate ?? 'Not reported'}</dd>
                </div>
                <div>
                  <dt>Pixel format</dt>
                  <dd>
                    {stream.pixelFormat ?? 'Not reported'}{stream.bitDepth
                      ? ` · ${stream.bitDepth}-bit`
                      : ''}
                  </dd>
                </div>
                <div>
                  <dt>Primaries</dt>
                  <dd>{stream.colorPrimaries ?? 'Not reported'}</dd>
                </div>
                <div>
                  <dt>Transfer</dt>
                  <dd>{stream.colorTransfer ?? 'Not reported'}</dd>
                </div>
                <div>
                  <dt>Matrix / range</dt>
                  <dd>
                    {stream.colorSpace ?? 'Not reported'} · {stream.colorRange ?? 'Not reported'}
                  </dd>
                </div>
                <div>
                  <dt>HDR transfer</dt>
                  <dd>{stream.hdrFormat ?? 'Not reported'}</dd>
                </div>
                <div>
                  <dt>Static HDR</dt>
                  <dd>
                    {stream.hasHdrStaticMetadata
                      ? 'Reported in stream headers'
                      : 'Not reported in stream headers'}
                  </dd>
                </div>
                <div>
                  <dt>Dynamic HDR detected</dt>
                  <dd>
                    {stream.dynamicHdrFormats?.length
                      ? stream.dynamicHdrFormats.join(' · ')
                      : 'Not reported in stream headers'}
                  </dd>
                </div>
              </dl>
              <p class="header-note">
                Frame metadata may contain additional HDR information. Encode compatibility is
                checked separately.
              </p>
            {/if}
          </div>
        {:else}
          <p class="quiet-message">No streams were reported for this source.</p>
        {/each}
      </div>
      <div class="inspector-note">
        <Info size={14} aria-hidden="true" /><span
          >{sample
            ? 'Sample values for interface review. No file has been read.'
            : 'Source information is read with ffprobe.'}</span
        >
      </div>
    </div>
  {:else}
    <div class="inspector-empty">
      <FileSearch size={35} strokeWidth={1.1} aria-hidden="true" />
      <h2>A closer look.</h2>
      <p>Select a source to inspect its video, audio, and subtitle streams.</p>
      <dl class="empty-metadata">
        <div>
          <dt>Container</dt>
          <dd>—</dd>
        </div>
        <div>
          <dt>Duration</dt>
          <dd>—</dd>
        </div>
        <div>
          <dt>Dimensions</dt>
          <dd>—</dd>
        </div>
        <div>
          <dt>Streams</dt>
          <dd>—</dd>
        </div>
      </dl>
    </div>
    <div class="inspector-note">
      <Info size={14} aria-hidden="true" /><span>Your source files stay unchanged.</span>
    </div>
  {/if}
</aside>

<style>
  .video-color {
    margin-top: 12px;
    display: grid;
    gap: 7px;
    font-size: 11px;
  }
  .video-color div {
    display: grid;
    grid-template-columns: 100px minmax(0, 1fr);
    gap: 8px;
  }
  .video-color dt,
  .header-note {
    color: var(--muted-foreground);
  }
  .video-color dd {
    margin: 0;
    overflow-wrap: anywhere;
  }
  .header-note {
    font-size: 10px;
    line-height: 1.5;
    margin-top: 10px;
  }
</style>
