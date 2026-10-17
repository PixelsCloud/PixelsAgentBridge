"""Build the bounded cursor acceptance window; do not run it unattended on a user's desktop."""
from build_dxgi_probe import build_probe

if __name__ == "__main__":
    build_probe("windows_cursor_fixture.cpp", "windows-cursor", libraries=("gdi32.lib",))
