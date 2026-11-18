"""Recover a fully off-client visual cover through the packaged Slint editor."""

import argparse
import ctypes as C
import hashlib
import json
from pathlib import Path
import struct
import subprocess
import sys
import time

ROOT = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(ROOT))
from overlay import native
from PySide6.QtGui import QColor, QImage, QPalette
from PySide6.QtTest import QTest
from PySide6.QtWidgets import QApplication, QLabel, QWidget

native.enable_dpi()
u, w = native.u, native.W
native.bind(u, "GetWindow", w.HWND, w.HWND, w.UINT)
native.bind(u, "PostMessageW", w.BOOL, w.HWND, w.UINT, w.WPARAM, w.LPARAM)
app = QApplication([])
target = QWidget()
target.setWindowTitle("SlintPickerFixture A")
target.resize(800, 500)
palette=target.palette()
palette.setColor(QPalette.ColorRole.Window,QColor("#486684"))
target.setPalette(palette)
target.setAutoFillBackground(True)
target.move(40,70)
fixture_text = QLabel("SLINT TEST Z9", target)
fixture_text.setStyleSheet("color:white; background:#486684; font-size:20px")
fixture_text.setGeometry(105, 108, 190, 42)
target.show()

parser = argparse.ArgumentParser()
parser.add_argument("program", type=lambda path: Path(path).resolve())
parser.add_argument("output", type=lambda path: Path(path).resolve())
parser.add_argument("--deps", type=Path, required=True)
args = parser.parse_args()
program, output = args.program, args.output
output.mkdir(parents=True, exist_ok=False)
config = output / "config.json"
ready = output / "ready.json"
samples = output / "samples.jsonl"
first_id = "1" * 32
second_id = "2" * 32
sys.path.insert(0, str(args.deps.resolve(strict=True)))
import numpy as np
from visual_tracking.vision import learn

QTest.qWait(100)
dpi = u.GetDpiForWindow(int(target.winId()))
scale = dpi / 96
reference = [101, 104, 181, 48]
x, y, width, height = (round(v * scale) for v in reference)
image = target.grab().toImage().convertToFormat(QImage.Format.Format_Grayscale8)
pixels = np.frombuffer(image.constBits(), np.uint8).reshape(image.height(), image.bytesPerLine())[:, :image.width()]
template = learn(pixels[y:y+height, x:x+width], [0, 0, width, height], "features")
asset = b"VTP1" + struct.pack("<II", template.shape[1], template.shape[0]) + template.tobytes()
asset_id = "a" * 32
(output / "visual-templates").mkdir()
(output / "visual-templates" / f"{asset_id}.vt1").write_bytes(asset)
binding = {
    "version": 1, "calibration_id": "b" * 32, "reference_rect_dip": reference,
    "calibration_dpi": dpi, "template_id": asset_id, "template_sha256": hashlib.sha256(asset).hexdigest(),
    "edge_limit_dip": 32, "size_ratio": 1.25, "tracking_kind": "features", "search_limit_dip": 500,
    "relation": {"mode": "position_only", "anchor_x": "left", "anchor_y": "top",
                 "offset_dip": [19, 256], "fixed_size_dip": [160, 40],
                 "margins_dip": [0, 0, 0, 0], "padding_physical": 0},
}
config.write_text(json.dumps({"version": 2, "visual_check_interval_seconds": 15, "regions": [
    {"id": first_id, "text": "第一条测试", "x": 120, "y": 360,
     "position_mode": "visual", "visual_binding": binding},
    {"id": second_id, "text": "第二条测试", "x": 300},
]}), encoding="utf-8")

