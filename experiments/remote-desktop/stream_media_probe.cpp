// Isolated media child for the reliable-stream experiment. Only compressed
// H.264 crosses anonymous pipes; no raw desktop pixels are persisted/read back.
#include "nvenc_encoder.h"
#include "pattern_window.h"
#include "d3d_presenter.h"
#include <d3d11_1.h>
#include <shellscalingapi.h>
#include <algorithm>
#include <memory>

static constexpr uint32_t magic = 0x31424150; // PAB1, little endian local IPC
static constexpr uint32_t max_packet = 16 * 1024 * 1024;
static const auto deadline = Clock::now() + std::chrono::seconds(90);

static void read_bytes(void* data, uint32_t size, NativePresenter* viewer = nullptr) {
    auto* cursor = static_cast<uint8_t*>(data);
    while (size) {
        if (Clock::now() >= deadline) throw std::runtime_error("Media experiment deadline");
        if (viewer) viewer->pump();
        DWORD available = 0;
        if (!PeekNamedPipe(GetStdHandle(STD_INPUT_HANDLE), nullptr, 0, nullptr, &available, nullptr))
            throw std::runtime_error("Parent pipe ended");
        if (!available) { Sleep(2); continue; }
        DWORD read = 0;
        if (!ReadFile(GetStdHandle(STD_INPUT_HANDLE), cursor, (std::min)(size, static_cast<uint32_t>(available)), &read, nullptr) || !read)
            throw std::runtime_error("Parent pipe read failed");
        cursor += read; size -= read;
    }
}
static void write_bytes(const void* data, uint32_t size) {
    const auto* cursor = static_cast<const uint8_t*>(data);
    while (size) {
        DWORD written = 0;
        if (!WriteFile(GetStdHandle(STD_OUTPUT_HANDLE), cursor, size, &written, nullptr) || !written)
            throw std::runtime_error("Parent pipe write failed");
        cursor += written; size -= written;
    }
}
template<class T> static T read_value(NativePresenter* viewer = nullptr) {
    T value{}; read_bytes(&value, sizeof(value), viewer); return value;
}
template<class T> static void write_value(T value) { write_bytes(&value, sizeof(value)); }
static unsigned number(const char* value, unsigned maximum) {
    const std::string text(value);
    size_t end = 0;
    if (text.empty() || text[0] == '-') throw std::runtime_error("Invalid number");
    const auto n = std::stoul(text, &end);
    if (end != text.size() || n == 0 || n > maximum) throw std::runtime_error("Number outside bounds");
    return static_cast<unsigned>(n);
}
static std::string screen_id(const Display& d) {
    std::string name;
    for (const wchar_t* c = d.screen.DeviceName; *c; ++c) name += static_cast<char>(*c);
    return std::to_string(d.gpu.AdapterLuid.HighPart) + ":" + std::to_string(d.gpu.AdapterLuid.LowPart) + ":" + name;
}
static void inventory(const std::vector<Display>& found) {
    std::cout << '[';
    bool first = true;
    for (const auto& d : found) {
        if (!first) std::cout << ',';
        first = false;
        const auto& r = d.screen.DesktopCoordinates;
        const auto id = screen_id(d);
        const std::wstring wide(id.begin(), id.end());
        const bool rotated = d.screen.Rotation == DXGI_MODE_ROTATION_ROTATE90 || d.screen.Rotation == DXGI_MODE_ROTATION_ROTATE270;
        const char* rotation = d.screen.Rotation == DXGI_MODE_ROTATION_ROTATE90 ? "clockwise90" :
            d.screen.Rotation == DXGI_MODE_ROTATION_ROTATE180 ? "clockwise180" :
            d.screen.Rotation == DXGI_MODE_ROTATION_ROTATE270 ? "clockwise270" : "identity";
        UINT dpi_x = 96, dpi_y = 96;
        check(GetDpiForMonitor(d.screen.Monitor, MDT_EFFECTIVE_DPI, &dpi_x, &dpi_y), "Monitor DPI");
        MONITORINFO monitor{}; monitor.cbSize = sizeof(monitor);
        if (!GetMonitorInfoW(d.screen.Monitor, &monitor)) throw std::runtime_error("Monitor info failed");
        std::cout << "{\"id\":" << quoted(wide.c_str()) << ",\"name\":" << quoted(d.screen.DeviceName)
            << ",\"x\":" << r.left << ",\"y\":" << r.top
            << ",\"desktop_width\":" << r.right-r.left << ",\"desktop_height\":" << r.bottom-r.top
            << ",\"pixel_width\":" << (rotated ? r.bottom-r.top : r.right-r.left)
            << ",\"pixel_height\":" << (rotated ? r.right-r.left : r.bottom-r.top)
            << ",\"scale_milli\":" << dpi_x * 1000 / 96 << ",\"rotation\":\"" << rotation
            << "\",\"primary\":" << ((monitor.dwFlags & MONITORINFOF_PRIMARY) ? "true" : "false") << '}';
    }
    std::cout << "]\n";
}
static bool idr(const std::vector<uint8_t>& bytes) {
    for (size_t i = 0; i + 4 < bytes.size(); ++i) {
        if (!bytes[i] && !bytes[i+1] && bytes[i+2] == 1 && (bytes[i+3] & 31) == 5) return true;
    }
    return false;
}
static void capture(const Display& display, unsigned fps, const std::string& mode) {
    const bool synthetic = mode == "synthetic";
    if (!synthetic && mode != "desktop" && mode != "animate") throw std::runtime_error("Unknown source mode");
    // Rotation metadata is enumerated, but rotated native rendering still needs
    // acceptance. Fail explicitly instead of sending a mismapped desktop.
    if (display.screen.Rotation != DXGI_MODE_ROTATION_IDENTITY)
        throw std::runtime_error("Rotated media adapter not yet validated");
    ComPtr<ID3D11Device> device;
    ComPtr<ID3D11DeviceContext> context;
    check(D3D11CreateDevice(display.adapter.Get(), D3D_DRIVER_TYPE_UNKNOWN, nullptr,
        D3D11_CREATE_DEVICE_BGRA_SUPPORT | D3D11_CREATE_DEVICE_VIDEO_SUPPORT,
        nullptr, 0, D3D11_SDK_VERSION, &device, nullptr, &context), "Capture device");
    ComPtr<IDXGIOutputDuplication> duplication;
    ComPtr<ID3D11Texture2D> texture;
    ComPtr<ID3D11RenderTargetView> target;
    ComPtr<ID3D11DeviceContext1> context1;
    std::unique_ptr<PatternWindow> pattern;
    const auto& rect = display.screen.DesktopCoordinates;
    UINT width = rect.right-rect.left, height = rect.bottom-rect.top;
    if (!synthetic) {
        check(display.output->DuplicateOutput(device.Get(), &duplication), "DuplicateOutput");
        DXGI_OUTDUPL_DESC description{}; duplication->GetDesc(&description);
        if (description.ModeDesc.Width != width || description.ModeDesc.Height != height)
            throw std::runtime_error("Screen changed; refresh generation");
        if (mode == "animate") {
            pattern = std::make_unique<PatternWindow>(); pattern->initialize(device.Get(), rect);
        }
    } else {
        check(context.As(&context1), "Synthetic D3D11.1 context");
        D3D11_TEXTURE2D_DESC desc{};
        desc.Width = width; desc.Height = height;
        desc.MipLevels = desc.ArraySize = desc.SampleDesc.Count = 1;
        desc.Format = DXGI_FORMAT_B8G8R8A8_UNORM;
        desc.BindFlags = D3D11_BIND_RENDER_TARGET | D3D11_BIND_SHADER_RESOURCE;
        check(device->CreateTexture2D(&desc, nullptr, &texture), "Synthetic texture");
        check(device->CreateRenderTargetView(texture.Get(), nullptr, &target), "Synthetic target");
    }
    Encoder encoder; encoder.initialize(device.Get(), width, height, fps);
    write_value(magic); write_value(width); write_value(height);
    const auto start = Clock::now(); auto next = start;
    uint64_t index = 0;
    LONGLONG last_present = 0;
    while (true) {
        const auto command = read_value<uint8_t>();
        if (command == 0) return;
        if (command != 1) throw std::runtime_error("Invalid capture command");
        std::this_thread::sleep_until(next);
        std::vector<uint8_t> packet;
        const auto sampled = Clock::now();
        {
            FrameLease lease{nullptr};
            if (synthetic) {
                for (unsigned q = 0; q < 4; ++q) {
                    const LONG x = static_cast<LONG>((q % 2) * width / 2), y = static_cast<LONG>((q / 2) * height / 2);
                    const D3D11_RECT region{x,y,x+static_cast<LONG>(width/2),y+static_cast<LONG>(height/2)};
                    const auto color = test_color(index,q);
                    context1->ClearView(target.Get(), color.data(), &region, 1);
                }
                context->Flush();
            } else {
                const auto until = Clock::now() + std::chrono::seconds(30);
                while (!lease.duplication) {
                    if (Clock::now() > until) throw std::runtime_error("No desktop update before experiment timeout");
                    DWORD available = 0;
                    if (!PeekNamedPipe(GetStdHandle(STD_INPUT_HANDLE), nullptr, 0, nullptr, &available, nullptr))
                        throw std::runtime_error("Parent ended while waiting for desktop changes");
                    if (pattern) pattern->draw(context.Get(), index);
                    DXGI_OUTDUPL_FRAME_INFO info{}; ComPtr<IDXGIResource> resource;
                    const auto hr = duplication->AcquireNextFrame(100, &info, &resource);
                    if (hr == DXGI_ERROR_WAIT_TIMEOUT) continue;
                    check(hr, "AcquireNextFrame");
                    if (!info.LastPresentTime.QuadPart || info.LastPresentTime.QuadPart == last_present) {
                        duplication->ReleaseFrame(); continue;
                    }
                    lease.duplication = duplication.Get(); last_present = info.LastPresentTime.QuadPart;
                    check(resource.As(&texture), "Captured texture");
                }
            }
            encoder.encode(texture.Get(), index, nullptr, &packet);
        }
        if (!synthetic) texture.Reset();
        if (packet.empty() || packet.size() > max_packet) throw std::runtime_error("Encoded frame outside bounds");
        write_value(static_cast<uint32_t>(packet.size())); write_value(index);
        write_value(static_cast<uint64_t>(std::chrono::duration_cast<std::chrono::microseconds>(sampled-start).count()));
        write_value(static_cast<uint32_t>(idr(packet)));
        write_bytes(packet.data(), static_cast<uint32_t>(packet.size()));
        ++index;
        next = (std::max)(next + std::chrono::microseconds(1000000/fps), Clock::now());
    }
}
static void render(const Display& display, unsigned width, unsigned height, unsigned fps, bool verify) {
    MediaRuntime runtime;
    ComPtr<ID3D11Device> device; ComPtr<ID3D11DeviceContext> context;
    check(D3D11CreateDevice(display.adapter.Get(), D3D_DRIVER_TYPE_UNKNOWN, nullptr,
        D3D11_CREATE_DEVICE_BGRA_SUPPORT | D3D11_CREATE_DEVICE_VIDEO_SUPPORT,
        nullptr, 0, D3D11_SDK_VERSION, &device, nullptr, &context), "Render device");
    HardwareDecoder decoder(device.Get(), width, height, fps);
    NativePresenter presenter;
    presenter.initialize(device.Get(), width, height, fps, display.screen.DesktopCoordinates, true, verify);
    write_value(magic);
    uint64_t expected = 0;
    while (true) {
        const auto size = read_value<uint32_t>(&presenter);
        if (!size) {
            write_value(presenter.displayed); write_value(presenter.pixel_checks); write_value(presenter.max_color_error);
            return;
        }
        if (size > max_packet) throw std::runtime_error("Render packet too large");
        const auto index = read_value<uint64_t>(&presenter);
        if (index != expected) throw std::runtime_error("Render frame sequence mismatch");
        std::vector<uint8_t> packet(size); read_bytes(packet.data(), size, &presenter);
        const LONGLONG timestamp = static_cast<LONGLONG>(index) * 10000000 / fps;
        decoder.submit(packet, timestamp);
        decoder.receive([&](DecodedFrame& frame) {
            if (frame.timestamp != timestamp || presenter.displayed != index)
                throw std::runtime_error("Decoder reordered or duplicated a frame");
            presenter.render(frame, index);
        });
        if (presenter.displayed != index + 1)
            throw std::runtime_error("Decoder buffered a frame; asynchronous output adapter required");
        write_value(index); ++expected;
    }
}
int main(int argc, char** argv) {
    try {
        SetProcessDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2);
        const auto found = displays();
        if (argc == 2 && std::string(argv[1]) == "inventory") { inventory(found); return 0; }
        if (argc == 5 && std::string(argv[1]) == "capture") {
            for (const auto& d : found) if (screen_id(d) == argv[2]) {
                capture(d, number(argv[3],60),argv[4]); return 0;
            }
            throw std::runtime_error("Selected screen disappeared");
        }
        if (argc == 6 && std::string(argv[1]) == "render" && !found.empty()) {
            if (std::string(argv[5]) != "verify" && std::string(argv[5]) != "normal") throw std::runtime_error("Invalid render mode");
            const auto width = number(argv[2],8192), height = number(argv[3],8192);
            if (uint64_t(width)*height > 33554432) throw std::runtime_error("Render dimensions too large");
            render(found.front(),width,height,number(argv[4],60),std::string(argv[5]) == "verify"); return 0;
        }
        throw std::runtime_error("usage: stream-media inventory | capture SCREEN_ID FPS synthetic|desktop|animate | render WIDTH HEIGHT FPS normal|verify");
    } catch (const std::exception& e) { std::cerr << e.what() << '\n'; return 1; }
}
