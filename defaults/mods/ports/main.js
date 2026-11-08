// Ports: ssh tunnels from a remote machine's port to this computer, for
// opening a web app running there in a browser here. One rule is one tunnel,
// run by the core as a background terminal named after the rule, so it
// outlives this window and its state is free: a terminal carrying
// `herdr.port=<id>` is that tunnel.
//
// Rules live in shared("ports.rules"), so the add dialog and this panel (and
// anything else later) read one list. The machine's ssh target is looked up
// every time, so renaming a machine or moving it to another address doesn't
// touch a rule.

import { View, div, svg } from "gpui-kit";
import { v_flex, h_flex } from "gpui-base";
import { terminals, run, openOverlay, closeOverlay, theme, shared, setShared, hc, props } from "herdr";

const I = (name) => `mods/workspace-chrome/${name}.svg`;
const LABEL = "herdr.port";
// A live tunnel with nothing on screen for this long is up: ssh printed its
// warnings (if any) and is forwarding.
const SETTLE = 3000;

export const rules = () => {
  const v = shared("ports.rules");
  return Array.isArray(v) ? v.filter((r) => r && typeof r.id === "string") : [];
};
export const saveRules = (list) => setShared("ports.rules", list);

// `target` as `hc machine ls` writes it: a bare alias, user@host, and an
// optional :port — turned into ssh's own arguments (the port first, as `-p`).
export function targetArgs(target) {
  const m = /^(?:(.*)@)?(\[[^\]]+\]|[^:@\s]+)(?::(\d+))?$/.exec(String(target ?? "").trim());
  if (!m) return null;
  const a = [];
  if (m[3]) a.push("-p", m[3]);
  a.push(m[1] ? `${m[1]}@${m[2]}` : m[2]);
  return a;
}

// The local end listens as `localhost`, not 127.0.0.1: ssh then takes both
// loopback stacks (so a browser that tries ::1 first still lands here), and
// the browser's Host header reads `localhost:3000` — which is what http.sys
// (IIS, IIS Express, anything on .NET's old stack) matches a site's binding
// against. Asking such a site for `127.0.0.1:3000` gets a 400 instead.
const listen = (bind) => (bind === "0.0.0.0" || bind === "::" ? bind : "localhost");

export const forward = (r) => `${listen(r.bind)}:${r.local}:${String(r.host).includes(":") ? `[${r.host}]` : r.host}:${r.remote}`;

export function tunnelArgs(rule, target) {
  const to = targetArgs(target);
  if (!to) return null;
  return [
    "-N",
    "-o", "ExitOnForwardFailure=yes",
    "-o", "ConnectTimeout=10",
    "-o", "ServerAliveInterval=30",
    "-o", "ServerAliveCountMax=3",
    "-L", forward(rule),
    ...to,
  ];
}

export function newId(seed) {
  const s = String(seed ?? "").toLowerCase().replace(/[^a-z0-9]/g, "").slice(0, 12);
  const n = Math.random().toString(36).slice(2, 6);
  return `port-${s}${s ? "-" : ""}${n}${Math.random().toString(36).slice(2, 4)}`.slice(0, 32);
}

// Where the browser goes: always by name, on this computer. `localhost`
// reaches a listener on 127.0.0.1, ::1 or 0.0.0.0 alike, and it's the name
// http.sys (IIS, IIS Express, anything on .NET's old stack) matches a site's
// binding against — asking such a site for `127.0.0.1:5000` gets a 400.
export const url = (r) => `http://localhost:${r.local}`;

const sshLine = (r, target) => ["ssh", ...tunnelArgs(r, target) ?? [target]].join(" ");

export default class Ports extends View {
  init(_props, cx) {
    this.handle = cx.focus_handle();
    this.machineList = [];
    this.info = {}; // rule id -> { phase, address, at, error }
    this.tries = {}; // rule id -> { n, at } for the auto start's backoff
    this.reading = new Set();
    this.folded = new Set(JSON.parse(localStorage.getItem("ports.folded") ?? "[]"));
    // A row's menu is this same mod as a pointer popup: its items run here,
    // so they don't need the panel open. It keeps no timers of its own.
    if (props()?.menu) {
      this.menu = props().menu;
      this.at = 0;
      this.handle.focus();
      return;
    }
    this.refreshMachines(cx);
    cx.timer.every(15000, (cx) => this.refreshMachines(cx));
    cx.timer.every(2000, (cx) => this.check(cx));
    cx.timer.every(4000, (cx) => this.auto(cx));
  }

