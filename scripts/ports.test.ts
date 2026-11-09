import { expect, test } from "bun:test";
import { readFile } from "node:fs/promises";

// Exercise the shipped Ports mod's handlers the way the app calls them: a menu
// popup choosing an item, the 2-second state check, the 4-second auto start.
const source = (await readFile(new URL("../defaults/mods/ports/main.js", import.meta.url), "utf8"))
  .replace(/\r\n/g, "\n")
  .replace(/^import .*;\n/gm, "").replace("export default class Ports", "return class Ports").replace(/^export /gm, "");

const element = () => new Proxy({}, { get: (_target, key) => (key === "then" ? undefined : () => element()) });

const rule = { id: "port-remote-aaaa", machine: "remote", host: "127.0.0.1", remote: 5000, local: 5000, bind: "127.0.0.1", auto: true };

function harness(rules: unknown[], live: { address: string; name: string }[] = []) {
  const calls: string[][] = [];
  let shared = structuredClone(rules);
  let tunnels = live.map((t) => ({ ...t, labels: { "herdr.port": t.name } }));
  let next = 0;
  const Ports = new Function(
    "View", "div", "svg", "v_flex", "h_flex", "terminals", "run", "openOverlay", "closeOverlay",
    "theme", "shared", "setShared", "hc", "props", "localStorage",
    source,
  )(
    class {}, element, element, element, element,
    () => structuredClone(tunnels),
    (name: string, args: unknown) => {
      if (name === "terminal.kill") tunnels = tunnels.filter((t) => t.address !== (args as any).terminal);
    },
    () => "overlay", () => {}, () => ({}),
    (key: string) => (key === "ports.rules" ? structuredClone(shared) : null),
    (key: string, value: unknown) => { if (key === "ports.rules") shared = value; },
    async (args: string[]) => {
      calls.push(args);
      if (args[0] === "run") {
        const address = `t${++next}`;
        tunnels = [...tunnels, { address, name: args[args.indexOf("--name") + 1], labels: { "herdr.port": args[args.indexOf("--label") + 1].split("=")[1] } }];
        return { code: 0, stdout: JSON.stringify({ address }), stderr: "" };
      }
      if (args[0] === "kill" || args[0] === "rm") tunnels = tunnels.filter((t) => t.name !== args[1] && t.address !== args[1]);
      return { code: 0, stdout: "{}", stderr: "" };
    },
    () => ({}),
    { getItem: () => "[]", setItem: () => {} },
  );
  const view = new Ports();
  Object.assign(view, {
    handle: {}, machineList: [{ name: "remote", state: "connected", target: "remote" }],
    info: {}, tries: {}, held: new Map(), orphans: new Map(), reading: new Set(), folded: new Set(),
  });
  view.refreshMachines = () => {};
  const tasks: Promise<unknown>[] = [];
  const cx = {
    notify() {}, spawn(fn: (cx: unknown) => Promise<unknown>) { tasks.push(fn(cx)); },
    timer: { every() {}, after() {} }, focus_handle: () => ({ focus() {} }),
    write_to_clipboard() {}, open_url() {},
  };
  const settle = async () => { for (let i = 0; i < 20; i++) await Promise.resolve(); await Promise.allSettled(tasks.splice(0)); };
  const menu = (state: string) => {
    const items = view.menuFor(rule, {}, state);
    // What the app hands the popup copy of this mod.
    view.menu = { items };
    return { items, choose: (label: string) => view.choose(items.findIndex((i: any) => i.label === label), cx) };
  };
  return {
    view, cx, settle, menu, calls,
    get rules() { return shared as unknown[]; },
    get tunnels() { return tunnels; },
  };
}

test("Remove in a tunnel's menu ends the tunnel, not only the rule", async () => {
  const h = harness([rule], [{ address: "t9", name: rule.id }]);
  h.view.info[rule.id] = { phase: "up", address: "t9" };
  h.menu("up").choose("Remove");
  await h.settle();
  expect(h.rules).toEqual([]);
  expect(h.calls).toEqual([["kill", rule.id], ["rm", rule.id]]);
  expect(h.tunnels).toEqual([]);
});

test("a tunnel whose rule is gone is ended once it is clear no window is adding one", async () => {
  const h = harness([], [{ address: "t9", name: rule.id }]);
  // A tunnel can reach this listing before another window's new rule does.
  await h.view.check(h.cx);
  await h.settle();
  expect(h.calls).toEqual([]);
  // Same tunnel, a while later, and still no rule: ended by name.
  h.view.orphans.set(rule.id, Date.now() - 10000);
  await h.view.check(h.cx);
  await h.settle();
  expect(h.calls).toEqual([["kill", rule.id], ["rm", rule.id]]);
  // Another one with the rule's id, made since: ended the same way.
  h.tunnels.push({ address: "t10", name: rule.id, labels: { "herdr.port": rule.id } });
  await h.view.check(h.cx);
  h.view.orphans.set(rule.id, Date.now() - 10000);
  await h.view.check(h.cx);
  await h.settle();
  expect(h.calls.at(-2)).toEqual(["kill", rule.id]);
});

test("a stopped auto rule stays stopped, and another copy starting it lets go", async () => {
  const h = harness([rule]);
  h.view.start(rule, h.cx);
  await h.settle();
  h.view.stop(rule, h.cx);
  await h.settle();
  expect(h.tunnels).toEqual([]);
  for (let i = 0; i < 3; i++) { h.view.auto(h.cx); await h.settle(); }
  expect(h.calls.filter((c) => c[0] === "run")).toHaveLength(1);
  expect(h.view.stateOf(rule)).toBe("stopped");
  // The row menu runs in its own copy: its Start is a tunnel this one didn't make.
  h.tunnels.push({ address: "t77", name: rule.id, labels: { "herdr.port": rule.id } });
  await h.view.check(h.cx);
  expect(h.view.held.has(rule.id)).toBe(false);
  expect(h.view.stateOf(rule)).toBe("starting");
});

test("a tunnel that dies by itself still comes back", async () => {
  const h = harness([rule]);
  h.view.start(rule, h.cx);
  await h.settle();
  h.view.info[rule.id] = { phase: "up", address: "t1", at: 0 };
  // ssh exited: the core lists nothing for the rule.
  (h as any).tunnels.splice(0, h.tunnels.length);
  await h.view.check(h.cx);
  h.view.auto(h.cx);
  await h.settle();
  expect(h.calls.filter((c) => c[0] === "run")).toHaveLength(2);
});

test("Restart restarts, and does not leave the rule marked stopped", async () => {
  const h = harness([rule]);
  h.view.start(rule, h.cx);
  await h.settle();
  h.view.info[rule.id] = { phase: "up", address: "t1" };
  h.menu("up").choose("Restart");
  await h.settle();
  expect(h.view.held.has(rule.id)).toBe(false);
  expect(h.tunnels).toHaveLength(1);
  expect(h.view.stateOf(rule)).toBe("starting");
});

test("starting ends the leftovers by the rule's name, which the core knows", async () => {
  const h = harness([rule]);
  h.view.start(rule, h.cx);
  await h.settle();
  expect(h.calls.filter((c) => c[0] !== "run")).toEqual([["kill", rule.id], ["rm", rule.id]]);
  expect(h.calls.filter((c) => c[0] === "run")[0]).toContain(rule.id);
  // A restart ends the one running now, again by name.
  h.view.start(rule, h.cx);
  await h.settle();
  expect(h.calls.filter((c) => c[0] !== "run").at(-2)).toEqual(["kill", rule.id]);
  expect(h.tunnels).toHaveLength(1);
});
