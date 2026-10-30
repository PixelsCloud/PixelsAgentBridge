"""Shared fast Release configuration for smoke, standalone native tests and CI."""
import os
from pathlib import Path
import subprocess
import sys

from scripts.build_profile import apply_release_preset
from smoke.native_env import prepare

ROOT = Path(__file__).resolve().parents[1]


def environment(source=None):
    env = prepare(os.environ if source is None else source)
    apply_release_preset(env, "fast")
    env.update(CARGO_BUILD_JOBS="1", CMAKE_BUILD_PARALLEL_LEVEL="1",
               CMAKE_BUILD_PARALLEL="1", RAYON_NUM_THREADS="1")
    return env


def test_command(arguments):
    return ["cargo", "test", "--release", "--locked", "-j", "1", *arguments]


if __name__ == "__main__":
    arguments = sys.argv[1:]
    build = arguments[:1] == ['--build']
    command = test_command(arguments[1:] if build else arguments)
    if build:
        command[1] = 'build'
    raise SystemExit(subprocess.call(command, cwd=ROOT, env=environment()))