  // ---- machines ------------------------------------------------------

  refreshMachines(cx) {
    cx.spawn(async (cx) => {
      try {
        const r = await hc(["machine", "ls", "--json"]);
        if (r.code === 0) this.machineList = (JSON.parse(r.stdout).machines ?? []).filter((m) => !m.temporary);
      } catch {}
      cx.notify();
    });
  }

  machine(name) {
    return this.machineList.find((m) => m.name === name) ?? null;
  }

  // ---- tunnels -------------------------------------------------------

  live() {
    const map = new Map();
    for (const t of terminals()) {
      const id = t.labels?.[LABEL];
      if (id) map.set(id, t);
    }
    return map;
  }

  async screen(address) {
    const r = await hc(["read", address, "--json"]).catch(() => null);
    if (!r || r.code !== 0) return null;
    try {
      return String(JSON.parse(r.stdout).text ?? "");
    } catch {
      return null;
    }
  }

  start(rule, cx) {
    const info = (this.info[rule.id] = { phase: "starting", at: Date.now() });
    const fail = (why) => {
      info.phase = "failed";
      info.error = why;
      cx.notify();
    };
    const m = this.machine(rule.machine);
    if (!m) return fail(`there's no machine called ${rule.machine}`);
    if (m.via) return fail(`${rule.machine} is reached through a command, not plain ssh: it can't forward a port`);
    const args = tunnelArgs(rule, m.target);
    if (!args) return fail(`ssh can't read the target "${m.target}"`);
    // Two tunnels on one local port both bind on Windows, and the second one
    // silently takes over; so one rule per address, checked before starting.
    const liveNow = this.live();
    const clash = rules().find((r) => r.id !== rule.id && r.bind === rule.bind && r.local === rule.local && liveNow.has(r.id));
    if (clash) return fail(`127.0.0.1:${rule.local} is already forwarded from ${clash.machine}`);
    cx.notify();
    cx.spawn(async (cx) => {
      // The last attempt's leftovers hold both the name and the port.
      const old = this.live().get(rule.id);
      if (old) await hc(["kill", old.address]).catch(() => null);
      await hc(["rm", rule.id]).catch(() => null);
      const r = await hc([
        "run", "--bg", "--json", "--no-parent", "--name", rule.id, "--label", `${LABEL}=${rule.id}`,
        "--", "ssh", ...args,
      ]).catch((e) => ({ code: 1, stdout: "", stderr: String(e) }));
      let address = null;
      if (r.code === 0) {
        try {
          address = JSON.parse(r.stdout).address;
        } catch {}
      }
      if (address) {
        info.address = address;
        info.at = Date.now();
      } else {
        let why = (r.stderr || r.stdout || "").trim();
        try {
          why = JSON.parse(r.stdout).error?.message ?? why;
        } catch {}
        info.phase = "failed";
        info.error = why.split("\n").filter(Boolean).pop() ?? "it didn't start";
        cx.notify();
      }
    });
  }

  stop(rule, cx) {
    const t = this.live().get(rule.id);
    const address = t?.address ?? this.info[rule.id]?.address;
    this.info[rule.id] = { ...(this.info[rule.id] ?? {}), phase: "stopped" };
    if (address) run("terminal.kill", { terminal: address });
    cx.notify();
  }

