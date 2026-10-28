"""e2e tests for the second build pass: live updates (F06), views (F05), folder rescan and
tags that survive it (F07), export and --report (F08), settings and presets (S09/S10).

    python3 -m unittest discover -s e2e -v
"""

import json
import os
import subprocess
import time
from pathlib import Path

from harness import AcreageCase, write
from test_flows import MB, basic_tree


def cfg(home: Path, text: str):
    p = home / "acreage" / "config.toml"
    p.parent.mkdir(parents=True, exist_ok=True)
    p.write_text(text)
    return p


# ---------------------------------------------------------------- F06 live updates


class F06Live(AcreageCase):
    def setUp(self):
        super().setUp()
        self.root = basic_tree(self.tmp / "fx")

    def live_app(self):
        app = self.launch(self.root, "--apparent-size")
        app.wait_done()
        app.wait(lambda s: s["live"] is not None and int(s["live"]) >= 4, what="watching folders")
        return app

    def test_F06_H1_new_file_appears_and_blinks(self):
        app = self.live_app()
        before = app.state()
        write(self.root / "Downloads" / "fresh.iso", 30 * MB)
        s = app.wait(lambda s: s["items"] == before["items"] + 1, what="new file counted")
        self.assertEqual(s["total_size"], before["total_size"] + 30 * MB)
        self.assertGreater(s["blinking"], 0)
        app.wait(lambda s: s["blinking"] == 0, timeout=3, what="blink fades")

    def test_F06_H2_deleted_file_disappears(self):
        app = self.live_app()
        before = app.state()
        (self.root / "Downloads" / "arch.iso").unlink()
        s = app.wait(lambda s: s["items"] == before["items"] - 1, what="removal seen")
        self.assertEqual(s["total_size"], before["total_size"] - 60 * MB)

    def test_F06_H3_new_folder_is_scanned(self):
        app = self.live_app()
        before = app.state()
        d = self.tmp / "staging"
        for i in range(5):
            write(d / "inner" / f"f{i}.bin", MB)
        os.rename(d, self.root / "Projects")  # appears in one go, like a git clone finishing
        s = app.wait(lambda s: s["items"] == before["items"] + 5 and s["subscans"] == 0, what="new folder scanned")
        # And its contents are watched now too.
        write(self.root / "Projects" / "inner" / "later.bin", MB)
        app.wait(lambda s: s["items"] == before["items"] + 6, what="change inside the new folder")

    def test_F06_H4_file_grows(self):
        app = self.live_app()
        before = app.state()["total_size"]
        with open(self.root / "Videos" / "clip.webm", "r+b") as f:
            f.truncate(20 * MB)
        app.wait(lambda s: s["total_size"] == before + 8 * MB, what="size change")

    def test_F06_E1_live_updates_off(self):
        home = self.tmp / "cfg"
        cfg(home, "live_updates = false\n")
        app = self.launch(self.root, "--apparent-size", config_home=home)
        before = app.wait_done()
        self.assertIsNone(before["live"])
        write(self.root / "Downloads" / "fresh.iso", 30 * MB)
        time.sleep(1.5)
        self.assertEqual(app.state()["items"], before["items"])

    def test_F06_E2_tagged_file_deleted_outside(self):
        app = self.live_app()
        target = self.root / "Downloads" / "arch.iso"
        app.click_block(target)
        app.key("ctrl+1")
        app.wait(lambda s: s["tags"] == [f"{target}=Red"], what="tagged")
        target.unlink()
        app.wait(lambda s: s["tags"] == [], what="tag gone with the file")
        # A new file at the same path must not inherit the old tag.
        write(target, MB)
        time.sleep(1.0)
        self.assertEqual(app.state()["tags"], [])


# ---------------------------------------------------------------- F05 views


