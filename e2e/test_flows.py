"""canopy end-to-end tests, one class per recon flow. Case ids match replica/test-plan.md.

    cargo build --release && python3 -m unittest discover -s e2e -v
"""

import os
import subprocess
import time
import unittest
from pathlib import Path

from harness import CanopyCase, apparent_total, display, trash_dir, write

MB = 1 << 20


def basic_tree(root: Path):
    write(root / "Videos" / "holiday.mkv", 90 * MB)
    write(root / "Videos" / "talk.mkv", 40 * MB)
    write(root / "Videos" / "clip.webm", 12 * MB)
    write(root / "Downloads" / "arch.iso", 60 * MB)
    write(root / "Downloads" / "notes.txt", 2 * MB)
    for i in range(30):
        write(root / "Music" / f"song{i:02}.mp3", (4 + i % 5) * MB)
    return root


# ---------------------------------------------------------------- F01 find what is eating the disk


class F01FindSpace(CanopyCase):
    def test_F01_H1_total_matches_reference(self):
        root = basic_tree(self.tmp / "fx")
        os.link(root / "Videos" / "talk.mkv", root / "Downloads" / "talk-hardlink.mkv")
        (root / "Downloads" / "loop").symlink_to(root)
        app = self.launch(root, "--apparent-size")
        s = app.wait_done()
        self.assertEqual(s["total_size"], apparent_total(root))
        self.assertEqual(s["root"], str(root))
        self.assertGreater(s["blocks"], 30)

    def test_F01_H2_double_click_zooms_and_backspace_returns(self):
        root = basic_tree(self.tmp / "fx")
        app = self.launch(root, "--apparent-size")
        app.wait_done()
        app.click_block(root / "Videos", at="header", repeat=2)
        s = app.wait(lambda s: s["zoom"] == str(root / "Videos"), what="zoom into Videos")
        self.assertEqual(s["history"], 1)
        app.wait(lambda s: not s["animating"], what="zoom animation to end")
        app.key("BackSpace")
        s = app.wait(lambda s: s["zoom"] == str(root), what="zoom back to root")
        self.assertEqual(s["selected"], str(root / "Videos"))

    def test_F01_H3_picker_path_entry(self):
        root = basic_tree(self.tmp / "fx")
        app = self.launch()
        app.wait(lambda s: s["screen"] == "picker", what="picker")
        app.click(500, 108)  # the path field
        app.type(str(root))
        app.key("Return")
        s = app.wait_done()
        self.assertEqual(s["root"], str(root))

    def test_F01_E1_unreadable_folder(self):
        root = basic_tree(self.tmp / "fx")
        write(root / "locked" / "secret.bin", 5 * MB)
        (root / "locked").chmod(0o000)
        app = self.launch(root, "--apparent-size")
        s = app.wait_done()
        if os.geteuid() != 0:
            self.assertEqual(s["errors"], 1)

    def test_F01_E2_huge_directory_streams_and_folds(self):
        root = self.tmp / "fx"
        for i in range(12_000):  # > 2 scanner batches of 4096
            write(root / "many" / f"f{i:05}", 1000 + i)
        app = self.launch(root, "--apparent-size")
        s = app.wait_done()
        self.assertEqual(s["items"], 12_000)
        self.assertLess(s["blocks"], 12_000, "tiny files must fold into an aggregate")

    def test_F01_E3_odd_names(self):
        root = self.tmp / "fx"
        write(root / "Música ünïcødé 🎵" / "canción 🎸.flac", 20 * MB)
        write(root / ("x" * 250 + ".bin"), 30 * MB)
        raw = os.fsencode(root) + b"/bad-\xff\xfe-bytes.bin"
        with open(raw, "wb") as f:
            f.seek(10 * MB - 1)
            f.write(b"\0")
        app = self.launch(root, "--apparent-size")
        s = app.wait_done()
        self.assertEqual(s["items"], 3)
        app.click_block(root / "Música ünïcødé 🎵", at="header")
        s = app.wait(lambda s: s["selected"] == str(root / "Música ünïcødé 🎵"), what="select unicode folder")

    def test_F01_E4_resize_and_narrow_window(self):
        root = basic_tree(self.tmp / "fx")
        app = self.launch(root, "--apparent-size")
        before = app.wait_done()["rects"]
        app.resize(330, 260)
        s = app.wait(lambda s: s["rects"] != before, what="relayout after resize")
        app.shot("F01-E4-narrow")
        app.resize(1200, 760)
        app.wait(lambda s: s["rects"] != s.get("_never"), what="alive after resize back")

    def test_F01_E5_keyboard_only(self):
        root = basic_tree(self.tmp / "fx")
        app = self.launch(root, "--apparent-size")
        app.wait_done()
        app.key("Down")
        s = app.wait(lambda s: s["selected"] is not None, what="keyboard selection")
        first = s["selected"]
        app.key("Right")
        s = app.wait(lambda s: s["selected"] != first, what="selection moved right")
        # Pick a folder with arrows, then Enter zooms into it.
        for _ in range(4):
            if Path(app.state()["selected"]).is_dir():
                break
            app.key("Left")
        sel = app.state()["selected"]
        self.assertTrue(Path(sel).is_dir(), sel)
        app.key("Return")
        app.wait(lambda s: s["zoom"] == sel, what="Enter zooms")
        app.key("alt+Left")
        app.wait(lambda s: s["zoom"] == str(root), what="Alt+Left goes back")

    def test_F01_E6_mouse_back_button(self):
        root = basic_tree(self.tmp / "fx")
        app = self.launch(root, "--apparent-size")
        app.wait_done()
        app.click_block(root / "Music", at="header", repeat=2)
        app.wait(lambda s: s["zoom"] == str(root / "Music"), what="zoom Music")
        app.wait(lambda s: not s["animating"], what="anim end")
        app.click(600, 400, button=8)
        app.wait(lambda s: s["zoom"] == str(root), what="mouse back")

    def test_F01_E7_two_instances_same_folder(self):
        root = basic_tree(self.tmp / "fx")
        a = self.launch(root, "--apparent-size")
        sub = self.tmp / "second"
        sub.mkdir()
        # Second instance gets its own state file.
        from harness import App

        b = App(display(), [str(root), "--apparent-size"], sub)
        self.apps.append(b)
        sa, sb = a.wait_done(), b.wait_done()
        self.assertEqual(sa["total_size"], sb["total_size"])

    def test_F01_E8_rescan_picks_up_new_files(self):
        root = basic_tree(self.tmp / "fx")
        app = self.launch(root, "--apparent-size")
        before = app.wait_done()["items"]
        write(root / "Downloads" / "new.iso", 10 * MB)
        app.key("F5")
        app.wait(lambda s: s["scan"] == "done" and s["items"] == before + 1, what="rescan sees new file")

    def test_F01_N1_cli_rejects_missing_path(self):
        r = self.run_cli(self.tmp / "does-not-exist")
        self.assertEqual(r.returncode, 2)
        self.assertIn("Can't open", r.stderr)

    def test_F01_N2_cli_rejects_file(self):
        f = write(self.tmp / "file.bin", 10)
        r = self.run_cli(f)
        self.assertEqual(r.returncode, 2)
        self.assertIn("not a folder", r.stderr)

    def test_F01_N3_cli_rejects_unreadable_root(self):
        if os.geteuid() == 0:
            self.skipTest("root can read anything")
        d = self.tmp / "locked"
        d.mkdir()
        d.chmod(0o000)
        r = self.run_cli(d)
        self.assertEqual(r.returncode, 2)