  // What a tunnel is doing now. Only the ones not yet confirmed up are read
  // (one screen read each, in turn): a live terminal says nothing about
  // whether ssh got a password prompt or is forwarding.
  async check(cx) {
    const rulesNow = rules();
    if (!rulesNow.length) return;
    const live = this.live();
    let changed = false;
    for (const rule of rulesNow) {
      const info = (this.info[rule.id] ??= { phase: "unknown" });
      const t = live.get(rule.id);
      if (!t) {
        // Gone: whatever it printed is the reason, if it never got up.
        if (info.phase === "up" || info.phase === "waiting" || info.phase === "starting") {
          info.phase = "stopped";
          changed = true;
          const name = info.address ?? rule.id;
          if (this.reading.has(name)) continue;
          this.reading.add(name);
          const text = await this.screen(name);
          this.reading.delete(name);
          const line = text?.split("\n").map((l) => l.trim()).filter(Boolean).pop();
          if (line) info.error = line;
        }
        continue;
      }
      if (info.address !== t.address) {
        info.address = t.address;
        info.at = Date.now();
        info.phase = "starting";
        info.error = null;
        changed = true;
      }
      if (info.phase === "up" || info.phase === "waiting") continue;
      if (Date.now() - (info.at ?? 0) < 900) continue;
      if (this.reading.has(t.address)) continue;
      this.reading.add(t.address);
      const text = await this.screen(t.address);
      this.reading.delete(t.address);
      const tail = String(text ?? "").trimEnd();
      const last = tail.split("\n").pop() ?? "";
      if (/password[^\n]*:\s*$|passphrase[^\n]*:\s*$|\(yes\/no(\/\[fingerprint\])?\)\?\s*$/i.test(last)) {
        info.phase = "waiting";
        info.error = last;
        changed = true;
      } else if (/permission denied|could not resolve|connection refused|no route to host|bind: address already in use|administratively prohibited|network is unreachable/i.test(tail)) {
        info.phase = "waiting";
        info.error = last;
        changed = true;
      } else if (Date.now() - (info.at ?? 0) >= SETTLE) {
        info.phase = "up";
        info.error = null;
        delete this.tries[rule.id];
        changed = true;
      }
    }
    if (changed) cx.notify();
  }

  // Every auto rule with its machine here and nothing holding the name gets
  // its tunnel back, one at a time, backing off on a rule that keeps failing.
  auto(cx) {
    const now = Date.now();
    const live = this.live();
    for (const rule of rules()) {
      if (rule.auto === false || live.has(rule.id)) continue;
      const info = this.info[rule.id];
      if (info && (info.phase === "starting" || info.phase === "waiting") && now - (info.at ?? 0) < 20000) continue;
      const m = this.machine(rule.machine);
      if (!m) {
        // A rule whose machine is gone says so once, instead of retrying ssh
        // against a name that will never resolve.
        const why = `there's no machine called ${rule.machine}`;
        if (info?.error !== why) {
          this.info[rule.id] = { phase: "failed", at: now, error: why };
          cx.notify();
        }
        continue;
      }
      if (m.state !== "connected") continue;
      const tries = (this.tries[rule.id] ??= { n: 0, at: 0 });
      if (now - tries.at < Math.min(60000, 4000 * 2 ** tries.n)) continue;
      tries.at = now;
      tries.n += 1;
      this.start(rule, cx);
      return;
    }
  }

  // ---- menus ---------------------------------------------------------

  menuFor(rule, t, state) {
    const m = this.machine(rule.machine);
    const address = this.live().get(rule.id)?.address ?? this.info[rule.id]?.address;
    const items = [];
    if (state === "up") items.push({ label: "Open in browser", icon: "layers", open: url(rule) });
    if (state === "up" || state === "waiting") items.push({ label: "Restart", icon: "spinner", restart: rule.id });
    else items.push({ label: "Start", icon: "circle-check", start: rule.id });
    if (address) items.push({ label: "Show log", icon: "file-text", peek: address });
    items.push({ label: "Copy address", icon: "files", copy: url(rule) });
    items.push({ label: "Copy ssh command", icon: "terminal", copy: sshLine(rule, m?.target ?? rule.machine) });
    items.push({ label: "Remove", icon: "x", remove: rule.id });
    return items;
  }

