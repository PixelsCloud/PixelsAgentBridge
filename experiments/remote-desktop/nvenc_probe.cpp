// Independent experiment using NVENC's public API; not copied SDK sample code.
// Reference: pinned hwcodec nv_encode.cpp; see sources.toml and README.md.
#include "dxgi_support.h"
#include <nvEncodeAPI.h>
#include <algorithm>
#include <fstream>
#include <memory>
#include <thread>

class Encoder {
    HMODULE library_ = nullptr;
    NV_ENCODE_API_FUNCTION_LIST api_{};
    void* session_ = nullptr;
    NV_ENC_OUTPUT_PTR bitstream_ = nullptr;
    NV_ENC_REGISTERED_PTR registered_ = nullptr;
    NV_ENC_INPUT_PTR mapped_ = nullptr;
    ComPtr<ID3D11Texture2D> retained_;
    UINT width_ = 0, height_ = 0;

    static void require(NVENCSTATUS status, const char* stage) {
        if (status != NV_ENC_SUCCESS)
            throw std::runtime_error(std::string(stage) + " NVENC status=" + std::to_string(status));
    }

    void release_input() {
        if (mapped_) {
            require(api_.nvEncUnmapInputResource(session_, mapped_), "UnmapInputResource");
            mapped_ = nullptr;
        }
        if (registered_) {
            require(api_.nvEncUnregisterResource(session_, registered_), "UnregisterResource");
            registered_ = nullptr;
        }
        retained_.Reset();
    }

    void stop() noexcept {
        // On any failure, destroy the session before releasing the capture lease
        // or texture: the driver may still own an accepted input resource.
        if (session_) {
            if (!mapped_ && !registered_ && bitstream_)
                api_.nvEncDestroyBitstreamBuffer(session_, bitstream_);
            api_.nvEncDestroyEncoder(session_);
            session_ = nullptr;
        }
        mapped_ = nullptr;
        registered_ = nullptr;
        bitstream_ = nullptr;
        retained_.Reset();
    }

public:
    Encoder() = default;
    Encoder(const Encoder&) = delete;
    Encoder& operator=(const Encoder&) = delete;
    ~Encoder() {
        stop();
        if (library_) FreeLibrary(library_);
    }

