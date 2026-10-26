<script lang="ts">
  import { api, type DirNode } from "../lib/api";
  import { store } from "../lib/store.svelte";
  import Icon from "./Icon.svelte";
  import TreeNode from "./TreeNode.svelte";

  let {
    name,
    path,
    icon = "dir",
    depth = 0,
    hasChildren = true,
    badge = "",
    auto = true,
  }: {
    name: string;
    path: string;
    icon?: string;
    depth?: number;
    hasChildren?: boolean;
    badge?: string;
    /** 현재 폴더의 상위이면 자동으로 펼칠지 */
    auto?: boolean;
  } = $props();

  let expanded = $state(false);
  let children = $state.raw<DirNode[] | null>(null);

  async function toggle(e: MouseEvent) {
    e.stopPropagation();
    expanded = !expanded;
    if (expanded && !children) {
      try {
        children = await api.subdirs(path);
      } catch {
        children = [];
      }
    }
  }

  // 현재 폴더가 이 노드 아래에 있으면 자동으로 펼친다
  $effect(() => {
    const cur = store.path;
    if (auto && !expanded && depth < 12 && cur !== path && isAncestor(path, cur)) {
      expanded = true;
      if (!children) api.subdirs(path).then((c) => (children = c), () => (children = []));
    }
  });

  function isAncestor(a: string, b: string) {
    const sep = a.includes("\\") ? "\\" : "/";
    const base = a.endsWith(sep) ? a : a + sep;
    return b.startsWith(base);
  }
</script>

<div
  class="row"
  class:current={store.path === path}
  class:drop={store.dropOver === path}
  style="padding-left:{8 + depth * 14}px"
  data-drop-path={path}
  onclick={() => store.open(path)}
  onkeydown={(e) => e.key === "Enter" && store.open(path)}
  role="treeitem"
  aria-selected={store.path === path}
  aria-expanded={hasChildren ? expanded : undefined}
  tabindex="-1"
>
  {#if hasChildren}
    <button class="twisty" class:open={expanded} onclick={toggle} aria-label="펼치기">
      <Icon name="chevron" size={12} />
    </button>
  {:else}
    <span class="twisty"></span>
  {/if}
  <span class="ico {icon}"><Icon name={icon} size={15} /></span>
  <span class="label">{name}</span>
  {#if badge}<span class="badge">{badge}</span>{/if}
</div>
{#if expanded && children}
  {#each children as c (c.path)}
    <TreeNode name={c.name} path={c.path} depth={depth + 1} hasChildren={c.has_children} {auto} />
  {/each}
{/if}

<style>
  .row {
    display: flex;
    align-items: center;
    gap: 4px;
    height: 26px;
    padding-right: 8px;
    border-radius: 5px;
    margin: 0 6px;
    color: var(--text-dim);
    white-space: nowrap;
  }
  .row:hover {
    background: var(--hover);
    color: var(--text);
  }
  .row.current {
    background: var(--accent-soft);
    color: var(--text);
  }
  .row.drop {
    background: var(--accent-soft);
    outline: 1px dashed var(--accent);
  }
  .twisty {
    width: 14px;
    height: 14px;
    display: inline-flex;
    align-items: center;
    justify-content: center;
    flex: none;
    color: var(--text-faint);
    transition: transform 0.1s;
  }
  .twisty.open {
    transform: rotate(90deg);
  }
  .ico {
    display: inline-flex;
    flex: none;
    color: #6ea0ff;
  }
  .ico.cloud,
  .ico.volume {
    color: #9aa3b8;
  }
  .label {
    overflow: hidden;
    text-overflow: ellipsis;
  }
  .badge {
    margin-left: auto;
    font-size: 11px;
    color: var(--text-faint);
    border: 1px solid var(--border);
    border-radius: 4px;
    padding: 0 5px;
    line-height: 16px;
  }
</style>
