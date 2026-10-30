import { SvelteSet } from "svelte/reactivity";
import { ask, message, open as openDialog } from "@tauri-apps/plugin-dialog";
import { getCurrentWindow, UserAttentionType } from "@tauri-apps/api/window";
import {
  api,
  baseName,
  fmtDuration,
  fmtSize,
  scopeLabel,
  type CloudDone,
  type CloudEstimate,
  type CloudProgress,
  type ConvertSpec,
  type Entry,
  type Mark,
  type Place,
  type SaveSpec,
  type Scope,
  type Settings,
} from "./api";

export interface Toast {
  id: number;
  message: string;
  error?: boolean;
  undo?: boolean;
}

export type Dialog =
  | { type: "rename"; path: string; name: string }
  | { type: "newFolder" }
  | { type: "quickFolders" }
  | { type: "convertValue"; kind: "resize" | "size" | "quality"; paths: string[] }
  | { type: "watermark"; paths: string[] }
  | { type: "batchRename"; paths: string[] };

const DEFAULT_SETTINGS: Settings = {
  kinds: { image: true, video: false, pdf: false, doc: false, other: false },
  sort_by: "name",
  sort_desc: false,
  show_hidden: false,
  show_info: true,
  show_hud: false,
  keep_running: true,
  view_mode: "grid",
  include_subfolders: false,
  thumb_size: 168,
  quick_folders: Array(9).fill(null),
  last_folder: null,
  convert_save: "new",
  convert_folder: null,
  culling: false,
  cull_filter: "all",
  similar: { level: 3, keep: 1, rules: ["sharp", "exposure", "noise", "resolution", "newest"] },
  organize: { pattern: "{년}/{년}-{월}", level: "city", guess_place: true, event_hours: 6 },
  organize_copy: false,
  face_accuracy: "normal",
  face_profile: "all",
};

class Store {
  path = $state("");
  parent = $state<string | null>(null);
  /** 폴더 내용. 통째로 바꾸므로 깊은 반응성은 끈다 (수만 개여도 빠르게). */
  entries = $state.raw<Entry[]>([]);
  listMs = $state(0);
  selected = new SvelteSet<number>();
  focus = $state(-1);
  anchor = $state(-1);
  /** 뷰어에 띄운 항목의 entries 위치 (-1이면 닫힘) */
  viewer = $state(-1);
  settings = $state<Settings>(DEFAULT_SETTINGS);
  places = $state.raw<Place[]>([]);
  toast = $state<Toast | null>(null);
  dialog = $state<Dialog | null>(null);
  /** 그리드가 이 항목까지 스크롤해야 할 때 (포커스 이동) */
  scrollTo = $state(-1);
  loading = $state(false);
  error = $state<string | null>(null);
  /** 끌어서 옮기는 중인 항목 수와 커서 위치 */
  dragging = $state<{ count: number; x: number; y: number; copy: boolean } | null>(null);
  /** 끌고 있는 커서 아래의 폴더 */
  dropOver = $state<string | null>(null);
  /** 앱 실행부터 첫 화면까지 (ms) */
  launchMs = $state(0);
  /** 바깥에서 사진을 열라는 요청을 받은 시각 (Unix ms, 표시되면 0으로) */
  openRequestAt = 0;
  /** 마지막 열기 요청 → 사진 표시까지 (ms) */
  lastOpenMs = $state(0);
  /** 남김·버림·별점 (경로 → 표시). 표시가 있는 것만 들어 있다. */
  marks = $state.raw<Record<string, Mark>>({});
  /** 보기 필터와 무관한 폴더 전체 사진 수 */
  imageTotal = $state(0);
  /** 비교 화면에 띄운 사진 경로 (null이면 닫힘) */
  compare = $state<string[] | null>(null);
  /** 비교 화면을 열 때 미리 남김으로 표시할 사진 */
  compareKeep: string[] = [];
  /** 지금 폴더 목록(보기 필터)에 없는 사진도 비교할 수 있게 */
  compareExtra: Record<string, Entry> = {};
  /** 비슷한 사진 정리 화면 */
  similar = $state(false);
  /** 인물 화면 (연 범위) */
  people = $state<Scope | null>(null);
  /** 함께 보는 다른 폴더 (모음). 다른 폴더로 이동하면 비운다. */
  extra = $state<string[]>([]);
  /** 마지막 목록에서 읽은 폴더 수 */
  dirCount = $state(1);
  /** 재생 중인 동영상 */
  video = $state<Entry | null>(null);
  /** 편집기에 연 사진 */
  editor = $state<{ path: string; entry: Entry } | null>(null);
  /** 날짜·장소별 정리 화면: 고른 항목 (비어 있으면 폴더 전체) */
  organize = $state<{ folder: string; paths: string[] } | null>(null);
  /** 변환 진행 상황 (끝난 수 / 전체) */
  progress = $state<{ done: number; total: number; label: string } | null>(null);
  /** 탐색 기록 */
  private history: string[] = [];
  hpos = $state(-1);
  private toastTimer = 0;

