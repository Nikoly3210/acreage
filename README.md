# Acreage

See what's taking up your disk. A live map of every folder and file, labelled and to scale, that keeps up as things change. For Linux.

![Acreage on a home folder](site/img/hero-dark.png)

- **Live.** Blocks appear while the scan runs and update when files change on disk.
- **Sizes that match `du`.** Hardlinks counted once, btrfs subvolumes included, sparse files flagged.
- **Keyboard all the way.** hjkl/arrows, Enter, Backspace, `/` to filter, Shift+F10 for the menu, `?` for every key. Tiles cleanly in i3 and Sway.
- **Filters** like `*.iso;>1gb;<6months;|:green`, tags, several views, Trash with confirm, reports in text/CSV/JSON.

## Install

- Arch: `yay -S acreage` (AUR, from the first release)
- Flathub: coming
- From source (recent stable Rust; developed with 1.99):

```
git clone https://github.com/Nikoly3210/acreage acreage
cd acreage
cargo build --release
./target/release/acreage ~
```

## Use

```
acreage [PATH]                 open the map (no PATH: pick a disk)
acreage PATH --report - [--depth N] [--files] [--format text|csv|json]
                               print a report without opening a window
  --apparent-size              file length instead of space on disk
  --cross-fs                   also go into other filesystems below PATH
  -f, --filter EXPR            start with a filter
  --theme dark|light
```

Settings live in `~/.config/acreage/config.toml`. Press `?` in the app for every shortcut.

## Privacy

No account, no telemetry, no network requests. Nothing leaves your machine.

## Problems

Open an issue at https://github.com/Nikoly3210/acreage/issues with your distro, display server (X11/Wayland) and the output of `acreage --version`.

## Development

```
cargo test                                         # unit + scanner tests
cargo build --release && python3 -m unittest discover -s e2e -v   # end-to-end, needs Xvfb and xdotool
cargo run --example gallery                        # design system gallery
```

## Licence

GPL-3.0-or-later. See [LICENSE](LICENSE).
