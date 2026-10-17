#pragma once
#include "mf_decoder.h"
#include <dxgi1_3.h>
#include <array>
#include <deque>
#include <thread>
#include "viewer_input.h"
#include "../software_codec_ffi.h"
#include "cursor_overlay.h"

// RGB test quadrants change every frame; differing corners detect channel,
// orientation, crop and stale-surface errors. Values avoid clipping extremes.
inline std::array<float, 4> test_color(uint64_t frame, unsigned quadrant) {
    return {float(24 + (frame * 3 + quadrant * 43) % 208) / 255.0f,
        float(24 + (frame * 7 + quadrant * 67) % 208) / 255.0f,
        float(24 + (frame * 11 + quadrant * 97) % 208) / 255.0f, 1.0f};
}

class NativePresenter {
    CursorOverlay cursor_;
    bool cursor_tracking_ = false;
    ViewerInput input_;
    static LRESULT CALLBACK window_proc(HWND window, UINT message, WPARAM wparam, LPARAM lparam) {
        auto* self = reinterpret_cast<NativePresenter*>(GetWindowLongPtrW(window, GWLP_USERDATA));
        if (message == WM_NCCREATE) {
            self = static_cast<NativePresenter*>(reinterpret_cast<CREATESTRUCTW*>(lparam)->lpCreateParams);
            SetWindowLongPtrW(window, GWLP_USERDATA, reinterpret_cast<LONG_PTR>(self));
        }
        if (self && message == WM_SETCURSOR && LOWORD(lparam) == HTCLIENT && self->refresh_local_cursor()) return TRUE;
        if (self && self->input_.message(window, message, wparam, lparam, self->viewport_)) return 0;
        return DefWindowProcW(window, message, wparam, lparam);
    }
    HWND window_ = nullptr;
    HWND parent_ = nullptr;
    DecodedFrame retained_;
    uint64_t retained_index_ = 0;
    bool was_minimized_ = false;
    bool occluded_ = false;
    bool presenting_ = false;
    // Present statistics can arrive several refresh intervals after submission
    // in a composed child window. Do not serialize decoding behind that delay.
    std::deque<std::pair<UINT, bool>> pending_presents_;
    RECT viewport_{};
    unsigned rotation_ = 0;
    HANDLE latency_ = nullptr;
    ComPtr<ID3D11Device> device_;
    ComPtr<ID3D11DeviceContext> context_;
    ComPtr<ID3D11VideoDevice> video_;
    ComPtr<ID3D11VideoContext> video_context_;
    ComPtr<ID3D11VideoProcessorEnumerator> enumerator_;
    ComPtr<ID3D11VideoProcessor> processor_;
    // Some video drivers round an NV12 source rectangle down to chroma
    // boundaries. Convert the entire coded surface before cropping odd visible
    // dimensions in RGB. This stays on the GPU and is absent for even sources.
    ComPtr<ID3D11Texture2D> crop_rgb_;
    ComPtr<ID3D11VideoProcessorEnumerator> crop_enumerator_;
    ComPtr<ID3D11VideoProcessor> crop_processor_;
    ComPtr<IDXGISwapChain2> swapchain_;
    ComPtr<ID3D11Query> finished_;
    ComPtr<ID3D11Texture2D> pixel_;
    UINT width_ = 0, height_ = 0;
    UINT output_width_ = 960, output_height_ = 540;
    bool refresh_local_cursor() {
        POINT point{};
        if (!GetCursorPos(&point) || WindowFromPoint(point) != window_) return false;
        ScreenToClient(window_, &point);
        const bool remote = cursor_tracking_ && input_.enabled() && PtInRect(&viewport_,point);
        SetCursor(remote ? nullptr : LoadCursorW(nullptr,IDC_ARROW));
        return true;
    }
public:
    void cursor_update(const std::vector<uint8_t>& packet) {
        cursor_.update(device_.Get(), packet);
        cursor_tracking_ = !packet.empty() && packet.front() == 1;
        refresh_local_cursor();
        // Repaint the retained sample without inventing a video presentation.
        if (retained_.sample) render(retained_, retained_index_, false);
    }
    void input_available(bool available) { input_.available = available; }
    void input_enabled(bool enabled) { input_.enable(window_, enabled); refresh_local_cursor(); }
    uint64_t submitted = 0, displayed = 0, pixel_checks = 0;
    unsigned max_color_error = 0;
    uint64_t stats_disjoint = 0;

