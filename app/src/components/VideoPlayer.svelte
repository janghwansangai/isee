<script lang="ts">
  // 동영상 재생: 웹뷰 내장 플레이어 (mp4·mov·webm). 파일은 isee://file 로 조금씩 읽어 온다.
  import { onMount } from "svelte";
  import { invoke } from "@tauri-apps/api/core";
  import { store } from "../lib/store.svelte";
  import { api, fileUrl } from "../lib/api";
  import Icon from "./Icon.svelte";

  const entry = store.video!;
  let video = $state<HTMLVideoElement>();
  let failed = $state(false);

  onMount(() => {
    // 재생하는 동안 썸네일·분석 작업은 멈춘다 (뷰어와 같음)
    api.setViewerActive(true);
    video?.focus();
    return () => api.setViewerActive(store.viewer >= 0 || !!store.compare || !!store.editor);
  });

  function openDefault() {
    invoke("open_default", { path: entry.path });
    store.video = null;
  }
</script>

<div class="player" role="dialog" aria-label="동영상" tabindex="-1" oncontextmenu={(e) => e.preventDefault()}>
  <div class="topbar">
    <span class="title">{entry.name}</span>
    {#if entry.cloud}<span class="dim">클라우드에서 내려받으며 재생합니다</span>{/if}
    <span class="spacer"></span>
    <button class="btn" onclick={openDefault}>기본 앱에서 열기</button>
    <button class="icon-btn" title="닫기 (Esc)" onclick={() => (store.video = null)}><Icon name="close" /></button>
  </div>
  <div class="stage">
    {#if failed}
      <p class="msg">이 동영상은 앱 안에서 재생할 수 없습니다. <button class="link" onclick={openDefault}>기본 앱에서 열기</button></p>
    {:else}
      <!-- svelte-ignore a11y_media_has_caption -->
      <video bind:this={video} src={fileUrl(entry.path)} controls autoplay playsinline onerror={() => (failed = true)}></video>
    {/if}
  </div>
</div>

<style>
  .player {
    position: fixed;
    inset: 0;
    z-index: 60;
    display: flex;
    flex-direction: column;
    background: #000;
  }
  .topbar {
    display: flex;
    align-items: center;
    gap: 12px;
    height: 44px;
    padding: 0 10px 0 16px;
    background: var(--panel);
    border-bottom: 1px solid #1d1e22;
    white-space: nowrap;
  }
  .title {
    font-weight: 600;
    overflow: hidden;
    text-overflow: ellipsis;
  }
  .dim {
    color: var(--text-dim);
    font-size: 12px;
  }
  .spacer {
    flex: 1;
  }
  .stage {
    flex: 1;
    min-height: 0;
    display: flex;
    align-items: center;
    justify-content: center;
  }
  video {
    max-width: 100%;
    max-height: 100%;
    outline: none;
  }
  .msg {
    color: var(--text-dim);
  }
  .link {
    color: var(--accent);
  }
</style>
