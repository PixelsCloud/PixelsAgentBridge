// Isolated hardware-encoder qualification. Generated pixels only: no desktop,
// window, account, network or installed application is accessed. The bounded
// archive uses the same format as remote_desktop_replay.py.
#include "../crates/desktop-media/native/windows/mf_decoder.h"
#include "remote_desktop_pattern_proof.h"
#include <d3d11_1.h>
#include <filesystem>
#include <algorithm>
#include <thread>

#include "../crates/desktop-media/native/windows/mf_encoder.h"

class ProofArchive {
    HANDLE file_ = INVALID_HANDLE_VALUE;
public:
    explicit ProofArchive(const std::filesystem::path& path) {
        bool smoke = false;
        for (const auto& component : path) if (component == L"smoke") smoke = true;
        if (!path.is_absolute() || !smoke)
            throw std::runtime_error("An absolute smoke archive path is required");
        file_ = CreateFileW(path.c_str(), GENERIC_WRITE, FILE_SHARE_READ, nullptr, CREATE_NEW,
            FILE_ATTRIBUTE_NORMAL, nullptr);
        if (file_ == INVALID_HANDLE_VALUE) throw std::runtime_error("Cannot create new smoke archive");
    }
    ~ProofArchive() { if (file_ != INVALID_HANDLE_VALUE) CloseHandle(file_); }
    void write(const char* data, DWORD size) {
        DWORD written = 0;
        if (!WriteFile(file_, data, size, &written, nullptr) || written != size)
            throw std::runtime_error("Smoke archive write failed");
    }
    ProofArchive(const ProofArchive&) = delete;
};

int wmain(int argc, wchar_t** argv) {
    try {
        if (argc != 2) throw std::runtime_error("Usage: windows_mf_encoder_probe ARCHIVE.pab (new file only)");
        const std::filesystem::path path(argv[1]);
        ComPtr<IDXGIFactory1> factory;
        check(CreateDXGIFactory1(IID_PPV_ARGS(&factory)), "GPU inventory");
        ComPtr<IDXGIAdapter1> intel;
        for (UINT i = 0;; ++i) {
            ComPtr<IDXGIAdapter1> candidate;
            const auto hr = factory->EnumAdapters1(i, &candidate);
            if (hr == DXGI_ERROR_NOT_FOUND) break;
            check(hr, "GPU enumeration"); DXGI_ADAPTER_DESC1 desc{};
            check(candidate->GetDesc1(&desc), "GPU description");
            if (desc.VendorId == 0x8086) { intel = candidate; break; }
        }
        if (!intel) throw std::runtime_error("Intel adapter not present");
        ComPtr<ID3D11Device> device; ComPtr<ID3D11DeviceContext> context;
        check(D3D11CreateDevice(intel.Get(), D3D_DRIVER_TYPE_UNKNOWN, nullptr,
            D3D11_CREATE_DEVICE_BGRA_SUPPORT | D3D11_CREATE_DEVICE_VIDEO_SUPPORT, nullptr, 0,
            D3D11_SDK_VERSION, &device, nullptr, &context), "Intel D3D11 device");
        constexpr UINT width = 1920, height = 1080, fps = 30, count = 240;
        IntelHardwareEncoder encoder(device.Get(), width, height, fps);
        D3D11_TEXTURE2D_DESC texture{};
        texture.Width = width; texture.Height = height;
        texture.MipLevels = texture.ArraySize = texture.SampleDesc.Count = 1;
        texture.Format = DXGI_FORMAT_B8G8R8A8_UNORM; texture.Usage = D3D11_USAGE_DEFAULT;
        texture.BindFlags = D3D11_BIND_RENDER_TARGET;
        ComPtr<ID3D11Texture2D> rgb;
        check(device->CreateTexture2D(&texture, nullptr, &rgb), "Generated RGB texture");
        ProofArchive archive(path);
        archive.write("PABPROOF1", 9);
        for (const auto value : {width, height, fps}) archive.write(reinterpret_cast<const char*>(&value), 4);
        std::vector<uint8_t> pixels(size_t(width) * height * 4);
        std::vector<double> milliseconds; uint64_t total_bytes = 21;
        for (uint64_t i = 0; i < count; ++i) {
            const UINT pattern = 140 + UINT(i);
            for (UINT y = 0; y < height; ++y) for (UINT x = 0; x < width; ++x) {
                const UINT cx = x * 32 / width, cy = y * 18 / height;
                UINT r, g, b;
                if (cy == 0 || cy == 17) r = g = b = (((pattern >> (cx % 16)) & 1) ^ (cx >= 16)) ? 224 : 32;
                else { r = 32 + (pattern * 3 + cx * 29 + cy * 17) % 192;
                    g = 32 + (pattern * 5 + cx * 11 + cy * 31) % 192;
                    b = 32 + (pattern * 7 + cx * 37 + cy * 13) % 192; }
                auto* p = pixels.data() + (size_t(y) * width + x) * 4;
                p[0] = uint8_t(b); p[1] = uint8_t(g); p[2] = uint8_t(r); p[3] = 255;
            }
            context->UpdateSubresource(rgb.Get(), 0, nullptr, pixels.data(), width * 4, 0);
            const auto start = Clock::now(); auto packet = encoder.encode(rgb.Get(), i);
            milliseconds.push_back(std::chrono::duration<double, std::milli>(Clock::now() - start).count());
            const auto size = uint32_t(packet.size());
            total_bytes += size + 12;
            if (total_bytes > 64 * 1024 * 1024) throw std::runtime_error("Smoke archive size limit");
            archive.write(reinterpret_cast<const char*>(&size), 4);
            archive.write(reinterpret_cast<const char*>(&i), 8);
            archive.write(reinterpret_cast<const char*>(packet.data()), size);
        }
        std::sort(milliseconds.begin(), milliseconds.end());
        std::cout << "{\"encoder\":\"intel_mf_d3d11\",\"desktop_access\":false,\"frames\":" << count
            << ",\"median_ms\":" << milliseconds[count/2] << ",\"p95_ms\":" << milliseconds[count*95/100]
            << ",\"max_ms\":" << milliseconds.back() << ",\"archive_bytes\":" << total_bytes << "}\n";
        return 0;
    } catch (const std::exception& error) { std::cerr << error.what() << '\n'; return 1; }
}
