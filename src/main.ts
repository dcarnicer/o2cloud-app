import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { join } from "@tauri-apps/api/path";
import { getCurrentWebview } from "@tauri-apps/api/webview";
import * as L from "leaflet";
import "leaflet/dist/leaflet.css";
import { confirm, open, save } from "@tauri-apps/plugin-dialog";
import { relaunch } from "@tauri-apps/plugin-process";
import { check, type Update } from "@tauri-apps/plugin-updater";
import { getVersion } from "@tauri-apps/api/app";

/** WebView2 on Windows reports "Windows"; WKWebView on macOS reports "Mac". */
const IS_MAC = /Mac/i.test(navigator.userAgent);
const IS_WINDOWS = /Windows/i.test(navigator.userAgent);

const SHORTCUT = {
  refresh: IS_MAC ? "⌘R" : "Ctrl+R",
  search: IS_MAC ? "⌘F" : "Ctrl+F",
  up: IS_MAC ? "⌘↑" : "Ctrl+↑",
  selectAll: IS_MAC ? "⌘A" : "Ctrl+A",
  trash: IS_MAC ? "⌘⌫" : "Supr",
  newFolder: IS_MAC ? "⇧⌘N" : "Ctrl+Shift+N",
  open: IS_MAC ? "↩" : "Intro",
};

