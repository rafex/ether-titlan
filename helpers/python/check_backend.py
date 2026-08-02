#!/usr/bin/env python3
import compileall
import pathlib
import sys


ROOT = pathlib.Path(__file__).resolve().parents[2]
if not compileall.compile_dir(ROOT / "backend", quiet=1, force=True):
    sys.exit("Backend Python syntax check failed")
print("Backend Python syntax check passed")
