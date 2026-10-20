#pragma once
#include "mf_decoder.h"
#include <dxgi1_3.h>
#include <array>
#include <thread>

// RGB test quadrants change every frame; differing corners detect channel,
// orientation, crop and stale-surface errors. Values avoid clipping extremes.
inline std::array<float, 4> test_color(uint64_t frame, unsigned quadrant) {
    return {float(24 + (frame * 3 + quadrant * 43) % 208) / 255.0f,
        float(24 + (frame * 7 + quadrant * 67) % 208) / 255.0f,
        float(24 + (frame * 11 + quadrant * 97) % 208) / 255.0f, 1.0f};
}

class NativePresenter {
    HWND window_ = nullptr;
    HANDLE latency_ = nullptr;
    ComPtr<ID3D11Device> device_;
    ComPtr<ID3D11DeviceContext> context_;
    ComPtr<ID3D11VideoDevice> video_;
    ComPtr<ID3D11VideoContext> video_context_;
    ComPtr<ID3D11VideoProcessorEnumerator> enumerator_;
    ComPtr<ID3D11VideoProcessor> processor_;
    ComPtr<IDXGISwapChain2> swapchain_;
    ComPtr<ID3D11Query> finished_;
    ComPtr<ID3D11Texture2D> pixel_;
    UINT width_ = 0, height_ = 0;
    UINT output_width_ = 960, output_height_ = 540;
public:
    uint64_t submitted = 0, displayed = 0, pixel_checks = 0;
    unsigned max_color_error = 0;
    uint64_t stats_disjoint = 0;

