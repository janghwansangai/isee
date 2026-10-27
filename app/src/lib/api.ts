import { invoke } from "@tauri-apps/api/core";

export type Kind = "dir" | "image" | "video" | "pdf" | "doc" | "other";

export interface Entry {
  name: string;
  path: string;
  kind: Kind;
  size: number;
  mtime: number;
  /** 클라우드에만 있고 아직 내려받지 않은 파일 */
  cloud?: boolean;
  /** 하위 폴더·여러 폴더를 함께 볼 때 이 파일이 있는 폴더 (보여 주는 이름) */
  folder?: string;
}

/** 지금 보는 범위: 폴더 하나, 하위 폴더까지, 다른 폴더를 더해 함께 */
export interface Scope {
  path: string;
  extra: string[];
  recursive: boolean;
}

/** flag: 1 남김, -1 버림, 0 없음 · rating: 0~5 */
export interface Mark {
  flag: number;
  rating: number;
}

export interface Listing {
  path: string;
  parent: string | null;
  entries: Entry[];
  marks: Record<string, Mark>;
  image_total: number;
  /** 읽은 폴더 수 (폴더 하나면 1) */
  dirs: number;
  /** 파일이 너무 많아 한도까지만 모았다 */
  truncated: boolean;
  ms: number;
}

export interface Place {
  name: string;
  path: string;
  icon: string;
}

export interface DirNode {
  name: string;
  path: string;
  has_children: boolean;
}

export interface Info {
  path: string;
  name: string;
  size: number;
  mtime: number;
  format: string;
  width: number | null;
  height: number | null;
  taken: string | null;
  camera: string | null;
  lens: string | null;
  iso: number | null;
  exposure: string | null;
  fnumber: string | null;
  focal: string | null;
  gps: [number, number] | null;
  cloud?: boolean;
}

export interface Settings {
  kinds: { image: boolean; video: boolean; pdf: boolean; doc: boolean; other: boolean };
  sort_by: "name" | "date" | "size" | "type" | "rating";
  sort_desc: boolean;
  show_hidden: boolean;
  show_info: boolean;
  show_hud: boolean;
  keep_running: boolean;
  view_mode: "grid" | "list";
  /** 하위 폴더 사진도 함께 보기 (사진이 많아질 수 있어 기본은 꺼짐) */
  include_subfolders: boolean;
  thumb_size: number;
  quick_folders: (string | null)[];
  last_folder: string | null;
  convert_save: "new" | "overwrite" | "folder";
  convert_folder: string | null;
  culling: boolean;
  cull_filter: "all" | "picked" | "unflagged" | "rejected" | "rated";
  similar: SimilarOptions;
  organize: OrganizeOptions;
  organize_copy: boolean;
}

// ───── 인물 (8단계) ─────
export interface FaceRow {
  id: number;
  path: string;
  rect: [number, number, number, number];
  px: number;
  person: number | null;
  not_person: number | null;
}
export interface PersonGroup {
  person: number | null;
  name: string | null;
  faces: FaceRow[];
  cover: number;
  photos: number;
  auto: number;
}
export interface PeopleReport {
  groups: PersonGroup[];
  singles: number;
  small: number;
  analyzed: number;
  total: number;
  /** 얼굴이 아주 많아 빠른 방식으로 묶었다 */
  fast: boolean;
  ms: number;
}

/** 분석 시간 어림 (시작 전에 묻기용) */
export interface AnalysisEstimate {
  total: number;
  similar_todo: number;
  cloud: number;
  similar_ms: number;
  faces_todo: number;
  faces_ms: number;
}