  imageIndices = $derived(
    this.entries.flatMap((e, i) => (e.kind === "image" ? [i] : [])),
  );

  get canBack() {
    return this.hpos > 0;
  }
  get canForward() {
    return this.hpos < this.history.length - 1;
  }

  /** 지금 보는 범위 (폴더 하나 / 하위 폴더 포함 / 다른 폴더와 함께) */
  scope(path = this.path): Scope {
    return { path, extra: [...this.extra], recursive: this.settings.include_subfolders };
  }

  /** 폴더 하나만 보는지 (하위 폴더·다른 폴더를 함께 보지 않음) */
  get singleFolder() {
    return !this.extra.length && !this.settings.include_subfolders;
  }

  /** 다른 폴더를 더해 함께 본다 */
  async addFolders(paths: string[]) {
    const add = paths.filter((p) => p !== this.path && !this.extra.includes(p));
    if (!add.length || !this.path) return;
    this.extra = [...this.extra, ...add];
    await this.reload();
  }

  async removeFolder(p: string) {
    this.extra = this.extra.filter((x) => x !== p);
    await this.reload();
  }

  async clearFolders() {
    this.extra = [];
    await this.reload();
  }

  /** 함께 볼 폴더를 골라 더한다 (여러 개 고를 수 있음) */
  async pickAddFolders() {
    if (!this.path) return;
    const p = await openDialog({ directory: true, multiple: true, title: "함께 볼 폴더 고르기" });
    const list = Array.isArray(p) ? p : typeof p === "string" ? [p] : [];
    if (list.length) await this.addFolders(list);
  }

  /** 보는 범위 창 */
  scopeOpen = $state(false);

  async open(path: string, opts: { push?: boolean; selectPath?: string; keepScroll?: boolean } = {}) {
    const push = opts.push ?? true;
    this.loading = true;
    try {
      // 다른 폴더로 이동하면 함께 보던 폴더(모음)는 끝낸다
      if (path !== this.path && this.extra.length) this.extra = [];
      const l = await api.openFolder(this.scope(path));
      const prevPath = this.path;
      this.entries = l.entries;
      this.marks = l.marks;
      this.imageTotal = l.image_total;
      this.path = l.path;
      this.parent = l.parent;
      this.listMs = l.ms;
      this.dirCount = l.dirs;
      if (l.truncated) this.warnTruncated(l.entries.length);
      this.error = null;
      this.selected.clear();
      let focus = l.entries.length ? 0 : -1;
      if (opts.selectPath) {
        const i = l.entries.findIndex((e) => e.path === opts.selectPath);
        if (i >= 0) focus = i;
      } else if (prevPath && prevPath !== l.path) {
        // 상위 폴더로 올라가면 방금 있던 폴더에 포커스
        const i = l.entries.findIndex((e) => e.path === prevPath);
        if (i >= 0) focus = i;
      }
      this.focus = focus;
      this.anchor = focus;
      if (focus >= 0) this.selected.add(focus);
      if (!opts.keepScroll) this.scrollTo = focus;
      if (push && this.history[this.hpos] !== l.path) {
        this.history = this.history.slice(0, this.hpos + 1);
        this.history.push(l.path);
        this.hpos = this.history.length - 1;
      }
      document.title = `${baseName(l.path) || l.path} — iSee`;
    } catch (e) {
      this.error = `폴더를 열 수 없습니다: ${e}`;
      this.showToast(this.error, { error: true });
    } finally {
      this.loading = false;
    }
  }

  /** "분석 미리 해 두기"로 시작한 비슷한 사진 분석이 끝나면 알린다 */
  aheadSimilar = false;

