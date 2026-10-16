"""Build the isolated compressed-pipe media adapter, one MSVC job."""
from build_dxgi_probe import build_probe
from build_nvenc_probe import pinned_include

if __name__ == "__main__":
    build_probe("stream_media_probe.cpp", "stream-media", pinned_include(),
                ("mfplat.lib", "mfuuid.lib", "wmcodecdspuuid.lib", "ole32.lib", "shcore.lib"))
