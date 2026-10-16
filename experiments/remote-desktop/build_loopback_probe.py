"""Build the isolated NVENC -> D3D11 decoder -> native window experiment."""
from build_dxgi_probe import build_probe
from build_nvenc_probe import pinned_include


if __name__ == "__main__":
    build_probe("loopback_probe.cpp", "loopback", pinned_include(),
                ("mfplat.lib", "mfuuid.lib", "wmcodecdspuuid.lib", "ole32.lib", "psapi.lib"))