  /** 분석 미리 해 두기: 지금 범위의 비슷한 사진(그리고 설치했으면 얼굴) 분석을 화면 없이 뒤에서 시작 */
  async analyzeAhead() {
    if (!this.path) return;
    const sc = this.scope();
    const st = await api.facesStatus().catch(() => null);
    const faces = !!st?.installed;
    if (!(await this.confirmAnalysis(sc, faces ? "both" : "similar"))) return;
    try {
      const [done, total] = await api.analyzeFolder(sc);
      if (faces) await api.facesAnalyze(sc).catch(() => undefined);
      this.aheadSimilar = done < total;
      this.showToast(
        done < total
          ? `"${scopeLabel(sc)}" 분석을 뒤에서 시작했습니다 (사진 ${total.toLocaleString()}장). 아래 "백그라운드 작업"에서 볼 수 있습니다.`
          : `"${scopeLabel(sc)}"은 이미 분석해 두었습니다${faces ? " (얼굴은 뒤에서 확인 중)" : ""}`,
      );
    } catch (e) {
      this.showToast(`분석을 시작하지 못했습니다: ${e}`, { error: true });
    }
  }

  /** 새로 분석할 사진이 이보다 많으면 시작 전에 걸릴 시간을 알리고 묻는다 */
  static readonly BIG_ANALYSIS = 2000;
  /** 클라우드 드라이브 폴더에서 이보다 많이 분석하면 내려받아 작업하라고 안내한다 */
  static readonly CLOUD_HINT = 300;

  /** 사진이 많거나 클라우드 드라이브 폴더면 분석 전에 묻는다. what: 할 분석. 진행하면 true. */
  async confirmAnalysis(sc: Scope, what: "similar" | "faces" | "both" = "similar"): Promise<boolean> {
    let est;
    try {
      est = await api.analysisEstimate(sc);
    } catch {
      return true;
    }
    const similar = what !== "faces" ? est.similar_todo : 0;
    const faces = what !== "similar" ? est.faces_todo : 0;
    const todo = Math.max(similar, faces);
    const cloudy = !!est.cloud_drive && todo >= Store.CLOUD_HINT;
    if (todo <= Store.BIG_ANALYSIS && !cloudy) return true;
    const lines: string[] = [];
    if (similar) lines.push(`· 비슷한 사진 찾기: ${similar.toLocaleString()}장, ${fmtDuration(est.similar_ms)}`);
    if (faces) lines.push(`· 얼굴 찾기: ${faces.toLocaleString()}장, ${fmtDuration(est.faces_ms)}`);
    let text =
      `사진 ${est.total.toLocaleString()}장 중 ${todo.toLocaleString()}장을 새로 분석합니다.\n${lines.join("\n")}\n\n` +
      `분석은 뒤에서 계속되므로 다른 사진을 봐도 되고, 앱을 닫았다가 다시 열면 분석한 곳 다음부터 이어서 합니다. ` +
      `보는 동안 느리면 백그라운드 작업 창에서 잠시 멈출 수 있습니다.`;
    if (cloudy)
      text +=
        `\n\n이 폴더는 ${est.cloud_drive}에 있어 사진을 한 장씩 내려받으며 분석합니다. 인터넷 상태에 따라 아주 오래 걸리거나 멈춘 것처럼 보일 수 있습니다.\n` +
        `빠르고 안정적으로 하려면 ${est.cloud_drive}에서 이 폴더를 "오프라인 사용(항상 이 기기에 보관)"으로 바꾸거나, ` +
        `컴퓨터 폴더로 복사해 정리한 뒤 다시 올려 주세요.`;
    if (est.cloud) text += `\n\n클라우드에만 있는 ${est.cloud.toLocaleString()}장은 내려받지 않으므로 분석하지 않습니다.`;
    return ask(text, {
      title: cloudy ? `${est.cloud_drive} 폴더입니다` : "사진이 많습니다",
      kind: "warning",
      okLabel: "분석 시작",
      cancelLabel: "취소",
    });
  }

  // ───── 클라우드에만 있는 사진 내려받아 분석 ─────

  /** 클라우드 사진 내려받기 진행 (앱 전체에 하나, 모든 화면이 함께 본다) */
  cloud = $state.raw<CloudProgress | null>(null);
  /** 내려받기가 끝나 이어서 하는 분석이 끝나면 알린다 */
  private cloudWaiting: { faces: boolean; got: number; failed: number } | null = null;

