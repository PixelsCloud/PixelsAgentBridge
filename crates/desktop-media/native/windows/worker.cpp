// Isolated native media worker. Compressed H.264 and bounded cursor snapshots
// cross anonymous pipes; cursor sampling never reads back the desktop surface.
#include "nvenc_encoder.h"
#include "pattern_window.h"
#include "d3d_presenter.h"
#include "software_codec.h"
#include "wgc_capture.h"
#include "capture_events.h"
#include <d3d11_1.h>
#include <shellscalingapi.h>
#include <algorithm>
#include <memory>

static constexpr uint32_t magic = 0x31424150; // PAB1, little endian local IPC
static constexpr uint32_t max_packet = 16 * 1024 * 1024;

static void read_bytes(void* data, uint32_t size, NativePresenter* viewer = nullptr,
    const std::function<void()>& idle = {}) {
    auto* cursor = static_cast<uint8_t*>(data);
    while (size) {
        if (viewer) viewer->pump();
        DWORD available = 0;
        if (!PeekNamedPipe(GetStdHandle(STD_INPUT_HANDLE), nullptr, 0, nullptr, &available, nullptr))
            throw std::runtime_error("Parent pipe ended");
        if (!available) { if (idle) idle(); Sleep(2); continue; }
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
template<class T> static T read_value(NativePresenter* viewer = nullptr,
    const std::function<void()>& idle = {}) {
    T value{}; read_bytes(&value, sizeof(value), viewer, idle); return value;
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
#include "input_worker.h"
#include "input_fixture.h"
static void validate_wgc_desktop(const Display& selected);
#include "cursor_worker.h"
#include "cursor_fixture.h"
#include "win32_cursor_fixture.h"
#include "cursor_overlay_fixture.h"
static bool idr(const std::vector<uint8_t>& bytes) {
    for (size_t i = 0; i + 4 < bytes.size(); ++i) {
        if (!bytes[i] && !bytes[i+1] && bytes[i+2] == 1 && (bytes[i+3] & 31) == 5) return true;
    }
    return false;
}
static void validate_wgc_desktop(const Display& selected) {
    // WGC is only a fallback on the same ordinary desktop. A lock, session
    // switch or monitor change must still go through generation recovery.
    HDESK desktop = OpenInputDesktop(0, FALSE, DESKTOP_READOBJECTS);
    if (!desktop) throw std::runtime_error("Active capture desktop unavailable");
    std::wstring active;
    try { active = desktop_name(desktop); } catch (...) { CloseDesktop(desktop); throw; }
    CloseDesktop(desktop);
    if (active != L"Default" || desktop_name(GetThreadDesktop(GetCurrentThreadId())) != active)
        throw std::runtime_error("Capture desktop changed; renew generation");
    for (const auto& current : displays()) if (screen_id(current) == screen_id(selected)) {
        if (EqualRect(&selected.screen.DesktopCoordinates, &current.screen.DesktopCoordinates)
            && current.screen.Rotation == selected.screen.Rotation
            && current.screen.Monitor == selected.screen.Monitor) return;
        break;
    }
    throw std::runtime_error("Screen changed; refresh generation");
}
static void capture(const Display& display, unsigned fps, const std::string& mode, bool force_software, bool fail_encoder,
    const std::string& capture_token = {}) {
    const bool synthetic = mode == "synthetic";
    if (!synthetic && mode != "desktop" && mode != "animate") throw std::runtime_error("Unknown source mode");
    std::unique_ptr<WgcRuntime> wgc_runtime; // Released after the shared device.
    std::unique_ptr<CaptureEvents> coordination;
    if (!capture_token.empty()) coordination = std::make_unique<CaptureEvents>(capture_token, true);
    ComPtr<ID3D11Device> device;
    ComPtr<ID3D11DeviceContext> context;
    check(D3D11CreateDevice(display.adapter.Get(), D3D_DRIVER_TYPE_UNKNOWN, nullptr,
        D3D11_CREATE_DEVICE_BGRA_SUPPORT | D3D11_CREATE_DEVICE_VIDEO_SUPPORT,
        nullptr, 0, D3D11_SDK_VERSION, &device, nullptr, &context), "Capture device");
    ComPtr<IDXGIOutputDuplication> duplication;
    std::unique_ptr<WgcCapture> wgc;
    ComPtr<ID3D11Texture2D> texture;
    ComPtr<ID3D11RenderTargetView> target;
    ComPtr<ID3D11DeviceContext1> context1;
    std::unique_ptr<PatternWindow> pattern;
    const auto& rect = display.screen.DesktopCoordinates;
    const bool rotated = display.screen.Rotation == DXGI_MODE_ROTATION_ROTATE90 || display.screen.Rotation == DXGI_MODE_ROTATION_ROTATE270;
    UINT width = rotated ? rect.bottom-rect.top : rect.right-rect.left;
    UINT height = rotated ? rect.right-rect.left : rect.bottom-rect.top;
    if (!synthetic) {
        check(display.output->DuplicateOutput(device.Get(), &duplication), "DuplicateOutput");
        DXGI_OUTDUPL_DESC description{}; duplication->GetDesc(&description);
        // DXGI's mode is in desktop orientation; acquired textures remain in
        // native orientation. Comparing the mode to encoder dimensions rejects
        // every 90/270-degree display even when its layout has not changed.
        if (description.ModeDesc.Width != UINT(rect.right-rect.left)
            || description.ModeDesc.Height != UINT(rect.bottom-rect.top)
            || description.Rotation != display.screen.Rotation)
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
    CaptureEncoder encoder(device.Get(), width, height, fps, force_software, fail_encoder);
    const auto switch_wgc = [&] {
        if (wgc) return;
        if (synthetic) throw std::runtime_error("Synthetic capture cannot change backend");
        validate_wgc_desktop(display);
        duplication.Reset();
        wgc_runtime = std::make_unique<WgcRuntime>();
        wgc = std::make_unique<WgcCapture>(device.Get(), display, fps);
        if (coordination) coordination->activate();
        std::cerr << "media capture=wgc_gpu readback_per_frame=0\n";
    };
    const auto coordinate = [&] {
        if (coordination && coordination->requested()) switch_wgc();
    };
    std::cerr << "media encoder=" << (encoder.software() ? "openh264_software readback_per_frame=1" : "nvenc_gpu readback_per_frame=0") << '\n';
    write_value(magic); write_value(width); write_value(height);
    const auto start = Clock::now(); auto next = start;
    uint64_t index = 0;
    LONGLONG last_present = 0;
    while (true) {
        // Cursor recovery must also work while video has no consumption credit.
        const auto command = read_value<uint8_t>(nullptr, coordinate);
        if (command == 0) { if (mode != "desktop") encoder.report(); return; }
        if (command != 1) throw std::runtime_error("Invalid capture command");
        std::this_thread::sleep_until(next);
        std::vector<uint8_t> packet;
        auto sampled = Clock::now();
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
                while (!lease.duplication) {
                    DWORD available = 0;
                    if (!PeekNamedPipe(GetStdHandle(STD_INPUT_HANDLE), nullptr, 0, nullptr, &available, nullptr))
                        throw std::runtime_error("Parent ended while waiting for desktop changes");
                    if (pattern) pattern->draw(context.Get(), index);
                    coordinate();
                    if (wgc) {
                        validate_wgc_desktop(display);
                        texture = wgc->next(100);
                        if (!texture) continue;
                        // Do not encode a frame obtained across a desktop or
                        // geometry transition during the bounded wait.
                        validate_wgc_desktop(display);
                        break;
                    }
                    DXGI_OUTDUPL_FRAME_INFO info{}; ComPtr<IDXGIResource> resource;
                    const auto hr = duplication->AcquireNextFrame(100, &info, &resource);
                    if (hr == DXGI_ERROR_WAIT_TIMEOUT) continue;
                    if (hr == DXGI_ERROR_ACCESS_LOST) {
                        switch_wgc();
                        continue;
                    }
                    check(hr, "AcquireNextFrame");
                    if (!info.LastPresentTime.QuadPart || info.LastPresentTime.QuadPart == last_present) {
                        duplication->ReleaseFrame(); continue;
                    }
                    lease.duplication = duplication.Get(); last_present = info.LastPresentTime.QuadPart;
                    check(resource.As(&texture), "Captured texture");
                }
                sampled = Clock::now();
            }
            encoder.encode(texture.Get(), index, packet);
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
static void render_stream(VideoDecoder& decoder, NativePresenter& presenter, unsigned fps, bool interactive) {
    write_value(uint32_t(2)); // Version of the private event protocol, not network framing.
    write_value(uint8_t(4)); write_value(uint8_t(decoder.software() ? 2 : 1));
    uint64_t expected = 0;
    uint64_t cursor_sequence = 0;
    std::deque<std::pair<LONGLONG, uint64_t>> pending;
    auto last_output = Clock::now();
    const auto consume = [&](DecodedFrame& frame) {
        if (pending.empty() || pending.front().first != frame.timestamp)
            throw std::runtime_error("Decoder reordered or invented a timestamp");
        const auto index = pending.front().second;
        presenter.render(frame, index);
        pending.pop_front(); last_output = Clock::now();
        write_value(uint8_t(2)); write_value(index);
    };
    const auto drain_ready = [&] {
        if (pending.empty()) return;
        decoder.receive(consume);
        if (!pending.empty() && Clock::now() - last_output > std::chrono::seconds(5))
            throw std::runtime_error("Decoder output exceeded bounded latency");
    };
    while (true) {
        const auto size = read_value<uint32_t>(&presenter, drain_ready);
        if (size == UINT32_MAX-1) {
            const auto length = read_value<uint32_t>(&presenter, drain_ready);
            if (length < 9 || length > max_cursor_bytes+64) throw std::runtime_error("Invalid cursor render packet");
            const auto sequence = read_value<uint64_t>(&presenter, drain_ready);
            if (cursor_sequence == UINT64_MAX || sequence != cursor_sequence+1) throw std::runtime_error("Invalid cursor render sequence");
            std::vector<uint8_t> packet(length-8); read_bytes(packet.data(),static_cast<uint32_t>(packet.size()),&presenter,drain_ready);
            presenter.cursor_update(packet); cursor_sequence = sequence;
            write_value(uint8_t(6)); write_value(sequence); continue;
        }
        if (interactive && size == UINT32_MAX) {
            const auto enabled = read_value<uint8_t>(&presenter, drain_ready);
            if (enabled > 1) throw std::runtime_error("Invalid viewer input mode");
            presenter.input_enabled(enabled != 0); continue;
        }
        if (!size) {
            decoder.drain(consume);
            if (!pending.empty()) throw std::runtime_error("Decoder did not drain all submitted frames");
            presenter.finish();
            write_value(uint8_t(3));
            write_value(presenter.displayed); write_value(presenter.pixel_checks); write_value(presenter.max_color_error);
            return;
        }
        if (size > max_packet) throw std::runtime_error("Render packet too large");
        const auto index = read_value<uint64_t>(&presenter, drain_ready);
        if (index != expected) throw std::runtime_error("Render frame sequence mismatch");
        std::vector<uint8_t> packet(size); read_bytes(packet.data(), size, &presenter, drain_ready);
        const LONGLONG timestamp = static_cast<LONGLONG>(index) * 10000000 / fps;
        const auto deadline = Clock::now() + std::chrono::seconds(2);
        while (!decoder.try_submit(packet, timestamp)) {
            drain_ready(); presenter.pump();
            if (Clock::now() >= deadline) throw std::runtime_error("Decoder backpressure timed out");
            Sleep(1);
        }
        if (pending.empty()) last_output = Clock::now();
        pending.emplace_back(timestamp, index);
        // Acceptance permits another compressed packet, but gives no network
        // credit and never enables input. Only event 2 confirms rendered output.
        write_value(uint8_t(1)); write_value(index); ++expected;
        drain_ready();
        if (pending.size() > 4) throw std::runtime_error("Decoder exceeds the four-frame latency budget");
    }
}
static void render(const Display& display, unsigned width, unsigned height, unsigned fps, bool verify, HWND parent, unsigned rotation, bool interactive, bool streaming, bool force_software) {
    MediaRuntime runtime;
    ComPtr<ID3D11Device> device; ComPtr<ID3D11DeviceContext> context;
    check(D3D11CreateDevice(display.adapter.Get(), D3D_DRIVER_TYPE_UNKNOWN, nullptr,
        D3D11_CREATE_DEVICE_BGRA_SUPPORT | D3D11_CREATE_DEVICE_VIDEO_SUPPORT,
        nullptr, 0, D3D11_SDK_VERSION, &device, nullptr, &context), "Render device");
    VideoDecoder decoder(device.Get(), width, height, fps, force_software);
    NativePresenter presenter;
    presenter.input_available(interactive);
    presenter.initialize(device.Get(), width, height, fps, display.screen.DesktopCoordinates, true, verify, parent, rotation);
    write_value(magic);
    if (streaming) { render_stream(decoder, presenter, fps, interactive); return; }
    uint64_t expected = 0;
    while (true) {
        const auto size = read_value<uint32_t>(&presenter);
        if (interactive && size == UINT32_MAX) {
            const auto enabled = read_value<uint8_t>(&presenter);
            if (enabled > 1) throw std::runtime_error("Invalid viewer input mode");
            presenter.input_enabled(enabled != 0); continue;
        }
        if (!size) {
            presenter.finish();
            write_value(presenter.displayed); write_value(presenter.pixel_checks); write_value(presenter.max_color_error);
            return;
        }
        if (size > max_packet) throw std::runtime_error("Render packet too large");
        const auto index = read_value<uint64_t>(&presenter);
        if (index != expected) throw std::runtime_error("Render frame sequence mismatch");
        std::vector<uint8_t> packet(size); read_bytes(packet.data(), size, &presenter);
        const LONGLONG timestamp = static_cast<LONGLONG>(index) * 10000000 / fps;
        decoder.submit(packet, timestamp);
        unsigned received = 0;
        decoder.receive([&](DecodedFrame& frame) {
            if (frame.timestamp != timestamp || received != 0)
                throw std::runtime_error("Decoder reordered or duplicated a frame");
            presenter.render(frame, index);
            ++received;
        });
        if (received != 1)
            throw std::runtime_error("Decoder buffered a frame; asynchronous output adapter required");
        write_value(index); ++expected;
    }
}
extern "C" int pab_media_main(int argc, const char* const* argv) {
    try {
        SetProcessDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2);
        if (argc == 2 && std::string(argv[1]) == "cursor-win32-check") {
            win32_cursor_check(); return 0;
        }
        if (argc == 2 && std::string(argv[1]) == "cursor-shape-check") {
            cursor_shape_check(); return 0;
        }
        if (argc == 2 && std::string(argv[1]) == "cursor-overlay-check") {
            cursor_overlay_check(); return 0;
        }
        if (argc == 2 && std::string(argv[1]) == "input-recovery-check") {
            input_recovery_check(); return 0;
        }
        if (argc == 2 && std::string(argv[1]) == "recover-input") {
            InputReleases releases(input_recovery_path()); recover_input(releases); return 0;
        }
        const auto found = displays();
        if (argc == 3 && std::string(argv[1]) == "input-fixture" && !found.empty()) {
            input_fixture(found.front(), number(argv[2], 120)); return 0;
        }
        if (argc == 4 && std::string(argv[1]) == "input-fixture") {
            for (const auto& d : found) if (screen_id(d) == argv[2]) {
                input_fixture(d, number(argv[3], 120)); return 0;
            }
            throw std::runtime_error("Selected fixture screen disappeared");
        }
        if (argc == 3 && std::string(argv[1]) == "input") {
            for (const auto& d : found) if (screen_id(d) == argv[2]) { input_worker(d); return 0; }
            throw std::runtime_error("Selected input screen disappeared");
        }
        if ((argc == 3 || argc == 4) && std::string(argv[1]) == "cursor") {
            for (const auto& d : found) if (screen_id(d) == argv[2]) {
                cursor_worker(d, argc == 4 ? argv[3] : ""); return 0;
            }
            throw std::runtime_error("Selected cursor screen disappeared");
        }
        if (argc == 2 && std::string(argv[1]) == "inventory") { inventory(found); return 0; }
        if (argc == 4 && std::string(argv[1]) == "animate-display" && !found.empty()) {
            const auto fps = number(argv[2], 60), seconds = number(argv[3], 120);
            ComPtr<ID3D11Device> device; ComPtr<ID3D11DeviceContext> context;
            check(D3D11CreateDevice(found.front().adapter.Get(), D3D_DRIVER_TYPE_UNKNOWN, nullptr,
                D3D11_CREATE_DEVICE_BGRA_SUPPORT, nullptr, 0, D3D11_SDK_VERSION,
                &device, nullptr, &context), "Animation device");
            PatternWindow window; window.initialize(device.Get(), found.front().screen.DesktopCoordinates);
            const auto start = Clock::now();
            for (uint64_t frame = 0; frame < uint64_t(fps)*seconds; ++frame) {
                window.draw(context.Get(), frame);
                std::this_thread::sleep_until(start + std::chrono::microseconds((frame+1)*1000000/fps));
            }
            return 0;
        }
        if ((argc == 5 || argc == 6) && std::string(argv[1]) == "capture") {
            std::string mode(argv[4]);
            const bool fail_encoder = mode.size() > 13 && mode.compare(mode.size()-13,13,"-fail-encoder") == 0;
            if (fail_encoder) mode.resize(mode.size()-13);
            const bool software = mode.size() > 9 && mode.compare(mode.size()-9,9,"-software") == 0;
            if (software) mode.resize(mode.size()-9);
            for (const auto& d : found) if (screen_id(d) == argv[2]) {
                capture(d, number(argv[3],60),mode,software,fail_encoder,argc == 6 ? argv[5] : ""); return 0;
            }
            throw std::runtime_error("Selected screen disappeared");
        }
        if (argc >= 6 && argc <= 8 && std::string(argv[1]) == "render" && !found.empty()) {
            std::string mode(argv[5]);
            const bool software = mode.size() > 9 && mode.compare(mode.size()-9,9,"-software") == 0;
            if (software) mode.resize(mode.size()-9);
            const bool streaming = mode == "stream" || mode == "stream-verify" || mode == "stream-interactive";
            if (!streaming && mode != "verify" && mode != "normal" && mode != "interactive") throw std::runtime_error("Invalid render mode");
            const auto width = number(argv[2],8192), height = number(argv[3],8192);
            if (uint64_t(width)*height > 33554432) throw std::runtime_error("Render dimensions too large");
            HWND parent = nullptr;
            if (argc >= 7) {
                size_t end = 0; const std::string handle(argv[6]);
                parent = reinterpret_cast<HWND>(std::stoull(handle, &end));
                if (end != handle.size() || (parent && !IsWindow(parent))) throw std::runtime_error("Invalid viewer owner");
            }
            unsigned rotation = 0;
            if (argc == 8) {
                const std::string angle(argv[7]);
                if (angle == "90") rotation = 1;
                else if (angle == "180") rotation = 2;
                else if (angle == "270") rotation = 3;
                else if (angle != "0") throw std::runtime_error("Invalid rotation");
            }
            render(found.front(),width,height,number(argv[4],60),mode == "verify" || mode == "stream-verify", parent, rotation,
                mode == "interactive" || mode == "stream-interactive", streaming, software); return 0;
        }
        throw std::runtime_error("usage: stream-media inventory | capture SCREEN_ID FPS synthetic|desktop|animate | render WIDTH HEIGHT FPS normal|verify");
    } catch (const std::exception& e) { std::cerr << e.what() << '\n'; return 1; }
}