# ---------------------------------------------------------------- F02 narrow with a filter


class F02Filter(CanopyCase):
    def test_F02_H1_glob_filter_sizes(self):
        root = basic_tree(self.tmp / "fx")
        app = self.launch(root, "--apparent-size", "-f", "*.mkv")
        s = app.wait(lambda s: s.get("scan") == "done" and s["filter_applied"] == "*.mkv", what="filter")
        self.assertEqual(s["zoom_size"], 130 * MB)

    def test_F02_H2_typed_filter_and_range(self):
        root = basic_tree(self.tmp / "fx")
        app = self.launch(root, "--apparent-size")
        app.wait_done()
        app.key("slash")
        app.type("*.mp3;>6mb;<8mb")
        app.key("Return")
        s = app.wait(lambda s: s["filter_applied"] == "*.mp3;>6mb;<8mb", what="typed filter applied")
        self.assertEqual(s["zoom_size"], 6 * 7 * MB, "seven-MiB songs only (i%5==3)")

    def test_F02_E1_invalid_filter_keeps_previous(self):
        root = basic_tree(self.tmp / "fx")
        app = self.launch(root, "--apparent-size", "-f", "*.mkv")
        app.wait_done()
        app.key("slash", "ctrl+a")
        app.type("*.iso;>1gbb")
        app.key("Return")
        s = app.wait(lambda s: s["filter_error"] is not None, what="filter error")
        self.assertIn("gbb", s["filter_error"])
        self.assertEqual(s["filter_applied"], "*.mkv")

    def test_F02_E2_slash_shortcut_does_not_type(self):
        root = basic_tree(self.tmp / "fx")
        app = self.launch(root, "--apparent-size")
        app.wait_done()
        app.key("slash")
        app.type("iso")
        s = app.wait(lambda s: s["filter_text"] == "iso", what="filter text")
        self.assertEqual(s["filter_text"], "iso")

    def test_F02_E3_nothing_matches_then_clear(self):
        root = basic_tree(self.tmp / "fx")
        app = self.launch(root, "--apparent-size", "-f", "*.nothing")
        s = app.wait(lambda s: s.get("scan") == "done" and s["zoom_size"] == 0, what="empty filter result")
        app.shot("F02-E3-empty")
        app.key("slash", "ctrl+a", "BackSpace", "Return")
        app.wait(lambda s: s["filter_applied"] == "" and s["zoom_size"] > 0, what="filter cleared")

    def test_F02_E4_exclude_and_unicode_pattern(self):
        root = basic_tree(self.tmp / "fx")
        write(root / "Música 🎵" / "x.flac", 7 * MB)
        app = self.launch(root, "--apparent-size", "-f", "|*.mkv;|*.mp3;|*.webm;|*.iso;|*.txt")
        s = app.wait(lambda s: s.get("scan") == "done" and s["filter_applied"].startswith("|"), what="exclude filter")
        self.assertEqual(s["zoom_size"], 7 * MB)
        app.key("slash", "ctrl+a")
        app.type("🎵")
        app.key("Return")
        s = app.wait(lambda s: s["filter_applied"] == "🎵", what="emoji filter")
        self.assertEqual(s["zoom_size"], 7 * MB)

    def test_F02_E5_very_long_filter(self):
        root = basic_tree(self.tmp / "fx")
        long = ";".join(f"*.e{i}" for i in range(300))
        app = self.launch(root, "--apparent-size", "-f", long)
        s = app.wait(lambda s: s.get("scan") == "done" and s["filter_applied"] == long, what="long filter")
        self.assertEqual(s["zoom_size"], 0)