  /** 클라우드에만 있는 사진을 내려받아 분석할지 묻고 시작한다. faces: 얼굴 분석. 시작하면 true */
  async fetchCloud(sc: Scope, faces: boolean): Promise<boolean> {
    let est: CloudEstimate;
    try {
      est = await api.cloudEstimate(sc, faces);
    } catch (e) {
      this.showToast(`${e}`, { error: true });
      return false;
    }
    if (!est.count) {
      this.showToast("내려받아 분석할 사진이 없습니다");
      return false;
    }
    const n = `${est.count.toLocaleString()}장`;
    const size = fmtSize(est.bytes);
    const where = est.provider ?? "클라우드";
    if (est.free != null && est.free < est.bytes + 2 ** 30) {
      await message(
        `내려받을 사진은 ${size}인데 이 컴퓨터의 남은 공간은 ${fmtSize(est.free)}입니다.\n\n` +
          `공간을 비운 뒤 다시 해 주세요. 받고 나서도 1GB는 남아 있어야 합니다.`,
        { title: "저장 공간이 모자랍니다", kind: "warning", okLabel: "확인" },
      );
      return false;
    }
    const free = est.free != null ? ` (남은 공간 ${fmtSize(est.free)})` : "";
    // 처음 몇 장을 받아 보기 전에는 속도를 모르므로 초당 2MB로 대충 어림한다
    const guess = fmtDuration((est.bytes / 2e6) * 1000);
    const removal = est.can_evict
      ? `· 이 화면의 "다운로드 제거" 단추를 누르면 iSee가 받은 사진만 한 번에 지웁니다.\n` +
        `· Finder에서 사진이나 폴더를 오른쪽 클릭 → "다운로드 제거"로도 할 수 있습니다.`
      : `· 파일 탐색기에서 폴더를 오른쪽 클릭 → "공간 확보"(OneDrive)로 지울 수 있습니다.`;
    const ok = await ask(
      `클라우드에만 있는 사진 ${n}(${size})을 ${where}에서 이 컴퓨터로 내려받아 ${faces ? "얼굴을" : "비슷한 사진을"} 분석합니다.\n\n` +
        `· 내려받기는 백그라운드에서 2장씩 천천히 해서 사진 보기는 느려지지 않습니다. 화면을 닫거나 다른 사진을 봐도 계속합니다.\n` +
        `· 시간이 걸립니다. 인터넷 속도에 따라 다르며 ${guess} 걸릴 수 있습니다. 시작하면 남은 시간과 끝날 시각을 보여 드리고, 다 되면 알려 드립니다.\n` +
        `· 이 컴퓨터의 저장 공간을 ${size} 씁니다${free}.\n` +
        `· 클라우드의 사진은 지우거나 바꾸지 않습니다. 내려받은 사진은 클라우드 사진과 같은 파일입니다.\n\n` +
        `분석이 끝나면 내려받은 사진을 이 컴퓨터에서만 지워 공간을 되돌릴 수 있습니다. 클라우드의 사진과 분석 결과는 그대로 남습니다.\n` +
        removal,
      { title: "클라우드 사진 내려받아 분석", kind: "info", okLabel: "내려받아 분석", cancelLabel: "취소" },
    );
    if (!ok) return false;
    try {
      this.cloud = await api.cloudFetch(sc, faces);
      this.showToast(`${n} 내려받기를 시작했습니다. 아래 "백그라운드 작업"에서 남은 시간을 볼 수 있습니다.`);
      return true;
    } catch (e) {
      this.showToast(`내려받기를 시작하지 못했습니다: ${e}`, { error: true });
      return false;
    }
  }

