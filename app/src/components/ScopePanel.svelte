<script lang="ts">
  // 보는 범위: 하위 폴더 사진도 보기, 다른 폴더를 더해 함께 보기.
  // 사진이 많아질수록 목록·분석이 느려지므로 둘 다 필요할 때만 켠다 (기본은 폴더 하나).
  import { store } from "../lib/store.svelte";
  import { baseName } from "../lib/api";
  import Icon from "./Icon.svelte";

  let { onclose }: { onclose: () => void } = $props();
  let busy = $state(false);

  async function run(fn: () => Promise<unknown>) {
    if (busy) return;
    busy = true;
    try {
      await fn();
    } finally {
      busy = false;
    }
  }
</script>

<div class="backdrop" role="presentation" onclick={onclose}></div>
<div class="panel" role="dialog" aria-label="보는 범위">
  <b>보는 범위</b>
  <p class="note">사진이 많아지면 목록과 분석이 느려질 수 있어, 필요할 때만 켭니다. 한 번에 파일 50,000개까지 모읍니다.</p>

  <label class="row">
    <input
      type="checkbox"
      checked={store.settings.include_subfolders}
      disabled={busy}
      onchange={(e) => run(() => store.setSettings({ include_subfolders: e.currentTarget.checked }))}
    />
    <span class="what">
      <b>하위 폴더 사진도 보기</b>
      <span class="desc">이 폴더 안의 모든 폴더 사진을 한 화면에 모아 봅니다.</span>
    </span>
  </label>

  <div class="sec">
    <b>함께 보는 폴더</b>
    <span class="desc">다른 곳에 있는 폴더를 더해 한 화면에서 고르고 정리합니다. 다른 폴더로 이동하면 끝납니다.</span>
  </div>
  <ul class="folders">
    <li title={store.path}>
      <Icon name="dir" size={14} />
      <span class="name">{baseName(store.path) || store.path}</span>
      <span class="tag">지금 폴더</span>
    </li>
    {#each store.extra as p (p)}
      <li title={p}>
        <Icon name="dir" size={14} />
        <span class="name">{baseName(p) || p}</span>
        <button class="x" disabled={busy} title="빼기" onclick={() => run(() => store.removeFolder(p))}>✕</button>
      </li>
    {/each}
  </ul>
  <div class="actions">
    <button class="btn" disabled={busy} onclick={() => run(() => store.pickAddFolders())}>＋ 다른 폴더 더하기…</button>
    {#if store.extra.length}
      <button class="btn subtle" disabled={busy} onclick={() => run(() => store.clearFolders())}>
        {store.extra.length}곳 모두 빼기
      </button>
    {/if}
  </div>
  {#if !store.singleFolder}
    <p class="stat">폴더 {store.dirCount.toLocaleString()}개 · 사진 {store.imageTotal.toLocaleString()}장</p>
  {/if}
</div>

<style>
  .backdrop {
    position: fixed;
    inset: 0;
    z-index: 70;
  }
  .panel {
    position: fixed;
    left: 50%;
    top: 46px;
    transform: translateX(-50%);
    z-index: 71;
    width: 380px;
    padding: 12px 14px;
    border: 1px solid var(--border);
    border-radius: 10px;
    background: var(--panel);
    box-shadow: 0 12px 40px rgba(0, 0, 0, 0.45);
    font-size: 13px;
  }
  .note {
    margin: 6px 0 8px;
    font-size: 11px;
    color: var(--text-faint);
    line-height: 1.45;
  }
  .row {
    display: flex;
    gap: 10px;
    align-items: flex-start;
    padding: 8px 0;
    border-top: 1px solid var(--border);
    cursor: pointer;
  }
  .row input {
    margin-top: 2px;
  }
  .what,
  .sec {
    display: flex;
    flex-direction: column;
    gap: 2px;
  }
  .sec {
    padding-top: 8px;
    border-top: 1px solid var(--border);
  }
  .desc {
    font-size: 11px;
    color: var(--text-faint);
  }
  .folders {
    list-style: none;
    margin: 8px 0;
    padding: 0;
    max-height: 180px;
    overflow-y: auto;
  }
  .folders li {
    display: flex;
    align-items: center;
    gap: 6px;
    padding: 3px 0;
    color: var(--text-dim);
  }
  .folders .name {
    flex: 1;
    min-width: 0;
    overflow: hidden;
    white-space: nowrap;
    text-overflow: ellipsis;
  }
  .tag {
    font-size: 11px;
    color: var(--text-faint);
  }
  .x {
    border: 0;
    background: none;
    color: var(--text-faint);
    cursor: pointer;
  }
  .x:hover {
    color: var(--text);
  }
  .actions {
    display: flex;
    gap: 8px;
  }
  .stat {
    margin: 8px 0 0;
    font-size: 12px;
    color: var(--accent);
  }
</style>