    void initialize(ID3D11Device* device, UINT width, UINT height, unsigned fps) {
        width_ = width;
        height_ = height;
        library_ = LoadLibraryExW(L"nvEncodeAPI64.dll", nullptr, LOAD_LIBRARY_SEARCH_SYSTEM32);
        if (!library_) throw std::runtime_error("NVIDIA encoder driver unavailable");
        using Create = NVENCSTATUS(NVENCAPI*)(NV_ENCODE_API_FUNCTION_LIST*);
        using Version = NVENCSTATUS(NVENCAPI*)(uint32_t*);
        const auto create = reinterpret_cast<Create>(GetProcAddress(library_, "NvEncodeAPICreateInstance"));
        const auto version = reinterpret_cast<Version>(GetProcAddress(library_, "NvEncodeAPIGetMaxSupportedVersion"));
        if (!create || !version) throw std::runtime_error("Missing NVENC entry points");
        uint32_t supported = 0;
        require(version(&supported), "GetMaxSupportedVersion");
        if (supported < ((NVENCAPI_MAJOR_VERSION << 4) | NVENCAPI_MINOR_VERSION))
            throw std::runtime_error("Driver does not support the pinned NVENC API version");
        api_.version = NV_ENCODE_API_FUNCTION_LIST_VER;
        require(create(&api_), "CreateInstance");
        NV_ENC_OPEN_ENCODE_SESSION_EX_PARAMS open{};
        open.version = NV_ENC_OPEN_ENCODE_SESSION_EX_PARAMS_VER;
        open.apiVersion = NVENCAPI_VERSION;
        open.device = device;
        open.deviceType = NV_ENC_DEVICE_TYPE_DIRECTX;
        require(api_.nvEncOpenEncodeSessionEx(&open, &session_), "OpenEncodeSession");
        NV_ENC_PRESET_CONFIG preset{};
        preset.version = NV_ENC_PRESET_CONFIG_VER;
        preset.presetCfg.version = NV_ENC_CONFIG_VER;
        require(api_.nvEncGetEncodePresetConfigEx(session_, NV_ENC_CODEC_H264_GUID,
            NV_ENC_PRESET_P3_GUID, NV_ENC_TUNING_INFO_LOW_LATENCY, &preset), "GetPreset");
        auto& config = preset.presetCfg;
        config.profileGUID = NV_ENC_H264_PROFILE_HIGH_GUID;
        config.frameIntervalP = 1;
        config.gopLength = fps;
        config.rcParams.rateControlMode = NV_ENC_PARAMS_RC_CBR;
        config.rcParams.averageBitRate = 6000000;
        config.rcParams.maxBitRate = 6000000;
        config.rcParams.enableLookahead = 0;
        config.rcParams.lookaheadDepth = 0;
        config.rcParams.zeroReorderDelay = 1;
        config.encodeCodecConfig.h264Config.idrPeriod = fps;
        config.encodeCodecConfig.h264Config.repeatSPSPPS = 1;
        NV_ENC_INITIALIZE_PARAMS init{};
        init.version = NV_ENC_INITIALIZE_PARAMS_VER;
        init.encodeGUID = NV_ENC_CODEC_H264_GUID;
        init.presetGUID = NV_ENC_PRESET_P3_GUID;
        init.tuningInfo = NV_ENC_TUNING_INFO_LOW_LATENCY;
        init.encodeWidth = init.darWidth = width;
        init.encodeHeight = init.darHeight = height;
        init.frameRateNum = fps;
        init.frameRateDen = 1;
        init.enablePTD = 1;
        init.enableEncodeAsync = 0;
        init.encodeConfig = &config;
        require(api_.nvEncInitializeEncoder(session_, &init), "InitializeEncoder");
        NV_ENC_CREATE_BITSTREAM_BUFFER output{};
        output.version = NV_ENC_CREATE_BITSTREAM_BUFFER_VER;
        require(api_.nvEncCreateBitstreamBuffer(session_, &output), "CreateBitstreamBuffer");
        bitstream_ = output.bitstreamBuffer;
    }

    uint64_t encode(ID3D11Texture2D* texture, uint64_t index, std::ofstream* output) {
        try {
            D3D11_TEXTURE2D_DESC desc{};
            texture->GetDesc(&desc);
            if (desc.Width != width_ || desc.Height != height_ || desc.Format != DXGI_FORMAT_B8G8R8A8_UNORM)
                throw std::runtime_error("Texture changed; rebuild encoder generation");
            retained_ = texture;
            NV_ENC_REGISTER_RESOURCE resource{};
            resource.version = NV_ENC_REGISTER_RESOURCE_VER;
            resource.resourceType = NV_ENC_INPUT_RESOURCE_TYPE_DIRECTX;
            resource.resourceToRegister = texture;
            resource.width = width_;
            resource.height = height_;
            resource.bufferFormat = NV_ENC_BUFFER_FORMAT_ARGB;
            resource.bufferUsage = NV_ENC_INPUT_IMAGE;
            require(api_.nvEncRegisterResource(session_, &resource), "RegisterCaptureTexture");
            registered_ = resource.registeredResource;
            NV_ENC_MAP_INPUT_RESOURCE map{};
            map.version = NV_ENC_MAP_INPUT_RESOURCE_VER;
            map.registeredResource = registered_;
            require(api_.nvEncMapInputResource(session_, &map), "MapInputResource");
            mapped_ = map.mappedResource;
            NV_ENC_PIC_PARAMS pic{};
            pic.version = NV_ENC_PIC_PARAMS_VER;
            pic.inputBuffer = mapped_;
            pic.bufferFmt = map.mappedBufferFmt;
            pic.inputWidth = width_;
            pic.inputHeight = height_;
            pic.outputBitstream = bitstream_;
            pic.pictureStruct = NV_ENC_PIC_STRUCT_FRAME;
            pic.frameIdx = static_cast<uint32_t>(index);
            pic.inputTimeStamp = index;
            if (!index) pic.encodePicFlags = NV_ENC_PIC_FLAG_FORCEIDR | NV_ENC_PIC_FLAG_OUTPUT_SPSPPS;
            // A single outstanding input. NEED_MORE_INPUT is an explicit failure
            // for this low-latency experiment, never silently release/reuse input.
            require(api_.nvEncEncodePicture(session_, &pic), "EncodePicture");
            NV_ENC_LOCK_BITSTREAM lock{};
            lock.version = NV_ENC_LOCK_BITSTREAM_VER;
            lock.outputBitstream = bitstream_;
            require(api_.nvEncLockBitstream(session_, &lock), "LockBitstream");
            const uint64_t bytes = lock.bitstreamSizeInBytes;
            if (output) output->write(static_cast<const char*>(lock.bitstreamBufferPtr), lock.bitstreamSizeInBytes);
            const auto timestamp = lock.outputTimeStamp;
            require(api_.nvEncUnlockBitstream(session_, bitstream_), "UnlockBitstream");
            if (!bytes || timestamp != index) throw std::runtime_error("Missing or delayed encoded frame");
            if (output && !*output) throw std::runtime_error("Cannot write synthetic bitstream");
            release_input();
            return bytes;
        } catch (...) {
            stop();
            throw;
        }
    }
};

