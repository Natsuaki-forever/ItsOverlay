"""A packaged internal worker must exit on startup failure without any GUI."""
import argparse
import ctypes as C
from ctypes import wintypes as W
import json
import os
from pathlib import Path
import subprocess
import time
import uuid

parser=argparse.ArgumentParser();parser.add_argument('--exe',type=Path,required=True)
parser.add_argument('--output',type=Path,required=True);args=parser.parse_args()
out=args.output.resolve();out.mkdir(parents=True,exist_ok=False)
u=C.WinDLL('user32');callback_type=C.WINFUNCTYPE(W.BOOL,W.HWND,W.LPARAM)
u.EnumWindows.argtypes=[callback_type,W.LPARAM];u.IsWindowVisible.argtypes=[W.HWND]
u.GetWindowThreadProcessId.argtypes=[W.HWND,C.POINTER(W.DWORD)]
environment={k:v for k,v in os.environ.items() if not k.upper().startswith(('PYTHON','PYI_','_PYI_'))}
environment['PATH']=str(Path(os.environ['SystemRoot'])/'System32')
report=dict(passed=False,checks=[])
try:
    for name,parent in [('missing_pipe',os.getpid()),('invalid_parent',0)]:
        process=subprocess.Popen([str(args.exe.resolve()),'--pipe',r'\\.\pipe\overlay-missing-'+uuid.uuid4().hex,
            '--session','fixture-session','--parent-pid',str(parent)],env=environment,cwd=out,creationflags=0x08000000)
        visible=[];started=time.monotonic()
        try:
            @callback_type
            def visit(hwnd,unused):
                pid=W.DWORD();u.GetWindowThreadProcessId(hwnd,C.byref(pid))
                if pid.value==process.pid and u.IsWindowVisible(hwnd):visible.append(int(hwnd))
                return True
            while process.poll() is None and time.monotonic()-started<8:
                u.EnumWindows(visit,0);time.sleep(.03)
            assert process.poll()==2,dict(case=name,exit=process.poll(),windows=visible)
            assert not visible,dict(case=name,windows=visible)
            report['checks'].append(dict(case=name,exit_code=process.returncode,visible_windows=0))
        finally:
            if process.poll() is None:process.terminate();process.wait(timeout=5)
    report['passed']=True
finally:
    (out/'result.json').write_text(json.dumps(report,indent=2),encoding='utf-8')