// ───── 날짜·장소별 정리 (5단계) ─────
export type PlaceLevel = "country" | "province" | "city" | "town";
export interface OrganizeOptions {
  /** {년} {월} {일} {장소} {기간}, "/"로 하위 폴더 */
  pattern: string;
  level: PlaceLevel;
  /** 위치 없는 사진은 30분 안 앞뒤 사진의 위치로 추정 */
  guess_place: boolean;
  /** {기간}(이벤트)을 나누는 촬영 간격 (시간) */
  event_hours: number;
}
export interface OrganizeItem {
  path: string;
  size: number;
  mtime: number;
  companions: string[];
  date_source: "exif" | "video" | "filename" | "modified";
  time: number;
  place: string | null;
  place_guessed: boolean;
}
export interface OrganizePlan {
  folders: { rel: string; items: OrganizeItem[]; bytes: number }[];
  total: number;
  by_mtime: number;
  by_name: number;
  with_gps: number;
  guessed: number;
  no_place: number;
  companions: number;
  ms: number;
}
export interface OrganizeReport {
  files: number;
  folders: number;
  unchanged: number;
  failed: { path: string; error: string }[];
  ms: number;
}

// ───── 비슷한 사진 (4단계) ─────
export type SimilarRule = "sharp" | "exposure" | "noise" | "resolution" | "newest";
export interface SimilarOptions {
  /** 묶는 정도 1(거의 같은 것만) ~ 5(대충 비슷해도) */
  level: number;
  /** 묶음마다 추천할 장수 */
  keep: number;
  /** 앞에 있을수록 중요한 기준 */
  rules: SimilarRule[];
}
export interface Photo {
  path: string;
  size: number;
  mtime: number;
  width: number;
  height: number;
}
export interface SimilarMember extends Photo {
  score: number;
  keep: boolean;
  reasons: string[];
}
export interface SimilarReport {
  groups: { members: SimilarMember[]; taken: number }[];
  duplicates: Photo[][];
  blurry: Photo[];
  dark: Photo[];
  bright: Photo[];
  analyzed: number;
  total: number;
  /** 클라우드에만 있어(내려받지 않음) 분석하지 못한 사진 수 */
  cloud: number;
  ms: number;
}

export type Turn = "cw90" | "ccw90" | "r180" | "flip_h" | "flip_v";
export type OutFormat = "jpeg" | "png" | "webp" | "heic" | "avif" | "tiff" | "bmp";
export type SaveSpec = { mode: "new_file" } | { mode: "overwrite" } | { mode: "folder"; path: string };

export interface Watermark {
  png: number[];
  anchor: number;
  width_ratio: number;
  margin_ratio: number;
  opacity: number;
}

export interface ConvertSpec {
  turn?: Turn | null;
  resize?: { kind: "percent" | "long_side"; value: number } | null;
  format?: OutFormat | null;
  quality?: number | null;
  max_bytes?: number | null;
  strip_gps?: boolean;
  watermark?: Watermark | null;
  save?: SaveSpec;
  suffix?: string | null;
}

export interface ConvertDone {
  source: string;
  output: string;
  bytes: number;
  width: number;
  height: number;
  lossless: boolean;
  backup: string | null;
  replaced_source: boolean;
  quality: number | null;
}

export interface ConvertReport {
  done: ConvertDone[];
  failed: { path: string; error: string }[];
  ms: number;
}

export interface RenamePlanned {
  from: string;
  name: string;
}

export interface RenameRule {
  pattern: string;
  start: number;
  digits: number;
}

export interface OpResult {
  ok: string[];
  failed: { path: string; error: string }[];
}

export interface Stats {
  generated: number;
  avg_ms: number;
  mem_hits: number;
  disk_hits: number;
  pending: number;
  bg: BgStatus;
}

export interface BgStatus {
  paused: boolean;
  prefill: boolean;
  thumbs: number;
  similar: [number, number];
  /** 비슷한 사진 분석에서 클라우드에만 있어 넘긴 수 */
  similar_cloud: number;
  /** 분석하는(했던) 범위 이름 */
  similar_label: string;
  faces_label: string;
  faces: [number, number];
}

/** 시작 단계별 시각 (웹페이지 시작 기준 ms) */
export const startMarks: [string, number][] = [];
export function mark(name: string) {
  startMarks.push([name, performance.now()]);
}

export const isMac = navigator.userAgent.includes("Mac");
export const isWindows = navigator.userAgent.includes("Windows");

/** 윈도우(WebView2)는 http://isee.localhost/, 맥·리눅스는 isee://localhost/ 형식을 쓴다. */
const BASE = isWindows ? "http://isee.localhost/" : "isee://localhost/";