  /** iSee가 내려받은 사진의 다운로드를 제거할지 묻고 제거한다 (클라우드의 사진은 그대로). 제거하면 true */
  async evictCloud(count: number, bytes: number, provider: string | null): Promise<boolean> {
    const ok = await ask(
      `iSee가 분석하려고 내려받은 사진 ${count.toLocaleString()}장(${fmtSize(bytes)})을 이 컴퓨터에서 지우고 다시 클라우드에만 둡니다.\n\n` +
        `· ${provider ?? "클라우드"}의 사진은 지워지지 않습니다.\n` +
        `· 분석 결과는 그대로 남아 다시 내려받지 않아도 됩니다.\n` +
        `· 나중에 사진을 크게 보면 그때 다시 내려받습니다.`,
      { title: "다운로드 제거", kind: "info", okLabel: "다운로드 제거", cancelLabel: "취소" },
    );
    if (!ok) return false;
    try {
      const r = await api.cloudEvict();
      if (r.failed)
        this.showToast(
          `${r.count.toLocaleString()}장(${fmtSize(r.bytes)})의 다운로드를 제거했고 ${r.failed.toLocaleString()}장은 제거하지 못했습니다` +
            `${r.error ? ` (${r.error})` : ""}. Finder에서 "오프라인으로 사용 가능"을 켠 폴더의 사진은 제거되지 않습니다.`,
          { error: true },
        );
      else this.showToast(`${r.count.toLocaleString()}장(${fmtSize(r.bytes)})의 다운로드를 제거했습니다. 클라우드의 사진과 분석 결과는 그대로입니다.`);
      // 다시 클라우드에만 있게 된 사진에 ☁ 표시
      this.reload();
      return true;
    } catch (e) {
      this.showToast(`${e}`, { error: true });
      return false;
    }
  }

  /** 내려받기가 끝남: 이어서 시작한 분석까지 끝나면 알린다 */
  cloudDone(d: CloudDone) {
    this.cloud = d.progress;
    // 받은 사진의 ☁ 표시를 지운다
    this.reload();
    this.cloudWaiting = { faces: d.faces, got: d.progress.done - d.progress.failed, failed: d.progress.failed };
    const [done, total] = d.analysis;
    if (done >= total) {
      this.cloudAnalyzed(d.faces);
      return;
    }
    // 사진이 적으면 분석이 이 알림보다 먼저 끝날 수 있어 한 번 더 본다
    api.stats().then((s) => {
      const [dn, tt] = d.faces ? s.bg.faces : s.bg.similar;
      if (tt > 0 && dn >= tt) this.cloudAnalyzed(d.faces);
    });
  }

  /** 분석 진행이 끝까지 갔을 때 (내려받기 뒤 분석을 기다리던 중이면 알린다) */
  cloudAnalyzed(faces: boolean) {
    const w = this.cloudWaiting;
    if (!w || w.faces !== faces) return;
    this.cloudWaiting = null;
    const what = faces ? "얼굴" : "비슷한 사진";
    let text = `클라우드 사진 ${w.got.toLocaleString()}장을 내려받아 ${what} 분석을 마쳤습니다.`;
    if (w.failed) text += `\n\n${w.failed.toLocaleString()}장은 내려받지 못했습니다. 인터넷 연결을 확인한 뒤 다시 누르면 그 사진만 받습니다.`;
    if (w.got)
      text +=
        `\n\n다 보셨으면 "${faces ? "인물" : "비슷한 사진"}" 화면의 "다운로드 제거" 단추로 이 컴퓨터에 받은 사진을 지워 공간을 되돌릴 수 있습니다. ` +
        `클라우드의 사진과 분석 결과는 그대로 남습니다.`;
    // 다른 프로그램을 보고 있으면 Dock(작업 표시줄) 아이콘으로도 알린다
    if (!document.hasFocus()) getCurrentWindow().requestUserAttention(UserAttentionType.Informational).catch(() => undefined);
    message(text, { title: "클라우드 사진 분석을 마쳤습니다", kind: "info", okLabel: "확인" });
  }

  /** 너무 많아 일부만 모았을 때 (프로그램이 멈추거나 메모리가 모자라지 않게 한도에서 멈춤) */
  private warnedAt = "";
  private warnTruncated(n: number) {
    // 하위 폴더 때문에 너무 많아졌으면 하위 폴더 보기를 끄고 이 폴더만 다시 읽는다
    if (this.settings.include_subfolders) {
      message(
        `하위 폴더까지 모으니 파일이 ${n.toLocaleString()}개를 넘습니다.\n\n` +
          `프로그램이 느려지거나 멈추지 않도록 "하위 폴더 사진도 보기"를 껐습니다. ` +
          `하위 폴더는 하나씩 열어 보거나, 필요한 폴더만 "다른 폴더 함께 보기"로 더해 주세요.`,
        { title: "사진이 너무 많습니다", kind: "warning", okLabel: "확인" },
      );
      this.setSettings({ include_subfolders: false });
      return;
    }
    const key = JSON.stringify(this.scope());
    if (this.warnedAt === key) return;
    this.warnedAt = key;
    message(
      `파일이 너무 많아 처음 ${n.toLocaleString()}개까지만 보여 줍니다.\n\n` +
        `속도와 안정을 위해 더 작은 폴더를 열거나, 함께 보는 폴더를 줄여 주세요.`,
      { title: "사진이 너무 많습니다", kind: "warning", okLabel: "확인" },
    );
  }

