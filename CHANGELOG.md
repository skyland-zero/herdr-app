# Herdr changelog

## [0.1.20] - 2026-11-05
<!-- spotlight -->

### Fixed
- CPU and memory read as two small meters with their numbers beside them, so how busy a machine is shows at a glance; a machine near its limit turns the bar orange, and red at 90%.

## [0.1.19] - 2026-11-05
<!-- spotlight -->

### New
- **CPU and memory on the sidebar.** Each machine in the Spaces list shows its CPU and memory under its name; with one machine only, the numbers sit on the right of the Spaces heading. A machine you're connected to reports its own, Linux or Windows.
- Mods and agents can read the same numbers with `hostStats({ on })`, cached so asking on a timer starts nothing.
- `exec` takes a `timeout` in milliseconds, up to 10 minutes, locally and on another machine. The Worktree mod uses it so `git worktree add` and `remove` on a large repository finish instead of giving up at 30 seconds.

### Fixed
- The sidebar rail's machines, spaces and their badges sit on one line each, centred. (#453, thanks @log4b)
- The changes list fills its panel and scrolls through every changed file. (#474, thanks @shadownrx)
- The command palette shows ⌘ only on macOS, not on Windows and Linux. (#478, thanks @JJLiebig)
- Long theme names stay inside their cards in the settings window. (#437, thanks @martintrifunov)
- The settings window's frame draws its rounded background. (#457, thanks @yankewei)
- The command palette keeps scrolling past its group titles. (#531, thanks @patrick-xin)
- An agent's toast names it from the programs table, so a program you renamed reads as you named it. (#530, thanks @thuanlm215)
- A click a program takes for itself ends Herdr's text selection, so a copy can't repeat it. (#369, thanks @sundarshahi)
- `herdr-app check` keeps a mod's shared state in memory and leaves `shared.json` untouched. (#516, thanks @JJLiebig)
- New spaces, tabs and splits retry in the machine's home when the inherited folder is gone, and keep their launch folder while a burst of them starts. (#511, thanks @JJLiebig)

## [0.1.18] - 2026-11-04
<!-- spotlight -->

### New
- **Find in a terminal.** Ctrl+Shift+F (Cmd+F on macOS) opens a find bar in the focused terminal and searches its screen and scrollback. Enter and Shift+Enter move between matches, Escape closes it, and each terminal keeps its own query. (#436)
- **Scroll a terminal's history with a thumb.** A slim scrollbar at the terminal pane's right edge shows where you are in the scrollback and drags to move through it. Turn it off with `pane_scrollbars` in settings.json. (#428)
- Mods and agents can scroll a terminal's history with `terminal.scroll {lines | pages | to}`.
- A key can bind a command with typed, named arguments: `{"command": "layout.resize", "args": {"id": "sidebar", "px": 240}}`.
- A `keys.json` you never edited gets the release's new shortcuts when you upgrade. Once you edit it, even whitespace, the whole file stays as yours and new defaults are left out.

### Fixed
- On Windows, links open in your default browser and editor instead of being handed to Explorer's own arguments.
- On Windows, plain rectangles and backgrounds draw with a small shader, so busy windows use less of the graphics card.
- A tab you don't see keeps its terminals at the size the panes will draw them, so switching back doesn't reflow. (#340)
- A program starts at the size and display it's shown with, so its first frame isn't laid out for another pane.
- On macOS, updating the core leaves a complete app bundle behind, and Herdr finds a core in its current or older bundle folder.
- On a remote Windows machine, agent checks and the Files panel work instead of failing without `sh`.
- The agents and worktree lists read the panes once per draw instead of once per row, so big windows stay responsive. (#484, #485, #486)
- Selected markdown text draws behind the letters instead of over them.
- A symbol too big for its terminal cell is scaled down instead of covering its neighbors.
- Find keeps working after the core reconnects, its highlights follow scrolling, a paste goes into the find field, and Escape reaches the program once the terminal has focus.

## [0.1.17] - 2026-11-03
<!-- spotlight -->

### Fixed
- A Nerd Font icon in a title, a space name or an agent row draws its icon instead of an empty box, whatever the window's font is.

## [0.1.16] - 2026-11-03
<!-- spotlight -->

### New
- **Ligatures in the terminal.** Set `font_ligatures` to true in settings.json for programming ligatures in fonts like JetBrains Mono and Fira Code. The cursor and selections break them, so every cell stays clear.
- **A menu for a file row.** Right-click a row in the Files panel: Copy path, and on this computer Open in File Explorer (Finder on macOS, File Manager on Linux), or Show/Reveal for a file.
- **The Files panel watches only what it shows.** A build in `target/`, an install in `node_modules/`, a folder you have collapsed, and a window behind another one no longer make it read anything; it follows the folders you open and the checkout's git directory.

### Fixed
- A pane's program starts at the size it is shown at, so a full-screen program's first frame is not laid out for a smaller pane.
- Ctrl-[, Ctrl-I, Ctrl-M, Ctrl-minus and Ctrl-underscore reach a program the way it expects, whether or not it asked for modifyOtherKeys.
- The Files panel lists a Windows machine's folders: where there is no `sh`, it asks PowerShell instead of showing "cannot start sh".
- Git A/M/D marks appear in a folder that is a subdirectory of a checkout.
- Next and previous space follow the order the sidebar shows, including its machine groups.
- A space you drag stays where you put it, even while another machine is still connecting.
- A pane's focus ring hides while a dialog or a floating window has the keyboard.
- Dragging a selection out of a terminal ends it before what you release over acts on it.
- On Windows, a character the font lacks falls back to another font instead of a blank box.
- On a dark Linux desktop the window no longer opens with a light frame first.
- On Wayland, a second seat no longer takes input from the first.
- The keys dialog closes with the key that opened it, and on Linux Ctrl+? opens it.
- A window with no spaces takes the keyboard on startup.
- Mods see the window lose and regain focus. (#350)
- On Windows, installing over an older build no longer leaves a stale Herdr row in Settings → Apps.

## [0.1.15] - 2026-11-02

### New
- dsh-tui agents show their own icon and name, and start from the new agent menu. (#335, thanks @tsonglew)

### Fixed
- The command palette scrolls smoothly, from a wheel and a touchpad. (#348, thanks @patrick-xin)
- Add a machine lists the hosts in the files `~/.ssh/config` includes. (#328, thanks @ashrodan)
- Braille graphs like btop's are spaced evenly across their rows. (#351, thanks @leoscricca)
- A copy key reaches the program when nothing is selected. (#319, thanks @sllrs)
- An empty bitmap glyph in a font no longer crashes the window. (#316, thanks @ruilopesm)

## [0.1.14] - 2026-11-02

### New
- On Windows, a double-click installer (`herdr-app-windows-x86_64-setup.exe`) installs Herdr without a script: it puts the app in the same place, adds it to your PATH and the Start menu, and uninstalls from Settings → Apps. The PowerShell installer writes the same thing, so either one updates the other.

## [0.1.13] - 2026-11-02

### New
- Mods can take the keyboard: a mod whose `mod.json` says `"focusable": true` gets it when `pane.go` moves onto a copy, handles its own keys, and hands it back when you leave. (#283, thanks @Osc100)
- The sidebar reconnects a machine that was rebuilt or replaced, and keeps a failed attempt to retry instead of starting over.
- Update notes show only what applies to your operating system.
- Pig agents show their own icon and name, and start from the new agent menu. (#299, thanks @balcsida)

### Fixed
- Emoji draw in color: COLRv1 emoji render, and a color glyph with nothing in it falls back to its outline instead of vanishing.
- Emoji that ask for a wide, colored form draw wide and in color. (#287, thanks @occultsound)
- Nerd font icons use the terminal font you chose.
- Braille patterns draw as cell-sized sprites, in line with the text around them.
- Programs that ask for the terminal size get it right when in-band resizing is on.
- Mods no longer redraw when something unrelated to them changes, and update notes are read once.
- Virtual lists and drawn paths follow the window zoom. (#296, thanks @Calonca)
- The Files panel opens a directory link as a folder, loading its contents only when you expand it. (#301, thanks @azaeldrm)
- The Files panel marks the files inside a new folder. (#309, thanks @Nagell)
- A new tab with no space open starts a space instead of failing. (#303, thanks @sundarshahi)
- Moving focus back into a column split lands on its top pane. (#304, thanks @sundarshahi)
- A new space is created on the machine you're working in.
- The agents sidebar shows an unnamed tab's terminal title, not its tab number. (#281, thanks @sundarshahi)
- Grok Build shows its own icon. (#293, thanks @Zynoo71)
- A malformed target id is refused before it closes the work that's open. (#297, thanks @chanyang721)
- On macOS, programs in Herdr terminals can ask the system to control other apps. (#252)
- On macOS, terminal programs can use the microphone and camera. (#137, thanks @shivanandasai-altir)

## [0.1.12] - 2026-11-01

### New
- An OpenCode usage window (`ctrl-alt-u`) shows the OpenCode Go plan's 5-hour, weekly and monthly windows with their reset countdowns. It reads a Go key from the local `opencode` server and sends only that key, to `opencode.ai`; nothing is written down. On the status bar the same numbers appear when `shared.json` has `"opencode-usage": true`.
- A file dragged from the Files panel into a terminal pastes its path there.
- A click in the Files panel opens the file in a tab of its own; Markdown opens in Obsidian when one of its vaults holds it.
- Pinned and seen agents follow you between Herdr clients: the terminal carries them, so another window shows the same pins.
- On macOS, Settings → Keyboard chooses what Option does, and `"left"` / `"right"` makes only that one act as Alt.

### Fixed
- The Files panel reads folders natively, so non-ASCII names stay right and folders open at once.
- A Files viewer tab shows the file, not the browser.
- Mod pictures and icons update when their file is saved.
- Horizontal scrolling reaches programs that track the mouse, like nvim and tmux.
- Rounded corners are drawn as arcs and dashed box lines as dashes. (#255)
- On Windows, Escape stays separate in win32 input mode. (#162)
- Ctrl+Insert and Shift+Insert copy and paste in terminals. (#258)
- A remote machine's login shell is loaded when checking for and launching agents, so its PATH has the tools set up there. (#262)
- On Linux, folder watching skips Git-ignored trees and lets go of very large ones; the Files panel falls back to its timer.
- The + menu shows the last known agents while it checks again. (#251)
- The sidebar header lines up with the sidebar in full screen on macOS. (#261)
- On Windows, install.sh in Git Bash points at the PowerShell installer. (#238)

## [0.1.11] - 2026-10-31

### Fixed
- The window no longer freezes or spikes the CPU for seconds at a time while mods run.
- Programs get the new colors right away when you change the theme or switch light and dark. (#122, thanks @geoqiao)
- On macOS, Vietnamese input methods like OpenKey, DauKey and Gõ Nhanh type in terminals. (#230, thanks @emizuki)
- Links a program breaks across lines open with Cmd+click. (thanks @pdeveltere)
- Editing the theme you're using repaints the window. (thanks @shawnyeager)
- Mod tooltips show in embedded windows. (thanks @ruilopesm)
- Opening the palette or a popup again closes it instead of stacking another. (#151, thanks @ruttybob)
- The status bar shows your real palette key. (#150, thanks @ruttybob)
- The Go-to popup keeps the selected row in view. (#152, thanks @ruttybob)
- No empty strip under the agents list in a wide sidebar. (thanks @kaofelix)
- A failed update check no longer says "Up to date". (#186, thanks @weichieh0204)

## [0.1.10] - 2026-10-30

### Fixed
- Images that Claude Code, yazi and other programs draw with kitty placeholders show up instead of rows of boxes. (#110)
- Images are the right size and sharp on Retina and other HiDPI screens. (#70, thanks @wpdmitry)
- Hover effects work in Claude Code and other programs that track the mouse.
- A drag that starts in another pane no longer reaches nvim or tmux, and a drag inside one keeps working when you let go outside it.
- Dragging a selection past the top or bottom of a terminal scrolls it.
- Closing a tab or space no longer brings its terminals back as unnamed tabs for a moment. (#108)
- The + menu starts agents with 1 to 9 while it's open, and shows your own shortcuts. (#102, #133, thanks @notHuman9504)
- Add a machine lists every host in your SSH config and shows the install prompt on top. (#95)
- Your own alert sounds play, and sound changes apply without a restart.
- A layout block that shows the focused agent stays in your layout when that agent ends.
- The Files panel keeps your open folders and preview when you switch spaces. (#86, thanks @ruilopesm)
- Dragging a space in the sidebar shows where it will land. (#114, thanks @takahashi-seiryu)
- Long agent titles cut off before the pin and the time. (#127, thanks @rashim2104)
- The + above Spaces lines up with the sidebar button. (#121, thanks @weskor)
- On macOS, a slow mouse wheel scrolls terminals. (#80, thanks @juanibiapina)
- On macOS, Cmd+V pastes copied files and screenshots into agents. (#105, thanks @WesleyBLDC)
- On macOS, Cmd+Shift+[ and ] switch tabs, and other Shift+punctuation shortcuts work.
- On macOS, Option types [ ] { } on ABC-QWERTZ and ABC-AZERTY.
- On macOS, Vietnamese Telex and other input methods compose. (#93)
- On macOS, keys typed with Fn held, like Fn+Esc for a backtick, reach terminals.
- On macOS, agents started from the + menu load your login setup, like Homebrew's PATH.
- On Windows, Ctrl+V pastes text, like Windows Terminal.
- On Windows, the Files panel opens folders with spaces or parentheses in their path. (#130)
- On Windows, the window buttons line up and stay crisp at every display scale.
- On Linux, Chinese typed through fcitx or ibus on X11 comes out right. (#97)
- On Linux, accent keys no longer type a stray character first. (#87, thanks @ruilopesm)
- On Linux, right-clicking a tab no longer opens the window manager's menu too. (#79, thanks @dev-gvs)
- `terminal.open` with a command starts in the folder you give it.
- `herdr-app check` no longer crashes on mods with scrolling lists or layout animations.

## [0.1.9] - 2026-10-30

### Fixed
- On macOS, updates keep Herdr's Apple signature again, so macOS stops forgetting the permissions you gave Herdr.
- Herdr no longer uses CPU in the background when the Changes or Files panel is open in a git repo.
- Mods use less CPU while they wait for work.

## [0.1.8] - 2026-10-30

### New
- **Option works as Alt on macOS.** Claude Code's Option shortcuts work, automatically on US keyboards. Change it with `option_as_alt` in settings.json.
- **Programs can copy to your clipboard,** also from other machines. Pi's or Claude Code's own mouse selection lands on your clipboard.
- **Your own commands in the + menu.** Entries in programs.json show up, shell functions included.

### Fixed
- Accent keys work on macOS and with ibus on Linux, and Caps Lock types capitals on macOS.
- @, [ and friends type on Slovenian, German and other Mac layouts in every program.
- Terminal colors show exactly as programs set them, so powerline prompts look right.
- Double-click selects a word, triple-click a line.
- Agents start from the + menu under fish, and one that fails at launch says why.
- On macOS, the window buttons stay clear of a narrow sidebar and the tabs.
- You can type a commit message in the Changes panel.
- Spaces named with emoji or accents work everywhere.
- The font list in Settings scrolls on its own.
- A downloaded update installs at the next start, even after a crash.

## [0.1.7] - 2026-10-29

### Fixed
- Herdr no longer freezes when a terminal shows an older Nerd Font icon it has no picture for, like some powerlevel10k battery icons. It draws them as text instead.

## [0.1.6] - 2026-10-29

### New
- **Coming from herdr?** The welcome says what carries over and gives you one line for your agent to bring your keybinds and settings across. Open it again with Show the welcome again in the command palette.
- **Updates at a glance.** Settings → Updates shows the app and the core on one row each, with What's new a click away.
- **herdr-core updates with Herdr.** After Herdr updates, its core moves to the latest version too. Your terminals keep running.
- **Screenshots in feedback.** Drop images straight onto the feedback form.

### Fixed
- Paste and copy in a peek go to the terminal you're peeking at, not the agent behind it.
- Scroll back through thousands of lines instead of a few hundred.
- Mod data, like topics and colors, can't get lost if Herdr closes in the middle of saving.
- Commands with an argument they don't know say so instead of acting on the current terminal.
- On Windows, file links at the end of a sentence open the right file.

## [0.1.5] - 2026-10-27
<!-- spotlight -->

### New
- **Drag panes where you want them.** Hover the top of a pane and grab the dots. Drop it on another pane to swap them, or near an edge to put it beside. A highlight shows where it lands.
  ![Dragging a pane beside another](https://media.herdr.dev/0.1.5/pane-drag.webp)
- **Ctrl-click a terminal to peek at it.** When an agent mentions another terminal, like t2, Ctrl-click (Cmd on macOS) its address to see it over your tab. Go to jumps there.
  ![Peeking at a terminal an agent mentioned](https://media.herdr.dev/0.1.5/peek.webp)
- **Agents can see the window.** `herdr-app shot` saves a picture of Herdr as it looks right now, so an agent building a mod can check its work. `herdr-app --skill` and `hc --skill` teach an agent the rest.
- **What changed, after every update.** Release notes like these, in Settings → Updates, including versions you skipped.

### Fixed
- nvim and other full-screen programs redraw right after you resize their pane.
- On Windows, AltGr and accent keys type the right characters, and Shift+Enter adds a new line in Claude Code.
- On Windows, new terminals start in PowerShell 7 when you have it, and Herdr follows their folder.
- On macOS, Herdr keeps the permissions you gave it across updates.
- Short menus open next to the pointer instead of near the top of the window.
- Removing a worktree asks once, then removes it.
- Mods can fetch from the web, know their size, and clean up when they close.

## [0.1.4] - 2026-10-23
<!-- spotlight -->

### New
- **See the agents that need you first.** Pick Priority beside the Agents title: agents waiting on you, then the ones that just finished, then the ones still working. Grouped by space is still there.
- **Pin the ones you're watching.** Hover an agent and click the pin. Pinned agents stay on top in both views until they end.
- **Fold what you don't need.** Click a space or section header to fold it; the header still shows how many agents are inside.

### Fixed
- Temporary agent sandboxes stay out of your spaces and machine lists.
- The agents list keeps its place when you scroll.
- An agent you're in moves to its new section right away.
- A pane closes when its terminal finishes instead of showing “has finished”.
- On macOS, the red close button hides Herdr instead of leaving it without a window.
- Mods reload cleanly when you save them, and tell you when a save didn't load.