class F05Views(AcreageCase):
    def test_F05_H1_views_keep_their_own_filter_and_zoom(self):
        root = basic_tree(self.tmp / "fx")
        app = self.launch(root, "--apparent-size")
        app.wait_done()
        app.key("ctrl+n")
        app.wait(lambda s: s["tabs"] == 2 and s["active"] == 1, what="second view")
        app.key("slash")
        app.type("*.mkv")
        app.key("Return", "Escape")
        s = app.wait(lambda s: s["tab_filters"] == ["", "*.mkv"], what="filter only in view 2")
        self.assertEqual(s["zoom_size"], 130 * MB)
        app.click_block(root / "Videos", at="header", repeat=2)
        app.wait(lambda s: s["tab_zooms"] == [str(root), str(root / "Videos")], what="zoom only in view 2")
        app.key("ctrl+Tab")
        s = app.wait(lambda s: s["active"] == 0, what="back to view 1")
        self.assertEqual(s["zoom"], str(root))
        self.assertEqual(s["filter_applied"], "")
        app.shot("F05-H1-tabs")
        app.key("ctrl+w")
        app.wait(lambda s: s["tabs"] == 1, what="closed a view")

    def test_F05_E1_last_view_cannot_close(self):
        root = basic_tree(self.tmp / "fx")
        app = self.launch(root, "--apparent-size")
        app.wait_done()
        app.key("ctrl+w")
        time.sleep(0.3)
        self.assertEqual(app.state()["tabs"], 1)


# ---------------------------------------------------------------- F07 refine and tags by path


class F07Refine(AcreageCase):
    def setUp(self):
        super().setUp()
        self.root = basic_tree(self.tmp / "fx")
        self.home = self.tmp / "cfg"
        cfg(self.home, "live_updates = false\n")  # so only the rescan can see the change

    def test_F07_H1_rescan_zoomed_folder_keeps_tags(self):
        app = self.launch(self.root, "--apparent-size", config_home=self.home)
        before = app.wait_done()
        target = self.root / "Downloads" / "arch.iso"
        app.click_block(target)
        app.key("ctrl+3")
        app.wait(lambda s: s["tags"] == [f"{target}=Green"], what="tagged")
        app.click_block(self.root / "Downloads", at="header", repeat=2)
        app.wait(lambda s: s["zoom"] == str(self.root / "Downloads"), what="zoomed")
        write(self.root / "Downloads" / "new.bin", 7 * MB)
        app.key("Escape", "F5")
        s = app.wait(lambda s: s["subscans"] == 0 and s["items"] == before["items"] + 1, what="folder rescanned")
        self.assertEqual(s["tags"], [f"{target}=Green"])
        self.assertTrue(any("Rescanned" in t for t in s["toasts"]), s["toasts"])
        self.assertEqual(s["zoom"], str(self.root / "Downloads"))

    def test_F07_H2_full_rescan_keeps_tags(self):
        app = self.launch(self.root, "--apparent-size", config_home=self.home)
        app.wait_done()
        target = self.root / "Videos" / "holiday.mkv"
        app.click_block(target)
        app.key("ctrl+2")
        app.wait(lambda s: s["tags"] == [f"{target}=Yellow"], what="tagged")
        app.key("ctrl+r")
        s = app.wait(lambda s: s["scan"] == "done" and s["tags"] == [f"{target}=Yellow"], what="tag survives rescan")


# ---------------------------------------------------------------- F08 export and --report


class F08Export(AcreageCase):
    def test_F08_H1_headless_text_report(self):
        root = basic_tree(self.tmp / "fx")
        r = self.run_cli(root, "--apparent-size", "--report", "-", "--depth", "1")
        self.assertEqual(r.returncode, 0, r.stderr)
        lines = r.stdout.splitlines()
        self.assertIn(str(root), lines[0])
        self.assertTrue(any(l.endswith("├─ Music") or l.endswith("└─ Music") for l in lines), r.stdout)
        self.assertTrue(any("Videos" in l and "142.0 MiB" in l for l in lines), r.stdout)
        self.assertFalse(any("holiday.mkv" in l for l in lines), "folders only by default")

    def test_F08_H2_headless_csv_json_and_filter(self):
        root = basic_tree(self.tmp / "fx")
        out = self.tmp / "r.csv"
        r = self.run_cli(root, "--apparent-size", "--report", out, "--format", "csv", "--files", "--depth", "2", "-f", "*.mkv")
        self.assertEqual(r.returncode, 0, r.stderr)
        rows = out.read_text().splitlines()
        self.assertEqual(rows[0], "size_bytes,percent,depth,kind,path")
        self.assertEqual(int(rows[1].split(",")[0]), 130 * MB)
        self.assertTrue(any(r.endswith("/Videos/holiday.mkv") for r in rows))
        r = self.run_cli(root, "--apparent-size", "--report", "-", "--format", "json", "--depth", "1")
        j = json.loads(r.stdout)
        self.assertEqual(j["root"], str(root))
        self.assertEqual({e["path"] for e in j["entries"] if e["depth"] == 1}, {str(root / n) for n in ["Videos", "Downloads", "Music"]})

    def test_F08_N1_headless_needs_a_path(self):
        r = self.run_cli("--report", "-")
        self.assertEqual(r.returncode, 2)
        self.assertIn("needs a PATH", r.stderr)

    def test_F08_H3_gui_export_saves_file(self):
        root = basic_tree(self.tmp / "fx")
        app = self.launch(root, "--apparent-size")
        app.wait_done()
        app.key("ctrl+e")
        s = app.wait(lambda s: s["side"] == "export", what="export panel")
        app.shot("F08-H3-export")
        app.key("ctrl+s")
        app.wait(lambda s: any("Saved report" in t for t in s["toasts"]), what="saved toast")
        files = list(app.home.glob("acreage-report-*.txt"))
        self.assertEqual(len(files), 1, files)
        self.assertIn(str(root), files[0].read_text().splitlines()[0])