  /** 같은 폴더를 다시 읽는다. 선택은 경로 기준으로, 포커스는 같은 위치(다음 파일)로 유지. */
  async reload() {
    if (!this.path) return;
    const keep = new Set(this.selectedPaths());
    const focusAt = this.focus;
    const viewerAt = this.viewer;
    const l = await api.openFolder(this.scope());
    this.entries = l.entries;
    this.marks = l.marks;
    this.imageTotal = l.image_total;
    this.listMs = l.ms;
    this.dirCount = l.dirs;
    if (l.truncated) this.warnTruncated(l.entries.length);
    this.selected.clear();
    l.entries.forEach((e, i) => keep.has(e.path) && this.selected.add(i));
    this.focus = Math.min(focusAt, l.entries.length - 1);
    if (this.selected.size === 0 && this.focus >= 0) this.selected.add(this.focus);
    if (viewerAt >= 0) {
      // 뷰어에서 지운 경우: 같은 자리(다음 사진), 없으면 앞 사진
      const imgs = this.imageIndices;
      if (imgs.length === 0) this.viewer = -1;
      else this.viewer = imgs.find((i) => i >= viewerAt) ?? imgs[imgs.length - 1];
    }
  }

  back() {
    if (this.canBack) this.open(this.history[--this.hpos], { push: false });
  }
  forward() {
    if (this.canForward) this.open(this.history[++this.hpos], { push: false });
  }
  up() {
    if (this.parent) this.open(this.parent);
  }

  selectedPaths(): string[] {
    return [...this.selected].sort((a, b) => a - b).map((i) => this.entries[i]?.path).filter(Boolean);
  }

  /** 작업 대상: 뷰어가 열려 있으면 지금 사진, 아니면 선택 항목 */
  targets(): string[] {
    if (this.viewer >= 0) return [this.entries[this.viewer].path];
    const s = this.selectedPaths();
    if (s.length) return s;
    return this.focus >= 0 ? [this.entries[this.focus].path] : [];
  }

  selectOnly(i: number) {
    this.selected.clear();
    if (i >= 0) this.selected.add(i);
    this.focus = i;
    this.anchor = i;
  }

  selectRange(to: number) {
    const from = this.anchor >= 0 ? this.anchor : to;
    this.selected.clear();
    for (let i = Math.min(from, to); i <= Math.max(from, to); i++) this.selected.add(i);
    this.focus = to;
  }

  selectAll() {
    this.entries.forEach((_, i) => this.selected.add(i));
  }

  showToast(message: string, opts: { error?: boolean; undo?: boolean } = {}) {
    clearTimeout(this.toastTimer);
    this.toast = { id: Date.now(), message, ...opts };
    // 오류와 되돌리기 안내는 오래 보여 준다
    this.toastTimer = window.setTimeout(() => (this.toast = null), opts.undo || opts.error ? 7000 : 3500);
    if (opts.error) api.perfLog(`오류: ${message}`, 0);
  }

  private report(r: { ok: string[]; failed: { path: string; error: string }[] }, verb: string) {
    if (r.failed.length) {
      const f = r.failed[0];
      this.showToast(
        `${r.ok.length}개 ${verb}, ${r.failed.length}개 실패 (${baseName(f.path)}: ${f.error})`,
        { error: true, undo: r.ok.length > 0 },
      );
    } else {
      this.showToast(`${r.ok.length}개를 ${verb}`, { undo: true });
    }
  }

  async trashTargets() {
    const paths = this.targets();
    if (!paths.length) return;
    // 사진은 바로 버리지만(되돌리기 가능), 폴더가 섞여 있으면 한 번 묻는다
    const dirs = this.entries.filter((e) => e.kind === "dir" && paths.includes(e.path)).length;
    if (dirs > 0) {
      const ok = await ask(`폴더 ${dirs}개가 포함되어 있습니다. 휴지통으로 보낼까요?`, {
        title: "휴지통으로 이동",
        kind: "warning",
        okLabel: "휴지통으로",
        cancelLabel: "취소",
      });
      if (!ok) return;
    }
    const r = await api.trash(paths);
    this.report(r, "휴지통으로 보냈습니다");
    await this.reload();
  }