static unsigned number(const char* text, unsigned maximum) {
    std::string value(text);
    size_t end = 0;
    if (value.empty() || value[0] == '-') throw std::runtime_error("Invalid number");
    const auto n = std::stoul(value, &end);
    if (end != value.size() || n > maximum) throw std::runtime_error("Number out of range");
    return static_cast<unsigned>(n);
}

// Optional moving-color source for a real DXGI acquisition benchmark. This is
// not the remote viewer or decoder; it never activates or sends user input.
class PatternWindow {
    HWND window_ = nullptr;
    ComPtr<IDXGISwapChain> swapchain_;
    ComPtr<ID3D11RenderTargetView> target_;
public:
    ~PatternWindow() {
        target_.Reset();
        swapchain_.Reset();
        if (window_ && IsWindow(window_)) DestroyWindow(window_);
    }
    void initialize(ID3D11Device* device, const RECT& screen) {
        WNDCLASSW cls{};
        cls.lpfnWndProc = DefWindowProcW;
        cls.hInstance = GetModuleHandleW(nullptr);
        cls.lpszClassName = L"PixelsNvencProbePattern";
        if (!RegisterClassW(&cls) && GetLastError() != ERROR_CLASS_ALREADY_EXISTS)
            throw std::runtime_error("Cannot register test pattern window");
        window_ = CreateWindowExW(WS_EX_NOACTIVATE | WS_EX_TOOLWINDOW | WS_EX_TOPMOST,
            cls.lpszClassName, L"Pixels GPU encoding test (auto closes)", WS_OVERLAPPED | WS_CAPTION | WS_SYSMENU,
            screen.left + 32, screen.top + 32, 640, 360, nullptr, nullptr, cls.hInstance, nullptr);
        if (!window_) throw std::runtime_error("Cannot create test pattern window");
        ComPtr<IDXGIDevice> dxgi;
        ComPtr<IDXGIAdapter> adapter;
        ComPtr<IDXGIFactory> factory;
        check(device->QueryInterface(IID_PPV_ARGS(&dxgi)), "DXGIDevice");
        check(dxgi->GetAdapter(&adapter), "GetAdapter");
        check(adapter->GetParent(IID_PPV_ARGS(&factory)), "GetFactory");
        DXGI_SWAP_CHAIN_DESC desc{};
        desc.BufferDesc.Format = DXGI_FORMAT_B8G8R8A8_UNORM;
        desc.SampleDesc.Count = 1;
        desc.BufferUsage = DXGI_USAGE_RENDER_TARGET_OUTPUT;
        desc.BufferCount = 2;
        desc.OutputWindow = window_;
        desc.Windowed = TRUE;
        desc.SwapEffect = DXGI_SWAP_EFFECT_FLIP_DISCARD;
        check(factory->CreateSwapChain(device, &desc, &swapchain_), "CreatePatternSwapChain");
        ComPtr<ID3D11Texture2D> buffer;
        check(swapchain_->GetBuffer(0, IID_PPV_ARGS(&buffer)), "GetPatternBackBuffer");
        check(device->CreateRenderTargetView(buffer.Get(), nullptr, &target_), "PatternRenderTarget");
        ShowWindow(window_, SW_SHOWNOACTIVATE);
    }
    void draw(ID3D11DeviceContext* context, uint64_t frame) {
        MSG message{};
        while (PeekMessageW(&message, nullptr, 0, 0, PM_REMOVE)) {
            TranslateMessage(&message);
            DispatchMessageW(&message);
        }
        if (!IsWindow(window_)) throw std::runtime_error("Test pattern window closed");
        const float color[] = {float(frame % 251) / 250.0f, float((frame * 7) % 251) / 250.0f,
            float((frame * 19) % 251) / 250.0f, 1.0f};
        context->ClearRenderTargetView(target_.Get(), color);
        check(swapchain_->Present(0, 0), "PresentPattern");
    }
};

