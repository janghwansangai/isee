<script lang="ts">
  import { store } from "../lib/store.svelte";
  import { api, baseName, type RenamePlanned } from "../lib/api";

  let { paths }: { paths: string[] } = $props();

  let pattern = $state("{이름}");
  let start = $state(1);
  let digits = $state(3);
  let plan = $state.raw<RenamePlanned[]>([]);
  let busy = $state(false);
  let input = $state<HTMLInputElement>();

  const tokens = [
    { t: "{이름}", d: "원래 이름" },
    { t: "{번호}", d: "순번" },
    { t: "{날짜}", d: "촬영일" },
    { t: "{시간}", d: "촬영 시각" },
    { t: "{폴더}", d: "폴더 이름" },
  ];
  const presets = ["{날짜}_{번호}", "{폴더}_{번호}", "{이름}_{번호}"];

  $effect(() => {
    input?.focus();
  });

  // 규칙이 바뀌면 잠시 뒤 미리보기를 다시 만든다 (촬영일은 엔진이 EXIF에서 읽음)
  let timer = 0;
  $effect(() => {
    const rule = { pattern, start: Number(start) || 1, digits: Number(digits) || 1 };
    clearTimeout(timer);
    timer = window.setTimeout(async () => {
      plan = await api.renamePlan(paths, rule);
    }, 120);
  });

  let duplicates = $derived.by(() => {
    const seen = new Set<string>();
    let d = 0;
    for (const p of plan) {
      const key = `${p.from.slice(0, p.from.length - baseName(p.from).length)}${p.name}`.toLowerCase();
      if (seen.has(key)) d++;
      seen.add(key);
    }
    return d;
  });
  let changed = $derived(plan.filter((p) => baseName(p.from) !== p.name).length);

  function insert(t: string) {
    const el = input;
    if (!el) {
      pattern += t;
      return;
    }
    const s = el.selectionStart ?? pattern.length;
    const e = el.selectionEnd ?? s;
    pattern = pattern.slice(0, s) + t + pattern.slice(e);
    queueMicrotask(() => {
      el.focus();
      el.setSelectionRange(s + t.length, s + t.length);
    });
  }

  async function apply() {
    if (busy || duplicates || !changed) return;
    busy = true;
    try {
      const n = await api.renameApply(plan);
      store.dialog = null;
      store.showToast(`${n}개 이름을 바꿨습니다`, { undo: n > 0 });
      await store.reload();
    } catch (e) {
      store.showToast(`이름을 바꾸지 못했습니다: ${e}`, { error: true });
    } finally {
      busy = false;
    }
  }
</script>

<h3>이름 일괄 변경</h3>
<p class="desc">{paths.length}개 · 확장자는 그대로 둡니다. 촬영일이 없는 파일은 수정한 날짜를 씁니다.</p>

<label class="pattern">
  규칙
  <input
    type="text"
    bind:this={input}
    bind:value={pattern}
    spellcheck="false"
    onkeydown={(e) => e.key === "Enter" && apply()}
  />
</label>
<div class="chips">
  {#each tokens as t (t.t)}
    <button class="chip" onclick={() => insert(t.t)} title={t.d}>{t.t}</button>
  {/each}
  <span class="sep"></span>
  {#each presets as p (p)}
    <button class="chip preset" onclick={() => (pattern = p)}>{p}</button>
  {/each}
</div>
<div class="nums">
  <label>시작 번호 <input type="number" min="0" bind:value={start} /></label>
  <label>자릿수 <input type="number" min="1" max="8" bind:value={digits} /></label>
</div>

<div class="list">
  {#each plan.slice(0, 12) as p (p.from)}
    <div class="item">
      <span class="from">{baseName(p.from)}</span>
      <span class="arrow">→</span>
      <span class="to" class:same={baseName(p.from) === p.name}>{p.name}</span>
    </div>
  {/each}
  {#if plan.length > 12}<div class="more">외 {plan.length - 12}개</div>{/if}
</div>
{#if duplicates}
  <p class="warn">새 이름이 {duplicates}개 겹칩니다. {"{번호}"}를 넣어 주세요.</p>
{/if}

<div class="actions">
  <button class="btn" onclick={() => (store.dialog = null)}>취소</button>
  <button class="btn primary" onclick={apply} disabled={busy || duplicates > 0 || !changed}>
    {changed}개 바꾸기
  </button>
</div>

<style>
  .pattern {
    display: flex;
    flex-direction: column;
    gap: 4px;
    font-size: 12px;
    color: var(--text-dim);
  }
  .pattern input {
    font-size: 14px;
    padding: 7px 10px;
  }
  .chips {
    display: flex;
    flex-wrap: wrap;
    gap: 6px;
    margin: 8px 0;
  }
  .chip {
    padding: 2px 8px;
    border-radius: 10px;
    border: 1px solid var(--border);
    font-size: 12px;
    color: var(--accent);
  }
  .chip.preset {
    color: var(--text-dim);
  }
  .sep {
    width: 8px;
  }
  .nums {
    display: flex;
    gap: 14px;
    font-size: 12px;
    color: var(--text-dim);
  }
  .nums input {
    width: 70px;
    margin-left: 4px;
  }
  .list {
    margin-top: 10px;
    max-height: 220px;
    overflow-y: auto;
    border: 1px solid var(--border);
    border-radius: 6px;
    padding: 6px 10px;
    font-size: 12px;
  }
  .item {
    display: grid;
    grid-template-columns: 1fr 16px 1fr;
    gap: 6px;
    padding: 2px 0;
  }
  .from {
    color: var(--text-faint);
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  .to {
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  .to.same {
    color: var(--text-faint);
  }
  .arrow,
  .more {
    color: var(--text-faint);
  }
  .warn {
    color: var(--danger);
    font-size: 12px;
    margin: 8px 0 0;
  }
</style>