export function thumbUrl(e: Pick<Entry, "path" | "size" | "mtime">, index: number): string {
  return `${BASE}thumb?p=${encodeURIComponent(e.path)}&s=${e.size}&m=${e.mtime}&i=${index}`;
}

/** 폴더 목록 밖에서 다루는 사진 (비슷한 사진 화면 등)을 목록 항목 모양으로 */
export function photoEntry(p: Photo): Entry {
  return { name: baseName(p.path), path: p.path, kind: "image", size: p.size, mtime: p.mtime };
}

/** 인물 화면 얼굴 썸네일 */
export function faceUrl(id: number): string {
  return `${BASE}face?id=${id}`;
}

/** 파일 그대로 (범위 요청 지원: 동영상 재생용) */
export function fileUrl(path: string): string {
  return `${BASE}file?p=${encodeURIComponent(path)}`;
}

/** 화면 상자(w×h 실제 픽셀)에 맞는 크기로 디코딩한 사진. 0×0이면 원본 해상도. */
export function viewUrl(path: string, w: number, h: number): string {
  return `${BASE}view?p=${encodeURIComponent(path)}&w=${w}&h=${h}`;
}

export const api = {
  places: () => invoke<Place[]>("places"),
  subdirs: (path: string) => invoke<DirNode[]>("subdirs", { path }),
  openFolder: (sc: Scope) => invoke<Listing>("open_folder", { path: sc.path, extra: sc.extra, recursive: sc.recursive }),
  setViewport: (start: number, end: number) => invoke("set_viewport", { start, end }),
  setViewerActive: (active: boolean) => invoke("set_viewer_active", { active }),
  fileInfo: (path: string) => invoke<Info>("file_info", { path }),
  trash: (paths: string[]) => invoke<OpResult>("trash", { paths }),
  transfer: (paths: string[], dest: string, copy: boolean) =>
    invoke<OpResult>("transfer", { paths, dest, copy }),
  rename: (path: string, name: string) => invoke<string>("rename", { path, name }),
  createFolder: (parent: string, name: string) =>
    invoke<string>("create_folder", { parent, name }),
  undo: () => invoke<{ message: string } | null>("undo"),
  reveal: (path: string) => invoke("reveal", { path }),
  getSettings: () => invoke<Settings>("get_settings"),
  updateSettings: (patch: Partial<Settings>) => invoke<Settings>("update_settings", { patch }),
  takePendingOpen: () => invoke<string | null>("take_pending_open"),
  frontendReady: (marks: [string, number][]) =>
    invoke<number>("frontend_ready", { pageOrigin: performance.timeOrigin, marks }),
  stats: () => invoke<Stats>("stats"),
  perfLog: (label: string, ms: number) => invoke("perf_log", { label, ms }),
  contextMenu: (images: number, total: number) => invoke("context_menu", { images, total }),
  setMarks: (paths: string[], flag?: number, rating?: number) =>
    invoke<Record<string, Mark>>("set_marks", { paths, flag: flag ?? null, rating: rating ?? null }),
  convert: (paths: string[], spec: ConvertSpec) => invoke<ConvertReport>("convert", { paths, spec }),
  renamePlan: (paths: string[], rule: RenameRule) =>
    invoke<RenamePlanned[]>("rename_plan", { paths, rule }),
  renameApply: (items: RenamePlanned[]) => invoke<number>("rename_apply", { items }),
  analyzeFolder: (sc: Scope) =>
    invoke<[number, number]>("analyze_folder", { path: sc.path, extra: sc.extra, recursive: sc.recursive }),
  analysisEstimate: (sc: Scope) =>
    invoke<AnalysisEstimate>("analysis_estimate", { path: sc.path, extra: sc.extra, recursive: sc.recursive }),
  stopAnalysis: () => invoke("stop_analysis"),
  similarReport: (sc: Scope, options: SimilarOptions) =>
    invoke<SimilarReport>("similar_report", { path: sc.path, options, extra: sc.extra, recursive: sc.recursive }),
  organizeScan: (paths: string[], folder: string, recursive: boolean) =>
    invoke<{ photos: number; videos: number; ms: number }>("organize_scan", { paths, folder, recursive }),
  organizePlan: (options: OrganizeOptions) => invoke<OrganizePlan>("organize_plan", { options }),
  organizeApply: (
    root: string,
    options: OrganizeOptions,
    renames: Record<string, string>,
    skip: string[],
    copy: boolean,
  ) => invoke<OrganizeReport>("organize_apply", { root, options, renames, skip, copy }),
  bgControl: (action: string) => invoke<BgStatus>("bg_control", { action }),
  facesStatus: () => invoke<{ installed: boolean; download: number; analyzing: [number, number] }>("faces_status"),
  facesInstall: () => invoke("faces_install"),
  facesAnalyze: (sc: Scope) =>
    invoke<[number, number]>("faces_analyze", { folder: sc.path, recursive: sc.recursive, extra: sc.extra }),
  facesStop: () => invoke("faces_stop"),
  facesReport: (sc: Scope) =>
    invoke<PeopleReport>("faces_report", { folder: sc.path, recursive: sc.recursive, extra: sc.extra }),
  facesName: (faces: number[], person: number | null, name: string | null) =>
    invoke<number>("faces_name", { faces, person, name }),
  facesReject: (faces: number[], person: number) => invoke("faces_reject", { faces, person }),
  facesApart: (faces: number[], others: number[]) => invoke("faces_apart", { faces, others }),
  facesDeletePerson: (person: number) => invoke("faces_delete_person", { person }),
  facesClear: () => invoke("faces_clear"),
  facesOrganize: (sc: Scope, root: string, mode: "copy" | "main", peopleIds: number[]) =>
    invoke<OrganizeReport>("faces_organize", { folder: sc.path, recursive: sc.recursive, root, mode, peopleIds, extra: sc.extra }),
  shellIntegration: (install?: boolean) =>
    invoke<{ installed: boolean; count: number }>("shell_integration", { install: install ?? null }),
};

