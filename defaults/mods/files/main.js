// A lazy file browser for the focused pane's folder, with bounded read-only previews.
import { View, div, svg } from "gpui-kit";
import { v_flex, h_flex, Editor, EditorState } from "gpui-base";
import { spaces, panes, focused, exec, readDir, readTextFile, watchFolder, theme, settings, run, window, notify, props, home } from "herdr";

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
    this.viewer = p?.file ? { file: p.file, on: p.on ?? null } : null;
    if (this.viewer) { this.loadViewer(cx); return; }
    this.saved = new Map();
    this.hinted = new Set();
    this.entries = new Map(); this.folders = new Set(); this.pending = new Set(); this.marks = {}; this.marksToken = 0;
    this.refresh(cx);
    // The folder watch keeps an open panel live; the timer is for when it can't
    // start (a network drive) and for a remote machine, which has no watch.
    cx.timer.every(10000, cx => { if (!this.watched) this.refresh(cx); });
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

  watch(root, cx) {
    if (this.on || this.watched === root) return;
    this.watched = root;
    const context = this.context;
    cx.spawn(async cx => {
      try {
        while (this.context === context) {
          const changed = await watchFolder(root);
          if (!this.current(context)) return;
          await this.relist(changed, context, cx);
        }
      } catch { if (this.context === context) this.watched = null; }
    });
  }

  // Re-read only the open folders a change can alter: the parent of what
  // changed. A path named another way (a symlinked root) re-reads them all.
  async relist(changed, context, cx) {
    const root = plain(context.cwd).replace(/\/+$/, "");
    const dirs = new Set();
    let unknown = false;
    for (const path of changed) {
      const p = plain(path);
      if (p !== root && !p.startsWith(root + "/")) { unknown = true; continue; }
      const rel = p.slice(root.length).replace(/^\/+/, ""), cut = rel.lastIndexOf("/");
      const dir = cut < 0 ? "" : rel.slice(0, cut);
      if (dir === "" || this.folders.has(dir)) dirs.add(dir);
    }
    for (const dir of unknown || !dirs.size ? ["", ...this.folders] : dirs) {
      try { await this.loadFolder(dir, context); }
      catch (e) { if (this.current(context)) this.error = String(e); }
    }
    if (!this.current(context)) return;
    cx.notify();
    this.marksSoon(context, cx);
  }

  // git is the costly part of a refresh: one run after a burst of saves.
  marksSoon(context, cx) {
    const token = this.marksToken = this.marksToken + 1;
    cx.timer.after(400, async cx => {
      if (token !== this.marksToken || !this.current(context)) return;
      try { await this.loadMarks(context, cx); } catch { /* the next change tries again */ }
    });
  }

  async loadMarks(context, cx) {
    const { cwd, on } = context;
    const top = await exec("git", ["rev-parse", "--show-toplevel"], { cwd, on }).catch(() => null);
    const marks = {};
    if (top?.code === 0) {
      // Writes nothing under .git (index lock, fsmonitor cookie): the folder watch would refresh again.
      const status = await exec("git", ["--no-optional-locks", "-c", "core.fsmonitor=false", "status", "--porcelain=v1", "-z", "--no-renames", "--untracked-files=normal"], { cwd, on }).catch(() => null);
      if (status?.code === 0) for (const entry of status.stdout.split("\0").filter(Boolean).slice(0, TOTAL)) {
        const xy = entry.slice(0, 2);
        marks[`${slashed(top.stdout.trim())}/${entry.slice(3)}`] = xy === "??" || xy.includes("A") ? "A" : xy.includes("U") ? "U" : xy.includes("D") ? "D" : "M";
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
    const entries = await directory(path ? `${root}/${path}` : root, on);
    if (!this.current(context) || (path && !this.folders.has(path))) return;
    const used = [...this.entries.entries()].filter(([p]) => p !== path && (p === "" || this.folders.has(p))).reduce((n, [, e]) => n + e.length, 0);
    this.entries.set(path, entries.slice(0, Math.max(0, Math.min(PER_FOLDER, TOTAL - used))).sort((a, b) => Number(b.dir) - Number(a.dir) || a.name.localeCompare(b.name)));
    if (entries.length > PER_FOLDER || used + entries.length > TOTAL) this.limited = true;
  }

  refresh(cx) {
    const { cwd, on } = location(), next = key();
    if (next !== this.context?.key) {
      if (this.context) this.saved.set(this.context.key, { folders: this.folders, file: this.file });
      const saved = this.saved.get(next);
      this.context = { key: next, cwd, on }; this.root = cwd; this.on = on; this.entries = new Map(); this.folders = new Set(saved?.folders);
      this.pending = new Set(); this.marks = {}; this.show(null); this.watched = null; this.error = null; this.limited = false;
      this.loading = false;
      if (saved?.file) this.show(saved.file, cx);
    }
    if (!cwd || this.loading) return;
    const context = this.context;
    this.loading = true;
    this.limited = false;
    cx.spawn(async cx => {
      try {
        for (const path of ["", ...this.folders]) {
          if (!this.current(context)) return;
          await this.loadFolder(path, context);
          // Rows as each folder lands; git marks are slower and come after.
          if (this.current(context)) cx.notify();
        }
        if (!this.current(context)) return;
        this.watch(cwd, cx);
        await this.loadMarks(context, cx);
        this.error = null;
      } catch (e) { if (this.current(context)) this.error = String(e); }
      finally { if (this.context === context) this.loading = false; cx.notify(); }
    });
  }

  toggle(path, cx) {
    if (this.folders.has(path)) {
      for (const folder of [...this.folders]) if (folder === path || folder.startsWith(path + "/")) this.folders.delete(folder);
      // The rows stay cached: opening it again paints at once, then re-reads.
    } else {
      this.folders.add(path);
      if (!this.pending.has(path)) {
        const context = this.context;
        this.pending.add(path);
        cx.spawn(async cx => {
          try { await this.loadFolder(path, context); }
          catch (e) { if (this.current(context)) this.error = String(e); }
          finally { if (this.context === context) this.pending.delete(path); cx.notify(); }
        });
      }
    }
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

  rows(prefix, depth, out, t) {
    if (out.length >= TOTAL || depth > 40) return out;
    for (const entry of this.entries.get(prefix) ?? []) {
      if (out.length >= TOTAL) break;
      const path = prefix ? `${prefix}/${entry.name}` : entry.name, open = this.folders.has(path), mark = this.marks[`${slashed(this.root)}/${path}`];
      out.push(h_flex().id(`file-${path}`).h(28).flex_none().pl(8 + depth * 18).pr(8).gap(7).items_center().rounded(7).cursor_pointer().hover(s => s.bg(t.hover))
        .on_click((_e, cx) => entry.dir ? this.toggle(path, cx) : this.open(path, cx))
        // A press may become a drag: the app follows it from here (`path.drag`), because
        // a mod's own mouse events stop at the row and only the app knows which pane
        // the pointer is over. A press that never moves is still a click, above.
        .on_mouse_down("left", (e, _cx) => {
          if (this.root) run("path.drag", { paths: [this.full(path)], label: entry.name, at: e.position, ...(this.on ? { on: this.on } : {}) });
        })
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
    if (this.viewer) {
      const name = this.viewer.file.split(/[\\/]/).pop();
      return v_flex().size_full().min_h(0).bg(t.background).text_color(t.foreground).text_size(13)
        .child(h_flex().h(32).flex_none().px(12).gap(8).items_center().border_b(1).border_color(t.border)
          .child(div().font_semibold().child(name))
          .child(div().flex_1().min_w(0).truncate().text_size(11.5).text_color(t.dim).child(this.viewer.file)))
        .child(this.code ? Editor.new(this.code).flex_1().min_h(0) : div().p(12).text_color(t.dim).child(this.error || "Loading file…"));
    }
    if (key() !== this.context?.key) this.refresh(cx);
    return v_flex().id("files").size_full().min_h(0).rounded(10).overflow_hidden().bg(t.background).border_1().border_color(t.border).text_color(t.foreground).text_size(13)
      .child(h_flex().h(42).flex_none().px(14).gap(8).items_center().border_b(1).border_color(t.border)
        .when(Boolean(this.file), el => el.child(div().id("files-back").w(20).cursor_pointer().text_color(t.muted).on_click((_e, cx) => { this.show(null); cx.notify(); }).child("←")))
        .child(div().font_semibold().child("Files"))
        .child(div().flex_1().min_w(0).truncate().text_right().text_size(12).text_color(t.dim).child(this.file ?? this.root?.split(/[\\/]/).pop() ?? "")))
      .when(!this.file, el => el.child(v_flex().id("files-tree").flex_1().min_h(0).overflow_y_scrollbar().p(6).children(this.rows("", 0, [], t))
        .when(Boolean(this.error) || !this.entries.get("")?.length, el => el.child(div().p(8).text_size(12).text_color(t.dim).child(this.error || (this.loading ? "Loading files…" : this.root ? "No files" : "No folder"))))
        .when(this.limited, el => el.child(div().p(8).text_size(11).text_color(t.dim).child("File list limited to 500 entries per folder and 5,000 total")))))
      .when(Boolean(this.file), el => el.child(this.code ? Editor.new(this.code).flex_1().min_h(0) : div().p(12).text_color(t.dim).child(this.fileError || "Loading file…")));
  }
}
