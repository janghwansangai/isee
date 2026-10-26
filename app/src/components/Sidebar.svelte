<script lang="ts">
  import { store } from "../lib/store.svelte";
  import { baseName } from "../lib/api";
  import TreeNode from "./TreeNode.svelte";

  let main = $derived(store.places.filter((p) => !["volume", "cloud"].includes(p.icon)));
  let volumes = $derived(store.places.filter((p) => p.icon === "volume"));
  let clouds = $derived(store.places.filter((p) => p.icon === "cloud"));
  // 현재 폴더를 품은 위치 중 가장 가까운 것 하나만 자동으로 펼친다 (홈과 사진이 둘 다 펼쳐지지 않게)
  let bestRoot = $derived(
    store.places
      .filter((p) => store.path === p.path || store.path.startsWith(p.path.replace(/[\\/]?$/, "/")) || store.path.startsWith(p.path.replace(/[\\/]?$/, "\\")))
      .sort((a, b) => b.path.length - a.path.length)[0]?.path,
  );
  let quick = $derived(
    store.settings.quick_folders
      .map((p, i) => ({ n: i + 1, path: p }))
      .filter((q): q is { n: number; path: string } => !!q.path),
  );
</script>

<nav class="sidebar" aria-label="폴더">
  <div class="section">위치</div>
  {#each main as p (p.path)}
    <TreeNode name={p.name} path={p.path} icon={p.icon} auto={p.path === bestRoot} />
  {/each}

  {#if volumes.length}
    <div class="section">드라이브</div>
    {#each volumes as p (p.path)}
      <TreeNode name={p.name} path={p.path} icon="volume" auto={p.path === bestRoot} />
    {/each}
  {/if}

  {#if clouds.length}
    <div class="section">클라우드</div>
    {#each clouds as p (p.path)}
      <TreeNode name={p.name} path={p.path} icon="cloud" auto={p.path === bestRoot} />
    {/each}
  {/if}

  <div class="section">
    빠른 이동 <span class="hint">숫자키 1~9</span>
    <button class="edit" onclick={() => (store.dialog = { type: "quickFolders" })}>설정</button>
  </div>
  {#each quick as q (q.n)}
    <TreeNode name={baseName(q.path)} path={q.path} badge={String(q.n)} hasChildren={false} auto={false} />
  {:else}
    <div class="empty">사진을 보면서 숫자키를 누르면<br />폴더를 지정하고 바로 옮깁니다</div>
  {/each}
</nav>

<style>
  .sidebar {
    height: 100%;
    overflow-y: auto;
    overflow-x: hidden;
    padding: 6px 0 16px;
    background: var(--panel);
    border-right: 1px solid var(--border);
  }
  .section {
    display: flex;
    align-items: center;
    gap: 6px;
    padding: 12px 14px 4px;
    font-size: 11px;
    font-weight: 600;
    color: var(--text-faint);
  }
  .hint {
    font-weight: 400;
  }
  .edit {
    margin-left: auto;
    color: var(--accent);
    font-size: 11px;
    font-weight: 500;
  }
  .empty {
    padding: 4px 14px;
    font-size: 11px;
    line-height: 1.5;
    color: var(--text-faint);
  }
</style>
