// A lazy file browser for the focused pane's folder, with bounded read-only previews.
import { View, div, svg } from "gpui-kit";
import { v_flex, h_flex, Editor, EditorState } from "gpui-base";
import { spaces, panes, focused, exec, readDir, readTextFile, watchDirs, watchChanges, unwatch, theme, settings, run, window, notify, props, home, openOverlay, closeOverlay } from "herdr";

function location() {
  const space = spaces().find(s => s.active), f = focused();
  const pane = f?.pane ? panes().find(p => p.pane === f.pane) : null;
  const on = f?.pane === null ? (f.terminal?.includes("/") ? f.terminal.split("/")[0] : null) : space?.machine ?? null;
  return { cwd: pane?.cwd || f?.cwd || space?.folder, on, space: space?.id };
}
const key = () => JSON.stringify(location());
const LANG = { ts: "js", tsx: "js", mts: "js", cts: "js", jsx: "js", mjs: "js", toml: "yaml", jsonc: "json", svelte: "html", vue: "html" };
const PER_FOLDER = 500, TOTAL = 5000;
// A click on one of these hands it to Obsidian when a vault holds it.
const MARKDOWN = new Set(["md", "markdown"]);
// git and Windows watchers name paths their own way (`D:/…`, `\\?\D:\…`):
// compare them in one shape.
const slashed = path => (path ?? "").replaceAll("\\", "/");
const plain = path => slashed(path).replace(/^\/\/\?\/UNC\//i, "//").replace(/^\/\/\?\//, "");
// Watch sets are per copy: a new folder gets a new one, and the old one is given back.
let WATCHED = 0;

// A file manager is a window on the machine Herdr runs on: a remote space's
// paths have none here, so those rows only offer Copy path.
const FILE_MANAGER = { windows: "File Explorer", macos: "Finder" };
const folderOf = path => path.slice(0, Math.max(path.lastIndexOf("/"), path.lastIndexOf("\\")));

// Hands a folder to this computer's file manager, or shows a file in the folder
// that holds it (`open -R`, Explorer's /select). `exec` waits for the program, so
// this runs off the UI thread; the caller turns a failure into a toast.
async function reveal(path, dir) {
  const platform = window()?.platform;
  if (platform === "windows") await exec("explorer.exe", [dir ? path : `/select,${path}`]);
  else if (platform === "macos") await exec("open", dir ? [path] : ["-R", path]);
  else await exec("xdg-open", [dir ? path : folderOf(path)]);
}

async function directory(path, on) {
  return (await readDir(path, { on })).filter(entry => entry.name !== ".git");
}

// The folders Obsidian calls vaults, from its own config; null when it isn't
// installed. It opens a note in the running app through a URI, but only one a
// vault holds: outside them the URI just makes an empty tab.
let VAULTS, VAULTS_AT = 0;
async function vaults() {
  // Re-read now and then: a vault added in Obsidian should work without a restart.
  if (VAULTS && Date.now() - VAULTS_AT < 10000) return VAULTS;
  VAULTS = null;
  const platform = window()?.platform;
  const where = { windows: "AppData\\Roaming\\obsidian\\obsidian.json", macos: "Library/Application Support/obsidian/obsidian.json" }[platform] ?? ".config/obsidian/obsidian.json";
  try {
    const config = JSON.parse(await readTextFile(`${home().replace(/[\\/]+$/, "")}${platform === "windows" ? "\\" : "/"}${where}`));
    VAULTS = Object.values(config.vaults ?? {}).map(vault => plain(vault.path).replace(/\/+$/, "")).filter(Boolean);
    VAULTS_AT = Date.now();
  } catch { /* no Obsidian: Markdown stays in Herdr */ }
  return VAULTS;
}

// Hands a URI to the system, which wakes the app that owns it. `start` and
// `open` return at once: `exec` would otherwise wait for that app to quit.
async function openUri(uri) {
  const platform = window()?.platform;
  try {
    if (platform === "windows") await exec("cmd", ["/c", "start", "", uri]);
    else if (platform === "macos") await exec("open", [uri]);
    else await exec("xdg-open", [uri]);
    return true;
  } catch { return false; }
}

export default class Files extends View {
  init(_props, cx) {
    // A tab of its own (`tab.new { mod: "files", props: { file, on } }`): one
    // file, full width. The panel is the browser. `props()` is the live copy
    // the shell keeps for this instance; the argument is only for `cx.new`.
    const p = props();
    // A row's menu: this same mod as a pointer popup, so its item runs here and
    // doesn't depend on the panel still being open (the agents sort menu does
    // the same). `menu` is neither a file tab nor the browser.
    if (p?.menu) {
      this.menuItems = p.menu.items ?? [];
      this.at = 0;
      this.handle = cx.focus_handle();
      this.handle.focus();
      return;
    }
    this.viewer = p?.file ? { file: p.file, on: p.on ?? null } : null;
    if (this.viewer) { this.loadViewer(cx); return; }
    this.saved = new Map();
    this.hinted = new Set();
    this.entries = new Map(); this.folders = new Set(); this.marks = {}; this.marksToken = 0;
    // What changed since the last paint, and the set the app watches for this copy.
    this.dirty = new Set(); this.all = false; this.marksDirty = false; this.reading = false;
    this.watched = false; this.watchStarted = false; this.wasActive = true;
    this.id = `files:${++WATCHED}`;
    this.refresh(cx);
    // A remote machine can't be watched, and an OS watcher can refuse a network
    // drive: a slow look while the panel is on screen keeps those live.
    cx.timer.every(5000, cx => {
      if (!(window()?.active ?? true) || !this.root) return;
      if (!this.on && !this.watchStarted) this.watch(cx);
      if (this.on || !this.watched) this.pump(cx, true);
    });
  }

  async loadViewer(cx) {
    const { file, on } = this.viewer;
    try {
      const text = await readTextFile(file, { on });
      if (text === null) { this.error = "This file is no longer available"; return; }
      if (text.includes("\0")) { this.error = "Binary file · no text preview"; return; }
      const t = theme(), ext = file.includes(".") ? file.split(".").pop() : "txt";
      this.code = EditorState.new({ language: LANG[ext] ?? ext, readonly: true,
        value: text.length > 400000 ? text.slice(0, 400000) + "\n… (file truncated)" : text,
        font_family: settings().font_family, font_size: settings().font_size ?? 13,
        syntax: { keyword: t.ansi5, string: t.ansi2, comment: t.dim, function: t.ansi4, type: t.ansi3, number: t.ansi1, constant: t.ansi6 } });
    } catch { this.error = "Cannot preview this file (binary, too large, or unreadable)"; }
    finally { cx.notify(); }
  }

  // What this copy shows, as the paths the OS watcher takes: the root, every open
  // folder, and the git dir (beside the worktree in a linked checkout).
  shown() {
    const dirs = [this.full("").replace(/[\\/]+$/, "")];
    for (const folder of this.folders) dirs.push(this.full(folder));
    if (this.git?.git && !this.on) dirs.push(this.git.git);
    return dirs;
  }

  // The app watches exactly these dirs, one level deep each: a build in an unshown
  // `target/` or `node_modules/` is not an event at all. Cheap when nothing changed,
  // and false when the app can't watch here (a network drive, a folder that went away).
  syncWatch() {
    if (this.on || !this.root) return false;
    try { watchDirs(this.id, this.shown()); this.watched = true; }
    catch { this.watched = false; }
    return this.watched;
  }

  // The app resolves when something this copy shows changed; nothing is read here,
  // so a burst of saves lands as one read on the next paint (and a hidden panel or
  // a window behind another one reads nothing at all).
  watch(cx) {
    if (this.on || !this.root) return;
    const context = this.context;
    this.watchStarted = true;
    cx.spawn(async cx => {
      try {
        await this.gitOf(context);            // the git dir belongs in the set too
        if (this.context !== context) return;
        // Nothing to watch here: the 5-second timer reads the folders instead.
        if (!this.syncWatch()) { this.watchStarted = false; return; }
        for (;;) {
          const changed = await watchChanges(this.id);
          if (this.context !== context) return;
          this.changed(changed);
          if (window()?.active ?? true) cx.notify();
        }
      } catch { if (this.context === context) { this.watched = false; this.watchStarted = false; } }
    });
  }

  // A changed dir names what to re-read: the root's own entries, an open folder's,
  // or the git dir's (marks, not rows).
  changed(dirs) {
    const root = plain(this.full("")).replace(/\/+$/, ""), git = plain(this.git?.git);
    for (const dir of dirs) {
      const p = plain(dir).replace(/\/+$/, "");
      if (git && p === git) { this.marksDirty = true; continue; }
      if (p === root) { this.dirty.add(""); continue; }
      if (p.startsWith(root + "/")) {
        const folder = p.slice(root.length + 1);
        if (this.folders.has(folder)) this.dirty.add(folder);
      }
    }
  }

  // Reads what changed (or everything, on a first look or a return to the window),
  // from a paint: off the UI thread, one read per burst, none while nothing shows.
  pump(cx, all = false) {
    if (!this.root || this.reading) return;
    const whole = all || this.all;
    if (!whole && !this.dirty.size && !this.marksDirty) return;
    const context = this.context, marks = whole || this.marksDirty;
    const dirs = whole ? ["", ...this.folders] : [...this.dirty].filter(dir => this.folders.has(dir));
    this.all = false; this.dirty.clear(); this.marksDirty = false;
    if (!dirs.length && !marks) return;
    this.reading = true;
    cx.spawn(async cx => {
      try { await Promise.all(dirs.map(dir => this.loadFolder(dir, context))); }
      catch (e) { if (this.current(context)) this.error = String(e); }
      finally {
        if (this.context === context) {
          this.reading = false;
          this.syncWatch();                   // the open folders may have moved
          cx.notify();
          if (marks) this.marksSoon(context, cx);
        }
      }
    });
  }

  // The checkout behind the shown folder, once per folder: `top` keys the marks,
  // and the git dir's index and HEAD say when marks may have moved.
  gitOf(context) {
    if (this.git) return Promise.resolve(this.git);
    if (!this.gitPromise) {
      const id = this.id, { cwd, on } = context;
      this.gitPromise = exec("git", ["rev-parse", "--absolute-git-dir", "--show-toplevel"], { cwd, on })
        .catch(() => null)
        .then(r => {
          const [git = null, top = null] = r?.code === 0 ? r.stdout.trim().split(/\r?\n/) : [];
          return { id, git, top };
        });
    }
    // A late answer for a folder this copy no longer shows is not cached.
    return this.gitPromise.then(({ id, git, top }) => {
      const found = { git, top };
      if (this.id === id) this.git = found;
      return found;
    });
  }

  // git is the costly part: one run after a burst, and none while the window is
  // behind another one — the next paint catches up.
  marksSoon(context, cx) {
    const token = this.marksToken = this.marksToken + 1;
    cx.timer.after(400, cx => {
      if (token !== this.marksToken || !this.current(context)) return;
      if (!(window()?.active ?? true)) { this.marksDirty = true; return; }
      this.loadMarks(context, cx).catch(() => { /* the next change tries again */ });
    });
  }

  async loadMarks(context, cx) {
    const { top } = await this.gitOf(context);
    const marks = {};
    if (top) {
      // Writes nothing under .git (index lock, fsmonitor cookie): its watch would fire again.
      const status = await exec("git", ["--no-optional-locks", "-c", "core.fsmonitor=false", "status", "--porcelain=v1", "-z", "--no-renames", "--untracked-files=all"], { cwd: top, on: context.on }).catch(() => null);
      if (status?.code === 0) for (const entry of status.stdout.split("\0").filter(Boolean).slice(0, TOTAL)) {
        const xy = entry.slice(0, 2);
        marks[slashed(`${top}/${entry.slice(3)}`)] = xy === "??" || xy.includes("A") ? "A" : xy.includes("U") ? "U" : xy.includes("D") ? "D" : "M";
      }
    }
    if (!this.current(context)) return;
    this.marks = marks;
    cx.notify();
  }

  current(context) {
    return this.context === context && key() === context.key;
  }

  async loadFolder(path, context) {
    const { on, cwd: root } = context;
    let entries;
    try { entries = await directory(path ? `${root}/${path}` : root, on); }
    catch (e) {
      // A folder that went away loses its row; the panel's own folder is the error.
      if (!path) throw e;
      if (this.current(context)) this.folders.delete(path);
      return;
    }
    if (!this.current(context) || (path && !this.folders.has(path))) return;
    const used = [...this.entries.entries()].filter(([p]) => p !== path && (p === "" || this.folders.has(p))).reduce((n, [, e]) => n + e.length, 0);
    this.entries.set(path, entries.slice(0, Math.max(0, Math.min(PER_FOLDER, TOTAL - used))).sort((a, b) => Number(b.dir) - Number(a.dir) || a.name.localeCompare(b.name)));
    if (entries.length > PER_FOLDER || used + entries.length > TOTAL) this.limited = true;
  }

  refresh(cx) {
    const { cwd, on } = location(), next = key();
    if (next !== this.context?.key) {
      if (this.context) {
        this.saved.set(this.context.key, { folders: this.folders, file: this.file });
        unwatch(this.id);                     // this copy shows another folder now
        this.id = `files:${++WATCHED}`;
      }
      const saved = this.saved.get(next);
      this.context = { key: next, cwd, on }; this.root = cwd; this.on = on; this.entries = new Map(); this.folders = new Set(saved?.folders);
      this.marks = {}; this.marksToken = 0; this.git = null; this.gitPromise = null; this.show(null);
      this.dirty = new Set(); this.all = false; this.marksDirty = false; this.reading = false;
      this.watched = false; this.watchStarted = false;
      this.error = null; this.limited = false;
      if (saved?.file) this.show(saved.file, cx);
    }
    if (!cwd) return;
    this.pump(cx, true);                      // the first look reads everything
    this.watch(cx);
  }

  toggle(path, cx) {
    if (this.folders.has(path)) {
      for (const folder of [...this.folders]) if (folder === path || folder.startsWith(path + "/")) this.folders.delete(folder);
      // The rows stay cached: opening it again paints at once, then re-reads.
      this.dirty.delete(path);
    } else {
      this.folders.add(path);
      this.dirty.add(path);
    }
    this.syncWatch();                         // the watch set follows what is open
    this.pump(cx);
    cx.notify();
  }

  // A click on a file: Markdown in Obsidian when a vault holds it, else a tab of
  // its own (the setting), else the panel's preview.
  async open(path, cx) {
    if (settings().files_open_in_tab === false) return this.show(path, cx);
    const ext = (path.includes(".") ? path.split(".").pop() : "").toLowerCase();
    if (!this.on && MARKDOWN.has(ext) && (await this.inObsidian(path))) return;
    this.openTab(path, cx);
  }

  async inObsidian(path) {
    const roots = await vaults();
    if (!roots?.length) return false;
    const file = this.full(path), flat = plain(file).toLowerCase();
    if (!roots.some(root => flat.startsWith(root.toLowerCase() + "/"))) {
      // Once per folder: the way in is Obsidian's own, not a URI's.
      if (!this.hinted.has(this.root)) {
        this.hinted.add(this.root);
        notify({ title: path, body: `Not in an Obsidian vault. Add ${this.root} as one (Open folder as vault) to open Markdown there.`, level: "info" });
      }
      return false;
    }
    return openUri(`obsidian://open?path=${encodeURIComponent(file)}`);
  }

  // Only text opens a tab: a binary or too-large file would show nothing there,
  // so it gets a toast instead of a tab that can't be read.
  async openTab(path, cx) {
    const context = this.context, file = this.full(path), on = this.on;
    try {
      const text = await readTextFile(file, { on });
      if (!this.current(context)) return;
      if (text === null || text.includes("\0")) return notify({ title: path, body: text === null ? "This file is no longer available" : "Binary file · no text preview", level: "warning" });
      run("tab.new", { name: path, mod: "files", props: { file, ...(on ? { on } : {}) } });
    } catch {
      if (this.current(context)) notify({ title: path, body: "Cannot open this file (binary, too large, or unreadable)", level: "warning" });
    }
  }

  show(path, cx) {
    this.code?.release(); this.code = null; this.file = path; this.fileError = null;
    if (!path || !cx) return;
    const context = this.context, root = this.root, on = this.on;
    cx.spawn(async cx => {
      try {
        const text = await readTextFile(`${root}/${path}`, { on });
        if (this.file !== path || !this.current(context)) return;
        if (text === null) { this.fileError = "This file is no longer available"; return; }
        if (text.includes("\0")) { this.fileError = "Binary file · no text preview"; return; }
        const t = theme(), ext = path.includes(".") ? path.split(".").pop() : "txt";
        this.code = EditorState.new({ language: LANG[ext] ?? ext, readonly: true,
          value: text.length > 400000 ? text.slice(0, 400000) + "\n… (file truncated)" : text,
          font_family: settings().font_family, font_size: 12,
          syntax: { keyword: t.ansi5, string: t.ansi2, comment: t.dim, function: t.ansi4, type: t.ansi3, number: t.ansi1, constant: t.ansi6 } });
      } catch { if (this.file === path && this.current(context)) this.fileError = "Cannot preview this file (binary, too large, or unreadable)"; }
      finally { cx.notify(); }
    });
  }

  // The absolute path of a row, in the separators its machine uses: this one's
  // (Windows paths have `\`) or a remote machine's (`/`).
  full(path) {
    if (this.on || window()?.platform !== "windows") return `${this.root.replace(/\/+$/, "")}/${path}`;
    const root = this.root.replaceAll("/", "\\").replace(/\\+$/, "");
    return `${root}\\${path.replaceAll("/", "\\")}`;
  }

  // What a right-click on a row offers. A file manager opens folders on this
  // computer only; Copy path works for any machine's paths.
  menuFor(path, dir) {
    const file = this.full(path), platform = window()?.platform;
    const app = FILE_MANAGER[platform] ?? "File Manager";
    const verb = dir ? "Open" : platform === "macos" ? "Reveal" : "Show";
    return { items: [
      ...(this.on ? [] : [{ label: `${verb} in ${app}`, icon: dir ? "folder" : "file-text", reveal: { path: file, dir } }]),
      { label: "Copy path", icon: "files", copy: file },
    ] };
  }

  // A menu is as tall as its items (the app fits popups to their content), and
  // the app closes it on Esc or a click beside it.
  menuView() {
    const t = theme(), list = this.menuItems;
    return v_flex().id("files-menu").track_focus(this.handle).w_full().min_h(0).p(5).bg(t.popover).text_size(12.8)
      .on_key_down((e, cx) => {
        if (e.key === "down") this.at = (this.at + 1) % list.length;
        else if (e.key === "up") this.at = (this.at + list.length - 1) % list.length;
        else if (e.key === "enter") this.choose(this.at, cx);
        else if (e.key === "escape") closeOverlay(props().overlay);
        else return;
        cx.notify();
      })
      .children(list.map((it, i) => h_flex().id(`files-menu-${i}`).h(31).px(8).gap(9).rounded(7).items_center().text_color(t.foreground)
        .when(i === this.at, el => el.bg(t.selected)).hover(el => el.bg(t.hover))
        .on_click((_e, cx) => this.choose(i, cx))
        .child(svg(`mods/workspace-chrome/${it.icon}.svg`).size(14).flex_none().text_color(t.muted))
        .child(div().flex_1().child(it.label))));
  }

  choose(i, cx) {
    const it = this.menuItems[i];
    if (!it) return;
    closeOverlay(props().overlay);
    if (it.copy !== undefined) cx.write_to_clipboard(it.copy);
    else if (it.reveal) reveal(it.reveal.path, it.reveal.dir).catch(() => notify({ title: it.label, body: "This folder did not open in the file manager", level: "warning" }));
    else if (it.run) run(it.run[0], it.run[1]);
  }

  rows(prefix, depth, out, t) {
    if (out.length >= TOTAL || depth > 40) return out;
    for (const entry of this.entries.get(prefix) ?? []) {
      if (out.length >= TOTAL) break;
      const path = prefix ? `${prefix}/${entry.name}` : entry.name, open = this.folders.has(path), mark = this.marks[slashed(this.full(path))];
      out.push(h_flex().id(`file-${path}`).h(28).flex_none().pl(8 + depth * 18).pr(8).gap(7).items_center().rounded(7).cursor_pointer().hover(s => s.bg(t.hover))
        .on_click((_e, cx) => entry.dir ? this.toggle(path, cx) : this.open(path, cx))
        // A press may become a drag: the app follows it from here (`path.drag`), because
        // a mod's own mouse events stop at the row and only the app knows which pane
        // the pointer is over. A press that never moves is still a click, above.
        .on_mouse_down("left", (e, _cx) => {
          if (this.root) run("path.drag", { paths: [this.full(path)], label: entry.name, at: e.position, ...(this.on ? { on: this.on } : {}) });
        })
        // A right press shows the row's menu at the pointer.
        .on_mouse_down("right", () => openOverlay({ mod: "files", kind: "popup", width: 240, props: { menu: this.menuFor(path, entry.dir) } }))
        .child(entry.dir ? svg(`mods/files/chevron-${open ? "down" : "right"}.svg`).size(12).flex_none().text_color(t.dim) : div().w(12).flex_none())
        .child(svg(`mods/files/${entry.dir ? "folder" : "file"}.svg`).size(14).flex_none().text_color(t.muted))
        .child(div().flex_1().min_w(0).truncate().child(entry.name))
        .when(Boolean(mark), el => el.child(div().text_size(11).text_color(mark === "A" ? t.ansi2 : mark === "D" || mark === "U" ? t.ansi1 : t.blocked).child(mark))));
      if (entry.dir && open) this.rows(path, depth + 1, out, t);
    }
    return out;
  }

  render(cx) {
    const t = theme();
    if (this.menuItems) return this.menuView();
    if (this.viewer) {
      const name = this.viewer.file.split(/[\\/]/).pop();
      return v_flex().size_full().min_h(0).bg(t.background).text_color(t.foreground).text_size(13)
        .child(h_flex().h(32).flex_none().px(12).gap(8).items_center().border_b(1).border_color(t.border)
          .child(div().font_semibold().child(name))
          .child(div().flex_1().min_w(0).truncate().text_size(11.5).text_color(t.dim).child(this.viewer.file)))
        .child(this.code ? Editor.new(this.code).flex_1().min_h(0) : div().p(12).text_color(t.dim).child(this.error || "Loading file…"));
    }
    if (key() !== this.context?.key) this.refresh(cx);
    const active = window()?.active ?? true;
    if (active !== this.wasActive) {
      this.wasActive = active;
      // Back in front: one full read, in case changes landed while nothing looked.
      if (active) this.all = true;
    }
    if (active) this.pump(cx);
    return v_flex().id("files").size_full().min_h(0).rounded(10).overflow_hidden().bg(t.background).border_1().border_color(t.border).text_color(t.foreground).text_size(13)
      .child(h_flex().h(42).flex_none().px(14).gap(8).items_center().border_b(1).border_color(t.border)
        .when(Boolean(this.file), el => el.child(div().id("files-back").w(20).cursor_pointer().text_color(t.muted).on_click((_e, cx) => { this.show(null); cx.notify(); }).child("←")))
        .child(div().font_semibold().child("Files"))
        .child(div().flex_1().min_w(0).truncate().text_right().text_size(12).text_color(t.dim).child(this.file ?? this.root?.split(/[\\/]/).pop() ?? "")))
      .when(!this.file, el => el.child(v_flex().id("files-tree").flex_1().min_h(0).overflow_y_scrollbar().p(6).children(this.rows("", 0, [], t))
        .when(Boolean(this.error) || !this.entries.get("")?.length, el => el.child(div().p(8).text_size(12).text_color(t.dim).child(this.error || (this.reading ? "Loading files…" : this.root ? "No files" : "No folder"))))
        .when(this.limited, el => el.child(div().p(8).text_size(11).text_color(t.dim).child("File list limited to 500 entries per folder and 5,000 total")))))
      .when(Boolean(this.file), el => el.child(this.code ? Editor.new(this.code).flex_1().min_h(0) : div().p(12).text_color(t.dim).child(this.fileError || "Loading file…")));
  }
}
