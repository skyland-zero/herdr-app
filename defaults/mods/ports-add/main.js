// Forward a port: one dialog, one rule. Opened from the Ports panel's + (the
// machine's or the panel's) as a dialog with props {machine}; writes to
// shared("ports.rules") like the panel does, and the panel starts it.
//
// It asks for the one thing that changes per web app — the remote port — and
// keeps the rest at the defaults a dev server wants: 127.0.0.1 on both ends,
// the same port number here as there.

import { View, div, svg } from "gpui-kit";
import { v_flex, h_flex, InputState, Input } from "gpui-base";
import { hc, theme, props, shared, setShared, closeOverlay } from "herdr";

const I = (name) => `mods/workspace-chrome/${name}.svg`;

const rules = () => {
  const v = shared("ports.rules");
  return Array.isArray(v) ? v.filter((r) => r && typeof r.id === "string") : [];
};

function newId(seed) {
  const s = String(seed ?? "").toLowerCase().replace(/[^a-z0-9]/g, "").slice(0, 12);
  return `port-${s}${s ? "-" : ""}${Math.random().toString(36).slice(2, 8)}`.slice(0, 32);
}

const port = (text) => {
  const n = Number(String(text ?? "").trim());
  return Number.isInteger(n) && n > 0 && n < 65536 ? n : null;
};

export default class PortsAdd extends View {
  init(_props, cx) {
    this.machineList = [];
    this.machine = props()?.machine ?? "";
    this.error = "";
    this.remote = InputState.new({ placeholder: "5000" });
    this.local = InputState.new({ placeholder: "the same" });
    this.host = InputState.new({ value: "127.0.0.1", placeholder: "127.0.0.1" });
    this.auto = true;
    this.focus = { remote: true, local: false, host: false };
    for (const [key, state] of [
      ["remote", this.remote],
      ["local", this.local],
      ["host", this.host],
    ]) {
      state.on("change", (_e, cx) => { this.error = ""; cx.notify(); });
      state.on("submit", (_e, cx) => this.add(cx));
      state.on("focus", (_e, cx) => { this.focus[key] = true; cx.notify(); });
      state.on("blur", (_e, cx) => { this.focus[key] = false; cx.notify(); });
    }
    this.remote.focus();
    cx.timer.after(600, () => this.remote.focus());
    this.handle = cx.focus_handle();
    cx.spawn(async (cx) => {
      const r = await hc(["machine", "ls", "--json"]).catch(() => null);
      try {
        this.machineList = r?.code === 0 ? (JSON.parse(r.stdout).machines ?? []).filter((m) => !m.temporary) : [];
      } catch {
        this.machineList = [];
      }
      if (!this.machine) this.machine = this.machineList.find((m) => m.state === "connected")?.name ?? this.machineList[0]?.name ?? "";
      cx.notify();
    });
  }

  get remotePort() {
    return port(this.remote.value());
  }

  get localPort() {
    return port(this.local.value()) ?? this.remotePort;
  }

  add(cx) {
    const remote = this.remotePort;
    if (!this.machine) return this.fail("Pick the machine first", cx);
    const named = this.machineList.find((m) => m.name === this.machine);
    if (!named) return this.fail(`There's no machine called ${this.machine}`, cx);
    if (named.via) return this.fail(`${this.machine} is reached through a command, not plain ssh: it can't forward a port`, cx);
    if (!remote) return this.fail("The remote port is a number from 1 to 65535", cx);
    if (!this.localPort) return this.fail("The local port is a number from 1 to 65535", cx);
    const host = this.host.value().trim() || "127.0.0.1";
    const taken = rules().find((r) => r.machine === this.machine && r.local === this.localPort && r.bind === "127.0.0.1");
    if (taken) {
      this.local.set_value(String(this.localPort));
      return this.fail(`127.0.0.1:${this.localPort} is already forwarded from ${this.machine}`, cx);
    }
    const id = newId(this.machine);
    const rule = { id, machine: this.machine, host, remote, local: this.localPort, bind: "127.0.0.1", auto: this.auto };
    setShared("ports.rules", [...rules(), rule]);
    closeOverlay(props().overlay);
  }

  fail(message, cx) {
    this.error = message;
    cx.notify();
  }

  close() {
    closeOverlay(props().overlay);
  }

