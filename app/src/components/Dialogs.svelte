<script lang="ts">
  import { open as openDialog } from "@tauri-apps/plugin-dialog";
  import { store } from "../lib/store.svelte";
  import { api, baseName } from "../lib/api";
  import ConvertValueDialog from "./ConvertValueDialog.svelte";
  import WatermarkDialog from "./WatermarkDialog.svelte";
  import BatchRenameDialog from "./BatchRenameDialog.svelte";

  let value = $state("");
  let input = $state<HTMLInputElement>();
  let busy = $state(false);

  // 대화상자가 열리면 입력칸을 채우고, 이름 바꾸기는 확장자를 뺀 부분만 선택한다
  $effect(() => {
    const d = store.dialog;
    if (!d) return;
    value = d.type === "rename" ? d.name : d.type === "newFolder" ? "새 폴더" : "";
    queueMicrotask(() => {
      if (!input) return;
      input.focus();
      const dot = value.lastIndexOf(".");
      input.setSelectionRange(0, d.type === "rename" && dot > 0 ? dot : value.length);
    });
  });

  async function submit() {
    const d = store.dialog;
    if (!d || busy) return;
    busy = true;
    try {
      if (d.type === "rename") {
        if (value && value !== d.name) {
          const to = await api.rename(d.path, value);
          await store.open(store.path, { push: false, selectPath: to, keepScroll: true });
          store.showToast("이름을 바꿨습니다", { undo: true });
        }
      } else if (d.type === "newFolder") {
        const p = await api.createFolder(store.path, value || "새 폴더");
        await store.open(store.path, { push: false, selectPath: p });
      }
      store.dialog = null;
    } catch (e) {
      store.showToast(String(e), { error: true });
    } finally {
      busy = false;
    }
  }

  async function pick(n: number) {
    const p = await openDialog({ directory: true, title: `${n}번 빠른 이동 폴더` });
    if (typeof p === "string") await store.setQuickFolder(n, p);
  }

  // 맥 한글 입력기는 입력칸에 포커스가 있을 때 첫 클릭의 "누름"을 가져가서,
  // 버튼에는 "뗌"만 도착하고 클릭이 일어나지 않는다. 대화상자 안에서 누름 없이 뗌만 오면 클릭으로 처리한다.
  let pressed = false;
  function onDownCapture() {
    pressed = true;
  }
  function onUpCapture(e: PointerEvent) {
    if (!pressed && e.button === 0) {
      const b = (e.target as HTMLElement).closest("button");
      if (b && !b.disabled) b.click();
    }
    pressed = false;
  }

  function onKey(e: KeyboardEvent) {
    e.stopPropagation();
    if (e.key === "Escape") store.dialog = null;
    if (e.key === "Enter" && (store.dialog?.type === "rename" || store.dialog?.type === "newFolder")) submit();
  }
</script>

{#if store.dialog}
  <div class="backdrop" onpointerdown={(e) => e.target === e.currentTarget && (store.dialog = null)} role="presentation">
    <div
      class="dialog"
      class:wide={store.dialog.type === "watermark" || store.dialog.type === "batchRename"}
      role="dialog"
      aria-modal="true"
      tabindex="-1"
      onkeydown={onKey}
      onpointerdowncapture={onDownCapture}
      onpointerupcapture={onUpCapture}
    >
      {#if store.dialog.type === "rename"}
        <h3>이름 바꾸기</h3>
        <input type="text" bind:this={input} bind:value spellcheck="false" />
        <div class="actions">
          <button class="btn" onclick={() => (store.dialog = null)}>취소</button>
          <button class="btn primary" onclick={submit} disabled={busy}>바꾸기</button>
        </div>
      {:else if store.dialog.type === "newFolder"}
        <h3>새 폴더</h3>
        <input type="text" bind:this={input} bind:value spellcheck="false" />
        <div class="actions">
          <button class="btn" onclick={() => (store.dialog = null)}>취소</button>
          <button class="btn primary" onclick={submit} disabled={busy}>만들기</button>
        </div>
      {:else if store.dialog.type === "convertValue"}
        <ConvertValueDialog kind={store.dialog.kind} paths={store.dialog.paths} />
      {:else if store.dialog.type === "watermark"}
        <WatermarkDialog paths={store.dialog.paths} />
      {:else if store.dialog.type === "batchRename"}
        <BatchRenameDialog paths={store.dialog.paths} />
      {:else if store.dialog.type === "quickFolders"}
        <h3>빠른 이동 폴더</h3>
        <p class="desc">사진을 보거나 고른 상태에서 숫자키를 누르면 그 폴더로 바로 옮깁니다. Shift+숫자는 복사합니다.</p>
        <div class="quick">
          {#each store.settings.quick_folders as q, i (i)}
            <div class="row">
              <span class="n">{i + 1}</span>
              <span class="path" title={q ?? ""}>{q ? baseName(q) : "지정 안 됨"}</span>
              <span class="full">{q ?? ""}</span>
              <button class="btn" onclick={() => pick(i + 1)}>선택…</button>
              <button class="btn" disabled={!q} onclick={() => store.setQuickFolder(i + 1, null)}>지우기</button>
            </div>
          {/each}
        </div>
        <div class="actions">
          <button class="btn primary" bind:this={input as never} onclick={() => (store.dialog = null)}>완료</button>
        </div>
      {/if}
    </div>
  </div>
{/if}

<style>
  .backdrop {
    position: fixed;
    inset: 0;
    z-index: 100;
    background: rgba(0, 0, 0, 0.45);
    display: flex;
    align-items: flex-start;
    justify-content: center;
    padding-top: 14vh;
  }
  .dialog {
    width: min(520px, calc(100vw - 32px));
    padding: 18px;
    border-radius: 10px;
    background: var(--panel-2);
    border: 1px solid var(--border);
    box-shadow: 0 20px 60px rgba(0, 0, 0, 0.5);
    outline: none;
  }
  .dialog.wide {
    width: min(640px, calc(100vw - 32px));
  }
  .dialog :global(h3) {
    margin: 0 0 12px;
    font-size: 14px;
  }
  .dialog :global(.desc) {
    margin: -4px 0 12px;
    color: var(--text-dim);
    font-size: 12px;
    line-height: 1.5;
  }
  input {
    width: 100%;
    padding: 7px 10px;
  }
  .dialog :global(.actions) {
    display: flex;
    justify-content: flex-end;
    gap: 8px;
    margin-top: 16px;
  }
  .quick {
    display: flex;
    flex-direction: column;
    gap: 6px;
  }
  .row {
    display: grid;
    grid-template-columns: 22px 140px 1fr auto auto;
    align-items: center;
    gap: 8px;
  }
  .n {
    text-align: center;
    font-weight: 600;
    color: var(--accent);
  }
  .path {
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  .full {
    font-size: 11px;
    color: var(--text-faint);
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
    direction: rtl;
    text-align: left;
  }
  .row .btn {
    padding: 3px 9px;
    font-size: 12px;
  }
</style>
