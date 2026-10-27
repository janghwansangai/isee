import { SvelteSet } from "svelte/reactivity";
import { ask, open as openDialog } from "@tauri-apps/plugin-dialog";
import {
  api,
  baseName,
  fmtSize,
  type ConvertSpec,
  type Entry,
  type Mark,
  type Place,
  type SaveSpec,
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
  /** 인물 화면 (연 폴더) */
  people = $state<string | null>(null);
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

  async open(path: string, opts: { push?: boolean; selectPath?: string; keepScroll?: boolean } = {}) {
    const push = opts.push ?? true;
    this.loading = true;
    try {
      const l = await api.openFolder(path);
      const prevPath = this.path;
      this.entries = l.entries;
      this.marks = l.marks;
      this.imageTotal = l.image_total;
      this.path = l.path;
      this.parent = l.parent;
      this.listMs = l.ms;
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

  /** 같은 폴더를 다시 읽는다. 선택은 경로 기준으로, 포커스는 같은 위치(다음 파일)로 유지. */
  async reload() {
    if (!this.path) return;
    const keep = new Set(this.selectedPaths());
    const focusAt = this.focus;
    const viewerAt = this.viewer;
    const l = await api.openFolder(this.path);
    this.entries = l.entries;
    this.marks = l.marks;
    this.imageTotal = l.image_total;
    this.listMs = l.ms;
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
    this.people = this.path;
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
    this.organize = { folder: this.path, paths: sel.length >= 2 ? sel : [] };
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
      prev.culling !== next.culling ||
      (next.culling && prev.cull_filter !== next.cull_filter);
    if (relist && this.path) this.reload();
  }
}

export const store = new Store();