    NativePresenter() = default;
    NativePresenter(const NativePresenter&) = delete;
    NativePresenter& operator=(const NativePresenter&) = delete;
    ~NativePresenter() {
        cursor_tracking_ = false;
        refresh_local_cursor();
        input_.release(window_);
        pixel_.Reset();
        processor_.Reset();
        enumerator_.Reset();
        swapchain_.Reset();
        if (latency_) CloseHandle(latency_);
        if (window_ && IsWindow(window_)) DestroyWindow(window_);
    }
    void observe_presentations() {
        if (pending_presents_.empty()) return;
        DXGI_FRAME_STATISTICS stats{};
        const auto result = swapchain_->GetFrameStatistics(&stats);
        if (result == DXGI_ERROR_FRAME_STATISTICS_DISJOINT) { ++stats_disjoint; return; }
        check(result, "Get actual presentation statistics");
        // Only count a specifically observed present ID. A skipped ID is not
        // evidence of a displayed frame; neither are Present calls or repaints.
        while (!pending_presents_.empty() && pending_presents_.front().first <= stats.PresentCount) {
            const auto present = pending_presents_.front();
            pending_presents_.pop_front();
            if (present.first == stats.PresentCount && present.second) ++displayed;
        }
    }
    void finish() {
        const auto deadline = Clock::now() + std::chrono::seconds(2);
        while (!pending_presents_.empty()) {
            pump();
            if (IsIconic(parent_ ? parent_ : window_) || occluded_) {
                pending_presents_.clear(); return;
            }
            if (Clock::now() > deadline) throw std::runtime_error("Final presentation was not confirmed");
            std::this_thread::sleep_for(std::chrono::milliseconds(1));
        }
    }
    void pump() {
        MSG message{};
        while (PeekMessageW(&message, nullptr, 0, 0, PM_REMOVE)) {
            TranslateMessage(&message);
            DispatchMessageW(&message);
        }
        if (!IsWindow(window_)) throw std::runtime_error("Viewer closed by user");
        if (parent_ && !IsWindow(parent_)) throw std::runtime_error("Viewer owner closed");
        observe_presentations();
        if (!swapchain_ || !processor_ || presenting_) return;
        if (parent_) {
            RECT bounds{}; GetClientRect(parent_, &bounds);
            const int top = MulDiv(72, GetDpiForWindow(parent_), 96);
            RECT position{}; GetWindowRect(window_, &position);
            POINT origin{position.left, position.top}; ScreenToClient(parent_, &origin);
            if (origin.y != top || position.right-position.left != bounds.right || position.bottom-position.top != (std::max)(1L, bounds.bottom-top))
                SetWindowPos(window_, HWND_TOP, 0, top, (std::max)(1L, bounds.right), (std::max)(1L, bounds.bottom-top), SWP_NOACTIVATE);
        }
        RECT client{}; GetClientRect(window_, &client);
        if (client.right <= 0 || client.bottom <= 0 || IsIconic(parent_ ? parent_ : window_)) { was_minimized_ = true; return; }
        bool repaint = was_minimized_;
        was_minimized_ = false;
        if (occluded_ && swapchain_->Present(0, DXGI_PRESENT_TEST) == S_OK) { occluded_ = false; repaint = true; }
        if (output_width_ != static_cast<UINT>(client.right) || output_height_ != static_cast<UINT>(client.bottom)) {
            output_width_ = client.right; output_height_ = client.bottom;
            check(swapchain_->ResizeBuffers(0, output_width_, output_height_, DXGI_FORMAT_UNKNOWN,
                DXGI_SWAP_CHAIN_FLAG_FRAME_LATENCY_WAITABLE_OBJECT), "Resize viewer swapchain");
            layout();
            repaint = true;
        }
        if (repaint && retained_.sample) render(retained_, retained_index_, false);
    }
    void layout() {
        const UINT display_width = rotation_ % 2 ? height_ : width_, display_height = rotation_ % 2 ? width_ : height_;
        const double scale = (std::min)(double(output_width_)/display_width, double(output_height_)/display_height);
        const LONG w = static_cast<LONG>(display_width*scale), h = static_cast<LONG>(display_height*scale);
        const LONG x = (output_width_-w)/2, y = (output_height_-h)/2;
        RECT destination{x,y,x+w,y+h}, target{0,0,static_cast<LONG>(output_width_),static_cast<LONG>(output_height_)};
        viewport_ = destination;
        video_context_->VideoProcessorSetStreamDestRect(processor_.Get(), 0, TRUE, &destination);
        video_context_->VideoProcessorSetOutputTargetRect(processor_.Get(), TRUE, &target);
        D3D11_VIDEO_COLOR black{}; black.RGBA.A = 1;
        video_context_->VideoProcessorSetOutputBackgroundColor(processor_.Get(), FALSE, &black);
    }
    void initialize(ID3D11Device* device, UINT width, UINT height, UINT fps,
                    const RECT& monitor, bool capture, bool verify, HWND parent = nullptr, unsigned rotation = 0) {
        parent_ = parent;
        rotation_ = rotation;
        device_ = device;
        width_ = width; height_ = height;
        device->GetImmediateContext(&context_);
        check(device_.As(&video_), "D3D11 video device");
        check(context_.As(&video_context_), "D3D11 video context");
        WNDCLASSW cls{};
        cls.lpfnWndProc = window_proc;
        cls.hInstance = GetModuleHandleW(nullptr);
        cls.hCursor = LoadCursorW(nullptr,IDC_ARROW);
        cls.lpszClassName = L"PixelsRemoteDesktopSurface";
        if (!RegisterClassW(&cls) && GetLastError() != ERROR_CLASS_ALREADY_EXISTS)
            throw std::runtime_error("Cannot register viewer window");
        const DWORD style = parent_ ? WS_CHILD : WS_OVERLAPPEDWINDOW;
        const DWORD extended = parent_ ? 0 : WS_EX_APPWINDOW;
        RECT bounds{0, 0, static_cast<LONG>(output_width_), static_cast<LONG>(output_height_)};
        if (!AdjustWindowRectEx(&bounds, style, FALSE, extended))
            throw std::runtime_error("Cannot compute viewer bounds");
        window_ = CreateWindowExW(extended,
            cls.lpszClassName, L"Pixels · Remote desktop", style,
            monitor.right - (bounds.right - bounds.left) - 32, monitor.bottom - (bounds.bottom - bounds.top) - 32,
            bounds.right - bounds.left, bounds.bottom - bounds.top,
            parent_, nullptr, cls.hInstance, this);
        if (!window_) throw std::runtime_error("Cannot create native viewer");
        // Avoid recursively capturing the test viewer in local desktop loopback.
        if (capture && !parent_ && !SetWindowDisplayAffinity(window_, WDA_EXCLUDEFROMCAPTURE))
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
        const PabVideoGeometry geometry(width, height);
        content.InputWidth = geometry.coded_width; content.InputHeight = geometry.coded_height;
        content.OutputWidth = output_width_; content.OutputHeight = output_height_;
        content.Usage = D3D11_VIDEO_USAGE_PLAYBACK_NORMAL;
        check(video_->CreateVideoProcessorEnumerator(&content, &enumerator_), "Create video processor enumerator");
        UINT flags = 0;
        check(enumerator_->CheckVideoProcessorFormat(DXGI_FORMAT_NV12, &flags), "Check NV12 processor input");
        if (!(flags & D3D11_VIDEO_PROCESSOR_FORMAT_SUPPORT_INPUT)) throw std::runtime_error("NV12 input unsupported");
        check(enumerator_->CheckVideoProcessorFormat(desc.Format, &flags), "Check BGRA processor output");
        if (!(flags & D3D11_VIDEO_PROCESSOR_FORMAT_SUPPORT_OUTPUT)) throw std::runtime_error("BGRA output unsupported");
        check(video_->CreateVideoProcessor(enumerator_.Get(), 0, &processor_), "Create video processor");
        if (rotation_) {
            D3D11_VIDEO_PROCESSOR_CAPS caps{};
            check(enumerator_->GetVideoProcessorCaps(&caps), "Get video processor capabilities");
            if (!(caps.FeatureCaps & D3D11_VIDEO_PROCESSOR_FEATURE_CAPS_ROTATION)) throw std::runtime_error("GPU video rotation unavailable");
        }
        video_context_->VideoProcessorSetStreamRotation(processor_.Get(), 0, rotation_ != 0,
            static_cast<D3D11_VIDEO_PROCESSOR_ROTATION>(rotation_));
        video_context_->VideoProcessorSetStreamFrameFormat(processor_.Get(), 0, D3D11_VIDEO_FRAME_FORMAT_PROGRESSIVE);
        video_context_->VideoProcessorSetStreamAutoProcessingMode(processor_.Get(), 0, FALSE);
        RECT source{0, 0, static_cast<LONG>(width), static_cast<LONG>(height)};
        RECT destination{0, 0, static_cast<LONG>(output_width_), static_cast<LONG>(output_height_)};
        video_context_->VideoProcessorSetStreamSourceRect(processor_.Get(), 0, TRUE, &source);
        video_context_->VideoProcessorSetStreamDestRect(processor_.Get(), 0, TRUE, &destination);
        video_context_->VideoProcessorSetOutputTargetRect(processor_.Get(), TRUE, &destination);
        layout();
        // NVENC's RGB-input conversion with explicit SDR VUI in this experiment is
        // validated as limited-range BT.709 by the synthetic corner checks.
        // This is NOT a general stream color-metadata negotiation policy.
        D3D11_VIDEO_PROCESSOR_COLOR_SPACE input_color{}, output_color{};
        input_color.YCbCr_Matrix = 1; // BT.709
        input_color.Nominal_Range = 1; // 16..235
        output_color.RGB_Range = 0; // full RGB
        video_context_->VideoProcessorSetStreamColorSpace(processor_.Get(), 0, &input_color);
        video_context_->VideoProcessorSetOutputColorSpace(processor_.Get(), &output_color);
        if (width != geometry.coded_width || height != geometry.coded_height) {
            check(enumerator_->CheckVideoProcessorFormat(desc.Format, &flags), "Check RGB crop input");
            if (!(flags & D3D11_VIDEO_PROCESSOR_FORMAT_SUPPORT_INPUT)) throw std::runtime_error("RGB crop input unsupported");
            D3D11_TEXTURE2D_DESC rgb{};
            rgb.Width = geometry.coded_width; rgb.Height = geometry.coded_height;
            rgb.MipLevels = rgb.ArraySize = rgb.SampleDesc.Count = 1;
            rgb.Format = desc.Format; rgb.Usage = D3D11_USAGE_DEFAULT;
            rgb.BindFlags = D3D11_BIND_RENDER_TARGET;
            check(device->CreateTexture2D(&rgb, nullptr, &crop_rgb_), "Create GPU crop surface");
            content.OutputWidth = content.InputWidth; content.OutputHeight = content.InputHeight;
            check(video_->CreateVideoProcessorEnumerator(&content, &crop_enumerator_), "Create crop conversion enumerator");
            check(video_->CreateVideoProcessor(crop_enumerator_.Get(), 0, &crop_processor_), "Create crop conversion processor");
            RECT coded{0, 0, static_cast<LONG>(geometry.coded_width), static_cast<LONG>(geometry.coded_height)};
            video_context_->VideoProcessorSetStreamFrameFormat(crop_processor_.Get(), 0, D3D11_VIDEO_FRAME_FORMAT_PROGRESSIVE);
            video_context_->VideoProcessorSetStreamAutoProcessingMode(crop_processor_.Get(), 0, FALSE);
            video_context_->VideoProcessorSetStreamSourceRect(crop_processor_.Get(), 0, TRUE, &coded);
            video_context_->VideoProcessorSetStreamDestRect(crop_processor_.Get(), 0, TRUE, &coded);
            video_context_->VideoProcessorSetOutputTargetRect(crop_processor_.Get(), TRUE, &coded);
            video_context_->VideoProcessorSetStreamColorSpace(crop_processor_.Get(), 0, &input_color);
            video_context_->VideoProcessorSetOutputColorSpace(crop_processor_.Get(), &output_color);
            video_context_->VideoProcessorSetStreamColorSpace(processor_.Get(), 0, &output_color);
        }
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
    void render(DecodedFrame& frame, uint64_t index, bool count = true) {
        pump();
        if (count) { retained_ = frame; retained_index_ = index; }
        // A minimized window still consumes decoded frames, but contributes no
        // displayed FPS. Holding only the latest decoder sample is bounded.
        if (IsIconic(parent_ ? parent_ : window_)) return;
        presenting_ = true;
        const auto queue_deadline = Clock::now() + std::chrono::seconds(2);
        while (pending_presents_.size() >= 3) {
            pump();
            if (IsIconic(parent_ ? parent_ : window_)) { presenting_ = false; return; }
            if (Clock::now() > queue_deadline) throw std::runtime_error("Presentation statistics stalled");
            std::this_thread::sleep_for(std::chrono::milliseconds(1));
        }
        if (WaitForSingleObject(latency_, 2000) != WAIT_OBJECT_0)
            throw std::runtime_error("Presentation queue wait timed out");
        ComPtr<ID3D11Texture2D> backbuffer;
        check(swapchain_->GetBuffer(0, IID_PPV_ARGS(&backbuffer)), "Get viewer backbuffer");
        D3D11_VIDEO_PROCESSOR_INPUT_VIEW_DESC input{};
        input.ViewDimension = D3D11_VPIV_DIMENSION_TEXTURE2D;
        input.Texture2D.ArraySlice = frame.subresource;
        if (crop_rgb_) {
            ComPtr<ID3D11VideoProcessorInputView> coded_source;
            check(video_->CreateVideoProcessorInputView(frame.texture.Get(), crop_enumerator_.Get(), &input, &coded_source), "Coded decoder input view");
            D3D11_VIDEO_PROCESSOR_OUTPUT_VIEW_DESC rgb_output{};
            rgb_output.ViewDimension = D3D11_VPOV_DIMENSION_TEXTURE2D;
            ComPtr<ID3D11VideoProcessorOutputView> rgb_destination;
            check(video_->CreateVideoProcessorOutputView(crop_rgb_.Get(), crop_enumerator_.Get(), &rgb_output, &rgb_destination), "GPU crop output view");
            D3D11_VIDEO_PROCESSOR_STREAM conversion{};
            conversion.Enable = TRUE; conversion.pInputSurface = coded_source.Get();
            check(video_context_->VideoProcessorBlt(crop_processor_.Get(), rgb_destination.Get(), 0, 1, &conversion), "Convert coded NV12 before RGB crop");
            input.Texture2D.ArraySlice = 0;
        }
        ComPtr<ID3D11VideoProcessorInputView> source;
        check(video_->CreateVideoProcessorInputView(crop_rgb_ ? crop_rgb_.Get() : frame.texture.Get(), enumerator_.Get(), &input, &source), "Decoder texture input view");
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
                const UINT x = viewport_.left + (viewport_.right-viewport_.left) * (1 + 2 * (quadrant % 2)) / 4;
                const UINT y = viewport_.top + (viewport_.bottom-viewport_.top) * (1 + 2 * (quadrant / 2)) / 4;
                D3D11_BOX box{x, y, 0, x + 1, y + 1, 1};
                context_->CopySubresourceRegion(pixel_.Get(), 0, 0, 0, 0, backbuffer.Get(), 0, &box);
                D3D11_MAPPED_SUBRESOURCE mapped{};
                check(context_->Map(pixel_.Get(), 0, D3D11_MAP_READ, 0, &mapped), "Test pixel readback");
                const auto* bgra = static_cast<const uint8_t*>(mapped.pData);
                const std::array<unsigned, 3> actual{bgra[2], bgra[1], bgra[0]};
                const unsigned xq = quadrant % 2, yq = quadrant / 2;
                const unsigned original = rotation_ == 1 ? yq + 2*(1-xq) : rotation_ == 2 ? 3-quadrant : rotation_ == 3 ? (1-yq) + 2*xq : quadrant;
                const auto color = test_color(index, original);
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
        cursor_.draw(device_.Get(), context_.Get(), backbuffer.Get(), viewport_,
            rotation_ % 2 ? height_ : width_, rotation_ % 2 ? width_ : height_);
        const auto hr = swapchain_->Present(1, 0);
        if (hr == DXGI_STATUS_OCCLUDED) { occluded_ = true; presenting_ = false; return; }
        check(hr, "Present decoded frame");
        // Resize may be requested while waiting for actual presentation. No
        // backbuffer/view reference may survive into that message pump.
        destination.Reset(); source.Reset(); backbuffer.Reset();
        if (count) ++submitted;
        UINT present_id = 0;
        check(swapchain_->GetLastPresentCount(&present_id), "Get submitted present ID");
        pending_presents_.emplace_back(present_id, count);
        observe_presentations();
        presenting_ = false;
    }
};
