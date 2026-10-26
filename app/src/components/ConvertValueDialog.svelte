<script lang="ts">
  import { store } from "../lib/store.svelte";
  import { baseName } from "../lib/api";

  let { kind, paths: targets }: { kind: "resize" | "size" | "quality"; paths: string[] } = $props();

  // svelte-ignore state_referenced_locally
  let value = $state(kind === "resize" ? 1600 : kind === "size" ? 1 : 85);
  // svelte-ignore state_referenced_locally
  let unit = $state(kind === "resize" ? "px" : kind === "size" ? "MB" : "");
  let input = $state<HTMLInputElement>();

  $effect(() => {
    input?.focus();
    input?.select();
  });

  const titles = { resize: "크기 직접 입력", size: "목표 용량 직접 입력", quality: "화질 직접 입력" };

  let saveText = $derived.by(() => {
    const s = store.saveSpec();
    if (s.mode === "overwrite") return "원본에 덮어쓰기 (되돌리기 가능)";
    if (s.mode === "folder") return `폴더 '${baseName(s.path)}'에 저장`;
    return "새 파일로 저장 (원본 보존)";
  });

  function submit() {
    const v = Number(value);
    if (!(v > 0)) return;
    // 대화상자를 닫으면 props(kind, paths)도 사라지므로, 변환을 먼저 시작하고 닫는다
    const paths = targets;
    if (kind === "resize") {
      store.runConvert(
        {
          resize:
            unit === "%"
              ? { kind: "percent", value: Math.min(100, Math.round(v)) }
              : { kind: "long_side", value: Math.round(v) },
        },
        "크기 변경",
        paths,
      );
    } else if (kind === "size") {
      store.runConvert({ max_bytes: Math.round(v * (unit === "MB" ? 1_000_000 : 1000)) }, "용량 줄이기", paths);
    } else {
      const q = Math.min(100, Math.max(1, Math.round(v)));
      store.runConvert({ quality: q }, `화질 ${q}`, paths);
    }
    store.dialog = null;
  }
</script>

<h3>{titles[kind]}</h3>
<p class="desc">사진 {targets.length}장 · {saveText}</p>
<div class="row">
  <input
    type="number"
    min="1"
    bind:this={input}
    bind:value
    onkeydown={(e) => e.key === "Enter" && submit()}
  />
  {#if kind === "resize"}
    <select bind:value={unit}>
      <option value="px">px (긴 변)</option>
      <option value="%">%</option>
    </select>
  {:else if kind === "size"}
    <select bind:value={unit}>
      <option value="KB">KB</option>
      <option value="MB">MB</option>
    </select>
    <span class="dim">이하</span>
  {:else}
    <span class="dim">1~100 (높을수록 선명하고 큼)</span>
  {/if}
</div>
<div class="actions">
  <button class="btn" onclick={() => (store.dialog = null)}>취소</button>
  <button class="btn primary" onclick={submit}>적용</button>
</div>

<style>
  .row {
    display: flex;
    align-items: center;
    gap: 8px;
  }
  .row input {
    width: 140px;
  }
  .dim {
    color: var(--text-dim);
    font-size: 12px;
  }
</style>