  field(t, state, focused, label, hint) {
    return v_flex().flex_1().min_w(0).gap(5)
      .child(div().text_size(11.5).font_medium().text_color(t.dim).child(label))
      .child(Input.new(state).h(34).px(10).gap(6).items_center().rounded(8).border_1()
        .border_color(focused ? t.accent : t.input).bg(t.background).text_size(13).text_color(t.foreground))
      .when(Boolean(hint), (el) => el.child(div().text_size(11).text_color(t.dim).child(hint)));
  }

  chip(t, m) {
    const on = m.name === this.machine;
    const here = m.state === "connected";
    return h_flex().id(`pa-machine-${m.name}`).h(30).px(10).gap(6).items_center().rounded(7).cursor_pointer()
      .bg(on ? t.selected : t.raised).border_1().border_color(on ? t.accent : t.border)
      .hover((s) => s.bg(on ? t.selected : t.hover)).text_size(12.5)
      .text_color(on ? t.foreground : t.muted)
      .on_click((_e, cx) => { this.machine = m.name; this.error = ""; cx.notify(); })
      .child(div().size(6).rounded(3).flex_none().bg(here ? t.ansi2 : t.blocked))
      .child(m.name);
  }

  render() {
    const t = theme();
    const remote = this.remotePort;
    const local = this.localPort;
    const machine = this.machine || "a machine";
    return v_flex()
      .id("ports-add")
      .track_focus(this.handle)
      .w_full()
      .min_h(0)
      .p(18)
      .gap(0)
      .bg(t.popover)
      .text_color(t.foreground)
      .on_key_down((e, cx) => {
        if (e.key !== "escape") return;
        cx.stop_propagation();
        this.close();
      })
      .child(h_flex().items_center()
        .child(div().flex_1().text_size(15).font_semibold().child("Forward a port"))
        .child(div().text_size(11).text_color(t.dim).child("Esc")))
      .child(div().pt(6).text_size(12.8).line_height("19px").text_color(t.muted)
        .child(`A tunnel to ${machine}: the web app there opens here, at localhost:${local ?? "…"}.`))
      .child(div().pt(16).pb(6).text_size(11.5).font_medium().text_color(t.dim).child("Machine"))
      .child(h_flex().gap(6).flex_wrap().when(!this.machineList.length, (el) =>
        el.child(div().text_size(12.5).text_color(t.dim).child("No machines yet: add one first"))))
      .child(h_flex().gap(6).flex_wrap().children(this.machineList.map((m) => this.chip(t, m))))
      .child(h_flex().pt(16).gap(10)
        .child(this.field(t, this.remote, this.focus.remote, "Remote port", "the one the app listens on there"))
        .child(this.field(t, this.local, this.focus.local, "Local port", "the one to open here")))
      .child(h_flex().pt(14).gap(10)
        .child(this.field(t, this.host, this.focus.host, "Remote host", "127.0.0.1 is the machine itself")))
      .child(h_flex().id("pa-auto").pt(14).gap(8).items_center().cursor_pointer()
        .on_click((_e, cx) => { this.auto = !this.auto; cx.notify(); })
        .child(div().size(15).flex_none().rounded(4).border_1().border_color(this.auto ? t.accent : t.input)
          .bg(this.auto ? t.accent : t.background).flex().items_center().justify_center()
          .when(this.auto, (el) => el.child(svg(I("check")).size(10).text_color(t.accent_foreground))))
        .child(div().text_size(12.5).text_color(this.auto ? t.foreground : t.muted).child("Keep it open while Herdr runs"))
        .child(div().text_size(11.5).text_color(t.dim).child("it reconnects by itself")))
      .when(Boolean(this.error), (el) => el.child(div().pt(12).text_size(12).text_color(t.blocked).child(this.error)))
      .child(h_flex().pt(18).gap(8).justify_end()
        .child(div().id("pa-cancel").h(32).px(14).flex().items_center().rounded(8).cursor_pointer()
          .border_1().border_color(t.border).bg(t.raised).text_size(12.5).text_color(t.muted)
          .hover((s) => s.bg(t.hover).text_color(t.foreground))
          .on_click(() => this.close()).child("Cancel"))
        .child(div().id("pa-add").h(32).px(16).flex().items_center().rounded(8).cursor_pointer()
          .bg(t.solid).text_color(t.solid_foreground).text_size(12.5).font_medium()
          .hover((s) => s.opacity(0.88))
          .on_click((_e, cx) => this.add(cx))
          .child(remote ? `Forward ${remote}` : "Forward")));
  }
}