int main(int argc, char** argv) {
    try {
        SetProcessDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2);
        unsigned selected = 0, seconds = 5, fps = 15;
        bool desktop = false, animate = false;
        std::string destination;
        for (int i = 1; i < argc; ++i) {
            const std::string arg(argv[i]);
            if (arg == "--capture") desktop = true;
            else if (arg == "--animate") animate = true;
            else if (arg == "--display" && i + 1 < argc) selected = number(argv[++i], 128);
            else if (arg == "--seconds" && i + 1 < argc) seconds = number(argv[++i], 60);
            else if (arg == "--fps" && i + 1 < argc) fps = number(argv[++i], 60);
            else if (arg == "--output" && i + 1 < argc) destination = argv[++i];
            else throw std::runtime_error("usage: nvenc-probe [--capture [--animate]] [--display N] [--fps 1..60] [--seconds 1..60] [--output synthetic.h264]");
        }
        if (!fps || !seconds) throw std::runtime_error("FPS and duration must be positive");
        if (desktop && !destination.empty()) throw std::runtime_error("Saving real desktop video is disabled in this experiment");
        if (animate && !desktop) throw std::runtime_error("--animate requires --capture");
        auto found = displays();
        if (selected >= found.size()) throw std::runtime_error("Display not attached in this session");
        const auto& d = found[selected];
        ComPtr<ID3D11Device> device;
        ComPtr<ID3D11DeviceContext> context;
        check(D3D11CreateDevice(d.adapter.Get(), D3D_DRIVER_TYPE_UNKNOWN, nullptr,
            D3D11_CREATE_DEVICE_BGRA_SUPPORT, nullptr, 0, D3D11_SDK_VERSION,
            &device, nullptr, &context), "D3D11CreateDevice");
        ComPtr<IDXGIOutputDuplication> duplication;
        ComPtr<ID3D11Texture2D> synthetic;
        ComPtr<ID3D11RenderTargetView> render_target;
        std::unique_ptr<PatternWindow> pattern;
        if (animate) {
            pattern = std::make_unique<PatternWindow>();
            pattern->initialize(device.Get(), d.screen.DesktopCoordinates);
        }
        if (desktop) check(d.output->DuplicateOutput(device.Get(), &duplication), "DuplicateOutput");
        else {
            D3D11_TEXTURE2D_DESC desc{};
            desc.Width = 1920; desc.Height = 1080;
            desc.MipLevels = desc.ArraySize = desc.SampleDesc.Count = 1;
            desc.Format = DXGI_FORMAT_B8G8R8A8_UNORM;
            desc.Usage = D3D11_USAGE_DEFAULT;
            desc.BindFlags = D3D11_BIND_RENDER_TARGET | D3D11_BIND_SHADER_RESOURCE;
            check(device->CreateTexture2D(&desc, nullptr, &synthetic), "CreateSyntheticTexture");
            check(device->CreateRenderTargetView(synthetic.Get(), nullptr, &render_target), "CreateRenderTargetView");
        }
        std::ofstream output;
        if (!destination.empty()) {
            if (GetFileAttributesA(destination.c_str()) != INVALID_FILE_ATTRIBUTES)
                throw std::runtime_error("Output already exists; choose a new filename");
            output.open(destination, std::ios::binary);
            if (!output) throw std::runtime_error("Cannot open output file");
        }
        std::unique_ptr<Encoder> encoder;
        std::vector<double> timings;
        uint64_t bytes = 0, frames = 0, changed = 0, pattern_updates = 0;
        LONGLONG last_present = 0;
        UINT width = 0, height = 0;
        const auto start = Clock::now();
        auto next = start;
        while (Clock::now() - start < std::chrono::seconds(seconds)) {
            std::this_thread::sleep_until(next);
            if (Clock::now() - start >= std::chrono::seconds(seconds)) break;
            ComPtr<ID3D11Texture2D> texture;
            FrameLease lease{nullptr};
            if (desktop) {
                if (pattern) pattern->draw(context.Get(), pattern_updates++);
                DXGI_OUTDUPL_FRAME_INFO info{};
                ComPtr<IDXGIResource> resource;
                const auto hr = duplication->AcquireNextFrame(100, &info, &resource);
                if (hr == DXGI_ERROR_WAIT_TIMEOUT) continue;
                check(hr, "AcquireNextFrame");
                lease.duplication = duplication.Get();
                check(resource.As(&texture), "CaptureTexture");
                if (!info.LastPresentTime.QuadPart || info.LastPresentTime.QuadPart == last_present) continue;
                last_present = info.LastPresentTime.QuadPart;
            } else {
                const float color[] = {float(frames % 251) / 250.0f, float((frames * 7) % 251) / 250.0f, float((frames * 19) % 251) / 250.0f, 1.0f};
                context->ClearRenderTargetView(render_target.Get(), color);
                context->Flush();
                texture = synthetic;
            }
            ++changed;
            if (!encoder) {
                D3D11_TEXTURE2D_DESC desc{};
                texture->GetDesc(&desc);
                width = desc.Width; height = desc.Height;
                encoder = std::make_unique<Encoder>();
                encoder->initialize(device.Get(), width, height, fps);
            }
            const auto before = Clock::now();
            bytes += encoder->encode(texture.Get(), frames, output.is_open() ? &output : nullptr);
            timings.push_back(std::chrono::duration<double, std::milli>(Clock::now() - before).count());
            ++frames;
            next = std::max(next + std::chrono::microseconds(1000000 / fps), Clock::now());
        }
        if (!frames) throw std::runtime_error("No changed frames encoded");
        const double elapsed = std::chrono::duration<double>(Clock::now() - start).count();
        std::sort(timings.begin(), timings.end());
        std::cout << "{\"schema_version\":1,\"stage\":\"direct_texture_h264_encode\",\"source\":\""
            << (desktop ? "dxgi_desktop" : "synthetic_gpu") << "\",\"display\":";
        identify(d, selected);
        std::cout << ",\"width\":" << width << ",\"height\":" << height
            << ",\"target_fps\":" << fps << ",\"seconds\":" << elapsed
            << ",\"changed_inputs\":" << changed << ",\"encoded_frames\":" << frames
            << ",\"encoded_fps\":" << frames / elapsed << ",\"encoded_bytes\":" << bytes
            << ",\"encode_p50_ms\":" << timings[timings.size() / 2]
            << ",\"encode_p95_ms\":" << timings[(timings.size() - 1) * 95 / 100]
            << ",\"test_pattern_updates\":" << pattern_updates
            << ",\"probe_pixel_cpu_readbacks\":0,\"probe_gpu_copies\":0,\"probe_conversions\":0"
            << ",\"driver_internal_copies_known\":false,\"decoded\":false,\"rendered\":false,\"effective_display_fps\":null}\n";
        return 0;
    } catch (const std::exception& error) {
        std::cerr << error.what() << '\n';
        return 1;
    }
}
