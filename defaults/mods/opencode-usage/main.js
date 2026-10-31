// OpenCode usage: the Go plan's five-hour, weekly and monthly windows, as a
// Herdr window over the layout.
//
//   GET https://opencode.ai/zen/go/v1/usage   (Bearer: a Go API key)
//   GET <local service>/api/credential        (the Go keys it knows)
//
// The local service is opencode's own background server: `service.json` in its
// state folder holds the url and password. Nothing leaves the machine except a
// Go key to opencode.ai, whose endpoint it is; the key is never written down.
//
// Optional: `workspace-status` draws the same numbers in the status bar once
// `"opencode-usage": true` is in `shared.json` — it reads `opencode-usage.last`
// from localStorage, which this mod writes.

import { View, div, svg } from "gpui-kit";
import { h_flex, v_flex } from "gpui-base";
import { theme, home, readTextFile, closeOverlay, props, nextCommand } from "herdr";

const USAGE_URL = "https://opencode.ai/zen/go/v1/usage";
const WINDOWS = [
  ["rolling", "Go 5h"],
  ["weekly", "Go week"],
  ["monthly", "Go month"],
];
const REFRESH_MS = 60_000;
const MIN_GAP_MS = 45_000;
const LAST_KEY = "opencode-usage.last";

const cache = () => {
  try {
    return JSON.parse(localStorage.getItem(LAST_KEY) || "null") || {};
  } catch {
    return {};
  }
};

const saveCache = (data) => {
  try {
    localStorage.setItem(LAST_KEY, JSON.stringify(data));
  } catch {
    // The numbers are still on screen this run.
  }
};

// Base64 by hand: this runtime's Buffer.from(text).toString("base64") comes back
// empty, and the service's password is plain ASCII anyway.
const BASE64 = "ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
function base64(text) {
  const bytes = [];
  for (const char of text) {
    const code = char.codePointAt(0);
    if (code < 0x80) bytes.push(code);
    else if (code < 0x800) bytes.push(0xc0 | (code >> 6), 0x80 | (code & 0x3f));
    else bytes.push(0xe0 | (code >> 12), 0x80 | ((code >> 6) & 0x3f), 0x80 | (code & 0x3f));
  }
  let out = "";
  for (let i = 0; i < bytes.length; i += 3) {
    const n = (bytes[i] << 16) | ((bytes[i + 1] ?? 0) << 8) | (bytes[i + 2] ?? 0);
    out += BASE64[(n >> 18) & 63] + BASE64[(n >> 12) & 63];
    out += i + 1 < bytes.length ? BASE64[(n >> 6) & 63] : "=";
    out += i + 2 < bytes.length ? BASE64[n & 63] : "=";
  }
  return out;
}

// The endpoint says no to anything that is not a Go key, so try the active one
// first and then the rest.
async function goKeys(base, auth) {
  const r = await fetch(`${base}/api/credential`, { headers: auth });
  if (!r.ok) return [];
  const body = await r.json();
  return (body?.data ?? [])
    .filter((c) => c?.integrationID === "opencode-go" && typeof c?.value?.key === "string")
    .sort((a, b) => Number(Boolean(b.active)) - Number(Boolean(a.active)))
    .map((c) => c.value.key);
}

async function readUsage() {
  let service;
  try {
    const text = await readTextFile(`${home().replace(/\\/g, "/")}/.local/state/opencode/service.json`);
    if (!text) return { live: false };
    service = JSON.parse(text);
  } catch {
    return { live: false };
  }
  if (!service?.url || !service?.password) return { live: false };
  const auth = { Authorization: `Basic ${base64(`opencode:${service.password}`)}` };
  const found = {};
  try {
    for (const key of await goKeys(service.url, auth)) {
      const r = await fetch(USAGE_URL, { headers: { Authorization: `Bearer ${key}` } });
      if (!r.ok) continue;
      const body = await r.json();
      if (body?.usage) {
        found.usage = body.usage;
        break;
      }
    }
  } catch {
    // Offline, or the endpoint changed: no windows rather than an error.
  }
  found.live = Boolean(found.usage);
  return found;
}

const pct = (window) => (typeof window?.percent === "number" ? Math.round(window.percent) : null);

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

// The percent this pace would reach by month end.
function forecast(monthly) {
  if (monthly === null) return null;
  const now = new Date();
  const day = now.getDate();
  const days = new Date(now.getFullYear(), now.getMonth() + 1, 0).getDate();
  if (day < 1 || days < 1) return null;
  const projected = (monthly / day) * days;
  return { projected: Math.min(999, projected), over: projected > 100 };
}

function ago(ms) {
  if (!ms) return "never";
  const seconds = Math.max(0, Math.round((Date.now() - ms) / 1000));
  if (seconds < 60) return "now";
  const minutes = Math.round(seconds / 60);
  return minutes < 60 ? `${minutes}m` : `${Math.round(minutes / 60)}h`;
}