# ---------------------------------------------------------------- F03 tag and clean up


class F03TagAndTrash(CanopyCase):
    def setUp(self):
        super().setUp()
        self.root = basic_tree(self.tmp / "fx")

    def test_F03_H1_tag_and_filter_by_tag(self):
        app = self.launch(self.root, "--apparent-size")
        app.wait_done()
        target = self.root / "Downloads" / "arch.iso"
        app.click_block(target)
        app.wait(lambda s: s["selected"] == str(target), what="select iso")
        app.key("ctrl+1")
        s = app.wait(lambda s: s["tags"] == [f"{target}=Red"], what="red tag")
        app.key("slash")
        app.type(":red")
        app.key("Return")
        s = app.wait(lambda s: s["filter_applied"] == ":red", what=":red filter")
        self.assertEqual(s["zoom_size"], 60 * MB)
        app.key("Escape")  # leave the filter field
        app.click(5, 16)  # nothing; keep focus on canvas
        app.key("ctrl+0")

    def test_F03_H2_trash_with_confirm(self):
        app = self.launch(self.root, "--apparent-size")
        before = app.wait_done()["total_size"]
        target = self.root / "Downloads" / "arch.iso"
        app.click_block(target)
        app.key("Delete")
        s = app.wait(lambda s: s["confirm_open"], what="confirm panel")
        self.assertEqual(s["confirm_items"], [str(target)])
        app.shot("F03-H2-confirm")
        app.key("Return")
        s = app.wait(lambda s: not s["confirm_open"], what="trash finished")
        self.assertFalse(target.exists())
        self.assertTrue(any(p.name.startswith("arch") for p in trash_dir().iterdir()))
        self.assertEqual(s["total_size"], before - 60 * MB)
        self.assertTrue(any("Moved 1 item" in t for t in s["toasts"]), s["toasts"])

    def test_F03_E1_double_enter_trashes_once(self):
        app = self.launch(self.root, "--apparent-size")
        app.wait_done()
        target = self.root / "Videos" / "holiday.mkv"
        app.click_block(target)
        app.key("Delete")
        app.wait(lambda s: s["confirm_open"], what="confirm")
        app.key("Return", "Return", "Return", delay=0.02)
        s = app.wait(lambda s: not s["confirm_open"], what="done")
        self.assertFalse(target.exists())
        self.assertFalse(any("Couldn't" in t for t in s["toasts"]), s["toasts"])
        self.assertEqual(sum("Moved" in t for t in s["toasts"]), 1)

    def test_F03_E2_file_vanished_before_confirm(self):
        app = self.launch(self.root, "--apparent-size")
        app.wait_done()
        target = self.root / "Videos" / "talk.mkv"
        app.click_block(target)
        app.key("Delete")
        app.wait(lambda s: s["confirm_open"], what="confirm")
        target.unlink()
        app.key("Return")
        s = app.wait(lambda s: not s["confirm_open"], what="done")
        self.assertFalse(any("Couldn't" in t for t in s["toasts"]), s["toasts"])
        self.assertNotIn(str(target), [r.rsplit("|", 4)[0] for r in s["rects"]])

    def test_F03_E3_escape_cancels(self):
        app = self.launch(self.root, "--apparent-size")
        app.wait_done()
        target = self.root / "Downloads" / "arch.iso"
        app.click_block(target)
        app.key("Delete")
        app.wait(lambda s: s["confirm_open"], what="confirm")
        app.key("Escape")
        app.wait(lambda s: not s["confirm_open"], what="cancelled")
        self.assertTrue(target.exists())

    def test_F03_E4_cannot_trash_scan_root(self):
        app = self.launch(self.root, "--apparent-size")
        app.wait_done()
        app.move(5, 108)  # over the root header
        app.key("Delete")
        time.sleep(0.5)
        self.assertFalse(app.state()["confirm_open"])
        self.assertTrue(self.root.exists())

    def test_F03_E5_rescan_while_trash_pending_keeps_tree_consistent(self):
        app = self.launch(self.root, "--apparent-size")
        app.wait_done()
        target = self.root / "Downloads" / "arch.iso"
        app.click_block(target)
        app.key("Delete")
        app.wait(lambda s: s["confirm_open"], what="confirm")
        app.click(150, 16)  # toolbar Rescan while the confirm panel is open
        s = app.wait(lambda s: s["scan"] == "done", what="rescan")
        if s["confirm_open"]:
            app.key("Return")
        s = app.wait(lambda s: not s["confirm_open"] and s["scan"] == "done", what="settled")
        time.sleep(0.5)
        s = app.state()
        self.assertEqual(s["total_size"], apparent_total(self.root), "tree must match the disk after rescan + trash")

    def test_F03_N1_trash_permission_denied(self):
        app = self.launch(self.root, "--apparent-size")
        app.wait_done()
        target = self.root / "Music" / "song00.mp3"
        app.click_block(self.root / "Music", at="header")
        (self.root / "Music").chmod(0o555)
        app.click_block(target)
        app.key("Delete")
        app.wait(lambda s: s["confirm_open"], what="confirm")
        app.key("Return")
        s = app.wait(lambda s: not s["confirm_open"], what="done")
        self.assertTrue(target.exists())
        self.assertTrue(any("Couldn't move" in t for t in s["toasts"]), s["toasts"])
        self.assertIn(str(target), [r.rsplit("|", 4)[0] for r in s["rects"]])


