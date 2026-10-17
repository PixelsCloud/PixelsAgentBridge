// Generated BGRA surfaces exercise the production NVENC padding path. Run in
// an owned output directory; writes only generated H.264 streams, no capture.
#include "../../crates/desktop-media/native/windows/software_codec.h"

// Fallback is deliberately unavailable in this GPU-only diagnostic. An NVENC
// failure must fail the check instead of silently passing via the CPU codec.
extern "C" void* pab_soft_encoder_new(uint32_t, uint32_t, uint16_t) { return nullptr; }
extern "C" void pab_soft_encoder_drop(void*) { std::terminate(); }
extern "C" bool pab_soft_encode(void*, const uint8_t*, size_t, size_t, PabCodecBytes*) { return false; }

int main() {
    try {
        MediaRuntime runtime;
        ComPtr<ID3D11Device> device;
        check(D3D11CreateDevice(nullptr, D3D_DRIVER_TYPE_HARDWARE, nullptr,
            D3D11_CREATE_DEVICE_BGRA_SUPPORT | D3D11_CREATE_DEVICE_VIDEO_SUPPORT,
            nullptr, 0, D3D11_SDK_VERSION, &device, nullptr, nullptr), "Create test device");
        for (auto size : {std::pair<UINT, UINT>{1298, 754}, {1297, 754}, {1298, 753}, {1297, 753}}) {
            const auto [width, height] = size;
            std::vector<uint8_t> pixels(size_t(width) * height * 4, 255);
            for (UINT y = 0; y < height; ++y)
                for (UINT x = 0; x < width; ++x) {
                    const uint8_t value = (!x || !y || x == width - 1 || y == height - 1) ? 255 : 64;
                    for (unsigned c = 0; c < 3; ++c) pixels[(size_t(y) * width + x) * 4 + c] = value;
                }
            D3D11_TEXTURE2D_DESC description{};
            description.Width = width; description.Height = height;
            description.MipLevels = description.ArraySize = description.SampleDesc.Count = 1;
            description.Format = DXGI_FORMAT_B8G8R8A8_UNORM; description.Usage = D3D11_USAGE_DEFAULT;
            D3D11_SUBRESOURCE_DATA initial{pixels.data(), width * 4, 0};
            ComPtr<ID3D11Texture2D> texture;
            check(device->CreateTexture2D(&description, &initial, &texture), "Create generated BGRA");
            CaptureEncoder encoder(device.Get(), width, height, 15, false);
            const auto name = "gpu-" + std::to_string(width) + "x" + std::to_string(height) + ".h264";
            std::ofstream stream(name, std::ios::binary | std::ios::trunc);
            if (!stream) throw std::runtime_error("Create generated stream");
            for (uint64_t index = 0; index < 3; ++index) {
                std::vector<uint8_t> packet;
                encoder.encode(texture.Get(), index, packet);
                if (encoder.software() || packet.empty()) throw std::runtime_error("GPU path unavailable");
                stream.write(reinterpret_cast<const char*>(packet.data()), packet.size());
            }
            if (!stream) throw std::runtime_error("Write generated stream");
            std::cout << "{\"width\":" << width << ",\"height\":" << height
                << ",\"hardware_frames\":3,\"file\":\"" << name << "\"}\n";
        }
        return 0;
    } catch (const std::exception& error) { std::cerr << error.what() << '\n'; return 1; }
}