  async transferTargets(dest: string, copy: boolean) {
    const paths = this.targets().filter((p) => p !== dest);
    if (!paths.length) return;
    const r = await api.transfer(paths, dest, copy);
    this.report(r, copy ? `'${baseName(dest)}'에 복사했습니다` : `'${baseName(dest)}'(으)로 옮겼습니다`);
    if (!copy) await this.reload();
  }

  /** 숫자키 1~9: 지정된 폴더로 이동 (Shift면 복사). 지정 안 됐으면 폴더를 고르게 한다. */
  async quickMove(n: number, copy: boolean) {
    let dest = this.settings.quick_folders[n - 1];
    if (!dest) {
      const picked = await openDialog({ directory: true, title: `${n}번 빠른 이동 폴더 선택` });
      if (typeof picked !== "string") return;
      await this.setQuickFolder(n, picked);
      dest = picked;
    }
    await this.transferTargets(dest, copy);
  }

  async setQuickFolder(n: number, path: string | null) {
    const q = [...this.settings.quick_folders];
    q[n - 1] = path;
    this.applySettings(await api.updateSettings({ quick_folders: q }));
  }

  async undo() {
    try {
      const r = await api.undo();
      this.showToast(r ? r.message : "되돌릴 작업이 없습니다");
      if (r) await this.reload();
    } catch (e) {
      this.showToast(`되돌리지 못했습니다: ${e}`, { error: true });
    }
  }

  markOf(path: string): Mark {
    return this.marks[path] ?? { flag: 0, rating: 0 };
  }

  /** 남김(1)·버림(-1)·지우기(0) 또는 별점(0~5). 되돌리기 가능. */
  async setMarks(paths: string[], flag?: number, rating?: number) {
    if (!paths.length) return;
    const r = await api.setMarks(paths, flag, rating);
    const next = { ...this.marks };
    for (const [p, m] of Object.entries(r)) {
      if (m.flag === 0 && m.rating === 0) delete next[p];
      else next[p] = m;
    }
    this.marks = next;
  }

  /** 지금 폴더 전체의 표시 개수 (보기 필터로 숨긴 사진도 센다) */
  cullCounts() {
    let picked = 0,
      rejected = 0;
    for (const m of Object.values(this.marks)) {
      if (m.flag === 1) picked++;
      else if (m.flag === -1) rejected++;
    }
    return { picked, rejected, unflagged: Math.max(0, this.imageTotal - picked - rejected) };
  }

  /** 버림으로 표시한 사진을 모두 휴지통으로 (보기 필터로 숨긴 것 포함, 되돌리기 가능) */
  async trashRejected() {
    const paths = Object.entries(this.marks)
      .filter(([, m]) => m.flag === -1)
      .map(([p]) => p);
    if (!paths.length) {
      this.showToast("버림으로 표시한 사진이 없습니다 (X로 표시)");
      return;
    }
    const r = await api.trash(paths);
    this.report(r, "휴지통으로 보냈습니다");
    await this.reload();
  }

  /** 고른 사진을 비교 화면으로 (2~9장) */
  openCompare(paths = this.imageTargets(), opts: { keep?: string[]; entries?: Entry[] } = {}) {
    if (paths.length < 2) {
      this.showToast("비교할 사진을 2장 이상 고르세요");
      return;
    }
    if (paths.length > 9) this.showToast(`처음 9장만 비교합니다 (고른 사진 ${paths.length}장)`);
    this.compareKeep = opts.keep ?? [];
    this.compareExtra = Object.fromEntries((opts.entries ?? []).map((e) => [e.path, e]));
    this.compare = paths.slice(0, 9);
  }

  /** 인물 화면 (지금 폴더) */
  openPeople() {
    if (!this.path) return;
    this.viewer = -1;
    this.compare = null;
    this.similar = false;
    this.organize = null;
    this.people = this.scope();
  }