# ---------------------------------------------------------------- F04 act on one item


class F04Actions(CanopyCase):
    def test_F04_H1_copy_path(self):
        root = basic_tree(self.tmp / "fx")
        app = self.launch(root, "--apparent-size")
        app.wait_done()
        target = root / "Downloads" / "arch.iso"
        app.click_block(target)
        app.key("y")
        app.wait(lambda s: any("Copied" in t for t in s["toasts"]), what="copied toast")
        clip = subprocess.run(["xclip", "-o", "-selection", "clipboard"], env=display().env, capture_output=True, text=True, timeout=5)
        self.assertEqual(clip.stdout, str(target))

    def test_F04_H2_open_and_terminal_via_menu_keys(self):
        root = basic_tree(self.tmp / "fx")
        app = self.launch(root, "--apparent-size")
        app.wait_done()
        target = root / "Downloads" / "arch.iso"
        app.click_block(target)
        app.key("ctrl+o", "ctrl+t")
        deadline = time.time() + 5
        while time.time() < deadline and not (app.record.exists() and len(app.record.read_text().splitlines()) >= 2):
            time.sleep(0.1)
        lines = app.record.read_text().splitlines()
        self.assertIn(f"xdg-open {Path.cwd()} {target}", lines[0].replace(os.getcwd(), str(Path.cwd())))
        self.assertTrue(lines[1].startswith(f"fake-term {root / 'Downloads'}"), lines[1])

    def test_F04_H3_context_menu_opens(self):
        root = basic_tree(self.tmp / "fx")
        app = self.launch(root, "--apparent-size")
        app.wait_done()
        app.click_block(root / "Videos" / "holiday.mkv", button=3)
        time.sleep(0.4)
        app.shot("F04-H3-menu")
        s = app.state()
        self.assertEqual(s["selected"], str(root / "Videos" / "holiday.mkv"))


