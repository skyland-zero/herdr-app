import { View, div, svg } from "gpui-kit";
import { h_flex } from "gpui-base";
import { spaces, agents, theme, run, window, shared, openOverlay, keys } from "herdr";

// OpenCode Go usage, when the person turned it on (`shared("opencode-usage")`,
// set by the `opencode-usage` mod's rows). Its numbers come from the cache that
// mod writes, so both always agree; with it off the bar is exactly as it was.
const WINDOWS = [
  ["rolling", "5h"],
  ["weekly", "week"],
  ["monthly", "month"],
];
// A reading this old shows dimmed, since opencode may not be running.
const STALE_MS = 10 * 60_000;
const LAST_KEY = "opencode-usage.last";

function cache() {
  try {
    return JSON.parse(localStorage.getItem(LAST_KEY) || "null") || {};
  } catch {
    return {};
  }
}

const pct = (w) => (typeof w?.percent === "number" ? Math.round(w.percent) : null);
const tone = (t, v) => (v === null ? t.dim : v >= 85 ? t.blocked : v >= 60 ? t.done : t.muted);

// "45m", "3h 20m", "5d 7h", "now"
function countdown(iso) {
  const at = Date.parse(iso ?? "");
  if (!Number.isFinite(at)) return null;
  const ms = at - Date.now();
  if (ms <= 0) return "now";
  const minutes = Math.round(ms / 60_000);
  if (minutes < 60) return `${Math.max(1, minutes)}m`;
  const hours = Math.floor(minutes / 60);
  if (hours < 24) {
    const rest = minutes % 60;
    return rest ? `${hours}h ${rest}m` : `${hours}h`;
  }
  const days = Math.floor(hours / 24);
  const rest = hours % 24;
  return rest ? `${days}d ${rest}h` : `${days}d`;
}

export default class WorkspaceStatus extends View {
  // One window: "5h 3% 1h 37m"
  usageWindow(key, label, usage, t) {
    const window = usage?.[key];
    const value = pct(window);
    const left = countdown(window?.resetsAt);
    return h_flex().id(`oc-${key}`).gap(5).items_center()
      .child(div().text_color(t.dim).child(label))
      .child(div().text_color(tone(t, value)).when(value !== null, (e) => e.font_semibold()).child(value === null ? "—" : `${value}%`))
      .when(Boolean(left), (e) => e.child(div().text_color(t.dim).child(left)));
  }

  usage(t) {
    const last = cache();
    const dim = !last.at || Date.now() - last.at > STALE_MS;
    return h_flex().id("opencode-usage-bar").gap(10).items_center().cursor_pointer()
      .when(dim, (e) => e.text_color(t.dim))
      .on_click(() => openOverlay({ mod: "opencode-usage", kind: "window", width: 236 }))
      .child(div().child("opencode"))
      .children(WINDOWS.map(([key, label]) => this.usageWindow(key, label, last.usage, t)));
  }

  // The palette key as actually bound, not a hardcoded one; without a
  // binding the hint stays useful ("commands") instead of lying.
  hint(t) {
    const mac = window()?.platform === "macos";
    const mod = { cmd: mac ? "⌘" : "Ctrl+", ctrl: mac ? "⌃" : "Ctrl+", shift: mac ? "⇧" : "Shift+", alt: mac ? "⌥" : "Alt+", option: mac ? "⌥" : "Alt+" };
    const b = keys().find((k) => k.section === "global" && k.action === "app.palette");
    const parts = b && b.keys.endsWith("--") ? [...b.keys.slice(0, -2).split("-"), "-"] : b?.keys.split("-");
    const key = parts && parts.map((p) => mod[p.toLowerCase()] ?? p.toUpperCase()).join("");
    return key ? `${key}  commands` : "commands";
  }

  render() {
    const t = theme();
    const space = spaces().find(s => s.active);
    const list = agents();
    const needs = list.filter(a => a.status === "blocked").length;
    const label = (icon, text) => h_flex().gap(6).items_center().child(svg(`mods/workspace-chrome/${icon}.svg`).size(13)).child(text);
    return h_flex().size_full().px(14).gap(16).items_center().bg(t.chrome).border_t(1).border_color(t.border).text_size(11.8).text_color(t.muted)
      .child(label(space?.machine ? "server" : "monitor", space?.machine ?? "This computer"))
      .when(Boolean(space?.branch), el => el.child(label("branch", space.branch)))
      .child(h_flex().gap(6).items_center().child(div().size(6).rounded(3).bg(t.dim)).child(`${list.length} agents`)
        .when(needs > 0, el => el.child(div().text_color(t.blocked).child(`· ${needs} needs you`))))
      .when(Boolean(shared("opencode-usage")), el => el.child(this.usage(t)))
      .child(div().flex_1())
      .child(div().id("palette").text_color(t.dim).on_click(() => run("app.palette")).child(this.hint(t)));
  }
}
