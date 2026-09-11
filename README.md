# bnclip

_Clipboard history "manager" for Wayland, in the spirit of cliphist_

- Write clipboard changes to a history file.
- Recall history with **dmenu**, **rofi**, **wofi**, **fuzzel** (or whatever other picker you like).
- **Text**, **file references**, and **images** are supported.
- Clipboard is preserved **byte-for-byte**.
- No concept of a picker, only pipes.
- Extras over cliphist: **pin** entries so they survive trimming, **ignore patterns** to skip what you never want stored, and file copies normalized to `file://` URIs so they paste back as files.

Requires [Rust](https://www.rust-lang.org/tools/install) and [wl-clipboard](https://github.com/bugaevc/wl-clipboard).

---

## Install

```sh
cargo install --path .
```

This puts the `bnclip` binary in `~/.cargo/bin` — make sure it's in your `$PATH`.

---

## Usage

### Listen for clipboard changes

```sh
wl-paste --type text --watch bnclip store
wl-paste --type image --watch bnclip store
```

Each `wl-paste --watch` negotiates exactly one MIME type per clipboard change,
so two watchers are needed to capture both text and images. Run them once per
session — for example, in your Hyprland config with `exec-once`.

### Select an old item

```sh
bnclip list | fuzzel --dmenu | cut -f1 | xargs -r bnclip decode | wl-copy
```

(`decode` takes the hash as an argument, hence `cut | xargs`.)
Bind it to something nice on your keyboard. Any picker works the same way —
`dmenu`, `rofi -dmenu`, `wofi -S dmenu`, `fzf --no-sort`.

File entries (`text/uri-list`) paste back as files with an explicit type:

```sh
bnclip list | fuzzel --dmenu | cut -f1 | xargs -r bnclip decode | wl-copy --type text/uri-list
```

### List format

`bnclip list` emits TSV without a header, newest first:

```
<hash>  <mime>  <pinned>  <timestamp>  <preview>
```

`mime` is one of `text/plain`, `text/uri-list`, or `image/*`.
Previews look like `hello world`, `[[ file: report.pdf ]]`, `[[ directory: Photos/ ]]`,
or `[[ image png 1.2 MiB ]]`.

### Pin an entry

```sh
bnclip list | fuzzel --dmenu | cut -f1 | xargs -r bnclip pin
```

Pinned entries survive history trimming. Running `pin` again unpins.

### Delete an entry

```sh
bnclip list | fuzzel --dmenu | cut -f1 | xargs -r bnclip delete
```

### Clear history

```sh
bnclip wipe
```

---

## Configuration

`~/.config/bnclip/config.toml` is created on first run. Fields and defaults:

| Key | Default | Description |
|---|---|---|
| `preview_width` | `100` | Max chars of text previews in `list` |
| `max_history_length` | `500` | Max entries; oldest unpinned evicted first |
| `max_store_size` | `"10.0 MiB"` | Larger copies are silently dropped |
| `min_store_size` | `"1 B"` | Smaller copies are silently dropped |
| `ignore_patterns` | `[]` | Regexes; matching text is never stored |

Example:

```toml
preview_width = 100
max_history_length = 500
max_store_size = "10.0 MiB"
min_store_size = "1 B"
ignore_patterns = ["^<meta", "^secret:"]
```
