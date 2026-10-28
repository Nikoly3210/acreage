# Changelog

## 0.1.0 — unreleased

First release.

- Live treemap: fills in while scanning, updates when files change (inotify)
- Sizes like `du`: hardlinks once, btrfs subvolumes of the same filesystem, sparse files flagged, disk usage or apparent size
- Filter language, tags, several views, rescan one folder, scan into other filesystems on demand
- Move to Trash with confirm; open, open folder, terminal here, copy path
- Reports in text, CSV and JSON, in the app and with `--report`
- Keyboard: hjkl/arrows, Enter, Backspace, `/`, Shift+F10, `+`/`-`, `?`, Ctrl+Q
- Settings in `~/.config/acreage/config.toml`; dark and light themes