  /** 편집기: 뷰어에서 보는 사진, 아니면 고른 사진 한 장 */
  openEditor() {
    const i = this.viewer >= 0 ? this.viewer : this.focus;
    const e = this.entries[i];
    if (!e || e.kind !== "image") {
      this.showToast("편집할 사진을 하나 골라 주세요");
      return;
    }
    this.compare = null;
    this.editor = { path: e.path, entry: e };
  }

  /** 날짜·장소별 정리: 2개 이상 골랐으면 그것만, 아니면 지금 폴더 전체 */
  openOrganize() {
    if (!this.path) return;
    const sel = this.selectedPaths();
    this.viewer = -1;
    this.compare = null;
    this.similar = false;
    // 하위 폴더·다른 폴더를 함께 보고 있으면 보이는 사진·동영상 전부가 대상
    const all = this.singleFolder ? [] : this.entries.filter((e) => e.kind === "image" || e.kind === "video").map((e) => e.path);
    this.organize = { folder: this.path, paths: sel.length >= 2 ? sel : all };
  }

  /** 비슷한 사진 정리 화면 (지금 폴더) */
  openSimilar() {
    if (!this.path) return;
    this.viewer = -1;
    this.compare = null;
    this.similar = true;
  }

  /** 작업 대상 중 사진만 */
  imageTargets(): string[] {
    const kinds = new Map(this.entries.map((e) => [e.path, e.kind]));
    return this.targets().filter((p) => kinds.get(p) === "image");
  }

  /** 설정의 저장 방식 */
  saveSpec(): SaveSpec {
    const s = this.settings;
    if (s.convert_save === "overwrite") return { mode: "overwrite" };
    if (s.convert_save === "folder" && s.convert_folder) return { mode: "folder", path: s.convert_folder };
    return { mode: "new_file" };
  }

  /** 빠른 변환 실행: 고른 사진 전부, 진행 상황 표시, 끝나면 결과와 되돌리기 */
  async runConvert(spec: ConvertSpec, label: string, paths = this.imageTargets()) {
    if (!paths.length) {
      this.showToast("변환할 사진을 골라 주세요");
      return;
    }
    if (this.progress) return; // 이미 변환 중
    this.progress = { done: 0, total: paths.length, label };
    try {
      const r = await api.convert(paths, { save: this.saveSpec(), ...spec });
      const n = r.done.length;
      let msg: string;
      if (n === 1 && r.failed.length === 0) {
        const d = r.done[0];
        msg = d.lossless
          ? `${label} 완료 (화질 손실 없음)`
          : `${label} 완료 · ${baseName(d.output)} ${d.width}×${d.height} ${fmtSize(d.bytes)}`;
      } else {
        const lossless = n > 0 && r.done.every((d) => d.lossless);
        const total = r.done.reduce((a, d) => a + d.bytes, 0);
        msg = `${label} · ${n}장 완료${lossless ? " (화질 손실 없음)" : ` · 합계 ${fmtSize(total)}`} · ${(r.ms / 1000).toFixed(1)}초`;
      }
      if (r.failed.length) {
        const f = r.failed[0];
        msg += ` · ${r.failed.length}장 실패 (${baseName(f.path)}: ${f.error})`;
      }
      this.showToast(msg, { error: r.failed.length > 0, undo: n > 0 });
    } catch (e) {
      this.showToast(`${label} 실패: ${e}`, { error: true });
    } finally {
      this.progress = null;
    }
    await this.reload();
  }

  async setSettings(patch: Partial<Settings>) {
    this.applySettings(await api.updateSettings(patch));
  }

  /** 새 설정을 반영하고, 목록에 영향을 주는 값(종류·정렬·숨김·빠른 고르기 필터)이 바뀌었으면 폴더를 다시 읽는다.
   *  화면에서 바꾼 경우와 메뉴에서 바꾼 경우(settings-changed 이벤트)가 모두 여기로 온다. */
  applySettings(next: Settings) {
    const prev = this.settings;
    this.settings = next;
    const relist =
      JSON.stringify(prev.kinds) !== JSON.stringify(next.kinds) ||
      prev.sort_by !== next.sort_by ||
      prev.sort_desc !== next.sort_desc ||
      prev.show_hidden !== next.show_hidden ||
      prev.include_subfolders !== next.include_subfolders ||
      prev.culling !== next.culling ||
      (next.culling && prev.cull_filter !== next.cull_filter);
    if (relist && this.path) this.reload();
  }
}

export const store = new Store();
