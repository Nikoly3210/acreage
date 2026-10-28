"""e2e tests for the fixes from replica/fixes.md: btrfs subvolumes (#1), scanning into another
filesystem (#2), keyboard completeness (#3) and robustness mid-scan (#6).

Mount tables are faked with CANOPY_MOUNTINFO, so no root or real btrfs is needed.
Mid-scan cases use CANOPY_TEST_PAUSE_AFTER to hold the scan open.
"""

import os
import shutil
import time
from pathlib import Path

from harness import CanopyCase, apparent_total, write
from test_flows import MB


def mountinfo(tmp: Path, lines):
    p = tmp / "mountinfo"
    p.write_text("\n".join(lines) + "\n")
    return {"CANOPY_MOUNTINFO": str(p)}


class Fix1Btrfs(CanopyCase):
    def test_FX1_H1_same_fs_subvolumes_entered_duplicates_not(self):
        fx = self.tmp / "fx"
        write(fx / "top.bin", 1 * MB)
        write(fx / "home" / "a.bin", 10 * MB)  # subvolume of the same btrfs: counted
        write(fx / "again" / "a.bin", 10 * MB)  # the same subvolume mounted again: not counted
        write(fx / "other" / "b.bin", 20 * MB)  # another filesystem: not counted
        env = mountinfo(self.tmp, [
            f"25 1 0:30 /@ {fx} rw - btrfs /dev/fake rw,subvol=/@",
            f"26 25 0:31 /@home {fx}/home rw - btrfs /dev/fake rw,subvol=/@home",
            f"27 25 0:31 /@home {fx}/again rw - btrfs /dev/fake rw,subvol=/@home",
            f"28 25 8:17 / {fx}/other rw - ext4 /dev/other rw",
        ])
        app = self.launch(fx, "--apparent-size", env=env)
        s = app.wait_done()
        kinds = dict(k.split("=") for k in s["kinds"])
        self.assertEqual(kinds["home"], "Dir")
        self.assertEqual(kinds["again"], "MountPoint")
        self.assertEqual(kinds["other"], "MountPoint")
        self.assertEqual(s["items"], 2)  # top.bin + home/a.bin


class Fix2EnterMount(CanopyCase):
    def test_FX2_H1_scan_into_other_filesystem(self):
        fx = self.tmp / "fx"
        write(fx / "big.bin", 50 * MB)
        for i in range(4):
            write(fx / "usb" / f"photo{i}.jpg", 8 * MB)
        env = mountinfo(self.tmp, [
            f"25 1 8:1 / {fx} rw - ext4 /dev/sda1 rw",
            f"26 25 8:33 / {fx}/usb rw - vfat /dev/sdc1 rw",
        ])
        app = self.launch(fx, "--apparent-size", env=env)
        s = app.wait_done()
        self.assertIn("usb=MountPoint", s["kinds"])
        self.assertEqual(s["items"], 1)
        app.shot("FX2-before")
        app.key("Down")  # select the first block…
        for _ in range(3):
            if app.state()["selected"] == str(fx / "usb"):
                break
            app.key("Right")
        app.wait(lambda s: s["selected"] == str(fx / "usb"), what="select the mount block")
        app.key("Return")
        s = app.wait(lambda s: s["items"] == 5 and s["subscans"] == 0, what="mount scanned")
        self.assertIn("usb=Dir", s["kinds"])
        # A full rescan keeps it entered.
        app.key("ctrl+r")
        s = app.wait(lambda s: s["scan"] == "done" and s["items"] == 5 and s["subscans"] == 0, what="re-entered after rescan")

    def test_FX2_E1_system_filesystem_is_left_out(self):
        fx = self.tmp / "fx"
        write(fx / "x.bin", MB)
        (fx / "run").mkdir()
        env = mountinfo(self.tmp, [
            f"25 1 8:1 / {fx} rw - ext4 /dev/sda1 rw",
            f"26 25 0:22 / {fx}/run rw - tmpfs tmpfs rw",
        ])
        app = self.launch(fx, env=env)
        app.wait_done()
        # A system filesystem holds nothing on disk: it doesn't get a block at all.
        self.assertFalse(any(k.startswith("run=") for k in app.state()["kinds"]))