# ---------------------------------------------------------------- F09 get back up / misc screens


class F09Navigation(CanopyCase):
    def test_F09_H1_up_from_deep(self):
        root = self.tmp / "fx"
        write(root / "a" / "b" / "c" / "d" / "deep.bin", 50 * MB)
        app = self.launch(root, "--apparent-size")
        app.wait_done()
        for name in ["a", "a/b", "a/b/c"]:
            app.click_block(root / name, at="header", repeat=2)
            app.wait(lambda s, n=name: s["zoom"] == str(root / n), what=f"zoom {name}")
            app.wait(lambda s: not s["animating"], what="anim end")
        for _ in range(3):
            app.key("BackSpace")
        app.wait(lambda s: s["zoom"] == str(root), what="back to root")

    def test_S11_errors_panel_from_status(self):
        root = basic_tree(self.tmp / "fx")
        write(root / "locked" / "x", 10)
        (root / "locked").chmod(0o000)
        if os.geteuid() == 0:
            self.skipTest("root reads everything")
        app = self.launch(root, "--apparent-size")
        app.wait_done()
        app.click(130, 760 - 11)
        app.wait(lambda s: s["errors_panel"], what="errors panel")

    def test_S10_help_overlay(self):
        root = basic_tree(self.tmp / "fx")
        app = self.launch(root, "--apparent-size")
        app.wait_done()
        app.click(1200 - 24, 80)  # ? in the filter bar
        app.wait(lambda s: s["help"], what="help overlay")
        app.key("Escape")
        app.wait(lambda s: not s["help"], what="help closed by Esc")


if __name__ == "__main__":
    unittest.main()