/** O2 allows characters and device names that Windows cannot save. */
function safeDownloadName(name: string): string {
  if (!IS_WINDOWS) return name;
  const reserved = /^(con|prn|aux|nul|com[1-9]|lpt[1-9])(\..*)?$/i;
  let safe = name
    .replace(/[<>:"/\\|?*\u0000-\u001f]/g, "_")
    .replace(/[. ]+$/g, "");
  if (!safe || safe === "." || safe === "..") safe = "_";
  if (reserved.test(safe)) safe = `_${safe}`;
  if (safe.length > 180) {
    const dot = safe.lastIndexOf(".");
    const extension = dot > 0 && safe.length - dot <= 32 ? safe.slice(dot) : "";
    safe = safe.slice(0, 180 - extension.length) + extension;
    safe = safe.replace(/[. ]+$/g, "");
    if (!safe) safe = "_";
  }
  return safe;
}

interface RemoteFolder {
  id: number;
  name: string;
  files: number;
  size: number;
}

interface CreatedFolder {
  folder: RemoteFolder;
  created: boolean;
}

interface PartRef {
  id: number;
  name: string;
  size: number;
}

interface LogicalFile {
  name: string;
  size: number;
  date: string | number | null;
  chunked: boolean;
  cli_split: boolean;
  manifest_id: number | null;
  normal_id: number | null;
  parts: PartRef[];
  incomplete: boolean;
}

interface VirtualFolder {
  key: string;
  name: string;
  files: number;
  size: number;
  kind: "camera" | "year" | "all" | string;
  /** A label's cover (signed O2 URL). */
  thumbnail?: string;
  /** Where a place is. */
  location?: { lat: number; lon: number };
  /** A moment's day (ISO). */
  date?: string;
}

interface Track {
  id: number;
  name: string;
  path: string | null;
  duration: number | null;
  url: string;
}

interface Slide {
  id: number;
  name: string;
  date: string | null;
  preview: string;
}

interface LogicalListing {
  path: string;
  folders: RemoteFolder[];
  files: LogicalFile[];
  warnings: string[];
  virtual_folders: VirtualFolder[];
  /** Last listing saved on disk; a fresh one is loading in the background. */
  stale: boolean;
}

interface CliInfo {
  path: string;
  version: string;
  source: string;
  available: boolean;
  supported: boolean;
  minimum_version: string;
}

interface StorageSpace {
  quota: number;
  used: number;
  free: number;
  softdeleted: number;
  nolimit: boolean;
}

interface SessionStatus {
  logged_in: boolean;
  raw: string;
  phone: string | null;
  since: string | null;
  session: string | null;
  renews_silently: boolean;
  storage: StorageSpace | null;
}

interface Settings {
  group_camera: boolean;
  hash_non_media: boolean;
  slideshow_music: string;
}

interface CliRelease {
  current: string;
  latest: string | null;
  update_available: boolean;
  asset_name: string | null;
  asset_url: string | null;
  notes: string | null;
}

interface TransferProgress {
  id: string;
  kind: "upload" | "download" | string;
  name: string;
  phase: string;
  current: number;
  total: number;
  bytes_done: number;
  bytes_total: number;
  message: string;
}

interface UploadSummary {
  uploaded: number;
  skipped: number;
  failed: string[];
  /** Files O2 already has with the same name and another size. */
  conflicts?: UploadConflict[];
  library_warning?: string | null;
}

/** A file the CLI didn't upload because O2 has one of another size (exit 5). */
interface UploadConflict {
  local: string;
  name: string;
  size: number;
  modified?: string | null;
  cloud: { size: number; modified?: string | null };
}

interface LibraryStats {
  files: number;
  bytes: number;
  copies: number;
  devices: number;
  last_sync: number | null;
  pending_snapshot: boolean;
  device_id: string;
}

interface LibraryCopy {
  folder_id: number | null;
  folder_path: string;
  remote_id: number | null;
  remote_name: string;
  display_name: string;
}

interface PhotoLibraryMatch {
  asset_id: string;
  sha256: string | null;
  copies: LibraryCopy[];
  in_target: boolean;
}

interface PathCheck {
  path: string;
  sha256: string | null;
  copies: LibraryCopy[];
  in_target: boolean;
  is_file: boolean;
}

interface PhotoAsset {
  id: string;
  name: string;
  size: number;
  date: number | null;
  duration: number;
  media_type: "video" | "photo" | string;
  width: number;
  height: number;
}

interface PhotoList {
  assets: PhotoAsset[];
  total: number;
  truncated: boolean;
}

interface PhotoAlbum {
  id: string;
  title: string;
}

interface PhotoAlbums {
  albums: PhotoAlbum[];
}

interface PhotoThumbnail {
  data: string;
}

interface PhotosAccess {
  available: boolean;
  status: string;
}

interface Crumb {
  id: number | null;
  name: string;
  path: string;
  virtual?: string;
}

type SortKey = "name" | "size" | "date";

/** A row of the file list. */
type VirtualEntry = { kind: "virtual"; key: string; name: string; folder: VirtualFolder };
type FolderEntry = { kind: "folder"; key: string; name: string; folder: RemoteFolder };
type FileEntry = { kind: "file"; key: string; name: string; file: LogicalFile };
type Entry = VirtualEntry | FolderEntry | FileEntry;

type MenuEntry =
  | { label: string; hint?: string; danger?: boolean; disabled?: boolean; run: () => void }
  | "separator";

const ROOT: Crumb = { id: null, name: "O2 Cloud", path: "/" };
const CAMERA: Crumb = { id: null, name: "Cámara", path: "/", virtual: "camera" };
/** What O2 groups photos into by itself (the CLI's `labels`). */
const MOMENTS: Crumb = { id: null, name: "Momentos", path: "/", virtual: "labels:moments" };
const PLACES: Crumb = { id: null, name: "Lugares", path: "/", virtual: "labels:places" };
const PEOPLE: Crumb = { id: null, name: "Personas", path: "/", virtual: "labels:people" };
/** What O2 files as music or documents, wherever it is. */
const MUSIC: Crumb = { id: null, name: "Música", path: "/", virtual: "media:music" };
const DOCUMENTS: Crumb = { id: null, name: "Documentos", path: "/", virtual: "media:documents" };
const PAGE = 500;
const PHOTO_PAGE = 300;
/** O2's per-file limit; the Photos view can filter to the files that hit it. */
const LARGE_FILE = 4 * 1024 ** 3;
const SORT_STORAGE = "o2cloud.sort";
const PLACES_VIEW_STORAGE = "o2cloud.placesView";
const MUSIC_VIEW_STORAGE = "o2cloud.musicView";
/** Música's folders open as `media:music-folder:<folder>`. */
const MUSIC_FOLDER = "media:music-folder:";
/** Folders created in this session that are known to be empty, with expiry. */
const knownEmptyFolders = new Map<number, number>();
const EMPTY_FOLDER_GRACE_MS = 2 * 60_000;

const state = {
  stack: [ROOT] as Crumb[],
  listing: null as LogicalListing | null,
  /** Folders at the root, listed in the sidebar. */
  rootFolders: [] as RemoteFolder[],
  cli: null as CliInfo | null,
  session: null as SessionStatus | null,
  settings: {
    group_camera: true,
    slideshow_music: "forget-me-not",
    hash_non_media: false,
  } as Settings,
  queue: new Map<string, TransferProgress>(),
  pendingRelease: null as CliRelease | null,
  loading: false,
  visibleLimit: PAGE,
  uploadsInFlight: 0,
  sort: loadSort(),
  filter: "",
  selected: new Set<string>(),
  /** Where a Shift range starts, and the row the arrow keys move from. */
  anchor: null as string | null,
  cursor: null as string | null,
  /** Rows being moved to the trash. */
  trashing: new Set<string>(),
  libraryStats: null as LibraryStats | null,
  photos: {
    available: false,
    status: "unknown",
    assets: [] as PhotoAsset[],
    total: 0,
    truncated: false,
    selected: new Set<string>(),
    matches: new Map<string, PhotoLibraryMatch>(),
    loading: false,
    syncingLibrary: false,
    /** Resolves when the current library match finishes; uploads wait for it. */
    matchPromise: null as Promise<void> | null,
    uploading: false,
    filter: "",
    mediaType: "video",
    onlyLarge: false,
    hideExisting: false,
    albums: [] as PhotoAlbum[],
    albumId: "",
    favoritesOnly: false,
    dateFilter: false,
    startDate: "",
    endDate: "",
    visibleLimit: PHOTO_PAGE,
  },
};

const $ = <T extends HTMLElement>(selector: string): T => {
  const el = document.querySelector<T>(selector);
  if (!el) throw new Error(`No existe ${selector}`);
  return el;
};

function currentCrumb(): Crumb {
  return state.stack[state.stack.length - 1];
}

function crumbKey(crumb: Crumb): string {
  return `${crumb.id ?? ""}|${crumb.path}|${crumb.virtual ?? ""}`;
}

function joinPath(parent: string, name: string): string {
  return parent === "/" ? `/${name}` : `${parent.replace(/\/+$/, "")}/${name}`;
}

function plural(count: number, one: string, many: string): string {
  return `${count.toLocaleString("es-ES")} ${count === 1 ? one : many}`;
}

function formatSize(bytes: number | null | undefined): string {
  if (bytes == null || bytes < 0) return "—";
  const units = ["B", "KB", "MB", "GB", "TB"];
  let value = bytes;
  let unit = 0;
  while (value >= 1024 && unit < units.length - 1) {
    value /= 1024;
    unit++;
  }
  const digits = value >= 100 || unit === 0 ? 0 : 1;
  return `${value.toFixed(digits).replace(".", ",")} ${units[unit]}`;
}

function timeOf(value: string | number | null): number | null {
  if (value == null || value === "") return null;
  const time = new Date(value).getTime();
  return Number.isNaN(time) ? null : time;
}

function formatDate(value: string | number | null): string {
  const time = timeOf(value);
  if (time == null) return "—";
  return new Date(time).toLocaleString("es-ES", {
    day: "2-digit",
    month: "2-digit",
    year: "numeric",
    hour: "2-digit",
    minute: "2-digit",
  });
}

function formatAgo(timestamp: number | null): string {
  if (timestamp == null) return "nunca";
  const seconds = Math.max(0, Math.round((Date.now() - timestamp) / 1000));
  if (seconds < 60) return "hace un momento";
  if (seconds < 3600) return `hace ${Math.round(seconds / 60)} min`;
  if (seconds < 86400) return `hace ${Math.round(seconds / 3600)} h`;
  return `hace ${Math.round(seconds / 86400)} días`;
}

interface ToastAction {
  label: string;
  run: () => void;
}

/**
 * Show a notification. With `action` it has that button and one to close
 * it. `timeout` 0 keeps it until it's closed: one with a button mustn't
 * vanish before it can be reached.
 */
function toast(
  message: string,
  type: "info" | "success" | "error" = "info",
  timeout = 5000,
  action?: ToastAction,
): () => void {
  const container = $("#toast-container");
  const el = document.createElement("div");
  el.className = `toast ${type}`;
  // Errors interrupt a screen reader; the rest wait their turn (the
  // container is a polite live region).
  if (type === "error") el.setAttribute("role", "alert");
  let leaving = false;
  const dismiss = () => {
    if (leaving) return;
    leaving = true;
    el.classList.add("leaving");
    window.setTimeout(() => el.remove(), 250);
  };
  if (action) {
    el.classList.add("actionable");
    const body = document.createElement("div");
    body.className = "toast-body";
    const text = document.createElement("div");
    text.textContent = message;
    const button = document.createElement("button");
    button.type = "button";
    button.className = "secondary-btn";
    button.textContent = action.label;
    button.addEventListener("click", () => {
      dismiss();
      action.run();
    });
    body.append(text, button);
    const close = document.createElement("button");
    close.type = "button";
    close.className = "icon-btn toast-close";
    close.setAttribute("aria-label", "Cerrar aviso");
    close.textContent = "✕";
    close.addEventListener("click", dismiss);
    el.append(body, close);
  } else {
    el.textContent = message;
  }
  container.appendChild(el);
  if (timeout > 0) window.setTimeout(dismiss, timeout);
  return dismiss;
}

function isSessionError(message: string): boolean {
  return /sesión de O2 ha caducado|Session expired|Authentication failed|Not logged in/i.test(message);
}

/** Show an error; if it says the O2 session is gone, ask for a new login. */
function reportError(error: unknown, timeout = 10000) {
  const message = String(error);
  toast(message, "error", timeout);
  if (isSessionError(message)) void refreshSession();
}

function loadSort(): { key: SortKey; dir: 1 | -1 } {
  try {
    const saved = JSON.parse(localStorage.getItem(SORT_STORAGE) ?? "null");
    if (
      saved &&
      ["name", "size", "date"].includes(saved.key) &&
      (saved.dir === 1 || saved.dir === -1)
    ) {
      return saved;
    }
  } catch {
    // Storage unavailable: use the default.
  }
  return { key: "name", dir: 1 };
}

/** A view chosen before (how Lugares or Música are shown), or `fallback`. */
function loadChoice<T extends string>(key: string, choices: readonly T[], fallback: T): T {
  try {
    const saved = localStorage.getItem(key);
    if (saved && (choices as readonly string[]).includes(saved)) return saved as T;
  } catch {
    // Storage unavailable: the default.
  }
  return fallback;
}

function saveChoice(key: string, value: string) {
  try {
    localStorage.setItem(key, value);
  } catch {
    // Not remembered this time.
  }
}

function saveSort() {
  try {
    localStorage.setItem(SORT_STORAGE, JSON.stringify(state.sort));
  } catch {
    // Not remembered this time.
  }
}

// ---------- Modals ----------

function setLoginStatus(message: string, type: "info" | "error" = "info") {
  const el = $("#login-status");
  el.textContent = message;
  el.className = `login-status ${type === "error" ? "error" : ""}`;
}

function showLoginModal() {
  $("#login-modal").classList.remove("hidden");
  setLoginStatus("");
}

function hideLoginModal() {
  $("#login-modal").classList.add("hidden");
}

function showSettings() {
  populateSettings();
  $("#settings-modal").classList.remove("hidden");
}

function hideSettings() {
  $("#settings-modal").classList.add("hidden");
}

function anyModalOpen(): boolean {
  return [...document.querySelectorAll(".modal")].some((m) => !m.classList.contains("hidden"));
}

const FOCUSABLE =
  'button:not([disabled]), [href], input:not([disabled]), select:not([disabled]), textarea:not([disabled]), [tabindex]:not([tabindex="-1"])';

function visibleFocusables(root: Element): HTMLElement[] {
  return [...root.querySelectorAll<HTMLElement>(FOCUSABLE)].filter(
    (el) => el.offsetParent !== null || el === document.activeElement,
  );
}

/**
 * Dialogs for keyboard and screen-reader users: focus moves into a dialog
 * when it opens, Tab stays inside it, and focus goes back where it was when
 * it closes. Works for every `.modal`, however it's shown or hidden.
 */
function manageDialogFocus() {
  const returnTo = new Map<Element, HTMLElement | null>();
  const observer = new MutationObserver((records) => {
    for (const record of records) {
      const modal = record.target as HTMLElement;
      const open = !modal.classList.contains("hidden");
      if (open && !returnTo.has(modal)) {
        returnTo.set(modal, document.activeElement as HTMLElement | null);
        // After the code that opened it has filled it in.
        window.setTimeout(() => {
          if (modal.contains(document.activeElement)) return;
          const dialog =
            modal.getAttribute("role") === "dialog"
              ? modal
              : (modal.querySelector<HTMLElement>("[role=dialog]") ?? modal);
          const first =
            dialog.querySelector<HTMLElement>("input:not([type=checkbox]):not([disabled])") ??
            visibleFocusables(dialog)[0];
          first?.focus();
          // Nothing to focus yet (still loading, or hidden): the dialog.
          if (!dialog.contains(document.activeElement)) dialog.focus();
        }, 0);
      } else if (!open && returnTo.has(modal)) {
        const previous = returnTo.get(modal);
        returnTo.delete(modal);
        if (previous && document.contains(previous)) previous.focus();
      }
    }
  });
  for (const modal of document.querySelectorAll(".modal")) {
    observer.observe(modal, { attributes: true, attributeFilter: ["class"] });
  }
  document.addEventListener("keydown", (event) => {
    if (event.key !== "Tab") return;
    const modal = [...document.querySelectorAll<HTMLElement>(".modal")].find(
      (m) => !m.classList.contains("hidden"),
    );
    if (!modal) return;
    const items = visibleFocusables(modal);
    if (!items.length) return;
    const first = items[0];
    const last = items[items.length - 1];
    if (event.shiftKey && (document.activeElement === first || !modal.contains(document.activeElement))) {
      event.preventDefault();
      last.focus();
    } else if (!event.shiftKey && (document.activeElement === last || !modal.contains(document.activeElement))) {
      event.preventDefault();
      first.focus();
    }
  });
}

// ---------- Top bar ----------

function renderSession() {
  const badge = $("#session-badge");
  const text = $("#session-text");
  if (state.session?.logged_in) {
    badge.classList.add("online");
    badge.classList.remove("offline");
    text.textContent = "Conectado";
    badge.title = [
      state.session.phone,
      state.session.storage ? storageLabel(state.session.storage) : null,
    ]
      .filter(Boolean)
      .join(" · ");
  } else {
    badge.classList.add("offline");
    badge.classList.remove("online");
    text.textContent = "Sin sesión";
    badge.title = "Pulsa para iniciar sesión";
  }
}

function storageLabel(storage: StorageSpace): string {
  return storage.nolimit
    ? `${formatSize(storage.used)} usados`
    : `${formatSize(storage.used)} de ${formatSize(storage.quota)}`;
}

async function refreshQuota() {
  const text = $("#quota-text");
  const bar = $("#quota-bar");
  try {
    const quota = await invoke<StorageSpace>("get_quota");
    text.textContent = storageLabel(quota);
    const limited = !quota.nolimit && quota.quota > 0;
    bar.classList.toggle("hidden", !limited);
    if (limited) {
      $("#quota-fill").style.width = `${Math.min(100, (quota.used / quota.quota) * 100)}%`;
    }
    $("#quota").title = [
      quota.free ? `Libre: ${formatSize(quota.free)}` : "",
      quota.softdeleted ? `En la papelera: ${formatSize(quota.softdeleted)}` : "",
    ]
      .filter(Boolean)
      .join(" · ");
  } catch {
    text.textContent = "—";
    bar.classList.add("hidden");
  }
}

// ---------- Sidebar and path bar ----------

type Place = { icon: string; label: string; stack: Crumb[]; active: boolean };

/** The sidebar: where files are, then what O2 groups by itself. */
function renderPlaces() {
  const top = state.stack[1];
  const places: Place[] = [
    { icon: "☁", label: "O2 Cloud", stack: [ROOT], active: state.stack.length === 1 },
  ];
  if (state.settings.group_camera) {
    places.push({
      icon: "📷",
      label: "Cámara",
      stack: [ROOT, CAMERA],
      active: top?.virtual?.startsWith("camera") ?? false,
    });
  }
  // A label opened from one of these keeps its place highlighted.
  const grouped = (items: (readonly [string, Crumb])[]): Place[] =>
    items.map(([icon, crumb]) => ({
      icon,
      label: crumb.name,
      stack: [ROOT, crumb],
      active: state.stack.some((c) => c.virtual === crumb.virtual),
    }));
  for (const folder of state.rootFolders) {
    places.push({
      icon: "📁",
      label: folder.name,
      stack: [ROOT, { id: folder.id, name: folder.name, path: joinPath("/", folder.name) }],
      active: top?.id === folder.id,
    });
  }

  // While in a grouped view, "O2 Cloud" is not where you are.
  if (state.stack.some((c) => c.virtual?.startsWith("label") || c.virtual?.startsWith("media:"))) {
    places[0].active = false;
  }
  fillPlaces($("#places"), places);
  fillPlaces(
    $("#memories"),
    grouped([
      ["✨", MOMENTS],
      ["📍", PLACES],
      ["👤", PEOPLE],
    ]),
  );
  fillPlaces(
    $("#library"),
    grouped([
      ["🎵", MUSIC],
      ["📄", DOCUMENTS],
    ]),
  );
}

function fillPlaces(nav: HTMLElement, places: Place[]) {
  nav.replaceChildren();
  for (const place of places) {
    const button = document.createElement("button");
    button.className = `place${place.active ? " active" : ""}`;
    if (place.active) button.setAttribute("aria-current", "page");
    button.title = place.label;
    const icon = document.createElement("span");
    icon.className = "place-icon";
    icon.textContent = place.icon;
    const label = document.createElement("span");
    label.className = "place-label";
    label.textContent = place.label;
    button.append(icon, label);
    button.addEventListener("click", () => navigate(place.stack));
    nav.appendChild(button);
  }
}

function renderPathbar() {
  const nav = $("#breadcrumbs");
  nav.replaceChildren();
  // Recuerdos and Biblioteca are sections of their own, not folders of
  // O2 Cloud: their path starts with them.
  const section = state.stack[1]?.virtual ?? "";
  const first = section.startsWith("labels:") || section.startsWith("media:") ? 1 : 0;
  state.stack.forEach((crumb, index) => {
    if (index < first) return;
    if (index > first) {
      const sep = document.createElement("span");
      sep.className = "crumb-sep";
      sep.textContent = "›";
      nav.appendChild(sep);
    }
    const button = document.createElement("button");
    button.className = "crumb";
    button.textContent = crumb.name;
    button.title = crumb.name;
    if (index === state.stack.length - 1) button.classList.add("current");
    button.addEventListener("click", () => navigate(state.stack.slice(0, index + 1)));
    nav.appendChild(button);
  });
  ($("#back-btn") as HTMLButtonElement).disabled = state.stack.length === 1;
  const crumb = currentCrumb();
  const kinds: Record<string, string> = { "media:music": "Música", "media:documents": "Documentos" };
  $("#drop-text").textContent = kinds[crumb.virtual ?? ""]
    ? `Suelta para subir a O2 Cloud: aparecerá en ${kinds[crumb.virtual!]}`
    : groupedView() === "labels"
      ? "Aquí no se puede subir"
      : `Suelta para subir a ${crumb.virtual ? "O2 Cloud" : crumb.name}`;
}

function navigate(stack: Crumb[]) {
  state.stack = stack;
  state.selected.clear();
  state.anchor = null;
  state.cursor = null;
  state.filter = "";
  ($("#search") as HTMLInputElement).value = "";
  hideMenu();
  void refresh();
}

function goUp() {
  if (state.stack.length > 1) navigate(state.stack.slice(0, -1));
}

function openFolder(folder: RemoteFolder) {
  const crumb = currentCrumb();
  navigate([
    ...state.stack,
    { id: folder.id, name: folder.name, path: joinPath(crumb.path, folder.name) },
  ]);
}

function openVirtualFolder(folder: VirtualFolder) {
  navigate([
    ...state.stack,
    { id: null, name: folderTitle(folder), path: currentCrumb().path, virtual: folder.key },
  ]);
}

/** A moment's name with its day as people read it ("… · 26 nov 2026"). */
function folderTitle(folder: VirtualFolder): string {
  if (folder.kind === "music-folder") return lastSegment(folder.name);
  const day = folder.date?.slice(0, 10);
  if (!day || !/^\d{4}-\d{2}-\d{2}$/.test(day)) return folder.name;
  return `${folder.name} · ${new Date(`${day}T12:00:00`).toLocaleDateString("es-ES", { dateStyle: "medium" })}`;
}

// ---------- Entries: sorting and filtering ----------

const collator = new Intl.Collator("es", { numeric: true, sensitivity: "base" });

function normalize(text: string): string {
  return text.normalize("NFD").replace(/\p{Diacritic}/gu, "").toLowerCase();
}

function sizeOf(entry: Entry): number {
  return entry.kind === "file" ? entry.file.size : entry.folder.size;
}

function dateOf(entry: Entry): number | null {
  return entry.kind === "file" ? timeOf(entry.file.date) : null;
}

/** By the chosen column; unknown dates last; then by name. */
function compareEntries(a: Entry, b: Entry): number {
  const { key, dir } = state.sort;
  if (key === "size") {
    const diff = sizeOf(a) - sizeOf(b);
    if (diff) return diff * dir;
  } else if (key === "date") {
    const ta = dateOf(a);
    const tb = dateOf(b);
    if (ta !== tb) {
      if (ta == null) return 1;
      if (tb == null) return -1;
      return (ta - tb) * dir;
    }
  }
  return collator.compare(a.name, b.name) * (key === "name" ? dir : 1);
}

function fileKey(file: LogicalFile): string {
  if (file.normal_id != null) return `file:${file.normal_id}`;
  return `${file.cli_split ? "cli" : "chunked"}:${file.name}`;
}

let entriesCache = {
  listing: null as LogicalListing | null,
  signature: "",
  entries: [] as Entry[],
};

/** Virtual folders in their own order, then folders, then files. */
function visibleEntries(): Entry[] {
  const listing = state.listing;
  if (!listing) return [];
  const signature = `${state.sort.key}:${state.sort.dir}:${state.filter}:${musicView}`;
  if (entriesCache.listing === listing && entriesCache.signature === signature) {
    return entriesCache.entries;
  }

  const needle = normalize(state.filter.trim());
  const matches = (name: string) => !needle || normalize(name).includes(needle);
  const virtual: Entry[] = (listing.virtual_folders ?? [])
    .filter((v) => matches(v.name))
    .map((folder): Entry => ({
      kind: "virtual",
      key: `virtual:${folder.key}`,
      // A folder of Música by its own name; its path is shown beside it.
      name: folder.kind === "music-folder" ? lastSegment(folder.name) : folder.name,
      folder,
    }));
  const folders: Entry[] = listing.folders
    .filter((f) => matches(f.name))
    .map((folder): Entry => ({ kind: "folder", key: `folder:${folder.id}`, name: folder.name, folder }))
    .sort(compareEntries);
  const files: Entry[] = listing.files
    .filter((f) => matches(f.name))
    .map((file): Entry => ({ kind: "file", key: fileKey(file), name: file.name, file }))
    .sort(compareEntries);

  let entries = [...virtual, ...folders, ...files];
  // Música shows its songs or its folders, not both.
  if (currentCrumb().virtual === "media:music") {
    entries = musicView === "folders" ? virtual.sort(compareEntries) : files;
  }
  entriesCache = { listing, signature, entries };
  return entries;
}

function selectableKeys(): string[] {
  return visibleEntries()
    .filter((entry) => entry.kind !== "virtual")
    .map((entry) => entry.key);
}

function selectedEntries(): (FolderEntry | FileEntry)[] {
  return visibleEntries().filter(
    (entry): entry is FolderEntry | FileEntry =>
      entry.kind !== "virtual" && state.selected.has(entry.key),
  );
}

// ---------- File list ----------

const rowByKey = new Map<string, HTMLTableRowElement>();

const SVG_NS = "http://www.w3.org/2000/svg";
const ICON_PATHS = {
  download: "M12 4v11m0 0-4-4m4 4 4-4M5 19h14",
  trash: "M4 7h16M10 11v6m4-6v6M9 7V5h6v2M6 7l1 12h10l1-12",
  play: "M8 5.5v13l10.5-6.5z",
};

function svgIcon(name: keyof typeof ICON_PATHS): SVGSVGElement {
  const svg = document.createElementNS(SVG_NS, "svg");
  svg.setAttribute("viewBox", "0 0 24 24");
  svg.setAttribute("fill", "none");
  svg.setAttribute("stroke", "currentColor");
  svg.setAttribute("stroke-width", "1.8");
  svg.setAttribute("stroke-linecap", "round");
  svg.setAttribute("stroke-linejoin", "round");
  svg.setAttribute("aria-hidden", "true");
  const path = document.createElementNS(SVG_NS, "path");
  path.setAttribute("d", ICON_PATHS[name]);
  svg.appendChild(path);
  return svg;
}

const VIDEO_NAME = /\.(mov|mp4|m4v|3gp|3g2|avi|mkv|webm|mts|m2ts|mpe?g|wmv|flv)$/i;
const AUDIO_NAME = /\.(mp3|m4a|flac|wav|aac|ogg|opus|aiff?)$/i;
const IMAGE_NAME = /\.(jpe?g|png|heic|heif|gif|webp|tiff?|bmp|dng|raw|cr[23]|nef|arw|avif)$/i;
const PDF_NAME = /\.pdf$/i;

/** A song the player may stream (whole, not in parts). */
function isAudio(file: LogicalFile): boolean {
  return AUDIO_NAME.test(file.name) && !file.chunked;
}

/** The last part of a path: `musica/Queen/Live` → `Live`. */
function lastSegment(path: string): string {
  return path.split("/").filter(Boolean).pop() ?? path;
}

const FILE_ICONS: [RegExp, string][] = [
  [IMAGE_NAME, "🖼️"],
  [VIDEO_NAME, "🎬"],
  [AUDIO_NAME, "🎵"],
  [/\.(zip|rar|7z|tar|gz|tgz|bz2|xz|dmg|iso)$/i, "🗜️"],
  [/\.pdf$/i, "📕"],
];

function entryIcon(entry: Entry): string {
  if (entry.kind === "virtual") {
    if (entry.folder.kind === "music-folder") return "💿";
    return entry.folder.kind === "camera" ? "📷" : entry.folder.kind === "year" ? "🗓️" : "🗂️";
  }
  if (entry.kind === "folder") return "📁";
  if (entry.file.chunked) return "🧩";
  return FILE_ICONS.find(([pattern]) => pattern.test(entry.name))?.[1] ?? "📄";
}

function makeAction(
  icon: keyof typeof ICON_PATHS,
  title: string,
  onClick: () => void,
  danger = false,
): HTMLButtonElement {
  const button = document.createElement("button");
  button.className = `row-action${danger ? " danger" : ""}`;
  button.title = title;
  button.setAttribute("aria-label", title);
  button.appendChild(svgIcon(icon));
  button.addEventListener("click", (event) => {
    event.stopPropagation();
    onClick();
  });
  button.addEventListener("dblclick", (event) => event.stopPropagation());
  return button;
}

function cell(className: string): HTMLTableCellElement {
  const td = document.createElement("td");
  td.className = className;
  return td;
}

function renderRow(entry: Entry): HTMLTableRowElement {
  const tr = document.createElement("tr");
  tr.className = `row ${entry.kind}-row`;
  tr.dataset.key = entry.key;
  tr.classList.toggle("selected", state.selected.has(entry.key));
  tr.classList.toggle("trashing", state.trashing.has(entry.key));
  tr.addEventListener("click", (event) => onRowClick(event, entry));
  tr.addEventListener("dblclick", () => openEntry(entry));
  tr.addEventListener("contextmenu", (event) => onRowContextMenu(event, entry));

  const check = cell("col-check");
  if (entry.kind !== "virtual") {
    const box = document.createElement("input");
    box.type = "checkbox";
    box.checked = state.selected.has(entry.key);
    box.setAttribute("aria-label", `Seleccionar ${entry.name}`);
    box.addEventListener("click", (event) => {
      event.stopPropagation();
      toggleSelected(entry.key);
      state.anchor = entry.key;
      state.cursor = entry.key;
      updateSelection();
    });
    box.addEventListener("dblclick", (event) => event.stopPropagation());
    check.appendChild(box);
  }

  const nameCell = cell("col-name");
  const wrap = document.createElement("div");
  wrap.className = "name-wrap";
  const icon = document.createElement("span");
  icon.className = "row-icon";
  icon.textContent = entryIcon(entry);
  if (entry.kind === "file" && previewable(entry.file)) watchRowThumbnail(tr, icon, entry.file);
  const name = document.createElement("span");
  name.className = "name-text";
  name.textContent = entry.name;
  name.title = entry.kind === "virtual" ? entry.folder.name : entry.name;
  wrap.append(icon, name);
  const musicFolderRow = entry.kind === "virtual" && entry.folder.kind === "music-folder";
  if (musicFolderRow) {
    // «12 canciones · musica/Queen»: where it is, under its own name.
    const meta = document.createElement("span");
    meta.className = "name-meta";
    const parent = entry.folder.name.split("/").slice(0, -1).join("/");
    meta.textContent = plural(entry.folder.files, "canción", "canciones") + (parent ? ` · ${parent}` : "");
    wrap.appendChild(meta);
  } else if (entry.kind === "virtual" || (entry.kind === "folder" && entry.folder.files > 0)) {
    const meta = document.createElement("span");
    meta.className = "name-meta";
    meta.textContent = plural(entry.folder.files, "archivo", "archivos");
    wrap.appendChild(meta);
  }
  if (entry.kind === "virtual" && !musicFolderRow) {
    const pill = document.createElement("span");
    pill.className = "pill";
    if (entry.folder.kind.startsWith("label")) {
      pill.textContent = "O2";
      pill.title = "Agrupado por O2 Cloud: las fotos siguen en sus carpetas";
    } else {
      pill.textContent = "virtual";
      pill.title = "Vista de la app: no mueve nada en O2";
    }
    wrap.appendChild(pill);
  }
  if (entry.kind === "file" && entry.file.chunked) {
    const pill = document.createElement("span");
    pill.className = `pill${entry.file.incomplete ? " warning" : ""}`;
    pill.textContent = entry.file.incomplete ? "incompleto" : "trozos";
    pill.title = entry.file.incomplete
      ? "Faltan el manifiesto o algún fragmento"
      : entry.file.cli_split
        ? "Archivo dividido por el CLI en partes .001; la app lo descarga unido"
        : "Archivo dividido automáticamente para superar el límite de 4 GB";
    wrap.appendChild(pill);
  }
  nameCell.appendChild(wrap);

  const sizeCell = cell("col-size");
  sizeCell.textContent = sizeOf(entry) ? formatSize(sizeOf(entry)) : "—";

  const dateCell = cell("col-date");
  dateCell.textContent = entry.kind === "file" ? formatDate(entry.file.date) : "";

  const actionsCell = cell("col-actions");
  const actions = document.createElement("div");
  actions.className = "row-actions";
  if (musicFolderRow) {
    const folder = entry.folder.key.slice(MUSIC_FOLDER.length);
    actions.appendChild(
      makeAction("play", `Reproducir ${entry.name}`, () =>
        void playSource({ kind: "music-folder", folder }),
      ),
    );
  }
  if (entry.kind === "file") {
    if (inMusic() || isAudio(entry.file)) {
      const file = entry.file;
      actions.appendChild(makeAction("play", "Reproducir", () => void playFile(file)));
    }
    actions.appendChild(makeAction("download", "Descargar", () => void downloadFiles([entry.file])));
  }
  if (entry.kind !== "virtual") {
    actions.appendChild(
      makeAction("trash", "Mover a la papelera", () => void trashEntries([entry]), true),
    );
  }
  actionsCell.appendChild(actions);

  tr.append(check, nameCell, sizeCell, dateCell, actionsCell);
  return tr;
}

function renderWarnings() {
  const box = $("#warnings");
  const warnings = state.listing?.warnings ?? [];
  box.replaceChildren();
  box.classList.toggle("hidden", !warnings.length);
  if (!warnings.length) return;
  const title = document.createElement("strong");
  title.textContent = "Avisos";
  const ul = document.createElement("ul");
  for (const warning of warnings) {
    const li = document.createElement("li");
    li.textContent = warning;
    ul.appendChild(li);
  }
  box.append(title, ul);
}

function describeListing(entries: Entry[], shown: number): string {
  const listing = state.listing;
  if (!listing) return state.loading ? "Cargando…" : "";
  const folders = entries.filter((e) => e.kind !== "file").length;
  const files = entries.length - folders;
  const view = currentCrumb().virtual ?? "";
  const nouns: Record<string, [string, string]> = {
    "labels:moments": ["momento", "momentos"],
    "labels:places": ["lugar", "lugares"],
    "labels:people": ["persona", "personas"],
  };
  const [one, many] = nouns[view] ?? ["carpeta", "carpetas"];
  const parts = [
    folders ? plural(folders, one, many) : "",
    files || !folders ? plural(files, "archivo", "archivos") : "",
  ].filter(Boolean);
  if (state.filter.trim()) {
    const total =
      (listing.virtual_folders?.length ?? 0) + listing.folders.length + listing.files.length;
    parts.push(`de ${total.toLocaleString("es-ES")}`);
  }
  if (shown < entries.length) parts.push(`mostrando ${shown.toLocaleString("es-ES")}`);
  if (view && !nouns[view] && !view.startsWith("label") && !view.startsWith("media:")) {
    parts.push("vista virtual");
  }
  return parts.join(" · ");
}

function renderSortHeaders() {
  // Virtual folders keep their own order (years newest first).
  const sortable = !!state.listing && (state.listing.folders.length > 0 || state.listing.files.length > 0);
  for (const button of document.querySelectorAll<HTMLButtonElement>(".sort")) {
    const active = sortable && button.dataset.sort === state.sort.key;
    button.classList.toggle("asc", active && state.sort.dir === 1);
    button.classList.toggle("desc", active && state.sort.dir === -1);
  }
}

function renderListing() {
  const listing = state.listing;
  const tbody = $("#file-list");
  tbody.replaceChildren();
  rowByKey.clear();
  // The rows go: so do their pending thumbnails.
  rowThumbnailObserver?.disconnect();

  $("#loading-state").classList.toggle("hidden", !(state.loading && !listing));
  $("#refresh-line").classList.toggle("hidden", !(state.loading && listing));
  renderSortHeaders();

  const entries = visibleEntries();
  const shown = entries.slice(0, state.visibleLimit);
  for (const entry of shown) {
    const tr = renderRow(entry);
    rowByKey.set(entry.key, tr);
    tbody.appendChild(tr);
  }

  const empty = !!listing && !entries.length && !state.loading;
  $("#empty-state").classList.toggle("hidden", !empty);
  if (empty) {
    const filtering = state.filter.trim();
    const [title, hint] = emptyTexts(currentCrumb().virtual ?? "");
    $("#empty-title").textContent = filtering ? `Nada coincide con «${filtering}».` : title;
    $("#empty-hint").textContent = filtering ? "Prueba con otro nombre." : hint;
  }
  $("#load-more-wrap").classList.toggle("hidden", shown.length >= entries.length);
  $("#folder-subtitle").textContent = describeListing(entries, shown.length);
  updateSelection();
  renderLabelViews();
}

/** What an empty view says: its title and a hint. */
function emptyTexts(view: string): [string, string] {
  switch (view) {
    case "labels:moments":
      return ["O2 no ha creado momentos todavía.", "Los crea a partir de las fechas de tus fotos."];
    case "labels:places":
      return ["Ningún lugar todavía.", "O2 agrupa las fotos que tienen ubicación GPS."];
    case "labels:people":
      return ["Ninguna persona.", "O2 las agrupa si activas el reconocimiento facial en su web."];
    case "media:music":
      return ["No hay música en O2 Cloud.", "Usa «Subir»: aparecerá aquí en unos segundos."];
    case "media:documents":
      return ["No hay documentos en O2 Cloud.", "Usa «Subir»: aparecerán aquí en unos segundos."];
    default:
      if (view.startsWith(MUSIC_FOLDER)) {
        return ["No hay canciones en esta carpeta.", "Puede que se hayan movido o borrado."];
      }
      return view.startsWith("label:")
        ? ["No hay fotos aquí.", "O2 puede haberlas quitado de este grupo."]
        : ["Esta carpeta está vacía.", "Arrastra archivos aquí o usa «Subir»."];
  }
}

// ---------- Previews: thumbnails in the rows, and a viewer ----------

/** What `o2cloud thumbnails` says of a file: signed links to a thumbnail
 * (about 320 px) and a preview (up to 1440 px), or null. */
interface FileThumbnail {
  id: number;
  type: string | null;
  thumbnail: string | null;
  preview: string | null;
}

/** Thumbnails asked for, by file ID (null: O2 has none). The links are
 * signed: kept for the session, asked again when one stops opening. */
const thumbnails = new Map<number, FileThumbnail | null>();
const thumbnailWaiters = new Map<number, ((thumbnail: FileThumbnail | null) => void)[]>();
let thumbnailQueue: number[] = [];
let thumbnailTimer = 0;
let rowThumbnailObserver: IntersectionObserver | null = null;

/** Whether O2 may have a picture of `file`: photos, videos and PDFs, whole. */
function previewable(file: LogicalFile): boolean {
  return (
    !file.chunked &&
    file.normal_id !== null &&
    (IMAGE_NAME.test(file.name) || VIDEO_NAME.test(file.name) || PDF_NAME.test(file.name))
  );
}

/** The thumbnail and preview of file `id`, asked for together with the
 * other rows that come into view at the same moment. */
function fileThumbnail(id: number): Promise<FileThumbnail | null> {
  if (thumbnails.has(id)) return Promise.resolve(thumbnails.get(id) ?? null);
  return new Promise((resolve) => {
    const waiting = thumbnailWaiters.get(id);
    if (waiting) {
      waiting.push(resolve);
      return;
    }
    thumbnailWaiters.set(id, [resolve]);
    thumbnailQueue.push(id);
    if (!thumbnailTimer) thumbnailTimer = window.setTimeout(() => void askThumbnails(), 80);
  });
}

async function askThumbnails() {
  thumbnailTimer = 0;
  const ids = thumbnailQueue.splice(0, 100);
  if (thumbnailQueue.length) thumbnailTimer = window.setTimeout(() => void askThumbnails(), 0);
  let found: FileThumbnail[] | null = null;
  try {
    found = await invoke<FileThumbnail[]>("file_thumbnails", { ids });
  } catch {
    // Offline or the session ended: the icons stay, and they're asked
    // for again when the rows are drawn again.
  }
  const byId = new Map((found ?? []).map((thumbnail) => [thumbnail.id, thumbnail]));
  for (const id of ids) {
    const thumbnail = byId.get(id) ?? null;
    // Left out by O2 (still processing a fresh upload): not remembered.
    if (byId.has(id)) thumbnails.set(id, thumbnail);
    const waiting = thumbnailWaiters.get(id) ?? [];
    thumbnailWaiters.delete(id);
    for (const resolve of waiting) resolve(thumbnail);
  }
}

/** Put the file's thumbnail in place of its icon once the row is in view. */
function watchRowThumbnail(row: HTMLTableRowElement, icon: HTMLElement, file: LogicalFile) {
  const id = file.normal_id!;
  if (thumbnails.has(id)) {
    showRowThumbnail(icon, thumbnails.get(id) ?? null, id);
    return;
  }
  rowThumbnailObserver ??= new IntersectionObserver(
    (entries) => {
      for (const entry of entries) {
        if (!entry.isIntersecting) continue;
        const target = entry.target as HTMLElement;
        rowThumbnailObserver?.unobserve(target);
        const rowIcon = target.querySelector<HTMLElement>(".row-icon");
        const rowId = Number(target.dataset.thumbnailId);
        if (rowIcon) void fileThumbnail(rowId).then((t) => showRowThumbnail(rowIcon, t, rowId));
      }
    },
    { root: $("#table-wrap"), rootMargin: "300px 0px" },
  );
  row.dataset.thumbnailId = String(id);
  rowThumbnailObserver.observe(row);
}

function showRowThumbnail(icon: HTMLElement, thumbnail: FileThumbnail | null, id: number) {
  if (!thumbnail?.thumbnail || !icon.isConnected) return;
  const image = new Image();
  image.className = "row-thumb";
  image.alt = "";
  image.decoding = "async";
  image.onload = () => {
    if (!icon.isConnected) return;
    icon.replaceChildren(image);
    icon.classList.add("has-thumb");
  };
  // An expired link: asked for again the next time the row is drawn.
  image.onerror = () => thumbnails.delete(id);
  image.src = thumbnail.thumbnail;
}

const viewer = {
  files: [] as LogicalFile[],
  index: 0,
  opener: null as HTMLElement | null,
  /** Bumped on every change: a late answer for another file is ignored. */
  generation: 0,
  retried: false,
};

function viewerOpen(): boolean {
  return !$("#viewer").classList.contains("hidden");
}

/** Open the viewer on `file`, going through the photos, videos and PDFs of
 * the list as it is on screen. */
function openViewer(file: LogicalFile) {
  viewer.files = visibleEntries().flatMap((entry) =>
    entry.kind === "file" && previewable(entry.file) ? [entry.file] : [],
  );
  viewer.index = viewer.files.findIndex((f) => f.normal_id === file.normal_id);
  if (viewer.index < 0) {
    viewer.files = [file];
    viewer.index = 0;
  }
  viewer.opener = document.activeElement as HTMLElement | null;
  hideMenu();
  $("#viewer").classList.remove("hidden");
  ($("#viewer-close") as HTMLElement).focus();
  void showViewerFile();
}

function closeViewer() {
  viewer.generation++;
  $("#viewer").classList.add("hidden");
  ($("#viewer-image") as HTMLImageElement).removeAttribute("src");
  const opener = viewer.opener;
  if (opener && document.contains(opener)) opener.focus();
}

async function showViewerFile() {
  const generation = ++viewer.generation;
  const file = viewer.files[viewer.index];
  const id = file.normal_id!;
  const image = $("#viewer-image") as HTMLImageElement;
  const empty = $("#viewer-empty");
  $("#viewer-title").textContent = file.name;
  $("#viewer-count").textContent =
    viewer.files.length > 1 ? `${viewer.index + 1} de ${viewer.files.length}` : "";
  $("#viewer-note").textContent = VIDEO_NAME.test(file.name)
    ? "Es un vídeo: aquí se ve un fotograma. Descárgalo para verlo entero."
    : PDF_NAME.test(file.name)
      ? "La primera página del PDF. Descárgalo para leerlo entero."
      : "";
  for (const button of ["#viewer-prev", "#viewer-next"]) {
    $(button).classList.toggle("hidden", viewer.files.length < 2);
  }
  image.classList.add("hidden");
  empty.textContent = "Cargando la vista previa…";
  empty.classList.remove("hidden");
  viewer.retried = false;

  const thumbnail = await fileThumbnail(id);
  if (generation !== viewer.generation) return;
  const url = thumbnail?.preview ?? thumbnail?.thumbnail;
  if (!url) {
    empty.textContent = "O2 no tiene vista previa de este archivo (o aún lo está procesando).";
    return;
  }
  image.onload = () => {
    if (generation !== viewer.generation) return;
    empty.classList.add("hidden");
    image.classList.remove("hidden");
  };
  image.onerror = () => {
    if (generation !== viewer.generation) return;
    // The link may have expired: ask for a new one, once.
    thumbnails.delete(id);
    if (!viewer.retried) {
      viewer.retried = true;
      void fileThumbnail(id).then((fresh) => {
        const again = fresh?.preview ?? fresh?.thumbnail;
        if (generation === viewer.generation && again && again !== url) image.src = again;
        else if (generation === viewer.generation) empty.textContent = "No se ha podido cargar la vista previa.";
      });
    } else {
      empty.textContent = "No se ha podido cargar la vista previa.";
    }
  };
  image.alt = file.name;
  image.src = url;
  // The neighbours' links, so the next one opens at once.
  for (const step of [1, -1]) {
    const next = viewer.files[viewer.index + step];
    if (next?.normal_id != null) void fileThumbnail(next.normal_id);
  }
}

function viewerStep(step: number) {
  if (viewer.files.length < 2) return;
  viewer.index = (viewer.index + step + viewer.files.length) % viewer.files.length;
  void showViewerFile();
}

/** Keys while the viewer is open. */
function viewerKey(event: KeyboardEvent) {
  switch (event.key) {
    case "Escape":
    case " ":
      event.preventDefault();
      closeViewer();
      return;
    case "ArrowRight":
      event.preventDefault();
      viewerStep(1);
      return;
    case "ArrowLeft":
      event.preventDefault();
      viewerStep(-1);
      return;
    case "Tab": {
      // Focus stays in the viewer.
      const buttons = [...document.querySelectorAll<HTMLElement>("#viewer button")].filter(
        (button) => !button.classList.contains("hidden"),
      );
      const at = buttons.indexOf(document.activeElement as HTMLElement);
      const next = event.shiftKey
        ? at <= 0
          ? buttons.length - 1
          : at - 1
        : (at + 1) % buttons.length;
      buttons[next]?.focus();
      event.preventDefault();
      return;
    }
    case "Enter":
      // A focused button presses itself.
      return;
    default:
      if (!event.metaKey && !event.ctrlKey) event.preventDefault();
  }
}

function bindViewer() {
  $("#viewer-close").addEventListener("click", closeViewer);
  $("#viewer-prev").addEventListener("click", () => viewerStep(-1));
  $("#viewer-next").addEventListener("click", () => viewerStep(1));
  $("#viewer-download").addEventListener("click", () => {
    const file = viewer.files[viewer.index];
    closeViewer();
    if (file) void downloadFiles([file]);
  });
  // A click on the dark area around the picture closes it.
  $("#viewer-stage").addEventListener("click", (event) => {
    if (event.target === event.currentTarget) closeViewer();
  });
}

// ---------- Moments, places and their slideshow ----------

/** How places are shown: on the map (by default) or as a list; remembered. */
let placesView: "list" | "map" = loadChoice(PLACES_VIEW_STORAGE, ["list", "map"] as const, "map");
/** How Música is shown: every song, or its folders; remembered. */
let musicView: "songs" | "folders" = loadChoice(MUSIC_VIEW_STORAGE, ["songs", "folders"] as const, "songs");
let map: L.Map | null = null;
let markers: L.LayerGroup | null = null;
let fittedPlaces = "";
let mapHasView = false;

function labelFolders(): VirtualFolder[] {
  return (state.listing?.virtual_folders ?? []).filter((f) => f.kind.startsWith("label-"));
}

/** The cards of moments, the map of places, and the slideshow button. */
function renderLabelViews() {
  const view = currentCrumb().virtual ?? "";
  const moments = view === "labels:moments";
  const places = view === "labels:places";
  const showMap = places && placesView === "map";
  $("#view-toggle").classList.toggle("hidden", !places);
  for (const button of document.querySelectorAll<HTMLButtonElement>("#view-toggle button")) {
    button.setAttribute("aria-pressed", String(button.dataset.view === placesView));
  }
  // The slideshow shows photos: not offered where O2 grouped only videos
  // (a «Un día como hoy» can be just that).
  const files = state.listing?.files ?? [];
  const onlyVideos = files.length > 0 && files.every((f) => VIDEO_NAME.test(f.name));
  $("#slideshow-btn").classList.toggle("hidden", !view.startsWith("label:") || onlyVideos);
  $("#music-toggle").classList.toggle("hidden", view !== "media:music");
  for (const button of document.querySelectorAll<HTMLButtonElement>("#music-toggle button")) {
    button.setAttribute("aria-pressed", String(button.dataset.view === musicView));
  }
  const playable = inMusic()
    ? !!state.listing?.files.length
    : !!state.listing?.files.some(isAudio);
  $("#play-music-btn").classList.toggle("hidden", !playable);
  // Nothing goes into a moment or place (O2 makes them), and none of these
  // views is a folder to create one in. Música and Documentos take uploads:
  // they go to O2 Cloud's root, and O2 files them there by their kind.
  $("#upload-btn").classList.toggle(
    "hidden",
    view.startsWith("label") || view.startsWith(MUSIC_FOLDER),
  );
  $("#new-folder-btn").classList.toggle(
    "hidden",
    view.startsWith("label") || view.startsWith("media:"),
  );
  const special = moments || showMap;
  const filter = normalize(state.filter.trim());
  const shown = labelFolders().filter((f) => !filter || normalize(f.name).includes(filter));
  // Loading, or nothing to show: the table's place says so (its loading
  // and empty states), without the table.
  const statesOnly = special && (!state.listing || shown.length === 0);
  $("#table-wrap").classList.toggle("hidden", special && !statesOnly);
  document.querySelector(".file-table")?.classList.toggle("hidden", special);
  $("#cards").classList.toggle("hidden", !moments || statesOnly);
  $("#map-wrap").classList.toggle("hidden", !showMap || statesOnly);
  if (special) $("#load-more-wrap").classList.add("hidden");
  if (moments && !statesOnly) renderCards();
  if (showMap && !statesOnly) renderMap();
}

function renderCards() {
  const list = $("#cards");
  const filter = normalize(state.filter.trim());
  const folders = labelFolders().filter((f) => !filter || normalize(f.name).includes(filter));
  list.replaceChildren(
    ...folders.map((folder) => {
      const item = document.createElement("li");
      const button = document.createElement("button");
      button.className = "card";
      button.type = "button";
      const title = folder.name;
      const day = folder.date?.slice(0, 10);
      if (folder.thumbnail) {
        const img = document.createElement("img");
        img.src = folder.thumbnail;
        img.alt = "";
        img.loading = "lazy";
        img.referrerPolicy = "no-referrer";
        button.appendChild(img);
      }
      const text = document.createElement("span");
      text.className = "card-text";
      const strong = document.createElement("strong");
      strong.textContent = title;
      const meta = document.createElement("span");
      const date = day ? new Date(`${day}T12:00:00`).toLocaleDateString("es-ES", { dateStyle: "medium" }) : "";
      meta.textContent = [date, `${folder.files} ${folder.files === 1 ? "foto" : "fotos"}`]
        .filter(Boolean)
        .join(" · ");
      text.append(strong, meta);
      button.appendChild(text);
      button.addEventListener("click", () => openVirtualFolder(folder));
      item.appendChild(button);
      return item;
    }),
  );
}

function renderMap() {
  if (!map) {
    map = L.map("map", { worldCopyJump: true, keyboard: true });
    L.tileLayer("https://tile.openstreetmap.org/{z}/{x}/{y}.png", {
      maxZoom: 19,
      // Plain text: a link would load the site in the app's own window.
      attribution: "© colaboradores de OpenStreetMap",
    }).addTo(map);
    map.attributionControl.setPrefix(false);
    markers = L.layerGroup().addTo(map);
  }
  // The container was hidden: Leaflet measures it again.
  map.invalidateSize();
  markers!.clearLayers();
  const filter = normalize(state.filter.trim());
  const located = labelFolders().filter(
    (f) => f.location && (!filter || normalize(f.name).includes(filter)),
  );
  // The map needs a view before its markers get their elements. It is
  // fitted to the places only when they change, so a pan or zoom stays.
  const points = located.map((f) => L.latLng(f.location!.lat, f.location!.lon));
  const placesKey = located.map((f) => f.key).join(",");
  if (placesKey !== fittedPlaces || !mapHasView) {
    if (points.length) map.fitBounds(L.latLngBounds(points), { padding: [40, 40], maxZoom: 12 });
    else map.setView([40.4, -3.7], 5);
    fittedPlaces = placesKey;
    mapHasView = true;
  }
  for (const folder of located) {
    const at = L.latLng(folder.location!.lat, folder.location!.lon);
    const label = `${folder.name}: ${folder.files} ${folder.files === 1 ? "foto" : "fotos"}`;
    const count = folder.files > 999 ? "999+" : String(folder.files);
    // Wide enough for its digits: a circle up to 2, a pill beyond.
    const width = Math.max(28, 12 + count.length * 7);
    const icon = L.divIcon({
      className: "place-marker",
      html: `<span aria-hidden="true">${count}</span>`,
      iconSize: [width, 28],
      iconAnchor: [width / 2, 14],
    });
    const marker = L.marker(at, { icon, title: label, alt: label, keyboard: true });
    marker.on("click", () => openVirtualFolder(folder));
    marker.on("keypress", (event: L.LeafletKeyboardEvent) => {
      if (event.originalEvent.key === "Enter" || event.originalEvent.key === " ") openVirtualFolder(folder);
    });
    // Names come from O2: as text, never as HTML.
    const tip = document.createElement("span");
    tip.textContent = label;
    marker.bindTooltip(tip);
    markers!.addLayer(marker);
    // Leaflet's marker is a focusable image: name it as a link to the place.
    const element = marker.getElement();
    element?.setAttribute("role", "button");
    element?.setAttribute("aria-label", `Abrir ${label}`);
  }
}

function setPlacesView(view: "list" | "map") {
  placesView = view;
  saveChoice(PLACES_VIEW_STORAGE, view);
  renderLabelViews();
}

function setMusicView(view: "songs" | "folders") {
  musicView = view;
  saveChoice(MUSIC_VIEW_STORAGE, view);
  state.selected.clear();
  renderListing();
}

// The slideshow: the photos, one every few seconds, with music on a loop.
const SLIDE_SECONDS = 5;
const slideshow = {
  slides: [] as Slide[],
  index: 0,
  playing: true,
  timer: 0,
  front: "a" as "a" | "b",
  /** Where focus goes back to when it closes. */
  opener: null as HTMLElement | null,
  /** Bumped on every change of slide and on close: late image events of
   * an older one are ignored. */
  generation: 0,
  /** The label it shows, to ask for fresh links once if they expired. */
  label: "",
  refetched: false,
  /** The music player was playing when it opened: it goes on after. */
  resumePlayer: false,
  opening: false,
  audio: null as AudioContext | null,
  gain: null as GainNode | null,
  muted: false,
};
const musicBuffers = new Map<string, AudioBuffer>();

function reducedMotion(): boolean {
  return window.matchMedia("(prefers-reduced-motion: reduce)").matches;
}

async function openSlideshow() {
  if (slideshow.opening) return;
  const crumb = currentCrumb();
  const id = crumb.virtual?.replace(/^label:/, "");
  if (!id) return;
  const button = $("#slideshow-btn") as HTMLButtonElement;
  slideshow.opening = true;
  button.disabled = true;
  let slides: Slide[];
  try {
    slides = await invoke<Slide[]>("label_slides", { id });
  } catch (error) {
    toast(String(error), "error");
    return;
  } finally {
    slideshow.opening = false;
    button.disabled = false;
  }
  // Somewhere else meanwhile: not opened over it.
  if (currentCrumb() !== crumb) return;
  if (!slides.length) {
    toast("No hay fotos que mostrar aquí.", "info");
    return;
  }
  slideshow.slides = slides;
  slideshow.label = id;
  slideshow.refetched = false;
  slideshow.index = 0;
  for (const selector of ["#slide-a", "#slide-b"]) {
    const img = $(selector) as HTMLImageElement;
    img.onload = img.onerror = null;
    img.removeAttribute("src");
    img.classList.remove("shown");
  }
  slideshow.opener = document.activeElement as HTMLElement | null;
  $("#slideshow-title").textContent = crumb.name.split(" · ")[0] || "Pase de fotos";
  // One music at a time; the player goes on after.
  const audio = playerAudio();
  slideshow.resumePlayer = !audio.paused;
  audio.pause();
  $("#slideshow").classList.remove("hidden");
  ($("#slide-play") as HTMLElement).focus();
  // With reduced motion it waits for a press of play rather than moving
  // by itself.
  setSlideshowPlaying(!reducedMotion());
  showSlide(0);
  void startMusic();
}

function showSlide(index: number, announce = false) {
  const { slides } = slideshow;
  slideshow.index = (index + slides.length) % slides.length;
  const slide = slides[slideshow.index];
  const generation = ++slideshow.generation;
  window.clearTimeout(slideshow.timer);
  const next = slideshow.front === "a" ? "b" : "a";
  const incoming = $(`#slide-${next}`) as HTMLImageElement;
  const outgoing = $(`#slide-${slideshow.front}`) as HTMLImageElement;
  outgoing.onload = outgoing.onerror = null;
  incoming.classList.toggle("instant", reducedMotion());
  outgoing.classList.toggle("instant", reducedMotion());
  incoming.onload = () => {
    if (generation !== slideshow.generation) return;
    incoming.classList.add("shown");
    outgoing.classList.remove("shown");
    incoming.removeAttribute("aria-hidden");
    outgoing.setAttribute("aria-hidden", "true");
    // Each photo stays its full time once it's on screen.
    if (slideshow.playing) scheduleSlide(SLIDE_SECONDS);
  };
  incoming.onerror = () => {
    if (generation !== slideshow.generation) return;
    void slideFailed();
  };
  incoming.referrerPolicy = "no-referrer";
  incoming.alt = slide.name;
  incoming.src = slide.preview;
  slideshow.front = next;
  const when = slide.date
    ? new Date(slide.date).toLocaleString("es-ES", { dateStyle: "medium", timeStyle: "short" })
    : "";
  const caption = $("#slide-caption");
  // Read out when moved by hand, not every few seconds while it plays.
  caption.setAttribute("aria-live", announce ? "polite" : "off");
  caption.textContent = `Foto ${slideshow.index + 1} de ${slides.length}${when ? ` · ${when}` : ""}`;
  // The next one is loaded meanwhile.
  const following = slides[(slideshow.index + 1) % slides.length];
  if (following) new Image().src = following.preview;
}

/** A photo didn't load: its link may have expired. Fresh links are asked
 * for once; after that the slideshow stops and says so. */
async function slideFailed() {
  if (!slideshowOpen()) return;
  if (!slideshow.refetched) {
    slideshow.refetched = true;
    try {
      const fresh = await invoke<Slide[]>("label_slides", { id: slideshow.label });
      if (!slideshowOpen()) return;
      if (fresh.length) {
        slideshow.slides = fresh;
        showSlide(Math.min(slideshow.index, fresh.length - 1));
        return;
      }
    } catch {
      // Said below.
    }
  }
  setSlideshowPlaying(false);
  toast("No se pudieron cargar las fotos. Cierra el pase y vuelve a abrirlo.", "error");
}

function scheduleSlide(seconds: number) {
  window.clearTimeout(slideshow.timer);
  slideshow.timer = window.setTimeout(() => showSlide(slideshow.index + 1), seconds * 1000);
}

function setSlideshowPlaying(playing: boolean) {
  slideshow.playing = playing;
  const button = $("#slide-play");
  button.textContent = playing ? "❚❚" : "▶";
  button.setAttribute("aria-label", playing ? "Pausar" : "Seguir");
  if (playing) scheduleSlide(SLIDE_SECONDS);
  else window.clearTimeout(slideshow.timer);
  if (slideshow.audio) void (playing ? slideshow.audio.resume() : slideshow.audio.suspend());
}

/** The chosen track on a gapless loop, faded in. */
async function startMusic() {
  const track = state.settings.slideshow_music;
  if (!track || track === "none") return;
  try {
    const audio = new AudioContext();
    slideshow.audio = audio;
    let buffer = musicBuffers.get(track);
    if (!buffer) {
      const data = await (await fetch(`/music/${track}.mp3`)).arrayBuffer();
      buffer = await audio.decodeAudioData(data);
      musicBuffers.set(track, buffer);
    }
    if (slideshow.audio !== audio) return; // closed meanwhile
    const source = audio.createBufferSource();
    source.buffer = buffer;
    source.loop = true;
    const gain = audio.createGain();
    gain.gain.setValueAtTime(0, audio.currentTime);
    gain.gain.linearRampToValueAtTime(slideshow.muted ? 0 : 0.6, audio.currentTime + 2);
    source.connect(gain).connect(audio.destination);
    source.start();
    slideshow.gain = gain;
    if (!slideshow.playing) await audio.suspend();
  } catch {
    // Without music the slideshow goes on.
  }
}

function setMuted(muted: boolean) {
  slideshow.muted = muted;
  const button = $("#slide-mute");
  button.setAttribute("aria-pressed", String(muted));
  button.setAttribute("aria-label", muted ? "Activar música" : "Silenciar música");
  button.classList.toggle("off", muted);
  const { audio, gain } = slideshow;
  if (audio && gain) {
    // The fade-in may still be ramping: it would undo this.
    gain.gain.cancelScheduledValues(audio.currentTime);
    gain.gain.setTargetAtTime(muted ? 0 : 0.6, audio.currentTime, 0.1);
  }
}

function closeSlideshow() {
  window.clearTimeout(slideshow.timer);
  slideshow.playing = false;
  slideshow.generation++;
  for (const selector of ["#slide-a", "#slide-b"]) {
    const img = $(selector) as HTMLImageElement;
    img.onload = img.onerror = null;
    img.removeAttribute("src");
  }
  const audio = slideshow.audio;
  slideshow.audio = null;
  slideshow.gain = null;
  if (audio) void audio.close();
  $("#slideshow").classList.add("hidden");
  if (slideshow.resumePlayer && playerAudio().getAttribute("src")) {
    void playerAudio().play().catch(() => {});
  }
  slideshow.resumePlayer = false;
  // WebKit doesn't focus a clicked button: back to the one that opens it.
  const opener = slideshow.opener;
  (opener && opener !== document.body && document.contains(opener) ? opener : $("#slideshow-btn")).focus();
}

function slideshowOpen(): boolean {
  return !$("#slideshow").classList.contains("hidden");
}

/** Keys while the slideshow is open; true if it took the key. */
function slideshowKey(event: KeyboardEvent): boolean {
  if (!slideshowOpen()) return false;
  if (event.key === "Tab") {
    const buttons = [...document.querySelectorAll<HTMLElement>(".slideshow-controls button")];
    const at = buttons.indexOf(document.activeElement as HTMLElement);
    const next = event.shiftKey ? (at <= 0 ? buttons.length - 1 : at - 1) : (at + 1) % buttons.length;
    buttons[next].focus();
    event.preventDefault();
    return true;
  }
  switch (event.key) {
    case "Escape":
      closeSlideshow();
      break;
    case "ArrowRight":
      showSlide(slideshow.index + 1, true);
      break;
    case "ArrowLeft":
      showSlide(slideshow.index - 1, true);
      break;
    case " ":
      // A focused button handles its own Space.
      if (event.target instanceof HTMLButtonElement) return false;
      setSlideshowPlaying(!slideshow.playing);
      break;
    case "m":
    case "M":
      setMuted(!slideshow.muted);
      break;
    default:
      return false;
  }
  event.preventDefault();
  return true;
}

// ---------- Music player ----------

/** The songs, in the order they're played; fetched when first needed. */
/** Every song, as the CLI lists them (by name). */
let allTracks: Track[] = [];
/** What the player goes through: every song, or one folder's. */
let tracks: Track[] = [];

/** Where the player's songs come from: every song, one of Música's
 * folders, or the songs of a folder view (by ID, in its order). */
type TrackSource =
  | { kind: "all" }
  | { kind: "music-folder"; folder: string }
  | { kind: "ids"; ids: number[] };
let trackSource: TrackSource = { kind: "all" };

/** The folder a song is in, from its path (`/a/b/x.mp3` → `/a/b`). */
function folderOfPath(path: string | null): string {
  const at = (path ?? "").lastIndexOf("/");
  return at <= 0 ? "/" : path!.slice(0, at);
}

function tracksFor(source: TrackSource): Track[] {
  if (source.kind === "all") return allTracks;
  if (source.kind === "music-folder") {
    return allTracks.filter((t) => folderOfPath(t.path) === source.folder);
  }
  const byId = new Map(allTracks.map((t) => [t.id, t]));
  return source.ids.flatMap((id) => byId.get(id) ?? []);
}

/** The songs on screen now: all of Música, one of its folders, or those of
 * the folder open. */
function currentTrackSource(): TrackSource {
  const folder = musicFolder();
  if (folder !== null) return { kind: "music-folder", folder };
  if (inMusic()) return { kind: "all" };
  const ids = visibleEntries().flatMap((entry) =>
    entry.kind === "file" && isAudio(entry.file) && entry.file.normal_id !== null
      ? [entry.file.normal_id]
      : [],
  );
  return { kind: "ids", ids };
}

/** Play a whole source from its first song (a folder of Música's ▶). */
async function playSource(source: TrackSource) {
  trackSource = source;
  tracks = tracksFor(source);
  if (!tracks.length && !(await loadTracks())) return;
  playTrack(0);
}
let trackIndex = -1;
/** The song whose link was asked for again (once per song, until it
 * plays), in case it had expired. */
let retriedTrackId: number | null = null;

function inMusic(): boolean {
  // All the songs, or one of Música's folders.
  return currentCrumb().virtual?.startsWith("media:music") ?? false;
}

/** The folder of Música open now (`/a/b`), or null for all the songs. */
function musicFolder(): string | null {
  const view = currentCrumb().virtual ?? "";
  return view.startsWith(MUSIC_FOLDER) ? view.slice(MUSIC_FOLDER.length) : null;
}

function playerAudio(): HTMLAudioElement {
  return $("#player-audio") as HTMLAudioElement;
}

function formatTime(seconds: number): string {
  if (!Number.isFinite(seconds) || seconds < 0) return "0:00";
  const s = Math.floor(seconds);
  const h = Math.floor(s / 3600);
  const m = Math.floor((s % 3600) / 60);
  const rest = String(s % 60).padStart(2, "0");
  return h ? `${h}:${String(m).padStart(2, "0")}:${rest}` : `${m}:${rest}`;
}

async function loadTracks(): Promise<boolean> {
  try {
    allTracks = await invoke<Track[]>("music_tracks");
  } catch (error) {
    toast(String(error), "error");
    return false;
  }
  tracks = tracksFor(trackSource);
  if (!tracks.length) {
    toast(
      trackSource.kind === "ids"
        ? "O2 no trata estas canciones como música (por ejemplo .flac u .ogg), así que no se pueden escuchar aquí. Descárgalas para oírlas."
        : "No hay música que reproducir.",
      "info",
      8000,
    );
  }
  return tracks.length > 0;
}

/** Play `file` (a row of Música), or the selection, or from the first. */
async function playFile(file?: LogicalFile) {
  // It plays through what's on screen: one folder's songs, or all of them.
  trackSource = currentTrackSource();
  tracks = tracksFor(trackSource);
  const id = file?.normal_id ?? null;
  // A song uploaded since the list was fetched: fetch it again.
  const known = id === null ? tracks.length > 0 : tracks.some((t) => t.id === id);
  if (!known && !(await loadTracks())) return;
  if (id === null && file) {
    toast("Este archivo no se puede reproducir aquí.", "info");
    return;
  }
  const index = id === null ? 0 : tracks.findIndex((t) => t.id === id);
  if (index < 0) {
    toast(
      inMusic()
        ? "Esta canción aún no se puede reproducir. Prueba en unos segundos."
        : "O2 no trata este archivo como música (por ejemplo .flac u .ogg), o lo acabas de subir. Descárgalo para oírlo, o prueba en unos segundos.",
      "info",
      8000,
    );
    return;
  }
  playTrack(index);
}

function playTrack(index: number) {
  if (!tracks.length) return;
  trackIndex = (index + tracks.length) % tracks.length;
  const track = tracks[trackIndex];
  const audio = playerAudio();
  audio.src = track.url;
  ($("#player-seek") as HTMLInputElement).value = "0";
  $("#player-time").textContent = "0:00";
  // The button shows what really happens (playback can be refused).
  void audio.play().catch(() => {}).finally(updatePlayButton);
  $("#player").classList.remove("hidden");
  $("#player-title").textContent = track.name.replace(/\.[^.]+$/, "");
  $("#player-title").title = track.path ?? track.name;
  $("#player-duration").textContent = formatTime(track.duration ?? 0);
  if ("mediaSession" in navigator) {
    navigator.mediaSession.metadata = new MediaMetadata({ title: track.name, album: "O2 Cloud" });
  }
}

/** Stop and hide the player (closed, or logged out). */
function stopPlayer() {
  const audio = playerAudio();
  audio.pause();
  audio.removeAttribute("src");
  audio.load();
  trackIndex = -1;
  $("#player").classList.add("hidden");
  if ("mediaSession" in navigator) navigator.mediaSession.metadata = null;
}

function updatePlayButton() {
  const playing = !playerAudio().paused;
  const button = $("#player-toggle");
  button.textContent = playing ? "❚❚" : "▶";
  button.setAttribute("aria-label", playing ? "Pausar" : "Reproducir");
}

function bindPlayer() {
  const audio = playerAudio();
  const seek = $("#player-seek") as HTMLInputElement;
  let seeking = false;
  $("#play-music-btn").addEventListener("click", () => {
    const selected = selectedEntries().find((e): e is FileEntry => e.kind === "file");
    void playFile(selected?.file);
  });
  $("#player-toggle").addEventListener("click", () => {
    if (audio.paused) void audio.play().catch(() => {});
    else audio.pause();
  });
  $("#player-prev").addEventListener("click", () => {
    // Back to the start first, as players do.
    if (audio.currentTime > 3) audio.currentTime = 0;
    else playTrack(trackIndex - 1);
  });
  $("#player-next").addEventListener("click", () => playTrack(trackIndex + 1));
  $("#player-close").addEventListener("click", stopPlayer);
  audio.addEventListener("play", updatePlayButton);
  audio.addEventListener("playing", () => {
    retriedTrackId = null;
  });
  audio.addEventListener("pause", updatePlayButton);
  audio.addEventListener("ended", () => playTrack(trackIndex + 1));
  audio.addEventListener("loadedmetadata", () => {
    // A stream of unknown length can't be moved through.
    const known = Number.isFinite(audio.duration);
    seek.max = String(known ? Math.floor(audio.duration) : 0);
    seek.disabled = !known;
    if (known) $("#player-duration").textContent = formatTime(audio.duration);
  });
  audio.addEventListener("timeupdate", () => {
    if (!seeking) seek.value = String(Math.floor(audio.currentTime));
    $("#player-time").textContent = formatTime(audio.currentTime);
    seek.setAttribute("aria-valuetext", `${formatTime(audio.currentTime)} de ${formatTime(audio.duration)}`);
  });
  audio.addEventListener("error", async () => {
    if (trackIndex < 0 || !audio.getAttribute("src")) return;
    const failed = tracks[trackIndex];
    // Its link may have expired: ask for them again, once per song.
    if (failed && retriedTrackId !== failed.id) {
      retriedTrackId = failed.id;
      if (await loadTracks()) {
        // Another song was chosen meanwhile: leave it.
        if (audio.getAttribute("src") !== failed.url) return;
        const again = tracks.findIndex((t) => t.id === failed.id);
        if (again >= 0) return playTrack(again);
      }
    }
    toast(`No se pudo reproducir «${failed?.name ?? "esta canción"}».`, "error");
    // On with the next one, unless it is the only one.
    if (tracks.length > 1) playTrack(trackIndex + 1);
  });
  seek.addEventListener("input", () => {
    seeking = true;
    $("#player-time").textContent = formatTime(Number(seek.value));
  });
  seek.addEventListener("change", () => {
    audio.currentTime = Number(seek.value);
    seeking = false;
  });
  if ("mediaSession" in navigator) {
    // Media keys act only while there is a song, and not over the slideshow.
    navigator.mediaSession.setActionHandler("play", () => {
      if (audio.getAttribute("src") && !slideshowOpen()) void audio.play().catch(() => {});
    });
    navigator.mediaSession.setActionHandler("pause", () => audio.pause());
    navigator.mediaSession.setActionHandler("previoustrack", () => playTrack(trackIndex - 1));
    navigator.mediaSession.setActionHandler("nexttrack", () => playTrack(trackIndex + 1));
  }
}

function bindLabelViews() {
  bindPlayer();
  bindViewer();
  $("#slideshow-btn").addEventListener("click", () => void openSlideshow());
  for (const button of document.querySelectorAll<HTMLButtonElement>("#view-toggle button")) {
    button.addEventListener("click", () => setPlacesView(button.dataset.view === "map" ? "map" : "list"));
  }
  for (const button of document.querySelectorAll<HTMLButtonElement>("#music-toggle button")) {
    button.addEventListener("click", () =>
      setMusicView(button.dataset.view === "folders" ? "folders" : "songs"),
    );
  }
  $("#slide-prev").addEventListener("click", () => showSlide(slideshow.index - 1, true));
  $("#slide-next").addEventListener("click", () => showSlide(slideshow.index + 1, true));
  $("#slide-play").addEventListener("click", () => setSlideshowPlaying(!slideshow.playing));
  $("#slide-mute").addEventListener("click", () => setMuted(!slideshow.muted));
  $("#slide-close").addEventListener("click", closeSlideshow);
}

// ---------- Selection ----------

function toggleSelected(key: string) {
  if (state.selected.has(key)) state.selected.delete(key);
  else state.selected.add(key);
}

function selectRange(from: string, to: string, additive: boolean) {
  const keys = selectableKeys();
  const i = keys.indexOf(from);
  const j = keys.indexOf(to);
  if (i < 0 || j < 0) {
    state.selected = new Set([to]);
    return;
  }
  const range = keys.slice(Math.min(i, j), Math.max(i, j) + 1);
  state.selected = additive ? new Set([...state.selected, ...range]) : new Set(range);
}

function selectAll() {
  state.selected = new Set(selectableKeys());
  updateSelection();
}

function clearSelection() {
  state.selected.clear();
  state.anchor = null;
  updateSelection();
}

function onRowClick(event: MouseEvent, entry: Entry) {
  if (entry.kind === "virtual") {
    openEntry(entry);
    return;
  }
  const additive = event.metaKey || event.ctrlKey;
  if (event.shiftKey && state.anchor) {
    selectRange(state.anchor, entry.key, additive);
  } else if (additive) {
    toggleSelected(entry.key);
    state.anchor = entry.key;
  } else {
    state.selected = new Set([entry.key]);
    state.anchor = entry.key;
  }
  state.cursor = entry.key;
  updateSelection();
}

/** Arrow keys: move the selection one row; with Shift, extend it. */
function moveSelection(step: number, extend: boolean) {
  const keys = selectableKeys();
  if (!keys.length) return;
  const from = state.cursor ? keys.indexOf(state.cursor) : -1;
  const next =
    from < 0 ? (step > 0 ? 0 : keys.length - 1) : Math.max(0, Math.min(keys.length - 1, from + step));
  const key = keys[next];
  state.cursor = key;
  if (extend && state.anchor) {
    selectRange(state.anchor, key, false);
  } else {
    state.selected = new Set([key]);
    state.anchor = key;
  }
  if (!rowByKey.has(key)) {
    state.visibleLimit += PAGE;
    renderListing();
  } else {
    updateSelection();
  }
  rowByKey.get(key)?.scrollIntoView({ block: "nearest" });
}

/** Reflect the selection on the rows on screen and in the action bar. */
function updateSelection() {
  for (const [key, tr] of rowByKey) {
    const selected = state.selected.has(key);
    tr.classList.toggle("selected", selected);
    const box = tr.querySelector<HTMLInputElement>(".col-check input");
    if (box) box.checked = selected;
  }

  const all = selectableKeys().length;
  const selectAllBox = $("#select-all") as HTMLInputElement;
  selectAllBox.checked = all > 0 && state.selected.size === all;
  selectAllBox.indeterminate = state.selected.size > 0 && state.selected.size < all;
  selectAllBox.disabled = all === 0;

  const entries = selectedEntries();
  $("#selection-bar").classList.toggle("hidden", !entries.length);
  $("#folder-subtitle").classList.toggle("hidden", entries.length > 0);
  if (!entries.length) return;
  const size = entries.reduce((sum, entry) => sum + sizeOf(entry), 0);
  $("#selection-text").textContent = `${plural(entries.length, "seleccionado", "seleccionados")} · ${formatSize(size)}`;
  // Folders download with everything in them.
  ($("#selection-download") as HTMLButtonElement).disabled = false;
}

function openEntry(entry: Entry) {
  if (entry.kind === "virtual") openVirtualFolder(entry.folder);
  else if (entry.kind === "folder") openFolder(entry.folder);
  else if (inMusic() || isAudio(entry.file)) void playFile(entry.file);
  else if (previewable(entry.file)) openViewer(entry.file);
  else void downloadFiles([entry.file]);
}

/** Enter: open a single selected folder, or download the selected files. */
function openSelection() {
  const entries = selectedEntries();
  if (entries.length && entries[0].kind === "file" && (inMusic() || isAudio(entries[0].file))) {
    void playFile(entries[0].file);
    return;
  }
  if (entries.length === 1 && entries[0].kind === "folder") {
    openEntry(entries[0]);
    return;
  }
  if (entries.length === 1 && entries[0].kind === "file" && previewable(entries[0].file)) {
    openViewer(entries[0].file);
    return;
  }
  downloadSelected();
}

function downloadSelected() {
  const entries = selectedEntries();
  const files = entries
    .filter((entry): entry is FileEntry => entry.kind === "file")
    .map((entry) => entry.file);
  const folders = entries
    .filter((entry): entry is FolderEntry => entry.kind === "folder")
    .map((entry) => entry.folder);
  if (folders.length) void downloadFolders(folders, files);
  else if (files.length) void downloadFiles(files);
}

// ---------- Menus ----------

function showMenu(entries: MenuEntry[], x: number, y: number) {
  const menu = $("#menu");
  menu.replaceChildren();
  for (const entry of entries) {
    if (entry === "separator") {
      const separator = document.createElement("div");
      separator.className = "menu-separator";
      menu.appendChild(separator);
      continue;
    }
    const item = document.createElement("button");
    item.className = `menu-item${entry.danger ? " danger" : ""}`;
    item.setAttribute("role", "menuitem");
    item.disabled = !!entry.disabled;
    const label = document.createElement("span");
    label.textContent = entry.label;
    item.appendChild(label);
    if (entry.hint) {
      const hint = document.createElement("span");
      hint.className = "menu-hint";
      hint.textContent = entry.hint;
      item.appendChild(hint);
    }
    item.addEventListener("click", () => {
      hideMenu();
      entry.run();
    });
    menu.appendChild(item);
  }
  menu.classList.remove("hidden");
  // Keep it on screen.
  const rect = menu.getBoundingClientRect();
  menu.style.left = `${Math.max(8, Math.min(x, window.innerWidth - rect.width - 8))}px`;
  menu.style.top = `${Math.max(8, Math.min(y, window.innerHeight - rect.height - 8))}px`;
}

function hideMenu() {
  $("#menu").classList.add("hidden");
}

function menuOpen(): boolean {
  return !$("#menu").classList.contains("hidden");
}

function moveMenuFocus(step: number) {
  const items = [...document.querySelectorAll<HTMLButtonElement>("#menu .menu-item:not(:disabled)")];
  if (!items.length) return;
  const index = items.indexOf(document.activeElement as HTMLButtonElement);
  items[(index + step + items.length) % items.length].focus();
}

function selectionMenu(): MenuEntry[] {
  const entries = selectedEntries();
  const files = entries.filter((entry): entry is FileEntry => entry.kind === "file");
  const items: MenuEntry[] = [];
  if (entries.length === 1 && entries[0].kind === "folder") {
    const folder = entries[0];
    items.push({ label: "Abrir", hint: "↩", run: () => openEntry(folder) });
  }
  if (files.length === 1 && previewable(files[0].file) && !inMusic()) {
    const file = files[0].file;
    items.push({ label: "Vista previa", hint: "Espacio", run: () => openViewer(file) });
  }
  if (files.length === 1 && (inMusic() || isAudio(files[0].file))) {
    items.push({ label: "Reproducir", hint: "↩", run: () => void playFile(files[0].file) });
  }
  const folders = entries.length - files.length;
  if (entries.length) {
    items.push({
      label:
        entries.length === 1
          ? folders
            ? "Descargar carpeta…"
            : "Descargar…"
          : folders
            ? `Descargar ${entries.length} elementos…`
            : `Descargar ${files.length} archivos…`,
      hint: items.length ? undefined : "↩",
      run: downloadSelected,
    });
  }
  if (items.length) items.push("separator");
  items.push({
    label: "Mover a la papelera",
    hint: SHORTCUT.trash,
    danger: true,
    run: () => void trashEntries(entries),
  });
  return items;
}

/** Where O2 groups by itself: "labels" (nothing goes in), "media" (uploads
 * go to the root), or null for folders. */
function groupedView(): "labels" | "media" | null {
  const view = currentCrumb().virtual ?? "";
  // A folder of Música only shows songs: nothing is uploaded into it.
  if (view.startsWith("label") || view.startsWith(MUSIC_FOLDER)) return "labels";
  if (view.startsWith("media:")) return "media";
  return null;
}

function backgroundMenu(): MenuEntry[] {
  const refreshItem: MenuEntry = {
    label: "Recargar",
    hint: SHORTCUT.refresh,
    run: () => void refresh({ fresh: true }),
  };
  const grouped = groupedView();
  if (grouped === "labels") return [refreshItem];
  if (grouped === "media") {
    return [{ label: "Subir archivos…", run: () => void chooseFiles(false) }, "separator", refreshItem];
  }
  const items: MenuEntry[] = [
    { label: "Subir archivos…", run: () => void chooseFiles(false) },
    { label: "Subir carpeta…", run: () => void chooseFiles(true) },
  ];
  if (state.photos.available) {
    items.push({ label: "Subir desde Fotos…", run: () => void openPhotos() });
  }
  items.push(
    { label: "Nueva carpeta…", hint: SHORTCUT.newFolder, run: () => createFolder() },
    "separator",
    { label: "Recargar", hint: SHORTCUT.refresh, run: () => void refresh({ fresh: true }) },
  );
  return items;
}

function onRowContextMenu(event: MouseEvent, entry: Entry) {
  event.preventDefault();
  event.stopPropagation();
  if (entry.kind === "virtual") {
    showMenu([{ label: "Abrir", hint: "↩", run: () => openEntry(entry) }], event.clientX, event.clientY);
    return;
  }
  if (!state.selected.has(entry.key)) {
    state.selected = new Set([entry.key]);
    state.anchor = entry.key;
    state.cursor = entry.key;
    updateSelection();
  }
  showMenu(selectionMenu(), event.clientX, event.clientY);
}

// ---------- Transfers ----------

interface QueueRow {
  row: HTMLDivElement;
  name: HTMLSpanElement;
  message: HTMLSpanElement;
  fill: HTMLDivElement;
  detail: HTMLDivElement;
  cancel: HTMLButtonElement;
}

// Rows are updated in place, so the bar animates between updates and an
// indeterminate bar doesn't restart on every event.
const queueRows = new Map<string, QueueRow>();

/** Smoothed speed of each running transfer, in bytes per second. */
const transferRates = new Map<string, { bytes: number; time: number; rate: number }>();

function isFinished(item: TransferProgress): boolean {
  return item.phase === "done" || item.phase === "error";
}

function trackRate(item: TransferProgress) {
  if (isFinished(item)) {
    transferRates.delete(item.id);
    return;
  }
  const now = performance.now();
  const last = transferRates.get(item.id);
  if (!last) {
    transferRates.set(item.id, { bytes: item.bytes_done, time: now, rate: 0 });
    return;
  }
  // Back to the start: a failed attempt is being tried again.
  if (item.bytes_done < last.bytes) {
    transferRates.set(item.id, { bytes: item.bytes_done, time: now, rate: 0 });
    return;
  }
  const seconds = (now - last.time) / 1000;
  if (item.bytes_done <= last.bytes || seconds < 0.2) return;
  const instant = (item.bytes_done - last.bytes) / seconds;
  const rate = last.rate ? last.rate * 0.7 + instant * 0.3 : instant;
  transferRates.set(item.id, { bytes: item.bytes_done, time: now, rate });
}

function formatDuration(seconds: number): string {
  if (seconds < 60) return `${Math.max(1, Math.round(seconds))} s`;
  const minutes = Math.round(seconds / 60);
  if (minutes < 60) return `${minutes} min`;
  return `${Math.floor(minutes / 60)} h ${minutes % 60} min`;
}

/** How far along a transfer is, or null while nothing can be measured yet. */
function progressRatio(item: TransferProgress): number | null {
  if (isFinished(item)) return 1;
  if (item.phase === "queued") return 0;
  if (item.bytes_total > 0 && item.bytes_done > 0) {
    return Math.min(1, item.bytes_done / item.bytes_total);
  }
  if (item.total > 0 && item.current > 0) return Math.min(1, item.current / item.total);
  return null;
}

function transferDetail(item: TransferProgress): string {
  if (item.phase === "error" || item.bytes_total <= 0) return "";
  if (item.phase === "done" || item.phase === "queued") return formatSize(item.bytes_total);
  const parts = [`${formatSize(item.bytes_done)} de ${formatSize(item.bytes_total)}`];
  const rate = transferRates.get(item.id)?.rate ?? 0;
  // Nothing left to send (O2 is storing it): a speed would only go stale.
  if (rate > 0 && item.bytes_done < item.bytes_total) {
    parts.push(`${formatSize(rate)}/s`);
    const left = (item.bytes_total - item.bytes_done) / rate;
    if (left >= 1) parts.push(`quedan ${formatDuration(left)}`);
  }
  return parts.join(" · ");
}

function createQueueRow(): QueueRow {
  const row = document.createElement("div");
  const top = document.createElement("div");
  top.className = "queue-item-top";
  const name = document.createElement("span");
  name.className = "queue-name";
  const message = document.createElement("span");
  message.className = "queue-message";
  const cancel = document.createElement("button");
  cancel.type = "button";
  cancel.className = "link-btn queue-cancel hidden";
  cancel.textContent = "Cancelar";
  cancel.addEventListener("click", (event) => {
    event.stopPropagation();
    const id = cancel.dataset.id;
    if (!id) return;
    cancel.dataset.cancelling = "1";
    cancel.disabled = true;
    cancel.textContent = "Cancelando…";
    void invoke<boolean>("photos_cancel_export", { transferId: id })
      .then((cancelled) => {
        if (cancelled) return;
        delete cancel.dataset.cancelling;
        cancel.disabled = false;
        cancel.textContent = "Cancelar";
      })
      .catch(() => {
        delete cancel.dataset.cancelling;
        cancel.disabled = false;
        cancel.textContent = "Cancelar";
      });
  });
  top.append(name, message, cancel);

  const bar = document.createElement("div");
  bar.className = "progress";
  const fill = document.createElement("div");
  fill.className = "progress-fill";
  bar.appendChild(fill);

  const detail = document.createElement("div");
  detail.className = "queue-detail";
  row.append(top, bar, detail);
  return { row, name, message, fill, detail, cancel };
}

function updateQueueRow(view: QueueRow, item: TransferProgress) {
  view.row.className = `queue-item ${item.phase}`;
  const icon =
    item.kind === "download"
      ? "↓"
      : item.kind === "delete"
        ? "⌫"
        : item.kind === "library"
          ? "☁"
          : item.kind === "cli"
            ? "⚙"
            : "↑";
  view.name.textContent = `${icon} ${item.name}`;
  view.name.title = item.name;
  // Errors can be long: the first line is shown, all of it on hover.
  view.message.textContent = item.message.split("\n")[0];
  view.message.title = item.message;

  const ratio = progressRatio(item);
  view.fill.classList.toggle("indeterminate", ratio === null);
  view.fill.classList.toggle("done", item.phase === "done");
  view.fill.classList.toggle("error", item.phase === "error");
  view.fill.style.width = ratio === null ? "" : `${Math.round(ratio * 100)}%`;

  const detail = transferDetail(item);
  view.detail.textContent = detail;
  view.detail.classList.toggle("hidden", !detail);

  const canCancel = item.kind === "upload" && item.phase === "export";
  view.cancel.classList.toggle("hidden", !canCancel);
  if (!canCancel) {
    view.cancel.dataset.id = "";
    delete view.cancel.dataset.cancelling;
    view.cancel.disabled = false;
    view.cancel.textContent = "Cancelar";
  } else if (view.cancel.dataset.cancelling !== "1") {
    view.cancel.dataset.id = item.id;
    view.cancel.disabled = false;
    view.cancel.textContent = "Cancelar";
  }
}

/** The toolbar button: how many files are left, and how far the transfers are overall. */
function renderTransfersButton(filesLeft: number) {
  const active = [...state.queue.values()].filter((item) => !isFinished(item));
  const count = $("#transfers-count");
  count.textContent = String(filesLeft);
  count.classList.toggle("hidden", !filesLeft);
  $("#transfers-btn").classList.toggle("active", filesLeft > 0);

  const measured = active.filter((item) => item.bytes_total > 0);
  const total = measured.reduce((sum, item) => sum + item.bytes_total, 0);
  const done = measured.reduce((sum, item) => sum + Math.min(item.bytes_done, item.bytes_total), 0);
  $("#transfers-progress").classList.toggle("hidden", !total);
  if (total) $("#transfers-progress-fill").style.width = `${Math.round((done / total) * 100)}%`;
}

/** Finished transfers remembered at most. */
const MAX_FINISHED = 100;
/** Rows the transfers popover shows at once. */
const MAX_TRANSFER_ROWS = 50;
/** Queued transfers listed before the rest are summed up in one line. */
const QUEUED_ROWS = 3;

/**
 * Keep the queue bounded during long uploads by forgetting the oldest
 * finished transfers, those that went well before those that failed.
 * Queued and running ones stay, however many: the toolbar counts them.
 */
function forgetOldTransfers() {
  let finished = [...state.queue.values()].filter(isFinished).length;
  for (const phase of ["done", "error"]) {
    for (const [id, item] of state.queue) {
      if (finished <= MAX_FINISHED) return;
      if (item.phase === phase) {
        state.queue.delete(id);
        transferRates.delete(id);
        finished--;
      }
    }
  }
}

/**
 * For a file of a folder upload, the folder's transfer: the backend names
 * those `<folder's ID>/<file's>`.
 */
function folderOf(id: string): string | null {
  const slash = id.indexOf("/");
  return slash < 0 ? null : id.slice(0, slash);
}

/** The transfers to list, and the files of each folder upload, to list under it. */
function groupTransfers(): { top: TransferProgress[]; files: Map<string, TransferProgress[]> } {
  const top: TransferProgress[] = [];
  const files = new Map<string, TransferProgress[]>();
  for (const item of state.queue.values()) {
    const folder = folderOf(item.id);
    if (folder === null || !state.queue.has(folder)) {
      top.push(item);
      continue;
    }
    const siblings = files.get(folder);
    if (siblings) siblings.push(item);
    else files.set(folder, [item]);
  }
  return { top, files };
}

/** Files still to transfer: a folder upload counts those it hasn't finished. */
function countFilesLeft(top: TransferProgress[], files: Map<string, TransferProgress[]>): number {
  return top
    .filter((item) => !isFinished(item))
    .reduce((sum, item) => sum + (files.has(item.id) ? Math.max(1, item.total - item.current) : 1), 0);
}

type QueueLine = { item: TransferProgress; file: boolean } | { queued: number };

/**
 * What the popover lists, top to bottom: the transfers running, each
 * folder upload with the file it is on and those that failed; the next
 * few queued and how many more; then the finished, failed first, newest
 * first. A folder's files that went well aren't listed.
 */
function queueLines(top: TransferProgress[], files: Map<string, TransferProgress[]>): QueueLine[] {
  const lines: QueueLine[] = [];
  const add = (item: TransferProgress) => {
    lines.push({ item, file: false });
    const own = files.get(item.id) ?? [];
    for (const file of own.filter((file) => !isFinished(file))) lines.push({ item: file, file: true });
    for (const file of own.filter((file) => file.phase === "error").reverse()) {
      lines.push({ item: file, file: true });
    }
  };
  const queued = top.filter((item) => item.phase === "queued");
  top.filter((item) => !isFinished(item) && item.phase !== "queued").forEach(add);
  queued.slice(0, QUEUED_ROWS).forEach(add);
  if (queued.length > QUEUED_ROWS) lines.push({ queued: queued.length - QUEUED_ROWS });
  top.filter((item) => item.phase === "error").reverse().forEach(add);
  top.filter((item) => item.phase === "done").reverse().forEach(add);
  return lines.slice(0, MAX_TRANSFER_ROWS);
}

const queueEmpty = Object.assign(document.createElement("div"), {
  className: "queue-empty",
  textContent: "No hay transferencias ni operaciones.",
});
const queueMore = Object.assign(document.createElement("div"), { className: "queue-more" });

function renderQueue() {
  const list = $("#queue-list");
  const { top, files } = groupTransfers();
  const lines = queueLines(top, files);

  const shown = new Set(lines.flatMap((line) => ("item" in line ? [line.item.id] : [])));
  for (const [id, view] of queueRows) {
    if (!shown.has(id)) {
      view.row.remove();
      queueRows.delete(id);
    }
  }
  ($("#transfers-clear") as HTMLButtonElement).disabled = !lines.some(
    (line) => "item" in line && isFinished(line.item),
  );
  renderTransfersButton(countFilesLeft(top, files));

  const elements = lines.map((line) => {
    if (!("item" in line)) {
      queueMore.textContent = `Y ${line.queued} más en cola`;
      return queueMore;
    }
    let view = queueRows.get(line.item.id);
    if (!view) {
      view = createQueueRow();
      queueRows.set(line.item.id, view);
    }
    updateQueueRow(view, line.item);
    view.row.classList.toggle("file", line.file);
    return view.row;
  });
  if (!elements.length) elements.push(queueEmpty);
  // In order, moving only what is out of place.
  elements.forEach((element, index) => {
    if (list.children[index] !== element) list.insertBefore(element, list.children[index] ?? null);
  });
  while (list.children.length > elements.length) list.lastElementChild?.remove();
}

function clearFinishedTransfers() {
  for (const [id, item] of state.queue) {
    if (isFinished(item)) {
      state.queue.delete(id);
      transferRates.delete(id);
    }
  }
  renderQueue();
}

/** Show a transfer in the queue as soon as it is asked for. */
function queueTransfer(kind: "upload" | "download", name: string, size = 0): string {
  // randomUUID only exists in secure contexts.
  const id = crypto.randomUUID?.() ?? `t-${Date.now()}-${Math.random().toString(36).slice(2)}`;
  state.queue.set(id, {
    id,
    kind,
    name,
    phase: "queued",
    current: 0,
    total: 0,
    bytes_done: 0,
    bytes_total: size,
    message: "En cola",
  });
  return id;
}

/**
 * A long operation that is not a file transfer (delete, library sync, CLI
 * update). It shares the queue popover so every pending action is visible.
 */
function queueOperation(
  kind: "delete" | "library" | "cli",
  name: string,
  message: string,
  open = false,
): string {
  const id = crypto.randomUUID?.() ?? `op-${Date.now()}-${Math.random().toString(36).slice(2)}`;
  state.queue.set(id, {
    id,
    kind,
    name,
    phase: "running",
    current: 0,
    total: 0,
    bytes_done: 0,
    bytes_total: 0,
    message,
  });
  renderQueue();
  if (open) openTransfers();
  return id;
}

function finishOperation(id: string, message: string, failed = false) {
  const item = state.queue.get(id);
  if (!item) return;
  state.queue.set(id, {
    ...item,
    phase: failed ? "error" : "done",
    message,
  });
  renderQueue();
}

/** Rows still queued when their batch ends never started: say so. */
function failQueued(ids: string[], message: string) {
  for (const id of ids) {
    const item = state.queue.get(id);
    if (item?.phase === "queued") state.queue.set(id, { ...item, phase: "error", message });
  }
  renderQueue();
}

function transfersOpen(): boolean {
  return !$("#transfers-popover").classList.contains("hidden");
}

function openTransfers() {
  const popover = $("#transfers-popover");
  const button = $("#transfers-btn");
  const rect = button.getBoundingClientRect();
  popover.style.top = `${rect.bottom + 8}px`;
  popover.style.right = `${Math.max(12, window.innerWidth - rect.right)}px`;
  popover.classList.remove("hidden");
  button.setAttribute("aria-expanded", "true");
}

function closeTransfers() {
  $("#transfers-popover").classList.add("hidden");
  $("#transfers-btn").setAttribute("aria-expanded", "false");
}

// ---------- Loading ----------

// A listing can take several seconds; only the latest one may be shown.
let refreshSeq = 0;
/** The view `state.listing` belongs to. */
let listingView = "";

// ---------- Mutations and cached state ----------

/** How long a local change is trusted while O2's listing catches up. */
const MUTATION_GRACE_MS = 60_000;
/** A delete can wait for several whole-account CLI listings. */
const REMOVAL_GRACE_MS = 5 * 60_000;

/** Folders created here but not yet present in O2's listing, by view. */
const pendingFolderAdds = new Map<
  string,
  Map<number, { folder: RemoteFolder; until: number }>
>();
/** Entries deleted here but possibly still present in O2's listing, by view. */
const pendingRemovals = new Map<string, { keys: Set<string>; until: number }>();

function rememberFolderAdd(view: string, folder: RemoteFolder) {
  let folders = pendingFolderAdds.get(view);
  if (!folders) {
    folders = new Map();
    pendingFolderAdds.set(view, folders);
  }
  folders.set(folder.id, {
    folder,
    until: Date.now() + MUTATION_GRACE_MS,
  });
}

function rememberRemovals(view: string, keys: Set<string>) {
  let pending = pendingRemovals.get(view);
  if (!pending) {
    pending = { keys: new Set(), until: 0 };
    pendingRemovals.set(view, pending);
  }
  for (const key of keys) pending.keys.add(key);
  pending.until = Date.now() + REMOVAL_GRACE_MS;
}

/** Undo an optimistic removal when O2 rejects the delete. */
function forgetRemovals(view: string, keys: Set<string>) {
  const pending = pendingRemovals.get(view);
  if (!pending) return;
  for (const key of keys) pending.keys.delete(key);
  if (pending.keys.size === 0) pendingRemovals.delete(view);
}

/**
 * O2 lists writes and deletes with a delay, and a refresh can overlap with a
 * mutation. Keep showing what the user just did until the server agrees.
 */
function applyPendingMutations(view: string, listing: LogicalListing): LogicalListing {
  const now = Date.now();

  const adds = pendingFolderAdds.get(view);
  if (adds) {
    for (const [id, pending] of adds) {
      if (pending.until <= now) {
        adds.delete(id);
        continue;
      }
      if (listing.folders.some((folder) => folder.id === id)) {
        // The server listing already reflects the creation.
        adds.delete(id);
      } else {
        listing.folders.push(pending.folder);
      }
    }
    if (adds.size === 0) pendingFolderAdds.delete(view);
  }

  const removals = pendingRemovals.get(view);
  if (removals) {
    if (removals.until <= now) {
      pendingRemovals.delete(view);
    } else {
      listing.folders = listing.folders.filter(
        (folder) => !removals.keys.has(`folder:${folder.id}`),
      );
      listing.files = listing.files.filter(
        (file) => !removals.keys.has(fileKey(file)),
      );
    }
  }

  return listing;
}

/** Remove deleted rows from the current view without waiting for O2. */
function removeEntriesFromView(view: string, keys: Set<string>) {
  if (listingView !== view || !state.listing) return;
  state.listing.folders = state.listing.folders.filter(
    (folder) => !keys.has(`folder:${folder.id}`),
  );
  state.listing.files = state.listing.files.filter(
    (file) => !keys.has(fileKey(file)),
  );
  entriesCache.listing = null;
  if (state.stack.length === 1 && !currentCrumb().virtual) {
    state.rootFolders = state.listing.folders;
  }
  renderListing();
  renderPlaces();
}

/** A listing must never leave the window on “Cargando…” forever. */
const LISTING_TIMEOUT_MS = 100_000;

function withTimeout<T>(promise: Promise<T>, message: string): Promise<T> {
  return new Promise((resolve, reject) => {
    const timer = window.setTimeout(() => reject(new Error(message)), LISTING_TIMEOUT_MS);
    promise.then(
      (value) => {
        window.clearTimeout(timer);
        resolve(value);
      },
      (error) => {
        window.clearTimeout(timer);
        reject(error);
      },
    );
  });
}

/** One `ls` per view at a time; rapid refreshes collapse into one follow-up. */
const listingsInFlight = new Set<string>();
const pendingListings = new Map<string, boolean>();

/** List the current folder. `fresh` skips the backend's listing cache. */
async function refresh(options: { fresh?: boolean } = {}) {
  const crumb = currentCrumb();
  const view = crumbKey(crumb);
  if (listingsInFlight.has(view)) {
    // Back to a view whose listing is on its way: show that it's loading,
    // and use that listing when it comes (only a forced reload asks again).
    if (options.fresh) pendingListings.set(view, true);
    if (view !== listingView) state.listing = null;
    state.loading = true;
    renderPathbar();
    renderPlaces();
    renderListing();
    return;
  }
  ++refreshSeq;
  // A listing is only shown in the view it is for (fast clicks between
  // views leave several on their way).
  const stillHere = () => crumbKey(currentCrumb()) === view;

  // A folder created in this session is empty. Show it at once instead of
  // paying for another `ls` just to confirm what we already know. The marker
  // expires so an externally modified folder is not hidden for long.
  if (options.fresh && crumb.id != null) {
    knownEmptyFolders.delete(crumb.id);
  }
  const emptyUntil = crumb.id != null ? knownEmptyFolders.get(crumb.id) : undefined;
  if (emptyUntil != null && emptyUntil <= Date.now()) {
    knownEmptyFolders.delete(crumb.id!);
  }
  if (
    !options.fresh &&
    crumb.id != null &&
    !crumb.virtual &&
    (knownEmptyFolders.get(crumb.id) ?? 0) > Date.now()
  ) {
    state.listing = {
      path: crumb.path,
      folders: [],
      files: [],
      warnings: [],
      virtual_folders: [],
      stale: false,
    };
    listingView = view;
    state.loading = false;
    state.visibleLimit = PAGE;
    $("#table-wrap").scrollTop = 0;
    renderPathbar();
    renderPlaces();
    renderListing();
    return;
  }

  if (view !== listingView) {
    // Another folder: don't show the previous one's files meanwhile.
    state.listing = null;
    state.visibleLimit = PAGE;
    $("#table-wrap").scrollTop = 0;
  }
  listingsInFlight.add(view);
  state.loading = true;
  renderPathbar();
  renderPlaces();
  renderListing();
  try {
    const listing = await withTimeout(
      invoke<LogicalListing>("list_dir", {
        folderId: crumb.id,
        path: crumb.path,
        virtualPath: crumb.virtual ?? null,
        fresh: options.fresh ?? false,
      }),
      "O2 está tardando demasiado en listar la carpeta. Vuelve a intentarlo.",
    );
    if (!stillHere()) return;
    state.listing = applyPendingMutations(view, listing);
    listingView = view;
    if (listing.stale) {
      // Show the saved listing at once; the whole-account `ls` can take a
      // minute, so refresh it without making the window wait.
      listing.warnings.push(
        "Mostrando el último listado guardado. Pulsa Recargar para actualizarlo.",
      );
    }
    if (listing.path && listing.path !== crumb.path && listing.path !== "/") {
      crumb.path = listing.path;
    }
    if (state.stack.length === 1) state.rootFolders = listing.folders;
    // Forget selected rows that are gone.
    const keys = new Set(visibleEntries().map((entry) => entry.key));
    state.selected = new Set([...state.selected].filter((key) => keys.has(key)));
    renderWarnings();
    renderPlaces();
    // A cached listing can load without touching O2; check the session so
    // the badge and the login prompt stay accurate.
    if (!state.session?.logged_in) void refreshSession(false);
    if (options.fresh) void refreshQuota();
  } catch (error) {
    if (!stillHere()) return;
    reportError(error, 9000);
    if (listingView === view && state.listing) {
      // A failed refresh must not throw away the rows already on screen.
      state.listing = {
        ...state.listing,
        stale: false,
        warnings: [
          ...state.listing.warnings.filter(
            (warning) => !warning.startsWith("Mostrando el último listado guardado"),
          ),
          String(error),
        ],
      };
    } else {
      state.listing = {
        path: crumb.path,
        folders: [],
        files: [],
        warnings: [String(error)],
        virtual_folders: [],
        stale: false,
      };
      listingView = view;
    }
    renderWarnings();
  } finally {
    listingsInFlight.delete(view);
    const pending = pendingListings.get(view);
    if (pending !== undefined) {
      pendingListings.delete(view);
      if (crumbKey(currentCrumb()) === view) void refresh({ fresh: pending });
    }
    if (stillHere()) {
      state.loading = false;
      renderListing();
    }
  }
}

/** Check the O2 session. Close the login window if it's active; open it if not, unless `prompt` is false. */
async function refreshSession(prompt = true) {
  try {
    state.session = await invoke<SessionStatus>("get_session_status");
  } catch (error) {
    state.session = {
      logged_in: false,
      raw: String(error),
      phone: null,
      since: null,
      session: null,
      renews_silently: false,
      storage: null,
    };
  }
  renderSession();
  if (state.session.logged_in) {
    hideLoginModal();
  } else if (prompt) {
    showLoginModal();
    setLoginStatus("Inicia sesión para acceder a tus archivos.");
  }
}

// ---------- Actions ----------

function reportUploadSummary(summary: UploadSummary) {
  const conflicts = summary.conflicts?.length ?? 0;
  const skipped =
    (summary.skipped
      ? ` ${summary.skipped} ya estaba${summary.skipped === 1 ? "" : "n"} en O2.`
      : "") +
    (conflicts ? ` ${conflicts} con otra versión en O2: elige qué hacer.` : "");
  if (summary.failed.length) {
    toast(
      `Subidos ${summary.uploaded}. Fallaron ${summary.failed.length}.${skipped} ${summary.failed[0]}`,
      "error",
      10000,
    );
    console.error(summary.failed);
  } else {
    toast(
      `Subidos ${summary.uploaded} archivo${summary.uploaded === 1 ? "" : "s"}.${skipped} O2 puede tardar unos segundos en mostrarlos.`,
      "success",
    );
  }
  if (summary.library_warning) {
    toast(summary.library_warning, "error", 12000);
  }
}

/** Questions about upload conflicts wait for the one before. */
let conflictQueue: Promise<void> = Promise.resolve();

async function uploadPaths(
  paths: string[],
  knownHashes?: (string | null)[],
  onConflict?: "overwrite",
  target?: Crumb,
) {
  if (!paths.length) return;
  if (!state.session?.logged_in) {
    showLoginModal();
    return;
  }
  const crumb = target ?? currentCrumb();
  const view = crumbKey(crumb);
  if (crumb.id != null) knownEmptyFolders.delete(crumb.id);
  const refreshIfStillHere = () => {
    if (crumbKey(currentCrumb()) === view) void refresh({ fresh: true });
  };
  // The backend runs uploads one at a time, in order; until a path's turn
  // comes it shows here as queued.
  const transferIds = paths.map((path) => queueTransfer("upload", path.split(/[\\/]/).pop() || path));
  renderQueue();
  openTransfers();
  state.uploadsInFlight++;
  try {
    const summary = await invoke<UploadSummary>("upload_paths", {
      paths,
      folderId: crumb.id,
      folderPath: crumb.path,
      transferIds,
      knownHashes: knownHashes ?? null,
      onConflict: onConflict ?? null,
    });
    reportUploadSummary(summary);
    // The CLI lists what it just uploaded straight away: the list shows it
    // now, and once more later with what O2 adds (a song's length…).
    if (crumbKey(currentCrumb()) === view) await refresh({ fresh: true });
    window.setTimeout(refreshIfStillHere, 20000);
    // Files O2 has in another version: ask (one question at a time), then
    // upload the ones to overwrite (the old copies go to O2's trash).
    if (summary.conflicts?.length) {
      const conflicts = summary.conflicts;
      conflictQueue = conflictQueue.then(async () => {
        const overwrite = await askUploadConflicts(conflicts, crumb.path);
        if (overwrite.length) void uploadPaths(overwrite, undefined, "overwrite", crumb);
      });
    }
  } catch (error) {
    reportError(error);
  } finally {
    state.uploadsInFlight--;
    failQueued(transferIds, "No se ha subido");
  }
}

/**
 * Hash local photos/videos and ask about files already anywhere in O2 before
 * opening the upload queue. Folders are checked by the normal target-folder
 * rule only.
 */
async function prepareUploadPaths(paths: string[]) {
  if (!paths.length) return;
  if (!state.session?.logged_in) {
    showLoginModal();
    return;
  }
  const crumb = currentCrumb();
  // The check hashes files before the upload queue starts, which can take a
  // while on a slow disk. Show it as an operation with live progress instead
  // of a toast that disappears.
  const checkId = queueOperation(
    "library",
    "Comprobando duplicados",
    "Buscando en O2…",
    true,
  );
  let checks: PathCheck[] | null = null;
  try {
    checks = await invoke<PathCheck[]>("library_check_paths", {
      paths,
      folderId: crumb.id,
      folderPath: crumb.path,
      progressId: checkId,
    });
    finishOperation(checkId, "Comprobación terminada");
  } catch (error) {
    finishOperation(checkId, "No se pudo comprobar", true);
    console.error(error);
  }
  if (!checks || checks.length !== paths.length) {
    await uploadPaths(paths, undefined, undefined, crumb);
    return;
  }

  const duplicateIndexes = new Set<number>();
  const duplicates: ConflictItem[] = [];
  checks.forEach((check, index) => {
    if (check.is_file && check.copies.length && !check.in_target) {
      duplicateIndexes.add(index);
      duplicates.push({
        id: String(index),
        name: check.path.split(/[\\/]/).pop() || check.path,
        copies: check.copies,
      });
    }
  });

  let uploadDuplicates = new Set<string>();
  if (duplicates.length) {
    const choice = await askLibraryConflict(duplicates, crumb.virtual ? "/" : crumb.path);
    if (choice.kind === "cancel") return;
    uploadDuplicates = choice.upload;
  }

  const remaining: string[] = [];
  const hashes: (string | null)[] = [];
  paths.forEach((path, index) => {
    if (duplicateIndexes.has(index) && !uploadDuplicates.has(String(index))) return;
    remaining.push(path);
    hashes.push(checks?.[index]?.sha256 ?? null);
  });
  if (!remaining.length) {
    toast("No hay nada que subir: ya estaba todo en O2.", "success");
    return;
  }
  await uploadPaths(remaining, hashes, undefined, crumb);
}

async function chooseFiles(folder: boolean) {
  const selected = await open({
    multiple: true,
    directory: folder,
    title: folder ? "Elige carpetas para subir" : "Elige archivos para subir",
  });
  if (!selected) return;
  const paths = Array.isArray(selected) ? selected : [selected];
  await prepareUploadPaths(paths);
}

// ---------- Photos (macOS) ----------

function photoDateMs(value: string, endOfDay: boolean): number | null {
  if (!value) return null;
  const date = new Date(`${value}T${endOfDay ? "23:59:59.999" : "00:00:00"}`);
  return Number.isNaN(date.getTime()) ? null : date.getTime();
}

function photoIsoDate(date: Date): string {
  const year = date.getFullYear();
  const month = String(date.getMonth() + 1).padStart(2, "0");
  const day = String(date.getDate()).padStart(2, "0");
  return `${year}-${month}-${day}`;
}

/** Default range for the date filter: from one month ago through today. */
function photoDefaultDateRange(): { start: string; end: string } {
  const end = new Date();
  const start = new Date(end.getFullYear(), end.getMonth() - 1, end.getDate());
  if (start.getDate() !== end.getDate()) {
    // The previous month is shorter, so clamp to its last day.
    start.setDate(0);
  }
  return { start: photoIsoDate(start), end: photoIsoDate(end) };
}

function photoListArgs(offset: number) {
  return {
    mediaType: state.photos.mediaType,
    minSize: 0,
    albumId: state.photos.albumId || null,
    favoritesOnly: state.photos.favoritesOnly,
    startMs: state.photos.dateFilter ? photoDateMs(state.photos.startDate, false) : null,
    endMs: state.photos.dateFilter ? photoDateMs(state.photos.endDate, true) : null,
    offset,
    limit: PHOTO_PAGE,
  };
}

function photoNoun(): { one: string; many: string } {
  if (state.photos.mediaType === "photo") return { one: "foto", many: "fotos" };
  if (state.photos.mediaType === "all") return { one: "elemento", many: "elementos" };
  return { one: "vídeo", many: "vídeos" };
}

function formatClipDuration(seconds: number): string {
  const total = Math.max(0, Math.round(seconds));
  const hours = Math.floor(total / 3600);
  const minutes = Math.floor((total % 3600) / 60);
  const rest = total % 60;
  const mm = String(minutes).padStart(2, "0");
  const ss = String(rest).padStart(2, "0");
  return hours ? `${hours}:${mm}:${ss}` : `${minutes}:${ss}`;
}

function photoKey(asset: PhotoAsset): string {
  return `${asset.name}\u0000${asset.size}`;
}

/** Names and sizes already complete in the folder being browsed (chunks included). */
function existingPhotoKeys(): Set<string> {
  return new Set(
    (state.listing?.files ?? [])
      .filter((file) => !file.incomplete)
      .map((file) => `${file.name}\u0000${file.size}`),
  );
}

function photoIsHere(asset: PhotoAsset, existing: Set<string>): boolean {
  return asset.size > 0 && existing.has(photoKey(asset));
}

function photoMatch(asset: PhotoAsset): PhotoLibraryMatch | undefined {
  return state.photos.matches.get(asset.id);
}

function photoInTarget(asset: PhotoAsset, existing: Set<string>): boolean {
  return photoIsHere(asset, existing) || photoMatch(asset)?.in_target === true;
}

function visiblePhotos(existing: Set<string>): PhotoAsset[] {
  const needle = state.photos.filter.trim().toLowerCase();
  return state.photos.assets.filter((asset) => {
    if (needle && !asset.name.toLowerCase().includes(needle)) return false;
    if (state.photos.onlyLarge && asset.size < LARGE_FILE) return false;
    if (
      state.photos.hideExisting &&
      (photoInTarget(asset, existing) || (photoMatch(asset)?.copies.length ?? 0) > 0)
    ) {
      return false;
    }
    return true;
  });
}

function updatePhotosSelection() {
  const selected = state.photos.assets.filter((asset) => state.photos.selected.has(asset.id));
  const bytes = selected.reduce((sum, asset) => sum + asset.size, 0);
  $("#photos-selection").textContent = selected.length
    ? `${plural(selected.length, "seleccionado", "seleccionados")} · ${formatSize(bytes)}`
    : "Ninguno seleccionado";
  // Checking the shared library can take a while; it must not block choosing
  // an upload. `uploadPhotos` waits for the check before looking for
  // duplicates in other folders.
  ($("#photos-upload") as HTMLButtonElement).disabled =
    !selected.length || state.photos.loading || state.photos.uploading;
}

const photoThumbnails = new Map<string, string>();
let photoThumbnailObserver: IntersectionObserver | null = null;

async function loadPhotoThumbnail(assetId: string, image: HTMLImageElement) {
  const show = (source: string) => {
    image.src = source;
    image.classList.add("loaded");
    image
      .closest(".photos-row")
      ?.querySelector<HTMLElement>(".photos-icon")
      ?.classList.add("hidden");
  };
  const cached = photoThumbnails.get(assetId);
  if (cached) {
    show(cached);
    return;
  }
  try {
    const thumbnail = await invoke<PhotoThumbnail>("photos_thumbnail", {
      assetId,
      size: 64,
    });
    const source = `data:image/jpeg;base64,${thumbnail.data}`;
    photoThumbnails.set(assetId, source);
    show(source);
  } catch {
    image.remove();
  }
}

function observePhotoThumbnail(assetId: string, image: HTMLImageElement) {
  if (!photoThumbnailObserver) {
    photoThumbnailObserver = new IntersectionObserver(
      (entries) => {
        for (const entry of entries) {
          if (!entry.isIntersecting) continue;
          const target = entry.target as HTMLImageElement;
          photoThumbnailObserver?.unobserve(target);
          void loadPhotoThumbnail(target.dataset.assetId ?? "", target);
        }
      },
      { root: $("#photos-list") },
    );
  }
  image.dataset.assetId = assetId;
  photoThumbnailObserver.observe(image);
}

function createPhotoRow(
  asset: PhotoAsset,
  here: boolean,
  match?: PhotoLibraryMatch,
): HTMLLabelElement {
  const row = document.createElement("label");
  row.className = "photos-row";
  const copyPaths = [...new Set((match?.copies ?? []).map((copy) => copy.folder_path))];
  row.title = copyPaths.length ? `${asset.name} — ${copyPaths.join(", ")}` : asset.name;

  const check = document.createElement("input");
  check.type = "checkbox";
  check.checked = state.photos.selected.has(asset.id);
  check.addEventListener("change", () => {
    if (check.checked) state.photos.selected.add(asset.id);
    else state.photos.selected.delete(asset.id);
    updatePhotosSelection();
  });

  const thumbnail = document.createElement("img");
  thumbnail.className = "photos-thumb";
  thumbnail.alt = "";

  const icon = document.createElement("span");
  icon.className = "photos-icon";
  icon.textContent = asset.media_type === "video" ? "🎬" : "🖼";

  const name = document.createElement("span");
  name.className = "photos-name";
  name.textContent = asset.name;

  const meta = document.createElement("span");
  meta.className = "photos-meta";
  const details: string[] = [];
  if (asset.date) details.push(formatDate(asset.date));
  if (asset.media_type === "video" && asset.duration > 0) {
    details.push(formatClipDuration(asset.duration));
  }
  meta.textContent = details.join(" · ");

  const size = document.createElement("span");
  size.className = "photos-size";
  size.textContent = asset.size ? formatSize(asset.size) : "—";

  const badge = document.createElement("span");
  badge.className = "photos-badge";
  if (here) {
    badge.textContent = "Ya aquí";
  } else if (match?.copies.length) {
    badge.textContent = "Ya en O2";
    badge.classList.add("elsewhere");
  } else {
    badge.classList.add("hidden");
  }
  badge.title = copyPaths.join("\n");

  row.append(check, thumbnail, icon, name, meta, size, badge);
  if (photoThumbnails.has(asset.id)) void loadPhotoThumbnail(asset.id, thumbnail);
  else observePhotoThumbnail(asset.id, thumbnail);
  return row;
}

function renderPhotos() {
  const existing = existingPhotoKeys();
  const visible = visiblePhotos(existing);
  const noun = photoNoun();
  const totalBytes = visible.reduce((sum, asset) => sum + asset.size, 0);
  const onO2 = visible.filter(
    (asset) => photoInTarget(asset, existing) || (photoMatch(asset)?.copies.length ?? 0) > 0,
  ).length;

  $("#photos-loading").classList.toggle("hidden", !state.photos.loading);
  $("#photos-empty").classList.toggle("hidden", state.photos.loading || visible.length > 0);

  const library = state.photos.syncingLibrary
    ? " · sincronizando biblioteca…"
    : state.libraryStats
      ? ` · biblioteca: ${plural(state.libraryStats.files, "archivo", "archivos")}`
      : "";
  $("#photos-summary").textContent = state.photos.loading
    ? "Leyendo Fotos…"
    : `${plural(visible.length, noun.one, noun.many)} · ${formatSize(totalBytes)}${
        onO2 ? ` · ${onO2} ya en O2` : ""
      }${state.photos.truncated ? ` · ${state.photos.assets.length} de ${state.photos.total}` : ""}${library}`;

  const shown = visible.slice(0, state.photos.visibleLimit);
  $("#photos-list").replaceChildren(
    ...shown.map((asset) =>
      createPhotoRow(asset, photoInTarget(asset, existing), photoMatch(asset)),
    ),
  );
  $("#photos-more-wrap").classList.toggle(
    "hidden",
    !state.photos.truncated && visible.length <= shown.length,
  );
  $("#photos-dates-clear").classList.toggle("hidden", !state.photos.dateFilter);
  const pending = visible.filter(
    (asset) => !photoInTarget(asset, existing) && !(photoMatch(asset)?.copies.length ?? 0),
  ).length;
  ($("#photos-select-pending") as HTMLButtonElement).disabled =
    pending === 0 || state.photos.syncingLibrary;
  ($("#photos-select-all") as HTMLButtonElement).disabled = visible.length === 0;
  ($("#photos-select-none") as HTMLButtonElement).disabled = state.photos.selected.size === 0;
  updatePhotosSelection();

  if (!state.photos.loading && visible.length === 0) {
    $("#photos-empty-title").textContent =
      state.photos.assets.length === 0
        ? "No hay elementos en Fotos."
        : "No hay elementos que coincidan con el filtro.";
  }
}

function hidePhotos() {
  $("#photos-modal").classList.add("hidden");
}

interface ConflictItem {
  id: string;
  name: string;
  copies: LibraryCopy[];
}

type ConflictChoice =
  | { kind: "cancel" }
  | { kind: "continue"; upload: Set<string> };

let conflictResolve: ((choice: ConflictChoice) => void) | null = null;

/** Ask, file by file, which duplicates to upload again. */
function askLibraryConflict(
  items: ConflictItem[],
  destination: string,
): Promise<ConflictChoice> {
  if (conflictResolve) conflictResolve({ kind: "cancel" });
  const where = destination === "/" ? "la raíz de O2 Cloud" : destination;
  $("#library-conflict-text").textContent =
    items.length === 1
      ? `«${items[0].name}» ya está en O2 en otra carpeta. Marca «Subir copia» si quieres subir otra copia a ${where}; si no, se saltará.`
      : `${items.length} elementos ya están en O2 en otras carpetas. Marca cuáles quieres subir de nuevo a ${where}; los demás se saltarán.`;

  const list = $("#library-conflict-list");
  list.replaceChildren();
  for (const item of items) {
    const row = document.createElement("label");
    row.className = "conflict-item";

    const check = document.createElement("input");
    check.type = "checkbox";
    check.dataset.id = item.id;
    check.checked = false;

    const copy = document.createElement("div");
    copy.className = "conflict-copy";
    const name = document.createElement("strong");
    name.textContent = item.name;
    const paths = document.createElement("span");
    paths.textContent = [...new Set(item.copies.map((item) => item.folder_path))].join(", ");
    copy.append(name, paths);

    row.append(check, copy);
    list.appendChild(row);
  }

  $("#library-conflict-modal").classList.remove("hidden");
  return new Promise((resolve) => {
    conflictResolve = resolve;
  });
}

let uploadConflictResolve: ((overwrite: string[]) => void) | null = null;

/**
 * Ask which files to overwrite, of those O2 already has with the same name and
 * another size. Resolves with their local paths (none: all skipped).
 */
function askUploadConflicts(conflicts: UploadConflict[], destination: string): Promise<string[]> {
  if (uploadConflictResolve) uploadConflictResolve([]);
  const where = destination === "/" ? "la raíz de O2 Cloud" : destination;
  $("#upload-conflict-text").textContent =
    conflicts.length === 1
      ? `«${conflicts[0].name}» ya está en ${where} con otro tamaño. Márcalo para sobrescribirlo (la versión de O2 irá a la papelera); si no, se ignorará.`
      : `${conflicts.length} archivos ya están en ${where} con otro tamaño. Marca los que quieras sobrescribir (las versiones de O2 irán a la papelera); los demás se ignorarán.`;
  const list = $("#upload-conflict-list");
  list.replaceChildren();
  for (const conflict of conflicts) {
    const row = document.createElement("label");
    row.className = "conflict-item";
    const check = document.createElement("input");
    check.type = "checkbox";
    check.dataset.local = conflict.local;
    check.checked = false;
    const copy = document.createElement("div");
    copy.className = "conflict-copy";
    const name = document.createElement("strong");
    name.textContent = conflict.name;
    const sizes = document.createElement("span");
    sizes.textContent =
      `Aquí: ${formatSize(conflict.size)}, ${formatDate(conflict.modified ?? null)} · ` +
      `En O2: ${formatSize(conflict.cloud.size)}, ${formatDate(conflict.cloud.modified ?? null)}`;
    copy.append(name, sizes);
    row.append(check, copy);
    list.appendChild(row);
  }
  $("#upload-conflict-modal").classList.remove("hidden");
  return new Promise((resolve) => {
    uploadConflictResolve = resolve;
  });
}

function setAllUploadConflicts(checked: boolean) {
  for (const box of $("#upload-conflict-list").querySelectorAll<HTMLInputElement>(
    'input[type="checkbox"]',
  )) {
    box.checked = checked;
  }
}

function finishUploadConflicts() {
  const overwrite: string[] = [];
  for (const box of $("#upload-conflict-list").querySelectorAll<HTMLInputElement>(
    'input[type="checkbox"]:checked',
  )) {
    if (box.dataset.local) overwrite.push(box.dataset.local);
  }
  $("#upload-conflict-modal").classList.add("hidden");
  const resolve = uploadConflictResolve;
  uploadConflictResolve = null;
  resolve?.(overwrite);
}

function setAllConflictChoices(checked: boolean) {
  for (const box of $("#library-conflict-list").querySelectorAll<HTMLInputElement>(
    'input[type="checkbox"]',
  )) {
    box.checked = checked;
  }
}

function finishLibraryConflict(kind: "cancel" | "continue") {
  const upload = new Set<string>();
  if (kind === "continue") {
    for (const box of $("#library-conflict-list").querySelectorAll<HTMLInputElement>(
      'input[type="checkbox"]:checked',
    )) {
      if (box.dataset.id) upload.add(box.dataset.id);
    }
  }
  $("#library-conflict-modal").classList.add("hidden");
  const resolve = conflictResolve;
  conflictResolve = null;
  resolve?.(kind === "cancel" ? { kind: "cancel" } : { kind: "continue", upload });
}

function showPhotosBrowser() {
  $("#photos-permission").classList.add("hidden");
  $("#photos-browser").classList.remove("hidden");
  if (!state.photos.albums.length) void loadPhotoAlbums();
}

async function loadPhotoAlbums() {
  try {
    const result = await invoke<PhotoAlbums>("photos_albums");
    state.photos.albums = result.albums;
    const select = $("#photos-album") as HTMLSelectElement;
    select.replaceChildren();
    const all = document.createElement("option");
    all.value = "";
    all.textContent = "Todos los álbumes";
    select.appendChild(all);
    for (const album of result.albums) {
      const option = document.createElement("option");
      option.value = album.id;
      option.textContent = album.title;
      select.appendChild(option);
    }
    select.value = state.photos.albumId;
  } catch (error) {
    console.error(error);
  }
}

function showPhotosPermission(status: string) {
  state.photos.status = status;
  $("#photos-browser").classList.add("hidden");
  $("#photos-permission").classList.remove("hidden");
  const text = $("#photos-permission-text");
  const button = $("#photos-grant") as HTMLButtonElement;
  button.disabled = false;

  if (status === "not_determined") {
    text.textContent =
      "O2 Cloud necesita permiso para leer tu biblioteca y subir los originales, también los que solo están en iCloud.";
    button.textContent = "Permitir acceso a Fotos";
    button.classList.remove("hidden");
  } else if (status === "denied") {
    text.textContent =
      "Has denegado el acceso a Fotos. Actívalo en Ajustes del Sistema › Privacidad y seguridad › Fotos y vuelve a comprobarlo.";
    button.textContent = "Volver a comprobar";
    button.classList.remove("hidden");
  } else if (status === "restricted") {
    text.textContent =
      "El acceso a Fotos está restringido en este Mac (por ejemplo, por gestión de dispositivos).";
    button.classList.add("hidden");
  } else {
    text.textContent =
      "No se ha podido comprobar el acceso a Fotos. Si acabas de cambiarlo en Ajustes del Sistema, vuelve a comprobarlo.";
    button.textContent = "Volver a comprobar";
    button.classList.remove("hidden");
  }
}

// Changing the media type can start a second listing; only the latest may
// replace the rows.
let photosSeq = 0;

async function loadPhotos() {
  const seq = ++photosSeq;
  state.photos.loading = true;
  state.photos.syncingLibrary = true;
  state.photos.matchPromise = null;
  state.photos.assets = [];
  state.photos.matches.clear();
  state.photos.visibleLimit = PHOTO_PAGE;
  renderPhotos();

  const libraryPromise = invoke<LibraryStats>("library_sync", { force: false })
    .then((stats) => {
      state.libraryStats = stats;
    })
    .catch((error) => {
      console.error(error);
    });

  const listPromise = (async () => {
    try {
      const list = await invoke<PhotoList>("photos_list", photoListArgs(0));
      if (seq !== photosSeq) return;
      state.photos.assets = list.assets;
      state.photos.total = list.total;
      state.photos.truncated = list.truncated;
      state.photos.visibleLimit = list.assets.length;
    } catch (error) {
      if (seq !== photosSeq) return;
      state.photos.assets = [];
      state.photos.total = 0;
      state.photos.truncated = false;
      reportError(error);
    } finally {
      if (seq === photosSeq) {
        state.photos.loading = false;
        renderPhotos();
      }
    }
  })();

  // This runs in the background: the list is usable as soon as the assets
  // arrive, and the upload button only waits for it when it is pressed.
  const matchPromise = (async () => {
    await Promise.all([libraryPromise, listPromise]);
    if (seq !== photosSeq) return;
    try {
      const crumb = currentCrumb();
      const matches = await invoke<PhotoLibraryMatch[]>("library_match_photos", {
        assets: state.photos.assets,
        folderId: crumb.id,
        folderPath: crumb.path,
      });
      if (seq !== photosSeq) return;
      state.photos.matches = new Map(matches.map((match) => [match.asset_id, match]));
    } catch (error) {
      if (seq !== photosSeq) return;
      console.error(error);
    } finally {
      if (seq === photosSeq) {
        state.photos.syncingLibrary = false;
        renderPhotos();
      }
    }
  })();

  state.photos.matchPromise = matchPromise;
  await matchPromise;
}

function matchPhotoAssets(assets: PhotoAsset[]) {
  if (!assets.length) return;
  const crumb = currentCrumb();
  void invoke<PhotoLibraryMatch[]>("library_match_photos", {
    assets,
    folderId: crumb.id,
    folderPath: crumb.path,
  })
    .then((matches) => {
      for (const match of matches) state.photos.matches.set(match.asset_id, match);
      renderPhotos();
    })
    .catch((error) => console.error(error));
}

async function loadMorePhotos() {
  if (state.photos.loading || !state.photos.truncated) return;
  state.photos.loading = true;
  renderPhotos();
  try {
    const list = await invoke<PhotoList>("photos_list", photoListArgs(state.photos.assets.length));
    state.photos.assets.push(...list.assets);
    state.photos.total = list.total;
    state.photos.truncated = list.truncated;
    state.photos.visibleLimit = state.photos.assets.length;
    state.photos.loading = false;
    renderPhotos();
    matchPhotoAssets(list.assets);
  } catch (error) {
    state.photos.loading = false;
    renderPhotos();
    reportError(error);
  }
}

async function openPhotos() {
  if (!state.photos.available) {
    toast("Subir desde Fotos solo está disponible en macOS.", "info");
    return;
  }
  if (!state.session?.logged_in) {
    showLoginModal();
    return;
  }

  const crumb = currentCrumb();
  const destination = crumb.virtual ? "/" : crumb.path;
  $("#photos-where").textContent =
    destination === "/" ? "Se subirán a la raíz de O2 Cloud." : `Se subirán a ${destination}.`;
  state.photos.assets = [];
  state.photos.total = 0;
  state.photos.truncated = false;
  state.photos.selected.clear();
  state.photos.matchPromise = null;
  state.photos.filter = "";
  state.photos.onlyLarge = false;
  state.photos.hideExisting = false;
  state.photos.albumId = "";
  state.photos.favoritesOnly = false;
  state.photos.dateFilter = false;
  const dates = photoDefaultDateRange();
  state.photos.startDate = dates.start;
  state.photos.endDate = dates.end;
  state.photos.visibleLimit = PHOTO_PAGE;
  ($("#photos-search") as HTMLInputElement).value = "";
  ($("#photos-only-large") as HTMLInputElement).checked = false;
  ($("#photos-hide-existing") as HTMLInputElement).checked = false;
  ($("#photos-album") as HTMLSelectElement).value = "";
  ($("#photos-favorites") as HTMLInputElement).checked = false;
  ($("#photos-date-filter") as HTMLInputElement).checked = false;
  ($("#photos-start") as HTMLInputElement).value = dates.start;
  ($("#photos-end") as HTMLInputElement).value = dates.end;
  ($("#photos-media") as HTMLSelectElement).value = state.photos.mediaType;

  $("#photos-modal").classList.remove("hidden");
  $("#photos-permission").classList.add("hidden");
  $("#photos-browser").classList.add("hidden");

  try {
    const access = await invoke<PhotosAccess>("photos_access_status");
    state.photos.available = access.available;
    state.photos.status = access.status;
    if (!access.available) {
      hidePhotos();
      toast("Subir desde Fotos solo está disponible en macOS.", "info");
      return;
    }
    if (access.status === "authorized" || access.status === "limited") {
      if (access.status === "limited") {
        $("#photos-where").textContent +=
          " Acceso limitado: solo los elementos elegidos en Ajustes del Sistema.";
      }
      showPhotosBrowser();
      await loadPhotos();
    } else {
      showPhotosPermission(access.status);
    }
  } catch (error) {
    hidePhotos();
    reportError(error);
  }
}

async function handlePhotosPermission() {
  const button = $("#photos-grant") as HTMLButtonElement;
  const previous = button.textContent;
  button.disabled = true;
  button.textContent = "Comprobando…";
  try {
    const access =
      state.photos.status === "not_determined"
        ? await invoke<PhotosAccess>("photos_request_access")
        : await invoke<PhotosAccess>("photos_access_status");
    state.photos.available = access.available;
    state.photos.status = access.status;
    if (access.status === "authorized" || access.status === "limited") {
      showPhotosBrowser();
      await loadPhotos();
    } else {
      showPhotosPermission(access.status);
    }
  } catch (error) {
    reportError(error);
    showPhotosPermission(state.photos.status);
  } finally {
    button.disabled = false;
    if (button.textContent === "Comprobando…") button.textContent = previous;
  }
}

function selectPendingPhotos() {
  const existing = existingPhotoKeys();
  state.photos.selected.clear();
  for (const asset of visiblePhotos(existing)) {
    if (!photoInTarget(asset, existing) && !photoMatch(asset)?.copies.length) {
      state.photos.selected.add(asset.id);
    }
  }
  renderPhotos();
}

function selectAllPhotos() {
  state.photos.selected.clear();
  for (const asset of visiblePhotos(existingPhotoKeys())) {
    state.photos.selected.add(asset.id);
  }
  renderPhotos();
}

function clearPhotoSelection() {
  state.photos.selected.clear();
  renderPhotos();
}

function photosUploadSelected() {
  const assets = state.photos.assets.filter((asset) => state.photos.selected.has(asset.id));
  if (assets.length) void uploadPhotos(assets);
}

async function uploadPhotos(assets: PhotoAsset[]) {
  if (!assets.length || state.photos.uploading) return;
  if (!state.session?.logged_in) {
    hidePhotos();
    showLoginModal();
    return;
  }

  const uploadButton = $("#photos-upload") as HTMLButtonElement;
  const uploadLabel = uploadButton.textContent || "Subir seleccionados";
  state.photos.uploading = true;
  updatePhotosSelection();

  try {
    // The library check runs in the background; wait for it so the
    // "already in O2" question is based on the latest shared snapshots.
    if (state.photos.syncingLibrary) {
      uploadButton.textContent = "Comprobando duplicados…";
      if (state.photos.matchPromise) await state.photos.matchPromise;
      uploadButton.textContent = uploadLabel;
    }

    const crumb = currentCrumb();
    if (crumb.id != null) knownEmptyFolders.delete(crumb.id);
    const destination = crumb.virtual ? "/" : crumb.path;
    const duplicates = assets.filter((asset) => {
      const match = photoMatch(asset);
      return match && !match.in_target && match.copies.length > 0;
    });
    const duplicateIds = new Set(duplicates.map((asset) => asset.id));
    let toUpload = assets;
    if (duplicates.length) {
      const choice = await askLibraryConflict(
        duplicates.map((asset) => ({
          id: asset.id,
          name: asset.name,
          copies: photoMatch(asset)?.copies ?? [],
        })),
        destination,
      );
      if (choice.kind === "cancel") return;
      toUpload = assets.filter(
        (asset) => !duplicateIds.has(asset.id) || choice.upload.has(asset.id),
      );
    }
    if (!toUpload.length) {
      hidePhotos();
      toast("No hay nada que subir: ya estaba todo en O2.", "success");
      return;
    }

    const view = crumbKey(crumb);
    const refreshIfStillHere = () => {
      if (crumbKey(currentCrumb()) === view) void refresh({ fresh: true });
    };
    const transferIds = toUpload.map((asset) =>
      queueTransfer("upload", asset.name, asset.size),
    );
    hidePhotos();
    renderQueue();
    openTransfers();
    state.uploadsInFlight++;
    try {
      const summary = await invoke<UploadSummary>("upload_photos", {
        assetIds: toUpload.map((asset) => asset.id),
        folderId: crumb.id,
        folderPath: crumb.path,
        transferIds,
      });
      reportUploadSummary(summary);
      await refresh({ fresh: true });
      // O2 is eventually consistent on writes; refresh a couple more times.
      window.setTimeout(refreshIfStillHere, 5000);
      window.setTimeout(refreshIfStillHere, 20000);
    } catch (error) {
      reportError(error);
    } finally {
      state.uploadsInFlight--;
      failQueued(transferIds, "No se ha subido");
    }
  } finally {
    state.photos.uploading = false;
    uploadButton.textContent = uploadLabel;
    if (!$("#photos-modal").classList.contains("hidden")) updatePhotosSelection();
  }
}

/** One file: ask where to save it. Several: ask for a folder; nothing in it is replaced. */
async function downloadFiles(files: LogicalFile[], into?: string) {
  if (!files.length) return;
  let jobs: { file: LogicalFile; dest: string; overwrite: boolean }[];
  if (files.length === 1 && !into) {
    const dest = await save({
      title: "Guardar archivo",
      defaultPath: safeDownloadName(files[0].name),
    });
    if (!dest) return;
    jobs = [{ file: files[0], dest, overwrite: true }];
  } else {
    const dir =
      into ??
      (await open({
        directory: true,
        title: `Elige dónde guardar ${files.length} archivos`,
      }));
    if (!dir || Array.isArray(dir)) return;
    jobs = await Promise.all(
      files.map(async (file) => ({
        file,
        dest: await join(dir, safeDownloadName(file.name)),
        overwrite: false,
      })),
    );
  }

  const transferIds = jobs.map((job) => queueTransfer("download", job.file.name, job.file.size));
  renderQueue();
  openTransfers();
  const folderId = currentCrumb().id;
  let failed = 0;
  for (const [index, job] of jobs.entries()) {
    try {
      await invoke("download_entry", {
        transferId: transferIds[index],
        id: job.file.normal_id ?? 0,
        name: job.file.name,
        dest: job.dest,
        chunked: job.file.chunked,
        cliSplit: job.file.cli_split,
        manifestId: job.file.manifest_id,
        parts: job.file.parts,
        size: job.file.size,
        folderId,
        folderPath: currentCrumb().path,
        overwrite: job.overwrite,
      });
    } catch (error) {
      failed++;
      if (jobs.length === 1 || isSessionError(String(error))) {
        reportError(error);
        if (jobs.length > 1) break;
      }
    }
  }
  failQueued(transferIds, "Cancelada");
  if (jobs.length === 1) {
    if (!failed) toast(`Descarga completada: ${jobs[0].file.name}`, "success");
  } else if (failed) {
    toast(
      `Descargados ${jobs.length - failed} de ${jobs.length}. Los errores están en Transferencias.`,
      "error",
      10000,
    );
  } else {
    toast(`Descargados ${jobs.length} archivos`, "success");
  }
}

interface FolderDownload {
  files: number;
  downloaded: number;
  skipped: number;
  joined: number;
  failed: string[];
}

/**
 * Download folders, with everything in them, and any files selected with
 * them, into a local folder chosen once. Each folder lands as a folder of
 * its name; files already there with the same size are skipped and cut
 * downloads resumed, so downloading again carries on.
 */
async function downloadFolders(folders: RemoteFolder[], files: LogicalFile[] = []) {
  const count = folders.length + files.length;
  const dir = await open({
    directory: true,
    title: count === 1 ? `Elige dónde guardar «${folders[0].name}»` : `Elige dónde guardar ${count} elementos`,
  });
  if (!dir || Array.isArray(dir)) return;

  const transferIds = folders.map((folder) => queueTransfer("download", folder.name, folder.size));
  renderQueue();
  openTransfers();
  let failed = 0;
  let incomplete = 0;
  for (const [index, folder] of folders.entries()) {
    try {
      const result = await invoke<FolderDownload>("download_folder", {
        transferId: transferIds[index],
        id: folder.id,
        name: folder.name,
        dest: dir,
      });
      if (result.failed.length) incomplete++;
    } catch (error) {
      failed++;
      if (folders.length === 1 || isSessionError(String(error))) {
        reportError(error);
        if (folders.length > 1) break;
      }
    }
  }
  failQueued(transferIds, "Cancelada");
  if (files.length) await downloadFiles(files, dir);

  if (failed || incomplete) {
    if (folders.length > 1 || incomplete) {
      toast(
        "Algunas carpetas no se han descargado enteras. Los detalles están en Transferencias; descargarlas otra vez continúa donde se quedaron.",
        "error",
        12000,
      );
    }
  } else {
    toast(
      folders.length === 1 ? `Carpeta descargada: ${folders[0].name}` : `${folders.length} carpetas descargadas`,
      "success",
    );
  }
}

async function trashEntries(entries: (FolderEntry | FileEntry)[]) {
  if (!entries.length) return;
  const files = entries.filter((entry): entry is FileEntry => entry.kind === "file");
  const folders = entries.filter((entry): entry is FolderEntry => entry.kind === "folder");
  const what = entries.length === 1 ? `«${entries[0].name}»` : `${entries.length} elementos`;
  const note = folders.length
    ? folders.length === 1
      ? " La carpeta se mueve con todo su contenido."
      : " Las carpetas se mueven con todo su contenido."
    : groupedView()
      ? " Se borra el archivo de O2 Cloud, no solo de esta vista."
      : "";
  const ok = await confirm(`¿Mover ${what} a la papelera de O2?${note}`, {
    title: "Mover a la papelera",
    kind: "warning",
  });
  if (!ok) return;

  const crumb = currentCrumb();
  const view = crumbKey(crumb);
  const keys = new Set(entries.map((entry) => entry.key));
  let deleted = false;

  // Each CLI delete lists the whole account first, so hiding the rows now
  // is the difference between "nothing happens" and an obvious action.
  rememberRemovals(view, keys);
  if (crumb.virtual) rememberRemovals(crumbKey(ROOT), keys);
  for (const entry of entries) state.trashing.add(entry.key);
  removeEntriesFromView(view, keys);
  state.selected.clear();
  const operation = queueOperation("delete", what, "Eliminando…", true);

  try {
    await invoke("delete_entries", {
      // Field names inside the structs stay as in Rust.
      files: files.map(({ file }) =>
        file.chunked
          ? { ids: file.parts.map((part) => part.id), manifest_id: file.manifest_id }
          : { ids: file.normal_id != null ? [file.normal_id] : [], manifest_id: null },
      ),
      folders: folders.map(({ folder }) => folder.id),
    });
    deleted = true;
    finishOperation(
      operation,
      entries.length === 1 ? "Movido a la papelera" : "Movidos a la papelera",
    );
    toast(
      entries.length === 1 ? `${what} movido a la papelera` : `${what} movidos a la papelera`,
      "success",
    );
  } catch (error) {
    finishOperation(operation, String(error), true);
    reportError(error);
    forgetRemovals(view, keys);
    if (crumb.virtual) forgetRemovals(crumbKey(ROOT), keys);
  } finally {
    for (const entry of entries) state.trashing.delete(entry.key);
    state.selected.clear();
    if (!deleted) {
      // O2 rejected the delete (or only part of it): restore the real rows.
      void refresh({ fresh: true });
    }
  }
}

function createFolder() {
  // Moments, places, music and documents aren't folders.
  if (groupedView()) return;
  // Virtual views create in the root, like their uploads.
  const where = currentCrumb().virtual ? [ROOT] : state.stack;
  $("#folder-where").textContent = `En ${where.map((crumb) => crumb.name).join(" › ")}`;
  const input = $("#folder-name") as HTMLInputElement;
  input.value = "";
  $("#folder-error").textContent = "";
  $("#folder-modal").classList.remove("hidden");
  input.focus();
}

function hideFolderModal() {
  $("#folder-modal").classList.add("hidden");
}

async function submitFolder(event: SubmitEvent) {
  event.preventDefault();
  const crumb = currentCrumb();
  const name = ($("#folder-name") as HTMLInputElement).value.trim();
  const error = $("#folder-error");
  if (!name) {
    error.textContent = "Escribe un nombre.";
    return;
  }
  if (name.includes("/")) {
    error.textContent = "El nombre no puede llevar «/».";
    return;
  }
  const here = crumb.virtual ? state.rootFolders : (state.listing?.folders ?? []);
  if (here.some((folder) => folder.name === name)) {
    error.textContent = "Ya hay una carpeta con ese nombre aquí.";
    return;
  }

  const button = $("#folder-create") as HTMLButtonElement;
  button.disabled = true;
  button.textContent = "Creando…";
  error.textContent = "";
  try {
    const created = await invoke<CreatedFolder>("create_folder", {
      parentId: crumb.id,
      parentPath: crumb.path,
      name,
    });
    const folder = created.folder;
    if (created.created) {
      knownEmptyFolders.set(folder.id, Date.now() + EMPTY_FOLDER_GRACE_MS);
    }
    hideFolderModal();
    toast(`Carpeta «${name}» creada`, "success");
    // Virtual views create at the root, so that is the view to remember.
    const view = crumb.virtual ? crumbKey(ROOT) : crumbKey(crumb);
    rememberFolderAdd(view, folder);
    // A new folder only changes this view. Inserting it avoids the full
    // `ls` (up to a minute on a large account) the old refresh triggered.
    if (crumb.virtual) {
      if (!state.rootFolders.some((item) => item.id === folder.id)) {
        state.rootFolders.push(folder);
      }
    } else if (listingView === view && state.listing) {
      if (!state.listing.folders.some((item) => item.id === folder.id)) {
        state.listing.folders.push(folder);
      }
      if (state.stack.length === 1 && !state.rootFolders.some((item) => item.id === folder.id)) {
        state.rootFolders.push(folder);
      }
    } else if (!state.listing && crumbKey(currentCrumb()) === view) {
      // The first listing is still loading (it can take a minute on a large
      // account). Show the folder that was just created instead of a spinner.
      state.listing = {
        path: crumb.path,
        folders: [folder],
        files: [],
        warnings: [],
        virtual_folders: [],
        stale: false,
      };
      if (state.stack.length === 1 && !state.rootFolders.some((item) => item.id === folder.id)) {
        state.rootFolders.push(folder);
      }
    }
    // The table keeps its own sorted view of the listing; invalidate it so
    // the folder appears now, not after the next refresh.
    entriesCache.listing = null;
    renderPlaces();
    renderListing();
  } catch (err) {
    const message = String(err);
    if (isSessionError(message)) {
      hideFolderModal();
      reportError(err);
    } else {
      error.textContent = message;
    }
  } finally {
    button.disabled = false;
    button.textContent = "Crear";
  }
}

async function startLogin() {
  $("#login-btn").setAttribute("disabled", "true");
  setLoginStatus("Abriendo la ventana de login de O2…");
  try {
    const status = await invoke<SessionStatus>("login");
    state.session = status;
    renderSession();
    if (!status.logged_in) {
      setLoginStatus(
        "El CLI no ha confirmado la sesión. Vuelve a intentarlo.",
        "error",
      );
      return;
    }
    hideLoginModal();
    toast("Sesión iniciada", "success");
    await refresh({ fresh: true });
  } catch (error) {
    setLoginStatus(String(error), "error");
  } finally {
    $("#login-btn").removeAttribute("disabled");
  }
}

// ---------- Settings ----------

function populateSettings() {
  ($("#group-camera") as HTMLInputElement).checked = state.settings.group_camera;
  ($("#slideshow-music") as HTMLSelectElement).value = state.settings.slideshow_music || "forget-me-not";
  ($("#hash-non-media") as HTMLInputElement).checked = state.settings.hash_non_media;

  $("#settings-app-info").textContent = appVersion ? `Versión: ${appVersion}` : "";

  const cliDetails = $("#settings-cli-info");
  cliDetails.replaceChildren();
  if (state.cli?.available) {
    const lines = [
      `Versión: ${state.cli.version} (mínima ${state.cli.minimum_version})`,
      `Origen: ${state.cli.source}`,
      `Ruta: ${state.cli.path}`,
      state.cli.supported
        ? ""
        : "El CLI es demasiado antiguo. Pulsa «Buscar actualización» para actualizarlo.",
    ].filter(Boolean);
    for (const line of lines) {
      const div = document.createElement("div");
      div.textContent = line;
      cliDetails.appendChild(div);
    }
  } else {
    cliDetails.textContent =
      "CLI no instalado. Pulsa «Buscar actualización» para descargar la última versión.";
  }

  const libraryDetails = $("#settings-library");
  libraryDetails.replaceChildren();
  const stats = state.libraryStats;
  const libraryLines = stats
    ? [
        `${plural(stats.files, "archivo", "archivos")} · ${formatSize(stats.bytes)}`,
        `${plural(stats.copies, "copia", "copias")} en O2 · ${plural(stats.devices, "dispositivo", "dispositivos")}`,
        `Última sincronización: ${formatAgo(stats.last_sync)}`,
        stats.pending_snapshot ? "Hay cambios pendientes de subir a O2." : "",
      ].filter(Boolean)
    : ["Sin sincronizar todavía."];
  for (const line of libraryLines) {
    const div = document.createElement("div");
    div.textContent = line;
    libraryDetails.appendChild(div);
  }

  const sessionDetails = $("#settings-session");
  sessionDetails.replaceChildren();
  if (state.session?.logged_in) {
    const lines = [
      "Sesión activa",
      state.session.phone ? `Teléfono: ${state.session.phone}` : "",
      state.session.storage
        ? `Almacenamiento: ${storageLabel(state.session.storage)}`
        : "",
    ].filter(Boolean);
    for (const line of lines) {
      const div = document.createElement("div");
      div.textContent = line;
      sessionDetails.appendChild(div);
    }
  } else {
    sessionDetails.textContent = "No hay sesión iniciada.";
  }
}

async function saveSettings() {
  const groupCamera = ($("#group-camera") as HTMLInputElement).checked;
  const hashNonMedia = ($("#hash-non-media") as HTMLInputElement).checked;
  const slideshowMusic = ($("#slideshow-music") as HTMLSelectElement).value;
  try {
    state.settings = await invoke<Settings>("save_settings", {
      settings: {
        group_camera: groupCamera,
        hash_non_media: hashNonMedia,
        slideshow_music: slideshowMusic,
      },
    });
    toast("Ajustes guardados", "success");
    // The root view changes as soon as grouping is toggled. If we are inside
    // a virtual view and grouping is turned off, go back to the root.
    const inCamera = state.stack.some((c) => c.virtual?.startsWith("camera"));
    if (inCamera && !state.settings.group_camera) {
      navigate([ROOT]);
    } else if (state.stack.length === 1) {
      await refresh();
    } else {
      renderPlaces();
    }
  } catch (error) {
    toast(String(error), "error");
  }
}

async function syncUploadLibrary(reconcile: boolean) {
  const syncButton = $("#settings-library-sync") as HTMLButtonElement;
  const reconcileButton = $("#settings-library-reconcile") as HTMLButtonElement;
  const forgetButton = $("#settings-library-forget") as HTMLButtonElement;
  const indexButton = $("#settings-library-index") as HTMLButtonElement;
  syncButton.disabled = true;
  reconcileButton.disabled = true;
  forgetButton.disabled = true;
  indexButton.disabled = true;
  const details = $("#settings-library");
  details.textContent = reconcile ? "Reconciliando con O2…" : "Sincronizando biblioteca…";
  const operation = queueOperation(
    "library",
    reconcile ? "Reconciliar biblioteca" : "Sincronizar biblioteca",
    reconcile ? "Reconciliando con O2…" : "Sincronizando biblioteca…",
  );
  try {
    const stats = reconcile
      ? await invoke<LibraryStats>("library_reconcile")
      : await invoke<LibraryStats>("library_sync", { force: true });
    state.libraryStats = stats;
    finishOperation(operation, "Biblioteca actualizada");
    toast(
      reconcile
        ? "Biblioteca reconciliada con O2"
        : `Biblioteca sincronizada: ${plural(stats.files, "archivo", "archivos")}`,
      "success",
    );
  } catch (error) {
    finishOperation(operation, String(error), true);
    reportError(error);
  } finally {
    syncButton.disabled = false;
    reconcileButton.disabled = false;
    forgetButton.disabled = false;
    indexButton.disabled = false;
    populateSettings();
  }
}

async function forgetUploadLibrary() {
  const syncButton = $("#settings-library-sync") as HTMLButtonElement;
  const reconcileButton = $("#settings-library-reconcile") as HTMLButtonElement;
  const forgetButton = $("#settings-library-forget") as HTMLButtonElement;
  const indexButton = $("#settings-library-index") as HTMLButtonElement;
  const ok = await confirm(
    "Se borrarán los hashes y los snapshots de este equipo en O2. Los snapshots de otros equipos se conservarán y se volverán a sincronizar. ¿Olvidar la biblioteca de este equipo?",
    {
      title: "Olvidar biblioteca",
      kind: "warning",
    },
  );
  if (!ok) return;

  syncButton.disabled = true;
  reconcileButton.disabled = true;
  forgetButton.disabled = true;
  indexButton.disabled = true;
  const details = $("#settings-library");
  details.textContent = "Olvidando la biblioteca de este equipo…";
  const operation = queueOperation(
    "library",
    "Olvidar biblioteca",
    "Borrando snapshots de este equipo…",
  );
  try {
    const stats = await invoke<LibraryStats>("library_forget");
    state.libraryStats = stats;
    finishOperation(operation, "Biblioteca olvidada");
    toast("Biblioteca olvidada en este equipo", "success");
  } catch (error) {
    finishOperation(operation, String(error), true);
    reportError(error);
  } finally {
    syncButton.disabled = false;
    reconcileButton.disabled = false;
    forgetButton.disabled = false;
    indexButton.disabled = false;
    populateSettings();
  }
}

async function indexLibraryManifests() {
  const syncButton = $("#settings-library-sync") as HTMLButtonElement;
  const reconcileButton = $("#settings-library-reconcile") as HTMLButtonElement;
  const forgetButton = $("#settings-library-forget") as HTMLButtonElement;
  const indexButton = $("#settings-library-index") as HTMLButtonElement;
  syncButton.disabled = true;
  reconcileButton.disabled = true;
  forgetButton.disabled = true;
  indexButton.disabled = true;
  const details = $("#settings-library");
  details.textContent = "Indexando manifiestos…";
  const before = state.libraryStats?.files ?? 0;
  const operation = queueOperation(
    "library",
    "Indexar manifiestos",
    "Leyendo manifiestos…",
  );
  try {
    const stats = await invoke<LibraryStats>("library_index_manifests");
    state.libraryStats = stats;
    finishOperation(operation, "Manifiestos indexados");
    const added = stats.files - before;
    toast(
      added > 0
        ? `Biblioteca ampliada con ${plural(added, "archivo", "archivos")} de manifiestos.`
        : "No hay manifiestos nuevos que indexar.",
      "success",
    );
  } catch (error) {
    finishOperation(operation, String(error), true);
    reportError(error);
  } finally {
    syncButton.disabled = false;
    reconcileButton.disabled = false;
    forgetButton.disabled = false;
    indexButton.disabled = false;
    populateSettings();
  }
}

/** Show a CLI release found on GitHub in Settings and, if it's newer, the
 * button to install it. */
function showCliRelease(release: CliRelease) {
  const result = $("#cli-update-result");
  const install = $("#settings-install-update");
  state.pendingRelease = release;
  if (release.update_available && release.latest && release.asset_url) {
    result.textContent = `Hay una versión nueva: ${release.latest} (actual: ${release.current}).`;
    install.textContent = `Actualizar CLI a ${release.latest}`;
    install.classList.remove("hidden");
  } else {
    result.textContent = release.latest
      ? `Estás al día (${release.current}). Última publicada: ${release.latest}.`
      : "No se pudo determinar la última versión.";
    install.classList.add("hidden");
  }
}

async function checkCliUpdate() {
  const result = $("#cli-update-result");
  result.textContent = "Consultando GitHub…";
  $("#settings-install-update").classList.add("hidden");
  try {
    showCliRelease(await invoke<CliRelease>("check_cli_update"));
  } catch (error) {
    result.textContent = String(error);
  }
}

/** Closes the notification of a new CLI, if it's showing. */
let dismissCliUpdateNotice: (() => void) | null = null;

/**
 * On startup, ask GitHub whether there's a newer CLI and, if there is, say
 * so with a button that opens Settings at «Actualizar CLI». Offline or
 * without an answer it says nothing. Not with `O2CLOUD_CLI`: that CLI was
 * chosen by hand, and one installed from Settings wouldn't replace it.
 */
async function notifyCliUpdate() {
  if (state.cli?.source === "env") return;
  let release: CliRelease;
  try {
    release = await invoke<CliRelease>("check_cli_update");
  } catch {
    return;
  }
  showCliRelease(release);
  if (!release.update_available || !release.latest || !release.asset_url) return;
  // Settings is already open (no CLI yet, or too old) with the button ready.
  if (!$("#settings-modal").classList.contains("hidden")) return;
  const current = state.cli?.available ? ` (tienes la ${release.current})` : "";
  dismissCliUpdateNotice = toast(
    `Hay una versión nueva del CLI de O2 Cloud: ${release.latest}${current}.`,
    "info",
    0,
    {
      label: "Abrir Ajustes para actualizar",
      run: () => openSettingsAt("#settings-install-update"),
    },
  );
}

/** Open Settings with focus on one of its controls. */
function openSettingsAt(selector: string) {
  showSettings();
  // Runs before the dialog's own focus handling, which then leaves it.
  window.setTimeout(() => {
    const target = $(selector);
    target.scrollIntoView({ block: "center" });
    target.focus();
  }, 0);
}

// ---------- Updates of the app itself ----------

/** The app's version, shown in Settings. */
let appVersion = "";
/** A newer version found by the last check, ready to install. */
let pendingAppUpdate: Update | null = null;
/** Closes the notification of a new app version, if it's showing. */
let dismissAppUpdateNotice: (() => void) | null = null;

/** Whether an upload, download or other operation is still going on. */
function transfersRunning(): boolean {
  return [...state.queue.values()].some((item) => !isFinished(item));
}

/** Show the result of a check in Settings and, if there's a newer version,
 * the button to install it. */
function showAppUpdate(update: Update | null) {
  const result = $("#app-update-result");
  const install = $("#settings-install-app-update");
  pendingAppUpdate = update;
  if (update) {
    result.textContent = `Hay una versión nueva: ${update.version} (actual: ${update.currentVersion}).`;
    install.textContent = `Actualizar a ${update.version} y reiniciar`;
    install.classList.remove("hidden");
  } else {
    result.textContent = `Estás al día (${appVersion}).`;
    install.classList.add("hidden");
  }
}

async function checkAppUpdate() {
  const result = $("#app-update-result");
  result.textContent = "Consultando GitHub…";
  $("#settings-install-app-update").classList.add("hidden");
  try {
    showAppUpdate(await check());
  } catch (error) {
    result.textContent = `No se pudo comprobar: ${String(error)}`;
  }
}

/**
 * On startup, ask the latest release on GitHub whether there's a newer app
 * and, if there is, say so with a button that opens Settings at
 * «Actualizar y reiniciar». Offline or without an answer it says nothing.
 */
async function notifyAppUpdate() {
  let update: Update | null;
  try {
    update = await check();
  } catch {
    return;
  }
  showAppUpdate(update);
  if (!update) return;
  dismissAppUpdateNotice = toast(
    `Hay una versión nueva de O2 Cloud: ${update.version} (tienes la ${update.currentVersion}).`,
    "info",
    0,
    {
      label: "Abrir Ajustes para actualizar",
      run: () => openSettingsAt("#settings-install-app-update"),
    },
  );
}

/**
 * Download the new version, check its signature, install it and restart.
 * Not while a transfer runs: the restart (and on Windows the installer)
 * would cut it.
 */
async function installAppUpdate() {
  const update = pendingAppUpdate;
  if (!update) return;
  const result = $("#app-update-result");
  if (transfersRunning()) {
    result.textContent =
      "Hay transferencias en curso. Cuando terminen, vuelve a pulsar «Actualizar».";
    return;
  }
  const button = $("#settings-install-app-update") as HTMLButtonElement;
  button.disabled = true;
  dismissAppUpdateNotice?.();
  let total = 0;
  let received = 0;
  result.textContent = `Descargando ${update.version}…`;
  try {
    await update.downloadAndInstall((event) => {
      if (event.event === "Started") {
        total = event.data.contentLength ?? 0;
      } else if (event.event === "Progress") {
        received += event.data.chunkLength;
        result.textContent = total
          ? `Descargando ${update.version}… ${Math.min(100, Math.round((received * 100) / total))} %`
          : `Descargando ${update.version}… ${formatSize(received)}`;
      } else if (event.event === "Finished") {
        result.textContent = `Instalando ${update.version}…`;
      }
    });
  } catch (error) {
    result.textContent = `No se pudo actualizar: ${String(error)}`;
    button.disabled = false;
    return;
  }
  // Installed. A transfer started meanwhile goes on; the new version
  // starts with the next launch.
  if (transfersRunning()) {
    result.textContent = `${update.version} instalada. Se usará al volver a abrir O2 Cloud.`;
    button.classList.add("hidden");
    button.disabled = false;
    pendingAppUpdate = null;
    return;
  }
  result.textContent = "Reiniciando…";
  await relaunch();
}

async function installCliUpdate() {
  const release = state.pendingRelease;
  if (!release?.latest || !release.asset_url) return;
  const result = $("#cli-update-result");
  result.textContent = `Descargando ${release.latest}…`;
  $("#settings-install-update").setAttribute("disabled", "true");
  const operation = queueOperation(
    "cli",
    `Actualizar CLI a ${release.latest}`,
    `Descargando ${release.latest}…`,
  );
  try {
    const info = await invoke<CliInfo>("install_cli_update", {
      version: release.latest,
      url: release.asset_url,
    });
    state.cli = info;
    // Installed: nothing left to update until the next check finds more.
    state.pendingRelease = null;
    $("#settings-install-update").classList.add("hidden");
    dismissCliUpdateNotice?.();
    populateSettings();
    result.textContent = `CLI actualizado a ${info.version}.`;
    finishOperation(operation, "CLI actualizado");
    toast("CLI actualizado", "success");
    if (!state.session?.logged_in) {
      hideSettings();
      showLoginModal();
      setLoginStatus("CLI instalado. Inicia sesión para continuar.");
    }
  } catch (error) {
    finishOperation(operation, String(error), true);
    result.textContent = String(error);
    toast(String(error), "error", 10000);
  } finally {
    $("#settings-install-update").removeAttribute("disabled");
  }
}

async function logout() {
  try {
    await invoke("logout");
    // Signed links play without a session: nothing goes on after logout.
    if (slideshowOpen()) closeSlideshow();
    stopPlayer();
    tracks = [];
    allTracks = [];
    state.session = {
      logged_in: false,
      raw: "",
      phone: null,
      since: null,
      session: null,
      renews_silently: false,
      storage: null,
    };
    renderSession();
    populateSettings();
    toast("Sesión cerrada", "success");
    showLoginModal();
  } catch (error) {
    toast(String(error), "error");
  }
}

// ---------- Keyboard ----------

function isTyping(target: EventTarget | null): boolean {
  return (
    (target instanceof HTMLInputElement && target.type !== "checkbox") ||
    target instanceof HTMLTextAreaElement
  );
}

function onKeyDown(event: KeyboardEvent) {
  const mod = event.metaKey || event.ctrlKey;
  const key = event.key;
  const search = $("#search") as HTMLInputElement;
  // The slideshow and the viewer cover the window: no shortcut reaches
  // what's behind.
  if (slideshowOpen()) {
    slideshowKey(event);
    return;
  }
  if (viewerOpen()) {
    viewerKey(event);
    return;
  }
  // Enter on a button, link or map marker presses it, as everywhere.
  const target = event.target as HTMLElement | null;
  if (
    key === "Enter" &&
    target?.closest("button, a[href], [role=button], .leaflet-marker-icon, select")
  ) {
    return;
  }

  if (key === "Escape") {
    if (menuOpen()) hideMenu();
    else if (transfersOpen()) closeTransfers();
    else if (anyModalOpen()) {
      if (conflictResolve) {
        // Cancel the duplicate prompt but leave the Photos view open.
        finishLibraryConflict("cancel");
      } else if (uploadConflictResolve) {
        // Escape skips them all, as Continuar with nothing ticked does.
        setAllUploadConflicts(false);
        finishUploadConflicts();
      } else {
        hideSettings();
        hideFolderModal();
        hidePhotos();
        if (state.session?.logged_in) hideLoginModal();
      }
    } else if (event.target === search && search.value) {
      search.value = "";
      state.filter = "";
      renderListing();
    } else if (event.target === search) search.blur();
    else if (state.selected.size) clearSelection();
    return;
  }
  if (anyModalOpen()) return;
  if (menuOpen() && (key === "ArrowDown" || key === "ArrowUp")) {
    event.preventDefault();
    moveMenuFocus(key === "ArrowDown" ? 1 : -1);
    return;
  }
  if (mod && key.toLowerCase() === "r") {
    event.preventDefault();
    void refresh({ fresh: true });
    return;
  }
  if (mod && key.toLowerCase() === "f") {
    event.preventDefault();
    search.focus();
    search.select();
    return;
  }
  if (isTyping(event.target)) return;

  if (mod && event.shiftKey && key.toLowerCase() === "n") {
    event.preventDefault();
    createFolder();
  } else if (mod && key.toLowerCase() === "a") {
    event.preventDefault();
    selectAll();
  } else if (mod && (key === "ArrowUp" || key === "[")) {
    event.preventDefault();
    goUp();
  } else if (key === "Enter" || (mod && key === "ArrowDown")) {
    event.preventDefault();
    openSelection();
  } else if (key === "Delete" || (mod && key === "Backspace")) {
    event.preventDefault();
    void trashEntries(selectedEntries());
  } else if (!mod && (key === "ArrowDown" || key === "ArrowUp")) {
    event.preventDefault();
    moveSelection(key === "ArrowDown" ? 1 : -1, event.shiftKey);
  } else if (!mod && key === " ") {
    // Space: a preview, as Quick Look.
    const files = selectedEntries().filter((e): e is FileEntry => e.kind === "file");
    if (files.length === 1 && previewable(files[0].file)) {
      event.preventDefault();
      openViewer(files[0].file);
    }
  }
}

// ---------- Start ----------

/** Replace the macOS-only symbols in tooltips and the shortcuts panel. */
function applyPlatformLabels() {
  if (IS_MAC) return;
  const titles: Record<string, string> = {
    "back-btn": `Carpeta superior (${SHORTCUT.up})`,
    "refresh-btn": `Recargar (${SHORTCUT.refresh})`,
    "search-label": `Buscar en esta carpeta (${SHORTCUT.search})`,
    "new-folder-btn": `Nueva carpeta (${SHORTCUT.newFolder})`,
  };
  for (const [id, title] of Object.entries(titles)) {
    const element = document.getElementById(id);
    if (element) element.title = title;
  }
  const shortcuts: Record<string, string> = {
    refresh: SHORTCUT.refresh,
    search: SHORTCUT.search,
    up: SHORTCUT.up,
    selectAll: SHORTCUT.selectAll,
    trash: SHORTCUT.trash,
    newFolder: SHORTCUT.newFolder,
    open: SHORTCUT.open,
  };
  for (const element of document.querySelectorAll<HTMLElement>("[data-shortcut]")) {
    const key = element.dataset.shortcut;
    if (key && shortcuts[key]) element.textContent = shortcuts[key];
  }
  const arrows = document.querySelector<HTMLElement>("[data-shortcut-help='arrows']");
  if (arrows) arrows.textContent = "Moverse por la lista (con Mayús, ampliar la selección)";
}

async function bootstrap() {
  applyPlatformLabels();
  renderPathbar();
  renderPlaces();
  renderQueue();

  try {
    state.cli = await invoke<CliInfo>("get_cli_info");
  } catch (error) {
    toast(String(error), "error", 10000);
  }

  try {
    appVersion = await getVersion();
  } catch {
    // Settings just doesn't show it.
  }

  try {
    state.settings = await invoke<Settings>("get_settings");
    renderPlaces();
  } catch {
    // defaults
  }

  try {
    const access = await invoke<PhotosAccess>("photos_access_status");
    state.photos.available = access.available;
    state.photos.status = access.status;
  } catch {
    // Not macOS, or an older build without the Photos bridge.
    state.photos.available = false;
  }

  try {
    state.libraryStats = await invoke<LibraryStats>("library_status");
  } catch {
    // The library is optional; uploads still work without it.
  }

  await listen<TransferProgress>("transfer-progress", (event) => {
    const item = event.payload;
    trackRate(item);
    state.queue.set(item.id, item);
    forgetOldTransfers();
    renderQueue();
  });

  await listen<string>("library-warning", (event) => toast(event.payload, "error", 12000));
  await listen<{ stream: string; line: string }>("cli-log", (event) => {
    console.log(`[o2cloud:${event.payload.stream}] ${event.payload.line}`);
  });

  const webview = getCurrentWebview();
  await webview.onDragDropEvent((event) => {
    const payload = event.payload;
    const overlay = $("#drop-overlay");
    if (payload.type === "enter" || payload.type === "over") {
      overlay.classList.remove("hidden");
    } else if (payload.type === "drop") {
      overlay.classList.add("hidden");
      if (groupedView() === "labels") {
        toast("Aquí no se puede subir: suelta los archivos en una carpeta, en Música o en Documentos.", "info");
        return;
      }
      void prepareUploadPaths(payload.paths);
    } else {
      overlay.classList.add("hidden");
    }
  });

  // Toolbar and list
  $("#back-btn").addEventListener("click", goUp);
  $("#loading-retry").addEventListener("click", () => window.location.reload());
  $("#refresh-btn").addEventListener("click", () => void refresh({ fresh: true }));
  $("#search").addEventListener("input", (event) => {
    state.filter = (event.target as HTMLInputElement).value;
    state.visibleLimit = PAGE;
    state.selected.clear();
    renderListing();
  });
  $("#upload-btn").addEventListener("click", (event) => {
    event.stopPropagation();
    const rect = (event.currentTarget as HTMLElement).getBoundingClientRect();
    const items: MenuEntry[] = [
      { label: "Archivos…", run: () => void chooseFiles(false) },
      { label: "Carpeta…", run: () => void chooseFiles(true) },
    ];
    if (state.photos.available) {
      items.push("separator", { label: "Desde Fotos…", run: () => void openPhotos() });
    }
    showMenu(items, rect.left, rect.bottom + 4);
  });
  $("#new-folder-btn").addEventListener("click", createFolder);
  $("#selection-download").addEventListener("click", downloadSelected);
  $("#selection-trash").addEventListener("click", () => void trashEntries(selectedEntries()));
  $("#selection-clear").addEventListener("click", clearSelection);
  $("#select-all").addEventListener("change", (event) => {
    if ((event.target as HTMLInputElement).checked) selectAll();
    else clearSelection();
  });
  for (const button of document.querySelectorAll<HTMLButtonElement>(".sort")) {
    button.addEventListener("click", () => {
      const key = button.dataset.sort as SortKey;
      // Names A→Z first; sizes and dates largest/newest first.
      state.sort =
        state.sort.key === key
          ? { key, dir: state.sort.dir === 1 ? -1 : 1 }
          : { key, dir: key === "name" ? 1 : -1 };
      saveSort();
      renderListing();
    });
  }
  $("#load-more").addEventListener("click", () => {
    state.visibleLimit += PAGE;
    renderListing();
  });
  const tableWrap = $("#table-wrap");
  tableWrap.addEventListener("click", (event) => {
    if (!(event.target as HTMLElement).closest("tr")) clearSelection();
  });
  tableWrap.addEventListener("contextmenu", (event) => {
    if ((event.target as HTMLElement).closest("thead")) return;
    event.preventDefault();
    clearSelection();
    showMenu(backgroundMenu(), event.clientX, event.clientY);
  });
  tableWrap.addEventListener("scroll", hideMenu, { passive: true });

  // Top bar
  $("#transfers-btn").addEventListener("click", (event) => {
    event.stopPropagation();
    if (transfersOpen()) closeTransfers();
    else openTransfers();
  });
  $("#transfers-clear").addEventListener("click", clearFinishedTransfers);
  $("#settings-btn").addEventListener("click", showSettings);
  $("#session-badge").addEventListener("click", () => {
    if (state.session?.logged_in) showSettings();
    else showLoginModal();
  });

  // Modals
  $("#settings-close").addEventListener("click", hideSettings);
  $("#login-btn").addEventListener("click", () => void startLogin());
  $("#login-cancel").addEventListener("click", hideLoginModal);
  $("#folder-form").addEventListener("submit", (event) => void submitFolder(event as SubmitEvent));
  $("#folder-cancel").addEventListener("click", hideFolderModal);
  $("#save-settings").addEventListener("click", () => void saveSettings());
  $("#settings-library-sync").addEventListener("click", () => void syncUploadLibrary(false));
  $("#settings-library-reconcile").addEventListener("click", () =>
    void syncUploadLibrary(true),
  );
  $("#settings-library-forget").addEventListener("click", () => void forgetUploadLibrary());
  $("#settings-library-index").addEventListener("click", () => void indexLibraryManifests());
  $("#settings-check-app-update").addEventListener("click", () => void checkAppUpdate());
  $("#settings-install-app-update").addEventListener("click", () => void installAppUpdate());
  $("#settings-check-update").addEventListener("click", () => void checkCliUpdate());
  $("#settings-install-update").addEventListener("click", () => void installCliUpdate());
  $("#settings-login").addEventListener("click", () => {
    hideSettings();
    showLoginModal();
  });
  $("#settings-logout").addEventListener("click", () => void logout());

  // Photos
  $("#photos-close").addEventListener("click", hidePhotos);
  $("#photos-cancel").addEventListener("click", hidePhotos);
  $("#photos-grant").addEventListener("click", () => void handlePhotosPermission());
  $("#photos-search").addEventListener("input", (event) => {
    state.photos.filter = (event.target as HTMLInputElement).value;
    state.photos.visibleLimit = PHOTO_PAGE;
    renderPhotos();
  });
  $("#photos-media").addEventListener("change", (event) => {
    state.photos.mediaType = (event.target as HTMLSelectElement).value;
    state.photos.selected.clear();
    state.photos.visibleLimit = PHOTO_PAGE;
    void loadPhotos();
  });
  $("#photos-only-large").addEventListener("change", (event) => {
    state.photos.onlyLarge = (event.target as HTMLInputElement).checked;
    state.photos.visibleLimit = PHOTO_PAGE;
    renderPhotos();
  });
  $("#photos-hide-existing").addEventListener("change", (event) => {
    state.photos.hideExisting = (event.target as HTMLInputElement).checked;
    state.photos.visibleLimit = PHOTO_PAGE;
    renderPhotos();
  });
  $("#photos-album").addEventListener("change", (event) => {
    state.photos.albumId = (event.target as HTMLSelectElement).value;
    state.photos.selected.clear();
    state.photos.visibleLimit = PHOTO_PAGE;
    void loadPhotos();
  });
  $("#photos-favorites").addEventListener("change", (event) => {
    state.photos.favoritesOnly = (event.target as HTMLInputElement).checked;
    state.photos.selected.clear();
    state.photos.visibleLimit = PHOTO_PAGE;
    void loadPhotos();
  });
  $("#photos-date-filter").addEventListener("change", (event) => {
    state.photos.dateFilter = (event.target as HTMLInputElement).checked;
    if (state.photos.dateFilter && !state.photos.startDate && !state.photos.endDate) {
      const dates = photoDefaultDateRange();
      state.photos.startDate = dates.start;
      state.photos.endDate = dates.end;
      ($("#photos-start") as HTMLInputElement).value = dates.start;
      ($("#photos-end") as HTMLInputElement).value = dates.end;
    }
    state.photos.selected.clear();
    state.photos.visibleLimit = PHOTO_PAGE;
    void loadPhotos();
  });
  for (const input of ["photos-start", "photos-end"]) {
    const element = $(`#${input}`) as HTMLInputElement;
    element.addEventListener("change", () => {
      // Read both: one field may have been cleared while the other remains.
      state.photos.startDate = ($("#photos-start") as HTMLInputElement).value;
      state.photos.endDate = ($("#photos-end") as HTMLInputElement).value;
      state.photos.dateFilter = Boolean(
        state.photos.startDate || state.photos.endDate,
      );
      ($("#photos-date-filter") as HTMLInputElement).checked =
        state.photos.dateFilter;
      state.photos.selected.clear();
      state.photos.visibleLimit = PHOTO_PAGE;
      void loadPhotos();
    });
  }
  $("#photos-dates-clear").addEventListener("click", () => {
    state.photos.startDate = "";
    state.photos.endDate = "";
    state.photos.dateFilter = false;
    ($("#photos-date-filter") as HTMLInputElement).checked = false;
    ($("#photos-start") as HTMLInputElement).value = "";
    ($("#photos-end") as HTMLInputElement).value = "";
    state.photos.selected.clear();
    state.photos.visibleLimit = PHOTO_PAGE;
    void loadPhotos();
  });
  $("#photos-select-pending").addEventListener("click", selectPendingPhotos);
  $("#photos-select-all").addEventListener("click", selectAllPhotos);
  $("#photos-select-none").addEventListener("click", clearPhotoSelection);
  $("#photos-more").addEventListener("click", () => void loadMorePhotos());
  $("#photos-upload").addEventListener("click", photosUploadSelected);

  // Upload-library conflicts
  $("#library-conflict-cancel").addEventListener("click", () => finishLibraryConflict("cancel"));
  $("#library-conflict-skip").addEventListener("click", () => setAllConflictChoices(false));
  $("#library-conflict-upload").addEventListener("click", () => setAllConflictChoices(true));
  $("#library-conflict-continue").addEventListener("click", () =>
    finishLibraryConflict("continue"),
  );
  $("#upload-conflict-skip").addEventListener("click", () => setAllUploadConflicts(false));
  $("#upload-conflict-overwrite").addEventListener("click", () => setAllUploadConflicts(true));
  $("#upload-conflict-continue").addEventListener("click", finishUploadConflicts);

  // Close menus and the transfers popover when clicking elsewhere.
  document.addEventListener("mousedown", (event) => {
    const target = event.target as HTMLElement;
    if (menuOpen() && !target.closest("#menu")) hideMenu();
    if (transfersOpen() && !target.closest("#transfers-popover, #transfers-btn")) closeTransfers();
  });
  window.addEventListener("blur", hideMenu);
  window.addEventListener("resize", () => {
    hideMenu();
    if (transfersOpen()) openTransfers();
  });
  // No browser menu ("Reload", "Inspect"); ours is shown where it applies.
  window.addEventListener("contextmenu", (event) => {
    if (!isTyping(event.target)) event.preventDefault();
  });
  window.addEventListener("keydown", onKeyDown);

  // Alongside loading the account: GitHub may answer slowly or not at all.
  void notifyAppUpdate();
  void notifyCliUpdate();

  // Only now load the account: a listing can take ~50 s on a large account,
  // and the window must stay usable (creating a folder, opening Photos,…)
  // while it runs.
  if (state.cli?.available && state.cli.supported) {
    await refreshSession();
    // Ask for a fresh listing on startup. The CLI 0.12 cache makes this fast,
    // and it avoids a second large IPC response right after showing a stale
    // saved listing, which WKWebView can drop. Listing without a session
    // would open the CLI's login window on top of ours.
    if (state.session?.logged_in) await refresh({ fresh: true });
  } else if (state.cli?.available) {
    showSettings();
    toast(
      `El CLI instalado (${state.cli.version}) es anterior al mínimo (${state.cli.minimum_version}). Actualízalo desde Ajustes.`,
      "error",
      9000,
    );
  } else {
    showSettings();
    toast("Instala el CLI de O2 Cloud desde Ajustes para empezar.", "info", 9000);
  }
}

window.addEventListener("DOMContentLoaded", () => {
  manageDialogFocus();
  bindLabelViews();
  void bootstrap();
});