export function baseName(p: string): string {
  const parts = p.split(/[\\/]/).filter(Boolean);
  return parts[parts.length - 1] ?? p;
}

export function parentOf(p: string): string | null {
  const i = Math.max(p.lastIndexOf("/"), p.lastIndexOf("\\"));
  if (i <= 0) return p.startsWith("/") && p.length > 1 ? "/" : null;
  const parent = p.slice(0, i);
  return /^[A-Za-z]:$/.test(parent) ? parent + "\\" : parent;
}

export function fmtSize(n: number): string {
  if (n < 1024) return `${n} B`;
  if (n < 1024 * 1024) return `${(n / 1024).toFixed(0)} KB`;
  if (n < 1024 * 1024 * 1024) return `${(n / 1024 / 1024).toFixed(1)} MB`;
  return `${(n / 1024 / 1024 / 1024).toFixed(2)} GB`;
}

/** 범위를 사람이 읽는 이름으로: "1026세계박람회 (하위 폴더 포함) 외 2곳" */
export function scopeLabel(sc: Scope): string {
  let s = baseName(sc.path) || sc.path;
  if (sc.recursive) s += " (하위 폴더 포함)";
  if (sc.extra.length) s += ` 외 ${sc.extra.length}곳`;
  return s;
}

/** 걸릴 시간: "약 40초", "약 12분", "약 1시간 30분" */
export function fmtDuration(ms: number): string {
  const sec = Math.max(1, Math.round(ms / 1000));
  if (sec < 60) return `약 ${sec}초`;
  const min = Math.round(sec / 60);
  if (min < 60) return `약 ${min}분`;
  return `약 ${Math.floor(min / 60)}시간${min % 60 ? ` ${min % 60}분` : ""}`;
}

export function fmtDate(ms: number): string {
  const d = new Date(ms);
  const p = (v: number) => String(v).padStart(2, "0");
  return `${d.getFullYear()}-${p(d.getMonth() + 1)}-${p(d.getDate())} ${p(d.getHours())}:${p(d.getMinutes())}`;
}
