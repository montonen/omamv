# omamv

The Omarchy Markdown Viewer: open a folder, and every Markdown and text file
in it is one key away, rendered. It only reads; editing is left to the editor
beside it. It wears the active Omarchy theme and follows it live.

```bash
omamv .            # the current folder
omamv docs/        # that folder
omamv README.md    # its folder, with the file shown
omamv              # recent folders
```

Like `code .`, the command hands over to the window and gives the prompt back
(`-w` keeps it in the terminal). One instance serves every window; a folder
already open is brought forward.

## What it shows

- `.md` files rendered: GitHub's Markdown (tables, task lists, alerts,
  footnotes), highlighted code, Mermaid and PlantUML diagrams in the theme's
  colours. A diagram that cannot be drawn shows as code, with the reason.
- `.txt` files as they are.
- `.html` pages, such as reports from scans and jobs, when turned on in
  Settings (off by default): as their author made them, their own styles and
  pictures, but their scripts do not run and nothing loads from the network.
- A folder tree, right or left, with only the folders that hold such files,
  closed but for the way to the open file. Hidden folders and anything
  `.gitignore` leaves out are skipped (both are settings).

When the open file changes on disk the page is drawn again at the same
heading; files added or removed appear in the tree by themselves. A removed
file stays on screen, faded, until you pick another.

Scripts in a document never run. Nothing is fetched to draw a page except
pictures a document links from the web, and that can be turned off. A link to
a file omamv does not show opens nothing. PlantUML runs in its sandbox, so a
diagram cannot read other files.

## Keys

Press `?` in the window for all of them.

| Keys | |
| --- | --- |
| `j` `k` or `↓` `↑` · `d` `u` · `Space` · `g g` `G` | scroll |
| `Ctrl+P` | open a file: newest first, with ages; type to find any |
| `o` | outline: go to a heading |
| `y` | copy the file's path |
| `]` `[` or `J` `K` | next / previous file |
| `Ctrl+B` | folder tree: focus, or hide (`j k h l`, arrows, `Enter`, `Esc` inside) |
| `Ctrl+F` or `/` · `n` `N` | find |
| `Alt+←` `Alt+→` | back / forward |
| `Ctrl+O` · `Ctrl+Shift+O` | open a folder · recent folders |
| `Ctrl+,` | settings |

The status bar shows the keys that matter where the keyboard is, and, when a
folder opens, how fast it was read and drawn. A file that appears on disk
(an agent's new report) is named there too. YAML front matter shows as a
small block of facts above the text.

A folder opens on its README (then an index, then its first file), or, if
Settings say so, on quick open with nothing shown until you pick.

## Install

Arch / Omarchy:

```bash
sudo pacman -S --needed rust webkitgtk-6.0 libadwaita
sudo pacman -S --needed plantuml   # optional: draws PlantUML diagrams
scripts/install.sh
```

The script builds a release, links the binary into `~/.local/bin` (where the
Hyprland session looks), and installs the desktop entry and icon.

## Development

```bash
cargo test           # unit tests
scripts/smoke.sh     # the basics: the real binary started as a person would
scripts/check.sh     # the whole app, offscreen on GTK's Broadway display (runs smoke.sh too)
```

`OMAMV_DEBUG=1 omamv -w .` prints each step of opening a folder and a page.

`scripts/check.sh` runs a debug build through every key path, edits and
removes files in a copy of `tests/fixture`, switches the theme, and writes a
report with pictures to `target/check/`. It never touches the desktop.

Mermaid 11.12.0 is bundled in `assets/` (MIT, `assets/MERMAID-LICENSE`).

## License

MIT, see [LICENSE](LICENSE).