  menuView() {
    const t = theme();
    const list = this.menu.items ?? [];
    return v_flex().id("ports-menu").track_focus(this.handle).w_full().min_h(0).p(5).bg(t.popover).text_size(12.8)
      .on_key_down((e, cx) => {
        if (e.key === "down") this.at = (this.at + 1) % list.length;
        else if (e.key === "up") this.at = (this.at + list.length - 1) % list.length;
        else if (e.key === "enter") this.choose(this.at, cx);
        else if (e.key === "escape") closeOverlay(props().overlay);
        else return;
        cx.notify();
      })
      .children(list.map((it, i) => h_flex().id(`ports-menu-${i}`).h(31).px(8).gap(9).rounded(7).items_center().text_color(t.foreground)
        .when(i === this.at, (el) => el.bg(t.selected)).hover((el) => el.bg(t.hover))
        .on_click((_e, cx) => this.choose(i, cx))
        .child(svg(I(it.icon)).size(14).flex_none().text_color(t.muted))
        .child(div().flex_1().truncate().child(it.label))));
  }

  choose(i, cx) {
    const it = this.menu?.items?.[i];
    if (!it) return;
    closeOverlay(props().overlay);
    const rule = rules().find((r) => r.id === (it.start ?? it.restart ?? it.remove));
    if (it.open) cx.open_url(it.open);
    else if (it.copy !== undefined) cx.write_to_clipboard(it.copy);
    else if (it.peek) run("terminal.peek", { terminal: it.peek });
    else if (it.remove) saveRules(rules().filter((r) => r.id !== it.remove));
    else if (rule && it.start) this.start(rule, cx);
    else if (rule && it.restart) {
      this.stop(rule, cx);
      this.start(rule, cx);
    }
  }

  // ---- drawing -------------------------------------------------------

  dot(t, state) {
    const color = { up: t.ansi2, waiting: t.ansi3, starting: t.ansi3, failed: t.blocked, stopped: t.dim }[state] ?? t.dim;
    return div().size(7).flex_none().rounded(4).bg(color);
  }

  stateOf(rule) {
    const info = this.info[rule.id];
    if (this.live().has(rule.id)) return info?.phase === "waiting" ? "waiting" : info?.phase === "up" ? "up" : "starting";
    if (info?.phase === "failed") return "failed";
    if (info?.phase === "waiting" || info?.phase === "starting") return "failed";
    return "stopped";
  }

  ruleRow(rule, t) {
    const state = this.stateOf(rule);
    const info = this.info[rule.id];
    const address = this.live().get(rule.id)?.address ?? info?.address;
    const open = state === "up";
    return v_flex().flex_none().child(
      h_flex().id(`port-${rule.id}`).h(28).flex_none().pl(26).pr(6).gap(7).items_center().rounded(7).cursor_pointer()
        .hover((s) => s.bg(t.hover))
        .on_click((_e, cx) => {
          if (open) cx.open_url(url(rule));
          else if (state !== "starting") this.start(rule, cx);
        })
        .on_mouse_down("right", () => openOverlay({ mod: "ports", kind: "popup", width: 230, props: { menu: { items: this.menuFor(rule, t, state) } } }))
        .child(this.dot(t, state))
        .child(div().flex_none().text_color(state === "failed" ? t.muted : t.foreground).child(`${rule.bind === "0.0.0.0" ? "0.0.0.0" : "localhost"}:${rule.local}`))
        .child(div().flex_1().min_w(0).truncate().text_size(11.5).text_color(t.dim).child(`→ ${rule.host}:${rule.remote}`))
        .when(open, (el) => el.child(div().id(`port-open-${rule.id}`).px(4).rounded(4).text_color(t.dim).hover((s) => s.text_color(t.foreground))
          .on_click((_e, cx) => { cx.stop_propagation(); cx.open_url(url(rule)); }).child("↗")))
        .when(address, (el) => el.child(div().id(`port-log-${rule.id}`).px(4).rounded(4).text_color(t.dim).hover((s) => s.text_color(t.foreground))
          .on_click((_e, cx) => { cx.stop_propagation(); run("terminal.peek", { terminal: address }); }).child("⋯")))
        .when(state !== "stopped", (el) => el.child(div().id(`port-stop-${rule.id}`).px(4).rounded(4).text_color(t.dim).hover((s) => s.text_color(t.foreground))
          .on_click((_e, cx) => { cx.stop_propagation(); this.stop(rule, cx); }).child(icon("x", 11, t.dim)))),
    ).when(Boolean(info?.error) && state !== "up", (el) => el.child(
      div().pl(40).pr(10).pb(3).text_size(11).text_color(t.blocked)
        .truncate().child(String(info.error).split("\n")[0])));
  }