class Fix3Keyboard(CanopyCase):
    def setUp(self):
        super().setUp()
        self.fx = self.tmp / "fx"
        write(self.fx / "Videos" / "a.mkv", 90 * MB)
        write(self.fx / "Downloads" / "b.iso", 40 * MB)

    def test_FX3_H1_shift_f10_menu_from_keyboard(self):
        app = self.launch(self.fx, "--apparent-size")
        app.wait_done()
        app.key("Down")
        sel = app.wait(lambda s: s["selected"] is not None, what="selection")["selected"]
        app.key("shift+F10")
        app.wait(lambda s: s["kb_menu"], what="keyboard menu")
        app.shot("FX3-kb-menu")
        app.key("Escape")
        app.wait(lambda s: not s["kb_menu"], what="Esc closes it")
        app.key("shift+F10")
        app.wait(lambda s: s["kb_menu"], what="menu again")
        app.key("Return")  # first item is focused: Open
        deadline = time.time() + 5
        while time.time() < deadline and not app.record.exists():
            time.sleep(0.1)
        self.assertIn(sel, app.record.read_text())
        app.wait(lambda s: not s["kb_menu"], what="closed after acting")

    def test_FX3_H2_detail_keys_and_shortcut_list(self):
        home = self.tmp / "cfg"
        app = self.launch(self.fx, config_home=home)
        app.wait_done()
        app.key("minus")
        app.wait(lambda s: s["detail"] == "coarse", what="less detail")
        app.key("plus", "plus")
        app.wait(lambda s: s["detail"] == "fine", what="more detail")
        self.assertIn('detail = "fine"', (home / "canopy" / "config.toml").read_text())
        app.key("question")
        app.wait(lambda s: s["keys_window"], what="shortcut list")
        app.shot("FX3-keys")
        app.key("Escape")
        app.wait(lambda s: not s["keys_window"], what="closed")

    def test_FX3_H3_ctrl_q_quits(self):
        app = self.launch(self.fx)
        app.wait_done()
        app.key("ctrl+q")
        app.proc.wait(5)
        self.assertEqual(app.proc.returncode, 0)


class Fix6MidScan(CanopyCase):
    def setUp(self):
        super().setUp()
        self.fx = self.tmp / "fx"
        for d in range(30):
            for f in range(5):
                write(self.fx / f"d{d:02}" / "sub" / f"f{f}.bin", (d + 1) * 100_000)

    def paused_app(self):
        app = self.launch(self.fx, "--apparent-size", env={"CANOPY_TEST_PAUSE_AFTER": "2"})
        app.wait(lambda s: s.get("scan") == "paused", what="paused mid-scan")
        return app

    def test_FX6_E1_folders_deleted_mid_scan_just_vanish(self):
        app = self.paused_app()
        for d in range(10, 30):
            shutil.rmtree(self.fx / f"d{d:02}")
        app.key("p")
        s = app.wait(lambda s: s["scan"] == "done", what="scan finished")
        self.assertEqual(s["errors"], 0, "a vanished folder is not an unreadable one")
        self.assertEqual(s["total_size"], apparent_total(self.fx))

    def test_FX6_E2_trash_mid_scan(self):
        app = self.paused_app()
        target = self.fx / "d29"
        app.wait(lambda s: any(r.startswith(f"{target}|") for r in s["rects"]), what="block for d29")
        app.click_block(target, at="header")
        app.key("Delete")
        app.wait(lambda s: s["confirm_open"], what="confirm")
        app.key("Return")
        app.wait(lambda s: not s["confirm_open"], what="trashed")
        self.assertFalse(target.exists())
        app.key("p")
        s = app.wait(lambda s: s["scan"] == "done", what="scan finished")
        self.assertEqual(s["total_size"], apparent_total(self.fx))

    def test_FX6_E3_quit_mid_scan(self):
        app = self.paused_app()
        app.key("ctrl+q")
        app.proc.wait(5)
        self.assertEqual(app.proc.returncode, 0)