process = subprocess.Popen([
    str(program), "--check", "--check-seconds", "80", "--editor-host",
    "--shell-test", "--simulate-focus", "--target-hwnd", str(int(target.winId())), "--config", str(config),
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
        if pid.value == marker.get("editor_pid", 0) and u.GetWindow(hwnd, 4) == marker["host_hwnd"] and u.IsWindowVisible(hwnd):
            windows.append(hwnd)
        return True
    u.EnumWindows(visit, 0)
    return next(iter(windows), None)

def click(x, y):
    coords = (round(y * scale) << 16) | round(x * scale)
    for message, flags in ((0x200, 0), (0x201, 1), (0x202, 0)):
        u.PostMessageW(panel, message, flags, coords)

marker = {}
try:
    wait(ready.is_file)
    marker = json.loads(ready.read_text())
    def live_editor():
        if not samples.is_file(): return False
        state = json.loads(samples.read_text(encoding="utf-8").splitlines()[-1])
        marker["editor_pid"] = state.get("editor_pid", 0)
        return find_editor()
    panel = wait(live_editor)
    scale = u.GetDpiForWindow(panel) / 96
    QTest.qWait(320)
    native.bind(u, "FindWindowExW", w.HWND, w.HWND, w.HWND, w.LPCWSTR, w.LPCWSTR)
    native.bind(u, "SendMessageW", C.c_ssize_t, w.HWND, w.UINT, w.WPARAM, w.LPARAM)
    def snapshot():
        if not samples.is_file():
            return {}
        for line in reversed(samples.read_text(encoding="utf-8").splitlines()):
            try: return json.loads(line)
            except ValueError: continue
        return {}
    def find_canvas():
        candidate = None
        while True:
            candidate = u.FindWindowExW(None, candidate, "OverlayNativeSurface", "Overlay editor canvas")
            if not candidate or native.pid(candidate) == process.pid and u.IsWindowVisible(candidate):
                return candidate
    def bring(hwnd):
        native.bind(u,"SetForegroundWindow",w.BOOL,w.HWND)
        native.bind(u, "AttachThreadInput", w.BOOL, w.DWORD, w.DWORD, w.BOOL)
        native.bind(u, "BringWindowToTop", w.BOOL, w.HWND)
        caller=native.k.GetCurrentThreadId()
        foreground=u.GetForegroundWindow()
        owner=u.GetWindowThreadProcessId(foreground, None) if foreground else 0
        attached=owner and owner!=caller and u.AttachThreadInput(caller,owner,True)
        try:
            u.BringWindowToTop(hwnd)
            u.SetForegroundWindow(hwnd)
        finally:
            if attached: u.AttachThreadInput(caller,owner,False)
        QTest.qWait(150)
    def saved_regions():
        try: return json.loads(config.read_text(encoding="utf-8-sig"))["regions"]
        except (OSError, ValueError): return []
    def cover():
        return snapshot().get("effective_rects",{}).get(first_id)
    def tracking_reason():
        return snapshot().get("tracking_states",{}).get(first_id,{}).get("reason")
    def saved_binding():
        return next(row for row in saved_regions() if row["id"]==first_id)["visual_binding"]
    initial_binding=saved_binding()
    assert u.PostMessageW(panel,0x10,0,0)
    wait(lambda:not u.IsWindow(panel))
    bring(int(target.winId()))
    wait(lambda:cover() is not None)
    target.resize(800,260);target.update()
    bring(int(target.winId()))
    wait(lambda:cover() is None and tracking_reason()=="visual_cover_outside_client")
    def audits():
        return snapshot().get("visual_regions",{}).get(first_id,{}).get("audits",0)
    QTest.qWait(1200)
    check_count=audits()
    wait(lambda: audits() > check_count,20)
    assert cover() is None and saved_binding()==initial_binding
    assert u.PostMessageW(marker["host_hwnd"],0x8000+56,0,0)
    panel=wait(live_editor)
    scale=u.GetDpiForWindow(panel)/96
    QTest.qWait(260)
    click(347,130)
    QTest.qWait(320)
    click(352,229)
    wait(lambda:not u.IsWindowVisible(panel))
    drag_canvas=wait(find_canvas)
    bring(drag_canvas)
    drag_scale=u.GetDpiForWindow(drag_canvas)/96
    _, _, _, canvas_height=native.client_box(drag_canvas)
    cover_height=initial_binding["relation"]["fixed_size_dip"][1]
    drag_y=min(360, canvas_height / drag_scale - cover_height) + cover_height / 2
    def point(cx,cy): return (round(cy*drag_scale)<<16)|round(cx*drag_scale)
    for message, flags, xy in ((0x201,1,(135,drag_y)),(0x200,1,(135,drag_y-45)),(0x202,0,(135,drag_y-45))):
        u.SendMessageW(drag_canvas,message,flags,point(*xy))
    wait(lambda:u.IsWindowVisible(panel))
    QTest.qWait(300)
    assert saved_binding()==initial_binding
    click(352,229)
    wait(lambda:saved_binding()["relation"]["offset_dip"]!=initial_binding["relation"]["offset_dip"])
    assert saved_binding()["calibration_id"]==initial_binding["calibration_id"]
    assert saved_binding()["template_id"]==initial_binding["template_id"]
    assert u.PostMessageW(panel,0x10,0,0)
    wait(lambda:not u.IsWindow(panel))
    bring(int(target.winId()))
    wait(lambda:cover() is not None and 155<cover()["y"]<204)
    assert u.PostMessageW(marker["host_hwnd"],0x10,0,0)
    assert process.wait(timeout=7)==0
    (output / "result.json").write_text(json.dumps({
        "off_client_cover_hidden":True,
        "periodic_retry_did_not_change_saved_geometry":True,"cover_repositioned_in_windowed_mode":True,"reference_and_template_preserved":True}))

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
