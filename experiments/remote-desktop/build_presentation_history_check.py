"""Build the bounded presentation-observation regression with one compiler."""
from build_dxgi_probe import build_probe

if __name__ == "__main__":
    build_probe("verify_presentation_history.cpp", "presentation-history")
