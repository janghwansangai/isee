<script lang="ts">
  // 사용 설명서 (도움말 › 사용 설명서, F1): 어떤 기능이 있고 어떻게 쓰는지. 왼쪽 차례, 위쪽 찾기.
  import { isMac } from "../lib/api";

  let { onclose }: { onclose: () => void } = $props();

  const M = isMac ? "⌘" : "Ctrl+";
  const S = isMac ? "⇧" : "Shift+";
  const OPT = isMac ? "⌥" : "Alt+";
  const SHELL = isMac ? "Finder 우클릭(빠른 동작)에 iSee 변환 넣기" : "탐색기 우클릭 메뉴에 iSee 변환 넣기";

  /** 문단: **굵게** 만 쓴다 */
  type Section = { id: string; title: string; intro?: string; items: string[]; tip?: string };

  const sections: Section[] = [
    {
      id: "start",
      title: "처음 시작하기",
      intro: "iSee는 사진을 아주 빠르게 보고, 고르고, 정리하는 프로그램입니다.",
      items: [
        `**폴더 열기**: 왼쪽 목록(홈·사진·다운로드·클라우드 등)에서 폴더를 누르거나, 메뉴 파일 › 폴더 열기(${M}O). 위쪽 경로를 눌러 상위 폴더로 갈 수 있습니다.`,
        `**보기 방식**: 오른쪽 위 단추로 썸네일(${M}1) 또는 목록(${M}2, 파일 이름·날짜·크기). 목록에서는 머리글을 눌러 정렬합니다.`,
        "**썸네일 크기**: 오른쪽 위 막대를 끌어 크게·작게.",
        "**정렬**: 이름·날짜·크기·종류·별점 (툴바의 정렬, 또는 메뉴 보기 › 정렬).",
        "**파일 종류**: 처음에는 사진만 보입니다. 동영상·PDF·문서도 보려면 메뉴 보기 › 파일 종류.",
        "**정보 패널**: 오른쪽에 크기·촬영 날짜·카메라·위치가 나옵니다 (툴바 ⓘ 단추로 켜기·끄기).",
      ],
    },
    {
      id: "viewer",
      title: "사진 크게 보기",
      items: [
        "사진을 **두 번 클릭**하거나 Enter·Space를 누르면 크게 봅니다. Esc·Enter로 닫습니다.",
        "**←·→** 또는 마우스 휠로 이전·다음 사진. 넘기기가 아주 빠르도록 앞뒤 사진을 미리 준비해 둡니다.",
        `**확대**: 핀치 또는 Ctrl+휠(커서 위치로), +·− 키. 0은 화면에 맞춤, ${M}1은 실제 크기. F는 전체 화면.`,
        "**R / Shift+R**: 시계·반시계 방향 회전 (JPEG·HEIC는 화질 손실 없이).",
        "**Delete**: 휴지통으로 보내고 다음 사진으로.",
      ],
    },
    {
      id: "files",
      title: "고르기·옮기기·되돌리기",
      items: [
        `**여러 장 고르기**: 클릭, Shift+클릭(범위), ${M}클릭(하나씩 더하기), 썸네일 왼쪽 위 체크 칸. ${M}A는 모두.`,
        `**옮기기·복사**: 고른 사진을 왼쪽 폴더나 위쪽 경로로 끌어 놓으면 옮깁니다 (${isMac ? "⌥ Option" : "Alt"}을 누른 채 놓으면 복사).`,
        "**빠른 이동 폴더**: 숫자키 1~9에 자주 쓰는 폴더를 지정해 두면 키 하나로 옮깁니다 (Shift+숫자는 복사). 메뉴 정리 › 빠른 이동 폴더 설정.",
        "**이름 바꾸기**: F2. 여러 장은 우클릭 › 이름 일괄 변경.",
        "**휴지통**: Delete. 영구 삭제는 하지 않습니다.",
        `**되돌리기**: ${M}Z. 옮기기·복사·휴지통·회전·변환·정리를 한 단계씩 되돌립니다.`,
      ],
    },
    {
      id: "cull",
      title: "빠른 고르기 (남김·버림·별점)",
      intro: "많은 사진 가운데 남길 것을 빠르게 고를 때 씁니다. 표시는 파일을 건드리지 않고 앱에 저장합니다.",
      items: [
        "툴바 **빠른 고르기**를 켭니다.",
        "사진마다 **P** 남김, **X** 버림, **U** 표시 지우기, **0~5** 별점. 누르면 다음 사진으로 넘어갑니다.",
        "툴바에서 **남김만·미정만·버림만·별점 있는 것**만 골라 볼 수 있습니다.",
        "다 고른 뒤 **버린 N장 휴지통**을 누르면 한 번에 정리합니다.",
      ],
    },
    {
      id: "compare",
      title: "비교하기",
      items: [
        "사진을 2~9장 고르고 **C** (또는 툴바의 비교). 확대·이동하면 모든 칸이 함께 움직입니다.",
        "남길 사진을 클릭(또는 숫자키)으로 표시하고 **Delete**를 누르면 나머지가 휴지통으로 갑니다.",
        "2장이면 **O**(겹쳐 보기)·**Space**(깜빡이며 비교)로 작은 차이도 찾을 수 있습니다.",
      ],
    },
    {
      id: "similar",
      title: "비슷한 사진 정리",
      intro: "연속 촬영처럼 비슷한 사진을 묶고, 묶음마다 가장 잘 나온 사진에 ★ 추천을 붙입니다.",
      items: [
        `툴바 **비슷한 사진** (${M}${S}S). 처음 열 때 사진을 분석하고, 한 번 분석한 사진은 저장해 둡니다.`,
        "**묶는 정도**, **추천 기준**(선명함·노출·노이즈·해상도·최신), **남길 장수**를 바꾸면 바로 다시 묶습니다.",
        "사진을 눌러 남김·정리를 바꾸고, **나머지 N장 휴지통** 또는 **추천 외 N장 휴지통**으로 정리합니다.",
        "탭에서 **똑같은 파일**(사본), **흐림**, **너무 어두움·밝음** 사진만 모아 볼 수 있습니다.",
      ],
      tip: "사진이 많으면 메뉴 정리 › 분석 미리 해 두기로 먼저 분석해 두세요. 다른 일을 하는 동안 끝납니다.",
    },
    {
      id: "people",
      title: "인물 (얼굴별 분류)",
      intro: "사진 속 얼굴을 찾아 같은 사람끼리 묶습니다. 이름을 붙이면 다음부터 그 사람을 알아봅니다. 얼굴 데이터는 이 컴퓨터에만 저장됩니다.",
      items: [
        `툴바 **인물** (${M}${S}P). 처음 한 번 **설치**를 누르면 얼굴 인식 모델(약 37MB)을 내려받습니다.`,
        "**분석 시작**을 누르면 얼굴을 찾습니다. 화면을 닫아도 계속하고, 사진을 크게 보는 동안에는 잠시 멈춥니다.",
        "왼쪽 목록의 **이름 없는 인물 N**은 같은 사람으로 묶였지만 아직 이름이 없는 묶음입니다. 이름을 쓰고 **이름 붙이기**.",
        "얼굴을 클릭해 고른 뒤 이름을 붙이면 **고른 얼굴만** 그 이름이 됩니다. 같은 이름을 쓰면 그 사람과 합쳐집니다.",
        "이름 붙은 사람에게 자동으로 들어온 얼굴은 **노란 테두리**입니다. 맞으면 **모두 맞음**, 틀리면 골라서 **이 사람 아님**.",
        "이름 없는 묶음에 다른 사람이 섞였으면 골라서 **빼기**. 다시 같은 사람으로 묶지 않습니다.",
        "목록 맨 아래 **미분류**에는 같은 사람을 찾지 못했거나 옆모습이라 뺀 얼굴이 모여 있습니다. 골라서 이름을 붙이거나 다른 인물로 보냅니다.",
        "**정확도**: 낮음(많이 묶기) · 보통 · 높음(확실한 것만). 다른 학생이 같은 사람으로 섞이면 **높음**으로 바꾸세요. 대신 미분류가 늘어납니다. 다시 분석하지 않고 바로 다시 묶습니다.",
        "**옆모습**: 모두 묶기 · 많이 돌린 얼굴 빼기 · 정면에 가까운 얼굴만. 단체 사진의 옆모습을 분류에서 빼고 싶을 때 씁니다. 직접 이름을 붙인 얼굴은 그대로 둡니다.",
        "0.1.6 전에 분석한 사진은 얼굴 방향을 몰라 옆모습을 가리지 못합니다. 화면 위 **방향 다시 분석**을 누르면 됩니다 (붙인 이름은 그대로).",
        `**인물별 폴더로 복사**(여러 사람이 나온 사진은 사람마다) 또는 **주인공 기준 이동**. ${M}Z로 되돌립니다.`,
      ],
      tip: "정확도를 높이는 가장 좋은 방법은 사람마다 정면 사진 몇 장에 이름을 붙이는 것입니다. 쓸수록 정확해집니다.",
    },
    {
      id: "organize",
      title: "날짜·장소별 정리",
      items: [
        `메뉴 정리 › 날짜·장소별 정리 (${M}${S}D). 사진을 2장 이상 골랐으면 그 사진만, 아니면 폴더 전체를 정리합니다.`,
        "**기준**: 날짜 · 장소 · 날짜+장소 · 이벤트(촬영 간격으로 여행·행사를 나눔). 폴더 이름 규칙은 예시에서 고르거나 직접 씁니다.",
        "장소는 사진의 위치 정보로 찾습니다 (예: 제주 서귀포시). 위치가 인터넷으로 나가지 않습니다.",
        "미리보기에서 폴더 이름을 고치고 **이동** 또는 **복사**. 전체를 되돌리기 한 번으로 원래대로 돌립니다.",
      ],
    },
    {
      id: "edit",
      title: "사진 편집",
      items: [
        `사진을 고르고 **E** (${M}E). 자르기, 회전·반전, 밝기·대비·채도, 펜·형광펜·선·화살표·사각형·원·글자, 모자이크·흐리게.`,
        "그린 것은 저장하기 전까지 옮기고, 색을 바꾸고, 지울 수 있습니다.",
        `**새 파일로 저장**(${M}S) 또는 **원본에 저장**(원본은 보관되어 되돌릴 수 있음). 촬영 날짜·위치 정보는 그대로 남습니다.`,
      ],
    },
    {
      id: "convert",
      title: "우클릭 빠른 변환",
      items: [
        "사진을 우클릭하면 **회전·크기·형식·용량·화질·공유용 저장·워터마크·이름 일괄 변경**을 바로 할 수 있습니다. 여러 장도 한 번에.",
        "**저장 방식**: 새 파일로 · 원본에 덮어쓰기(되돌리기 가능) · 지정한 폴더로.",
        `${isMac ? "Finder" : "탐색기"}에서 바로 쓰려면 메뉴 정리 › ${SHELL}.`,
      ],
    },
    {
      id: "scope",
      title: "보는 범위: 하위 폴더 · 다른 폴더 함께",
      items: [
        "경로 옆 **이 폴더만 ▾** 단추에서 고릅니다. 사진이 많아질 수 있어 둘 다 처음에는 꺼져 있습니다.",
        "**하위 폴더 사진도 보기**: 이 폴더 안 모든 폴더의 사진을 한 화면에 모읍니다.",
        "**다른 폴더 더하기**: 다른 곳의 폴더를 더해 함께 고르고, 비슷한 사진·인물·날짜 정리를 한 번에 합니다.",
        "한 번에 파일 50,000개까지 모읍니다. 하위 폴더 때문에 넘으면 알리고 하위 폴더 보기를 자동으로 끕니다.",
      ],
    },
    {
      id: "background",
      title: "백그라운드 작업 · 분석 미리 해 두기",
      items: [
        "아래쪽 **백그라운드 작업**을 누르면 썸네일 미리 만들기·비슷한 사진·얼굴 분석이 얼마나 됐는지 보입니다.",
        "사진을 빨리 골라야 할 때는 **모두 잠시 멈춤**을 누르면 보기에만 컴퓨터 성능을 씁니다.",
        "**분석 미리 해 두기**(메뉴 정리)는 화면을 열지 않고 비슷한 사진·얼굴 분석을 뒤에서 해 둡니다. 끝나면 알려 줍니다.",
      ],
    },
    {
      id: "cloud",
      title: "동영상 · PDF · 클라우드 폴더",
      items: [
        "보기 › 파일 종류에서 동영상·PDF를 켜면 함께 보입니다. 동영상은 앱 안에서 재생합니다.",
        "구글 드라이브·iCloud·OneDrive 폴더에서 아직 내려받지 않은 사진은 ☁ 표시가 붙고, 썸네일은 원본을 내려받지 않고 보여 줍니다.",
        "클라우드 폴더의 사진을 많이 분석하면 한 장씩 내려받느라 오래 걸립니다. 드라이브 앱에서 그 폴더를 **오프라인 사용(항상 이 기기에 보관)**으로 바꾸거나, 컴퓨터로 복사한 뒤 정리하고 다시 올리면 빠르고 안정적입니다.",
      ],
    },
    {
      id: "trouble",
      title: "문제가 생기면",
      items: [
        "**미리보기가 안 나와요**: 클라우드 폴더라면 드라이브 앱이 그 계정에 정상 연결돼 있는지 확인하세요.",
        "**느려요**: 백그라운드 작업 창에서 모두 잠시 멈춤을 누르거나, 하위 폴더 보기를 끄세요.",
        "**오류 신고**: 도움말 › **오류 신고**를 누르면 브라우저에 신고 페이지가 열려요. 오류나 고쳤으면 하는 점을 적어 보내 주세요.",
        "**프로그램이 꺼졌어요**: 도움말 › iSee 정보의 **오류 기록 보기**로 찾은 파일 내용을 신고할 때 함께 붙여 주시면 고치는 데 도움이 됩니다.",
        `**새 버전**: 실행하면 하루 한 번 알려 줍니다. 직접 확인은 ${isMac ? "iSee" : "도움말"} › 업데이트 확인.`,
        "개인이 만든 프로그램이라 오류가 있을 수 있습니다. 중요한 사진은 미리 백업해 두세요 (도움말 › 면책 조항).",
      ],
    },
  ];

  const keys: [string, string, string][] = [
    ["Enter / Space / 두 번 클릭", "크게 보기", "닫기 / 다음 사진"],
    ["← → ↑ ↓", "칸 이동 (Shift: 범위 고르기)", "이전·다음 사진"],
    ["Delete", "휴지통으로", "휴지통으로, 다음 사진"],
    ["1~9", "빠른 이동 폴더로 옮기기 (Shift: 복사)", "같음"],
    [`${M}Z`, "되돌리기", "되돌리기"],
    ["F2", "이름 바꾸기", ""],
    ["R / Shift+R", "시계 / 반시계 방향 회전", "같음"],
    ["P / X / U", "남김 / 버림 / 표시 지우기", "같음"],
    ["0~5 (빠른 고르기)", "별점", "같음"],
    ["C", "고른 사진 비교", ""],
    ["E", "사진 편집", "같음"],
    [`${M}${S}S`, "비슷한 사진 정리", ""],
    [`${M}${S}D`, "날짜·장소별 정리", ""],
    [`${M}${S}P`, "인물", ""],
    [`${M}1 / ${M}2`, "썸네일 / 목록으로 보기", `${M}1: 실제 크기`],
    [isMac ? "⌘↑" : `${OPT}↑`, "상위 폴더", ""],
    ["+ / − / 0", "", "확대 / 축소 / 화면에 맞춤"],
    ["F", "", "전체 화면"],
    ["F1", "사용 설명서", "사용 설명서"],
  ];

  let query = $state("");
  let body = $state<HTMLElement>();
  let shown = $derived.by(() => {
    const q = query.trim();
    if (!q) return sections;
    return sections
      .map((s) => ({ ...s, items: s.items.filter((t) => t.includes(q)) }))
      .filter((s) => s.title.includes(q) || s.items.length || s.intro?.includes(q) || s.tip?.includes(q));
  });
  let keysShown = $derived(!query.trim() || keys.some((k) => k.join(" ").includes(query.trim())) || "단축키".includes(query.trim()));

  const esc = (t: string) => t.replace(/&/g, "&amp;").replace(/</g, "&lt;").replace(/>/g, "&gt;");
  const rich = (t: string) => esc(t).replace(/\*\*(.+?)\*\*/g, "<b>$1</b>");

  function go(id: string) {
    body?.querySelector(`#help-${id}`)?.scrollIntoView({ block: "start" });
  }