  machineHead(name, t, count, up) {
    const folded = this.folded.has(name);
    const m = this.machine(name);
    const here = m?.state === "connected";
    return h_flex().id(`ports-machine-${name}`).h(30).flex_none().px(8).gap(7).items_center().text_size(11.5).text_color(t.dim)
      .cursor_pointer().hover((s) => s.bg(t.hover))
      .on_click((_e, cx) => {
        folded ? this.folded.delete(name) : this.folded.add(name);
        localStorage.setItem("ports.folded", JSON.stringify([...this.folded]));
        cx.notify();
      })
      .child(icon("server", 13, t.dim))
      .child(div().flex_1().min_w(0).truncate().child(name))
      .when(count > 0, (el) => el.child(div().flex_none().child(String(count))))
      .child(div().size(6).rounded(3).flex_none().bg(here ? (up > 0 ? t.ansi2 : t.dim) : t.blocked))
      .child(div().id(`ports-add-${name}`).w(20).flex_none().flex().items_center().justify_center().rounded(4)
        .hover((s) => s.bg(t.hover).text_color(t.foreground))
        .on_click((_e, cx) => {
          cx.stop_propagation();
          openOverlay({ mod: "ports-add", kind: "dialog", width: 430, props: { machine: name } });
        }).child(svg(I("plus")).size(12)))
      .child(svg(I(folded ? "chevron-right" : "chevron-down")).size(11).flex_none());
  }

  render(cx) {
    const t = theme();
    if (this.menu) return this.menuView();
    const list = rules();
    const live = this.live();
    const names = [];
    for (const m of this.machineList) names.push(m.name);
    for (const r of list) if (!names.includes(r.machine)) names.push(r.machine);
    const up = list.filter((r) => this.stateOf(r) === "up").length;
    const here = this.machineList.find((m) => m.state === "connected")?.name ?? null;
    const rows = [];
    for (const name of names) {
      const mine = list.filter((r) => r.machine === name);
      rows.push(this.machineHead(name, t, mine.length, mine.filter((r) => this.stateOf(r) === "up").length));
      if (this.folded.has(name)) continue;
      if (!mine.length) {
        rows.push(div().id(`ports-empty-${name}`).h(26).pl(26).text_size(11.5).text_color(t.dim).flex().items_center().child("No ports"));
        continue;
      }
      for (const rule of mine) rows.push(this.ruleRow(rule, t));
    }
    return v_flex().id("ports").size_full().min_h(0).rounded(10).overflow_hidden().bg(t.background)
      .border_1().border_color(t.border).text_color(t.foreground).text_size(13)
      .child(h_flex().h(42).flex_none().px(14).gap(8).items_center().border_b(1).border_color(t.border)
        .child(div().font_semibold().child("Ports"))
        .child(div().flex_1().min_w(0).truncate().text_right().text_size(12).text_color(t.dim)
          .child(list.length ? `${up} of ${list.length} up` : "nothing forwarded yet"))
        .child(div().id("ports-add").size(24).flex_none().flex().items_center().justify_center().rounded(6)
          .hover((s) => s.bg(t.hover).text_color(t.foreground))
          .on_click(() => openOverlay({ mod: "ports-add", kind: "dialog", width: 430, props: { machine: here ?? "" } }))
          .child(svg(I("plus")).size(14))))
      .child(v_flex().id("ports-list").flex_1().min_h(0).p(6).gap(1).overflow_y_scrollbar().children(rows)
        .when(!names.length, (el) => el.child(div().p(8).text_size(12).text_color(t.dim)
          .child(this.machineList.length ? "Add a port to forward one to this computer" : "No machines yet: add one first"))));
  }
}

// icon(): the mod's own tinted svg, beside a row.
function icon(name, size, color) {
  return svg(I(name)).size(size).flex_none().text_color(color);
}
