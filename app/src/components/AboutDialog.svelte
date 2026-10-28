<script lang="ts">
  // iSee 정보: 만든 사람, 홈페이지, 면책 조항, 오류 기록.
  import { onMount } from "svelte";
  import { api, isMac, type AppInfo } from "../lib/api";
  import icon from "../assets/icon.png";

  let { focus, onclose }: { focus: "about" | "disclaimer"; onclose: () => void } = $props();
  let info = $state<AppInfo | null>(null);
  let terms = $state<HTMLElement>();

  onMount(() => {
    api.appInfo().then((i) => (info = i)).catch(() => undefined);
    if (focus === "disclaimer") terms?.scrollIntoView({ block: "start" });
  });

  const undoKey = isMac ? "⌘Z" : "Ctrl+Z";
</script>

<div class="backdrop" role="presentation" onclick={onclose}></div>
<div class="dialog" role="dialog" aria-label="iSee 정보">
  <div class="body">
    <div class="title">
      <img src={icon} alt="" />
      <div>
        <h2>iSee</h2>
        <span class="dim">빠른 사진 보기·정리 프로그램{info ? ` · 버전 ${info.version}` : ""}</span>
      </div>
    </div>
    <dl>
      <dt>만든 사람</dt>
      <dd>다있쌤 로디</dd>
      <dt>홈페이지</dt>
      <dd><button class="link" onclick={() => api.openHomepage()}>hwansang.kr</button></dd>
    </dl>

    <h3 bind:this={terms}>면책 조항</h3>
    <ol>
      <li>iSee는 다있쌤 로디가 개인적으로 직접 개발한 무료 프로그램입니다. 꾸준히 시험하고 고치고 있지만, 예상하지 못한 오류가 있을 수 있습니다.</li>
      <li>이 프로그램은 "있는 그대로" 제공되며, 오류가 없다거나 특정 목적에 맞는다는 등 어떠한 명시적·묵시적 보증도 하지 않습니다.</li>
      <li>사진 이동·삭제(휴지통)·이름 바꾸기·변환·편집·폴더 정리 등 이 프로그램을 사용하면서 생긴 데이터 손실이나 그 밖의 손해에 대해, 법이 허용하는 범위에서 제작자는 책임을 지지 않습니다.</li>
      <li>중요한 사진은 작업 전에 꼭 따로 백업해 두세요. 대부분의 작업은 {undoKey}로 되돌릴 수 있지만 모든 경우를 보장하지는 않습니다.</li>
      <li>비슷한 사진·인물(얼굴) 분석 결과는 틀릴 수 있습니다. 정리하거나 지우기 전에 직접 확인해 주세요.</li>
      <li>사진과 얼굴 데이터는 이 컴퓨터에만 저장되며 인터넷으로 보내지 않습니다. 인터넷은 업데이트 확인과, 설치를 누른 경우 얼굴 인식 모델 내려받기에만 씁니다.</li>
      <li>이 프로그램을 설치하거나 사용하면 위 내용에 동의한 것으로 봅니다.</li>
    </ol>

    {#if info?.error_log}
      <p class="log">
        프로그램 오류 기록이 있습니다. 문제가 생기면 이 파일을 함께 보내 주시면 고치는 데 도움이 됩니다.
        <button class="link" onclick={() => api.reveal(info!.error_log!)}>오류 기록 보기</button>
      </p>
    {/if}

    <p class="credits">
      얼굴 인식 모델 YuNet(MIT)·SFace(Apache-2.0, OpenCV Zoo) · 지명 자료 GeoNames(CC BY 4.0)·admdongkor(통계청 행정동 경계)
    </p>
  </div>
  <div class="foot">
    <button class="btn primary" onclick={onclose}>닫기</button>
  </div>
</div>

<style>
  .backdrop {
    position: fixed;
    inset: 0;
    z-index: 90;
    background: rgba(0, 0, 0, 0.45);
  }
  .dialog {
    position: fixed;
    left: 50%;
    top: 50%;
    transform: translate(-50%, -50%);
    z-index: 91;
    width: min(520px, calc(100vw - 32px));
    max-height: calc(100vh - 48px);
    display: flex;
    flex-direction: column;
    border: 1px solid var(--border);
    border-radius: 12px;
    background: var(--panel);
    box-shadow: 0 16px 48px rgba(0, 0, 0, 0.5);
    font-size: 13px;
  }
  .body {
    overflow-y: auto;
    padding: 20px 22px 8px;
  }
  .title {
    display: flex;
    align-items: center;
    gap: 14px;
  }
  .title img {
    width: 56px;
    height: 56px;
  }
  h2 {
    margin: 0 0 2px;
    font-size: 20px;
  }
  .dim {
    color: var(--text-dim);
    font-size: 12px;
  }
  dl {
    display: grid;
    grid-template-columns: auto 1fr;
    gap: 6px 16px;
    margin: 16px 0;
  }
  dt {
    color: var(--text-dim);
  }
  dd {
    margin: 0;
    font-weight: 600;
  }
  .link {
    padding: 0;
    border: 0;
    background: none;
    color: var(--accent);
    font: inherit;
    cursor: pointer;
  }
  .link:hover {
    text-decoration: underline;
  }
  h3 {
    margin: 4px 0 6px;
    padding-top: 12px;
    border-top: 1px solid var(--border);
    font-size: 14px;
  }
  ol {
    margin: 0;
    padding-left: 18px;
    line-height: 1.6;
    color: var(--text-dim);
  }
  li + li {
    margin-top: 4px;
  }
  .log {
    margin: 12px 0 0;
    padding: 8px 10px;
    border-radius: 6px;
    background: var(--panel-2);
    line-height: 1.5;
  }
  .credits {
    margin: 12px 0 0;
    font-size: 11px;
    color: var(--text-faint);
    line-height: 1.5;
  }
  .foot {
    display: flex;
    justify-content: flex-end;
    padding: 12px 22px 16px;
  }
</style>