</script>

<div class="backdrop" role="presentation" onclick={onclose}></div>
<div class="dialog" role="dialog" aria-label="사용 설명서">
  <div class="top">
    <b>iSee 사용 설명서</b>
    <input class="find" type="search" placeholder="찾기 (예: 휴지통, 인물, 클라우드)" bind:value={query} />
    <span class="spacer"></span>
    <button class="btn" onclick={onclose}>닫기</button>
  </div>
  <div class="cols">
    <nav class="toc">
      {#each shown as s (s.id)}
        <button onclick={() => go(s.id)}>{s.title}</button>
      {/each}
      {#if keysShown}<button onclick={() => go("keys")}>단축키</button>{/if}
    </nav>
    <div class="body" bind:this={body}>
      {#each shown as s (s.id)}
        <section id="help-{s.id}">
          <h3>{s.title}</h3>
          {#if s.intro}<p class="intro">{s.intro}</p>{/if}
          <ul>
            {#each s.items as t, i (i)}
              <!-- eslint-disable-next-line svelte/no-at-html-tags -- 앱 안의 고정 문장, **굵게**만 바꾼다 -->
              <li>{@html rich(t)}</li>
            {/each}
          </ul>
          {#if s.tip}<p class="tip">도움말: {s.tip}</p>{/if}
        </section>
      {/each}
      {#if keysShown}
        <section id="help-keys">
          <h3>단축키</h3>
          <table>
            <thead><tr><th>키</th><th>사진 목록</th><th>크게 보기</th></tr></thead>
            <tbody>
              {#each keys as [k, a, b] (k)}
                <tr><td class="k">{k}</td><td>{a}</td><td>{b}</td></tr>
              {/each}
            </tbody>
          </table>
        </section>
      {/if}
      {#if !shown.length && !keysShown}
        <p class="intro">"{query}"에 맞는 설명이 없습니다.</p>
      {/if}
    </div>
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
    width: min(920px, calc(100vw - 32px));
    height: min(680px, calc(100vh - 48px));
    display: flex;
    flex-direction: column;
    border: 1px solid var(--border);
    border-radius: 12px;
    background: var(--panel);
    box-shadow: 0 16px 48px rgba(0, 0, 0, 0.5);
    font-size: 13px;
  }
  .top {
    display: flex;
    align-items: center;
    gap: 12px;
    padding: 12px 16px;
    border-bottom: 1px solid var(--border);
  }
  .top b {
    font-size: 15px;
  }
  .find {
    width: 260px;
    padding: 5px 8px;
    border-radius: 6px;
    border: 1px solid var(--border);
    background: var(--panel-2);
    color: var(--text);
  }
  .spacer {
    flex: 1;
  }
  .cols {
    flex: 1;
    display: flex;
    min-height: 0;
  }
  .toc {
    width: 210px;
    flex: none;
    overflow-y: auto;
    padding: 8px;
    border-right: 1px solid var(--border);
  }
  .toc button {
    display: block;
    width: 100%;
    padding: 6px 8px;
    border: 0;
    border-radius: 6px;
    background: none;
    color: var(--text-dim);
    text-align: left;
    font: inherit;
    cursor: pointer;
  }
  .toc button:hover {
    background: var(--hover);
    color: var(--text);
  }
  .body {
    flex: 1;
    overflow-y: auto;
    padding: 4px 22px 24px;
  }
  section {
    padding-top: 14px;
  }
  h3 {
    margin: 0 0 6px;
    font-size: 15px;
  }
  .intro {
    margin: 0 0 6px;
    color: var(--text-dim);
    line-height: 1.6;
  }
  ul {
    margin: 0;
    padding-left: 18px;
    line-height: 1.7;
  }
  li :global(b) {
    color: var(--text);
  }
  li {
    color: var(--text-dim);
  }
  .tip {
    margin: 8px 0 0;
    padding: 8px 10px;
    border-radius: 6px;
    background: var(--accent-soft);
    line-height: 1.5;
  }
  table {
    width: 100%;
    border-collapse: collapse;
  }
  th,
  td {
    padding: 5px 8px;
    border-bottom: 1px solid var(--border);
    text-align: left;
  }
  th {
    color: var(--text-dim);
    font-weight: 600;
  }
  td {
    color: var(--text-dim);
  }
  .k {
    color: var(--text);
    white-space: nowrap;
    font-variant-numeric: tabular-nums;
  }
</style>
