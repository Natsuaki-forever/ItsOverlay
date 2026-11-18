"""Verify selection and persistence through the real Slint editor and native engine."""

import argparse
import ctypes as C
import json
from pathlib import Path
import subprocess
import sys
import time

ROOT = Path(__file__).resolve().parents[2]
sys.path.insert(0, str(ROOT))
from overlay import native
from PySide6.QtCore import QPoint
from PySide6.QtTest import QTest
from PySide6.QtWidgets import QApplication, QWidget

native.enable_dpi()
u, w = native.u, native.W
native.bind(u, "GetWindow", w.HWND, w.HWND, w.UINT)
native.bind(u, "PostMessageW", w.BOOL, w.HWND, w.UINT, w.WPARAM, w.LPARAM)
app = QApplication([])
target = QWidget()
target.setWindowTitle("ItsOverlay Slint fixture")
target.resize(800, 500)
target.show()

parser = argparse.ArgumentParser()
parser.add_argument("program", type=lambda path: Path(path).resolve())
parser.add_argument("output", type=lambda path: Path(path).resolve())
parser.add_argument("--cross-screen", action="store_true")
args = parser.parse_args()
program, output = args.program, args.output
output.mkdir(parents=True, exist_ok=False)
config = output / "config.json"
ready = output / "ready.json"
samples = output / "samples.jsonl"
first_id = "1" * 32
second_id = "2" * 32
config.write_text(json.dumps({"version": 1, "regions": [
    {"id": first_id, "text": "第一条测试"},
    {"id": second_id, "text": "第二条测试", "x": 300},
]}), encoding="utf-8")

process = subprocess.Popen([
    str(program), "--check", "--check-seconds", "24", "--editor-host",
    "--target-hwnd", str(int(target.winId())), "--config", str(config),
    "--ready-file", str(ready), "--sample-file", str(samples),
])

def wait(predicate, seconds=7):
    until = time.monotonic() + seconds
    while time.monotonic() < until:
        result = predicate()
        if result:
            return result
        assert process.poll() is None, "Engine exited before the check completed"
        QTest.qWait(30)
    raise AssertionError("Timed out waiting for " + predicate.__name__)

def find_editor():
    windows = []
    @native.ENUM
    def visit(hwnd, unused):
        pid = w.DWORD()
        u.GetWindowThreadProcessId(hwnd, C.byref(pid))
        if pid.value == marker["editor_pid"] and u.GetWindow(hwnd, 4) == marker["host_hwnd"] and u.IsWindowVisible(hwnd):
            windows.append(hwnd)
        return True
    u.EnumWindows(visit, 0)
    return next(iter(windows), None)

def clicked_row():
    if samples.is_file():
        for line in reversed(samples.read_text(encoding="utf-8").splitlines()):
            try:
                return json.loads(line).get("selected_id") == second_id
            except json.JSONDecodeError:
                continue
    return False

def click(x, y):
    coords = (round(y * scale) << 16) | round(x * scale)
    for message, flags in ((0x200, 0), (0x201, 1), (0x202, 0)):
        u.PostMessageW(panel, message, flags, coords)

marker = {}
try:
    wait(ready.is_file)
    marker = json.loads(ready.read_text())
    def current_editor():
        if samples.exists():
            for line in reversed(samples.read_text(encoding="utf-8").splitlines()):
                try:
                    marker["editor_pid"] = json.loads(line).get("editor_pid", marker["editor_pid"])
                    break
                except json.JSONDecodeError:
                    continue
        return find_editor()
    panel = wait(current_editor)
    screen_result = {}
    if args.cross_screen:
        hwnd = int(target.winId())
        original = native.monitor_device(hwnd)
        original_box = native.monitor_box(hwnd)[1]
        other = next(screen for screen in app.screens() if screen != target.screen())
        target.windowHandle().setScreen(other)
        target.move(other.availableGeometry().topLeft() + QPoint(30, 30))
        wait(lambda: native.monitor_device(hwnd) != original)
        destination = native.monitor_device(hwnd)
        destination_box = native.monitor_box(hwnd)[1]

        def move_panel(device, box):
            left, top, _, _ = box
            assert u.SetWindowPos(panel, None, left + 30, top + 30, 0, 0, 0x15)
            wait(lambda: native.monitor_device(panel) == device)
            QTest.qWait(200)
            assert find_editor() == panel
            return u.GetDpiForWindow(panel)

        screen_result["cross_screen_dpi"] = [
            move_panel(original, original_box),
            move_panel(destination, destination_box),
            move_panel(original, original_box),
        ]
    scale = u.GetDpiForWindow(panel) / 96
    QTest.qWait(320)
    click(115, 228)
    wait(clicked_row)
    QTest.qWait(200)
    click(350, 240)
    QTest.qWait(80)
    for character in "已保存":
        u.PostMessageW(panel, 0x100, 0xe7, 1)  # VK_PACKET precedes Unicode input
        u.PostMessageW(panel, 0x102, ord(character), 1)
        u.PostMessageW(panel, 0x101, 0xe7, 0xc0000001)
    QTest.qWait(120)
    u.PostMessageW(panel, 0x0010, 0, 0)
    wait(lambda: not u.IsWindow(panel))
    saved = json.loads(config.read_text(encoding="utf-8-sig"))["regions"]
    assert saved[0]["text"] == "第一条测试"
    assert "已保存" in saved[1]["text"], saved
    (output / "result.json").write_text(json.dumps({
        "selected_second_region": True, "saved_unicode_text": True, **screen_result
    }))
finally:
    if process.poll() is None:
        if marker:
            u.PostMessageW(marker["host_hwnd"], 0x8000 + 55, 0, 0)
        try:
            process.wait(timeout=4)
        except subprocess.TimeoutExpired:
            process.terminate()
            process.wait(timeout=4)
    target.close()
