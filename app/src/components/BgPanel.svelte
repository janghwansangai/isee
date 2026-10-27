<script lang="ts">
  // 백그라운드 작업 현황과 조절: 썸네일 미리 만들기(켜기·끄기), 비슷한 사진·얼굴 분석(중단), 모두 잠시 멈춤.
  // 빨리 골라야 할 때는 모두 멈추면 CPU를 보기에만 쓴다.
  import { api, type BgStatus } from "../lib/api";
  import { store } from "../lib/store.svelte";

  let { bg, onclose }: { bg: BgStatus | null; onclose: () => void } = $props();
  let busy = $state(false);

  async function act(action: string) {
    if (busy) return;
    busy = true;
    try {
      await api.bgControl(action);
    } catch (e) {
      store.showToast(`${e}`, { error: true });
    } finally {
      busy = false;
    }
  }

  const running = (p: [number, number]) => p[1] > 0 && p[0] < p[1];
</script>

<div class="backdrop" role="presentation" onclick={onclose}></div>
<div class="panel" role="dialog" aria-label="백그라운드 작업">
  <div class="head">
    <b>백그라운드 작업</b>
    <span class="spacer"></span>
    {#if bg}
      <button class="btn" class:primary={bg.paused} disabled={busy} onclick={() => act(bg!.paused ? "resume" : "pause")}>
        {bg.paused ? "▶ 다시 시작" : "⏸ 모두 잠시 멈춤"}
      </button>
    {/if}
  </div>
  <p class="note">사진을 빨리 골라야 할 때는 멈추면 보기·넘기기에만 컴퓨터 성능을 씁니다. 화면에 보이는 썸네일은 멈춰도 만듭니다.</p>
  {#if bg}
    <div class="row">
      <div class="what">
        <b>썸네일 미리 만들기</b>
        <span class="state">
          {#if !bg.prefill}꺼짐 · 화면에 보이는 것만 만듦
          {:else if bg.paused && bg.thumbs}멈춤 · {bg.thumbs.toLocaleString()}장 남음
          {:else if bg.thumbs}실행 중 · {bg.thumbs.toLocaleString()}장 남음
          {:else}쉬는 중{/if}
        </span>
        <span class="desc">폴더를 열면 아래쪽 사진까지 미리 만들어 스크롤이 빨라집니다.</span>
      </div>
      <label class="switch" title="켜기·끄기 (기억됩니다)">
        <input type="checkbox" checked={bg.prefill} disabled={busy} onchange={(e) => act(e.currentTarget.checked ? "prefill_on" : "prefill_off")} />
        <span>{bg.prefill ? "켜짐" : "꺼짐"}</span>
      </label>
    </div>
    <div class="row">
      <div class="what">
        <b>비슷한 사진 분석</b>
        <span class="state">
          {#if running(bg.similar)}{bg.paused ? "멈춤" : "실행 중"} · {bg.similar[0].toLocaleString()} / {bg.similar[1].toLocaleString()}
          {:else if bg.similar[1]}끝남 · {(bg.similar[1] - bg.similar_cloud).toLocaleString()}장 분석{#if bg.similar_cloud}<span class="cloud">{" · "}클라우드에만 있는 {bg.similar_cloud.toLocaleString()}장은 건너뜀</span>{/if}
          {:else}쉬는 중{/if}
        </span>
        <span class="desc">"비슷한 사진" 화면을 열었을 때만 합니다.</span>
      </div>
      <button class="btn" disabled={busy || !running(bg.similar)} onclick={() => act("stop_similar")}>중단</button>
    </div>
    <div class="row">
      <div class="what">
        <b>얼굴 분석</b>
        <span class="state">
          {#if running(bg.faces)}{bg.paused ? "멈춤" : "실행 중"} · {bg.faces[0].toLocaleString()} / {bg.faces[1].toLocaleString()}
          {:else}쉬는 중{/if}
        </span>
        <span class="desc">"인물" 화면에서 분석을 시작했을 때만 합니다.</span>
      </div>
      <button class="btn" disabled={busy || !running(bg.faces)} onclick={() => act("stop_faces")}>중단</button>
    </div>
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
    right: 12px;
    bottom: 34px;
    z-index: 71;
    width: 380px;
    padding: 12px 14px;
    border: 1px solid var(--border);
    border-radius: 10px;
    background: var(--panel);
    box-shadow: 0 12px 40px rgba(0, 0, 0, 0.45);
    font-size: 13px;
  }
  .head {
    display: flex;
    align-items: center;
    gap: 8px;
  }
  .spacer {
    flex: 1;
  }
  .note {
    margin: 6px 0 8px;
    font-size: 11px;
    color: var(--text-faint);
    line-height: 1.45;
  }
  .row {
    display: flex;
    align-items: center;
    gap: 10px;
    padding: 8px 0;
    border-top: 1px solid var(--border);
  }
  .what {
    flex: 1;
    display: flex;
    flex-direction: column;
    gap: 2px;
  }
  .state {
    font-size: 12px;
    color: var(--accent);
    font-variant-numeric: tabular-nums;
  }
  .cloud {
    color: #9fc4ff;
  }
  .desc {
    font-size: 11px;
    color: var(--text-faint);
  }
  .switch {
    display: flex;
    align-items: center;
    gap: 6px;
    font-size: 12px;
    color: var(--text-dim);
  }
  .btn:disabled {
    opacity: 0.4;
  }
</style>