export default class OpenCodeUsage extends View {
  init(_props, cx) {
    const last = cache();
    this.usage = last.usage ?? null;
    this.at = last.at ?? null;
    this.error = null;
    this.busy = false;
    this.expanded = localStorage.getItem("opencode-usage.expanded") === "1";
    this.handle = cx.focus_handle();
    this.handle.focus();
    this.closing = false;
    cx.spawn(async () => {
      for (;;) {
        const command = await nextCommand();
        if (command === "close" || command === "toggle") this.close();
      }
    });
    cx.timer.every(REFRESH_MS, (cx) => this.refresh(cx, false));
    this.refresh(cx, true);
  }

  close() {
    if (this.closing) return;
    this.closing = true;
    const id = props()?.overlay;
    if (id) closeOverlay(id);
  }

  toggleExpanded(cx) {
    this.expanded = !this.expanded;
    localStorage.setItem("opencode-usage.expanded", this.expanded ? "1" : "0");
    cx.notify();
  }

  refresh(cx, force) {
    if (this.busy) return;
    if (!force && this.at && Date.now() - this.at < MIN_GAP_MS) return;
    this.busy = true;
    cx.spawn(async (cx) => {
      try {
        const found = await readUsage();
        if (found.usage) this.usage = found.usage;
        if (found.live) {
          this.at = Date.now();
          this.error = null;
          saveCache({ usage: this.usage, at: this.at });
        }
      } catch (e) {
        this.error = String(e?.message ?? e);
      } finally {
        this.busy = false;
        cx.notify();
      }
    });
  }

  // A 3px bar: label and percent above, the track, the reset under it.
  bar(key, label, t) {
    const window = this.usage?.[key];
    const value = pct(window);
    const left = countdown(window?.resetsAt);
    const colour = value === null ? t.dim : value >= 85 ? t.blocked : value >= 60 ? t.done : t.working;
    return v_flex().id(`oc-${key}`).gap(2)
      .child(h_flex().justify_between().items_center()
        .child(div().text_size(10.5).text_color(t.muted).child(label))
        .child(div().text_size(10.5).text_color(value === null ? t.dim : t.foreground).child(value === null ? "—" : `${value}%`)))
      .child(div().h(3).w_full().rounded(2).bg(t.raised).overflow_hidden()
        .child(div().h_full().rounded(2).w(`${value === null ? 0 : Math.max(1.5, value)}%`).bg(colour)))
      .child(div().text_size(9).text_color(t.dim).child(left ? `resets in ${left}` : "no reset"));
  }

  render() {
    const t = theme();
    const projected = this.expanded ? forecast(pct(this.usage?.monthly)) : null;
    const button = (id, glyph, action, colour) =>
      div().id(id).w(18).h(18).flex().items_center().justify_center().rounded(4)
        .text_size(11).text_color(colour).cursor_pointer()
        .hover((s) => s.bg(t.hover).text_color(t.foreground))
        .on_mouse_down("left", (_e, cx) => cx.stop_propagation())
        .on_click((_e, cx) => {
          cx.stop_propagation();
          action(cx);
        })
        .child(glyph);
    const chevron = (down) => svg(`mods/opencode-usage/chevron-${down ? "down" : "up"}.svg`).size(12);
    return v_flex().id("root").w(236).bg(t.surface).p(11).gap(8).track_focus(this.handle)
      .on_key_down((e, cx) => {
        if (e.key === "escape") {
          cx.stop_propagation();
          this.close();
        }
      })
      .child(h_flex().justify_between().items_center()
        .child(h_flex().gap(5).items_center()
          .child(div().text_size(10).text_color(t.dim).child("⠿"))
          .child(svg("mods/opencode-usage/opencode.svg").size(10).text_color(this.error || !this.usage ? t.blocked : t.working))
          .child(div().text_size(10).font_semibold().text_color(t.foreground).child("OPENCODE"))
          .child(div().text_size(9.5).text_color(t.dim).child(ago(this.at))))
        .child(h_flex().gap(1).items_center()
          .child(button("expand", chevron(!this.expanded), (cx) => this.toggleExpanded(cx), this.expanded ? t.foreground : t.dim))
          .child(button("refresh", this.busy ? "…" : "↻", (cx) => this.refresh(cx, true), this.busy ? t.working : t.dim))
          .child(button("close", "×", () => this.close(), t.dim))))
      .child(v_flex().gap(6).children(WINDOWS.map(([key, label]) => this.bar(key, label, t))))
      .when(Boolean(projected), (el) =>
        el.child(div().h(1).w_full().bg(t.border))
          .child(h_flex().justify_between().items_center()
            .child(div().text_size(9.5).text_color(t.dim).child("Projected by month end"))
            .child(div().text_size(10).text_color(projected.over ? t.blocked : t.foreground).child(`${projected.projected.toFixed(0)}%`))))
      .when(Boolean(this.error), (el) => el.child(div().text_size(9.5).text_color(t.blocked).child(this.error)))
      .when(!this.usage && !this.error, (el) => el.child(div().text_size(9.5).text_color(t.dim).child("opencode is not running")));
  }
}
