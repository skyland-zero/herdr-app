// Branch and agent count under the agents list, where the status bar used to
// say them. Only those two: no machine, no usage, no key hint.
//
// It sits in the sidebar as its own block, so it reads as a section of the
// panel; on the 44px rail it keeps just the branch's icon.

import { View, div, svg } from "gpui-kit";
import { h_flex, v_flex } from "gpui-base";
import { spaces, agents, theme, layout } from "herdr";

function find(node, id) {
  if (node?.id === id) return node;
  for (const child of node?.children ?? []) {
    const found = find(child, id);
    if (found) return found;
  }
  return null;
}

// The sidebar's rail goes by its width, not a flag (the agents mod does the same).
const narrow = () => (find(layout(), "sidebar")?.size?.px ?? Infinity) < 160;

export default class SkylandInfo extends View {
  render() {
    const t = theme();
    const space = spaces().find(s => s.active);
    const list = agents();
    const needs = list.filter(a => a.status === "blocked").length;
    const branch = space?.branch ?? null;

    if (narrow()) {
      return v_flex().size_full().bg(t.chrome).items_center().justify_center()
        .child(svg("mods/workspace-chrome/branch.svg").size(14).text_color(branch ? t.muted : t.dim));
    }

    const row = () => h_flex().h(20).px(14).gap(7).items_center().text_size(12);
    return v_flex().size_full().bg(t.chrome).justify_center()
      // The shown space's branch, when its folder is a repo.
      .child(
        row()
          .child(svg("mods/workspace-chrome/branch.svg").size(13).flex_none().text_color(t.dim))
          .child(div().min_w(0).truncate().text_color(branch ? t.foreground : t.dim).child(branch ?? "no branch")),
      )
      .child(
        row()
          .child(div().w(13).flex_none().flex().items_center().justify_center().child(div().size(6).rounded(3).bg(needs > 0 ? t.blocked : t.dim)))
          .child(div().text_color(t.muted).child(`${list.length} agents`))
          .when(needs > 0, (el) => el.child(div().text_color(t.blocked).child(`\u00b7 ${needs} needs you`))),
      );
  }
}