    NativePresenter() = default;
    NativePresenter(const NativePresenter&) = delete;
    NativePresenter& operator=(const NativePresenter&) = delete;
    ~NativePresenter() {
        pixel_.Reset();
        processor_.Reset();
        enumerator_.Reset();
        swapchain_.Reset();
        if (latency_) CloseHandle(latency_);
        if (window_ && IsWindow(window_)) DestroyWindow(window_);
    }
    void pump() {
        MSG message{};
        while (PeekMessageW(&message, nullptr, 0, 0, PM_REMOVE)) {
            TranslateMessage(&message);
            DispatchMessageW(&message);
        }
        if (!IsWindow(window_)) throw std::runtime_error("Viewer closed by user");
        if (IsIconic(window_)) throw std::runtime_error("Viewer minimized; visible FPS cannot be measured");
    }
    void initialize(ID3D11Device* device, UINT width, UINT height, UINT fps,
                    const RECT& monitor, bool capture, bool verify) {
        device_ = device;
        width_ = width; height_ = height;
        device->GetImmediateContext(&context_);
        check(device_.As(&video_), "D3D11 video device");
        check(context_.As(&video_context_), "D3D11 video context");
        WNDCLASSW cls{};
        cls.lpfnWndProc = DefWindowProcW;
        cls.hInstance = GetModuleHandleW(nullptr);
        cls.lpszClassName = L"PixelsDecodeDisplayProbe";
        if (!RegisterClassW(&cls) && GetLastError() != ERROR_CLASS_ALREADY_EXISTS)
            throw std::runtime_error("Cannot register viewer window");
        // Fixed window size in this experiment. Production resize, input and
        // generation handling are deliberately not claimed here.
        const DWORD style = WS_OVERLAPPED | WS_CAPTION | WS_SYSMENU;
        RECT bounds{0, 0, static_cast<LONG>(output_width_), static_cast<LONG>(output_height_)};
        if (!AdjustWindowRectEx(&bounds, style, FALSE, WS_EX_NOACTIVATE | WS_EX_TOOLWINDOW | WS_EX_TOPMOST))
            throw std::runtime_error("Cannot compute viewer bounds");
        window_ = CreateWindowExW(WS_EX_NOACTIVATE | WS_EX_TOOLWINDOW | WS_EX_TOPMOST,
            cls.lpszClassName, L"Pixels H.264 hardware decode test (auto closes)", style,
            monitor.right - (bounds.right - bounds.left) - 32, monitor.bottom - (bounds.bottom - bounds.top) - 32,
            bounds.right - bounds.left, bounds.bottom - bounds.top,
            nullptr, nullptr, cls.hInstance, nullptr);
        if (!window_) throw std::runtime_error("Cannot create native viewer");
        // Avoid recursively capturing the test viewer in local desktop loopback.
        if (capture && !SetWindowDisplayAffinity(window_, WDA_EXCLUDEFROMCAPTURE))
            throw std::runtime_error("Cannot exclude loopback viewer from capture");
        ComPtr<IDXGIDevice> dxgi;
        ComPtr<IDXGIAdapter> adapter;
        ComPtr<IDXGIFactory2> factory;
        check(device_.As(&dxgi), "Renderer DXGI device");
        check(dxgi->GetAdapter(&adapter), "Renderer adapter");
        check(adapter->GetParent(IID_PPV_ARGS(&factory)), "Renderer factory");
        DXGI_SWAP_CHAIN_DESC1 desc{};
        desc.Width = output_width_; desc.Height = output_height_;
        desc.Format = DXGI_FORMAT_B8G8R8A8_UNORM;
        desc.SampleDesc.Count = 1;
        desc.BufferUsage = DXGI_USAGE_RENDER_TARGET_OUTPUT;
        desc.BufferCount = 2;
        desc.SwapEffect = DXGI_SWAP_EFFECT_FLIP_SEQUENTIAL;
        desc.Flags = DXGI_SWAP_CHAIN_FLAG_FRAME_LATENCY_WAITABLE_OBJECT;
        ComPtr<IDXGISwapChain1> chain;
        check(factory->CreateSwapChainForHwnd(device, window_, &desc, nullptr, nullptr, &chain), "Create viewer swapchain");
        check(chain.As(&swapchain_), "Waitable swapchain");
        check(factory->MakeWindowAssociation(window_, DXGI_MWA_NO_ALT_ENTER), "Disable fullscreen shortcut");
        check(swapchain_->SetMaximumFrameLatency(1), "Limit presentation queue");
        latency_ = swapchain_->GetFrameLatencyWaitableObject();
        if (!latency_) throw std::runtime_error("No frame latency waitable object");
        D3D11_VIDEO_PROCESSOR_CONTENT_DESC content{};
        content.InputFrameFormat = D3D11_VIDEO_FRAME_FORMAT_PROGRESSIVE;
        content.InputFrameRate = content.OutputFrameRate = {fps, 1};
        content.InputWidth = width; content.InputHeight = height;
        content.OutputWidth = output_width_; content.OutputHeight = output_height_;
        content.Usage = D3D11_VIDEO_USAGE_PLAYBACK_NORMAL;
        check(video_->CreateVideoProcessorEnumerator(&content, &enumerator_), "Create video processor enumerator");
        UINT flags = 0;
        check(enumerator_->CheckVideoProcessorFormat(DXGI_FORMAT_NV12, &flags), "Check NV12 processor input");
        if (!(flags & D3D11_VIDEO_PROCESSOR_FORMAT_SUPPORT_INPUT)) throw std::runtime_error("NV12 input unsupported");
        check(enumerator_->CheckVideoProcessorFormat(desc.Format, &flags), "Check BGRA processor output");
        if (!(flags & D3D11_VIDEO_PROCESSOR_FORMAT_SUPPORT_OUTPUT)) throw std::runtime_error("BGRA output unsupported");
        check(video_->CreateVideoProcessor(enumerator_.Get(), 0, &processor_), "Create video processor");
        video_context_->VideoProcessorSetStreamFrameFormat(processor_.Get(), 0, D3D11_VIDEO_FRAME_FORMAT_PROGRESSIVE);
        video_context_->VideoProcessorSetStreamAutoProcessingMode(processor_.Get(), 0, FALSE);
        RECT source{0, 0, static_cast<LONG>(width), static_cast<LONG>(height)};
        RECT destination{0, 0, static_cast<LONG>(output_width_), static_cast<LONG>(output_height_)};
        video_context_->VideoProcessorSetStreamSourceRect(processor_.Get(), 0, TRUE, &source);
        video_context_->VideoProcessorSetStreamDestRect(processor_.Get(), 0, TRUE, &destination);
        video_context_->VideoProcessorSetOutputTargetRect(processor_.Get(), TRUE, &destination);
        // NVENC's RGB-input conversion with explicit SDR VUI in this experiment is
        // validated as limited-range BT.709 by the synthetic corner checks.
        // This is NOT a general stream color-metadata negotiation policy.
        D3D11_VIDEO_PROCESSOR_COLOR_SPACE input_color{}, output_color{};
        input_color.YCbCr_Matrix = 1; // BT.709
        input_color.Nominal_Range = 1; // 16..235
        output_color.RGB_Range = 0; // full RGB
        video_context_->VideoProcessorSetStreamColorSpace(processor_.Get(), 0, &input_color);
        video_context_->VideoProcessorSetOutputColorSpace(processor_.Get(), &output_color);
        D3D11_QUERY_DESC query{D3D11_QUERY_EVENT, 0};
        check(device->CreateQuery(&query, &finished_), "Create GPU completion query");
        if (verify) {
            D3D11_TEXTURE2D_DESC staging{};
            staging.Width = staging.Height = staging.MipLevels = staging.ArraySize = staging.SampleDesc.Count = 1;
            staging.Format = DXGI_FORMAT_B8G8R8A8_UNORM;
            staging.Usage = D3D11_USAGE_STAGING;
            staging.CPUAccessFlags = D3D11_CPU_ACCESS_READ;
            check(device->CreateTexture2D(&staging, nullptr, &pixel_), "Create test-only pixel readback");
        }
        ShowWindow(window_, SW_SHOWNOACTIVATE);
        pump();
    }
    void render(DecodedFrame& frame, uint64_t index) {
        pump();
        if (WaitForSingleObject(latency_, 2000) != WAIT_OBJECT_0)
            throw std::runtime_error("Presentation queue wait timed out");
        ComPtr<ID3D11Texture2D> backbuffer;
        check(swapchain_->GetBuffer(0, IID_PPV_ARGS(&backbuffer)), "Get viewer backbuffer");
        D3D11_VIDEO_PROCESSOR_INPUT_VIEW_DESC input{};
        input.ViewDimension = D3D11_VPIV_DIMENSION_TEXTURE2D;
        input.Texture2D.ArraySlice = frame.subresource;
        ComPtr<ID3D11VideoProcessorInputView> source;
        check(video_->CreateVideoProcessorInputView(frame.texture.Get(), enumerator_.Get(), &input, &source), "Decoder texture input view");
        D3D11_VIDEO_PROCESSOR_OUTPUT_VIEW_DESC output{};
        output.ViewDimension = D3D11_VPOV_DIMENSION_TEXTURE2D;
        ComPtr<ID3D11VideoProcessorOutputView> destination;
        check(video_->CreateVideoProcessorOutputView(backbuffer.Get(), enumerator_.Get(), &output, &destination), "Backbuffer output view");
        D3D11_VIDEO_PROCESSOR_STREAM stream{};
        stream.Enable = TRUE;
        stream.pInputSurface = source.Get();
        check(video_context_->VideoProcessorBlt(processor_.Get(), destination.Get(), 0, 1, &stream), "GPU NV12 to swapchain");
        context_->End(finished_.Get());
        context_->Flush();
        const auto deadline = Clock::now() + std::chrono::seconds(2);
        BOOL done = FALSE;
        while (true) {
            const auto hr = context_->GetData(finished_.Get(), &done, sizeof(done), 0);
            check(hr, "GPU render completion");
            if (hr == S_OK && done) break;
            if (Clock::now() > deadline) throw std::runtime_error("GPU rendering timed out");
            std::this_thread::sleep_for(std::chrono::milliseconds(1));
        }
        // The decoded sample/array slice is still retained here. Only this
        // optional synthetic-only correctness mode reads pixels, four per frame.
        if (pixel_) {
            for (unsigned quadrant = 0; quadrant < 4; ++quadrant) {
                const UINT x = output_width_ * (1 + 2 * (quadrant % 2)) / 4;
                const UINT y = output_height_ * (1 + 2 * (quadrant / 2)) / 4;
                D3D11_BOX box{x, y, 0, x + 1, y + 1, 1};
                context_->CopySubresourceRegion(pixel_.Get(), 0, 0, 0, 0, backbuffer.Get(), 0, &box);
                D3D11_MAPPED_SUBRESOURCE mapped{};
                check(context_->Map(pixel_.Get(), 0, D3D11_MAP_READ, 0, &mapped), "Test pixel readback");
                const auto* bgra = static_cast<const uint8_t*>(mapped.pData);
                const std::array<unsigned, 3> actual{bgra[2], bgra[1], bgra[0]};
                const auto color = test_color(index, quadrant);
                unsigned error = 0;
                for (unsigned c = 0; c < 3; ++c)
                    error = (std::max)(error, static_cast<unsigned>(std::abs(int(bgra[2 - c]) - int(color[c] * 255 + .5f))));
                context_->Unmap(pixel_.Get(), 0);
                ++pixel_checks;
                max_color_error = (std::max)(max_color_error, error);
                if (error > 12) throw std::runtime_error("Decoded quadrant pixel mismatch: frame=" + std::to_string(index) +
                    " quadrant=" + std::to_string(quadrant) + " error=" + std::to_string(error) +
                    " RGB=" + std::to_string(actual[0]) + "," + std::to_string(actual[1]) + "," + std::to_string(actual[2]));
            }
        }
        const auto hr = swapchain_->Present(1, 0);
        if (hr == DXGI_STATUS_OCCLUDED) throw std::runtime_error("Viewer occluded; visible FPS cannot be measured");
        check(hr, "Present decoded frame");
        ++submitted;
        UINT present_id = 0;
        check(swapchain_->GetLastPresentCount(&present_id), "Get submitted present ID");
        // Confirm THIS present before submitting another. Counting Present calls
        // alone would hide dropped/queued frames. This intentionally serial
        // experiment bounds latency and holds at most one rendered frame.
        const auto display_deadline = Clock::now() + std::chrono::seconds(2);
        while (true) {
            pump();
            DXGI_FRAME_STATISTICS stats{};
            const auto result = swapchain_->GetFrameStatistics(&stats);
            if (result == DXGI_ERROR_FRAME_STATISTICS_DISJOINT) ++stats_disjoint;
            else {
                check(result, "Get actual presentation statistics");
                if (stats.PresentCount == present_id) { ++displayed; break; }
                if (stats.PresentCount > present_id) throw std::runtime_error("Unexpected presentation sequence");
            }
            if (Clock::now() > display_deadline)
                throw std::runtime_error("Frame display was not confirmed within two seconds");
            std::this_thread::sleep_for(std::chrono::milliseconds(1));
        }
    }
};