# ---------------------------------------------------------------- S09 settings, S10 presets


class S09Settings(AcreageCase):
    def test_S09_H1_saved_settings_apply(self):
        root = basic_tree(self.tmp / "fx")
        write(root / ".cache" / "big.bin", 500 * MB)
        home = self.tmp / "cfg"
        cfg(home, 'theme = "light"\nsize = "apparent"\nhide_dotfiles = true\n')
        app = self.launch(root, config_home=home)
        s = app.wait_done()
        self.assertEqual((s["theme"], s["size_mode"], s["hide_dotfiles"]), ("light", "apparent", True))
        self.assertEqual(s["zoom_size"], s["total_size"] - 500 * MB - os.lstat(root / ".cache").st_size)
        app.shot("S09-H1-light")

    def test_S09_H2_theme_flip_is_remembered(self):
        root = basic_tree(self.tmp / "fx")
        home = self.tmp / "cfg"
        app = self.launch(root, config_home=home)
        app.wait_done()
        app.key("ctrl+shift+t")
        app.wait(lambda s: s["theme"] == "light", what="light theme")
        text = (home / "acreage" / "config.toml").read_text()
        self.assertIn('theme = "light"', text)

    def test_S09_E1_broken_config_falls_back(self):
        root = basic_tree(self.tmp / "fx")
        home = self.tmp / "cfg"
        cfg(home, "detail = [not toml\n")
        app = self.launch(root, config_home=home)
        s = app.wait_done()
        self.assertEqual(s["theme"], "dark")
        self.assertTrue(any("Settings file ignored" in t for t in s["toasts"]), s["toasts"])

    def test_S09_H3_settings_panel_opens(self):
        root = basic_tree(self.tmp / "fx")
        app = self.launch(root)
        app.wait_done()
        app.key("ctrl+comma")
        app.wait(lambda s: s["side"] == "settings", what="settings panel")
        app.shot("S09-H3-settings")
        app.key("Escape")
        app.wait(lambda s: s["side"] == "none", what="closed")

    def test_S10_H1_apply_and_save_presets(self):
        root = basic_tree(self.tmp / "fx")
        home = self.tmp / "cfg"
        cfg(home, '[[presets]]\nname = "ISOs"\nfilter = "*.iso"\n')
        app = self.launch(root, "--apparent-size", config_home=home)
        app.wait_done()
        app.click(1200 - 50, 80)  # ▾ in the filter bar
        app.wait(lambda s: s["presets_menu"], what="presets menu")
        app.shot("S10-H1-presets")
        app.click(1200 - 300 - 8 + 80, 92 + 4 + 20 + 12)  # first preset
        s = app.wait(lambda s: s["filter_applied"] == "*.iso", what="preset applied")
        self.assertEqual(s["zoom_size"], 60 * MB)
        # Save a new one.
        app.key("slash", "ctrl+a")
        app.type("*.mp3;>6mb")
        app.key("Return", "Escape")
        app.wait(lambda s: s["filter_applied"] == "*.mp3;>6mb", what="typed filter")
        app.click(1200 - 50, 80)
        app.wait(lambda s: s["presets_menu"], what="presets menu again")
        app.click(1200 - 300 - 8 + 80, 92 + 4 + 20 + 24 + 7 + 12)  # "Save current filter"
        s = app.wait(lambda s: "*.mp3;>6mb" in s["presets"], what="preset saved")
        self.assertIn('filter = "*.mp3;>6mb"', (home / "acreage" / "config.toml").read_text())
