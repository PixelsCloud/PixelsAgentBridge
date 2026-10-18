#include "wgc_capture.h"
#include <d3d11_4.h>
#include <windows.graphics.capture.interop.h>
#include <windows.graphics.directx.direct3d11.interop.h>
#include <winrt/Windows.Foundation.h>
#include <winrt/Windows.Graphics.Capture.h>
#include <winrt/Windows.Graphics.DirectX.Direct3D11.h>
#include <thread>

namespace capture = winrt::Windows::Graphics::Capture;
namespace {
[[noreturn]] void capture_error(const winrt::hresult_error& error) {
    check(error.code().value, "Windows Graphics Capture");
    throw std::runtime_error("Windows Graphics Capture failed");
}
}

struct WgcCapture::Impl {
    capture::GraphicsCaptureItem item{nullptr};
    capture::Direct3D11CaptureFramePool pool{nullptr};
    capture::GraphicsCaptureSession session{nullptr};
    capture::Direct3D11CaptureFrame frame{nullptr};
    ComPtr<ID3D11Device> device;
    ComPtr<ID3D11DeviceContext> context;
    ComPtr<ID3D11VideoDevice> video;
    ComPtr<ID3D11VideoContext> video_context;
    ComPtr<ID3D11VideoProcessorEnumerator> enumerator;
    ComPtr<ID3D11VideoProcessor> processor;
    ComPtr<ID3D11VideoProcessorOutputView> output;
    ComPtr<ID3D11Texture2D> rotated;
    ComPtr<ID3D11Query> finished;
    UINT desktop_width = 0, desktop_height = 0;

    ~Impl() {
        // Close each independently, including partially initialized sessions.
        try { if (frame) frame.Close(); } catch (...) {}
        try { if (session) session.Close(); } catch (...) {}
        try { if (pool) pool.Close(); } catch (...) {}
    }

    void initialize(ID3D11Device* source, const Display& display, unsigned fps) {
        if (!capture::GraphicsCaptureSession::IsSupported())
            throw std::runtime_error("Windows Graphics Capture unavailable");
        device = source;
        device->GetImmediateContext(&context);
        // Free-threaded capture and the encoder share this device/context.
        ComPtr<ID3D11Multithread> multithread;
        check(context.As(&multithread), "Capture multithread interface");
        multithread->SetMultithreadProtected(TRUE);
        ComPtr<IDXGIDevice> dxgi;
        check(device.As(&dxgi), "Capture DXGI device");
        winrt::Windows::Graphics::DirectX::Direct3D11::IDirect3DDevice rt_device{nullptr};
        check(CreateDirect3D11DeviceFromDXGIDevice(dxgi.Get(),
            reinterpret_cast<IInspectable**>(winrt::put_abi(rt_device))), "Capture WinRT device");
        const auto interop = winrt::get_activation_factory<capture::GraphicsCaptureItem, IGraphicsCaptureItemInterop>();
        check(interop->CreateForMonitor(display.screen.Monitor,
            winrt::guid_of<capture::GraphicsCaptureItem>(), winrt::put_abi(item)), "Capture monitor");
        const auto& rect = display.screen.DesktopCoordinates;
        desktop_width = rect.right - rect.left;
        desktop_height = rect.bottom - rect.top;
        if (item.Size().Width != desktop_width || item.Size().Height != desktop_height)
            throw std::runtime_error("Screen changed; refresh generation");
        pool = capture::Direct3D11CaptureFramePool::CreateFreeThreaded(rt_device,
            winrt::Windows::Graphics::DirectX::DirectXPixelFormat::B8G8R8A8UIntNormalized, 2, item.Size());
        session = pool.CreateCaptureSession(item);
        session.IsCursorCaptureEnabled(false); // Existing independent cursor stream.
        const auto rotation = display.screen.Rotation;
        if (rotation != DXGI_MODE_ROTATION_IDENTITY && rotation != DXGI_MODE_ROTATION_UNSPECIFIED) {
            // WGC is already desktop-oriented. Undo rotation on the GPU so the
            // existing stream geometry, viewer rotation and input stay intact.
            const UINT inverse = rotation == DXGI_MODE_ROTATION_ROTATE90 ? 3 :
                rotation == DXGI_MODE_ROTATION_ROTATE180 ? 2 :
                rotation == DXGI_MODE_ROTATION_ROTATE270 ? 1 : 0;
            if (!inverse) throw std::runtime_error("Invalid capture rotation");
            check(device.As(&video), "Capture rotation device");
            check(context.As(&video_context), "Capture rotation context");
            D3D11_VIDEO_PROCESSOR_CONTENT_DESC content{};
            content.InputFrameFormat = D3D11_VIDEO_FRAME_FORMAT_PROGRESSIVE;
            content.InputFrameRate = content.OutputFrameRate = {fps, 1};
            content.InputWidth = desktop_width; content.InputHeight = desktop_height;
            content.OutputWidth = inverse % 2 ? desktop_height : desktop_width;
            content.OutputHeight = inverse % 2 ? desktop_width : desktop_height;
            content.Usage = D3D11_VIDEO_USAGE_PLAYBACK_NORMAL;
            check(video->CreateVideoProcessorEnumerator(&content, &enumerator), "Capture rotation enumerator");
            UINT flags = 0;
            check(enumerator->CheckVideoProcessorFormat(DXGI_FORMAT_B8G8R8A8_UNORM, &flags), "Capture rotation format");
            const UINT required = D3D11_VIDEO_PROCESSOR_FORMAT_SUPPORT_INPUT | D3D11_VIDEO_PROCESSOR_FORMAT_SUPPORT_OUTPUT;
            D3D11_VIDEO_PROCESSOR_CAPS caps{};
            check(enumerator->GetVideoProcessorCaps(&caps), "Capture rotation capabilities");
            if ((flags & required) != required || !(caps.FeatureCaps & D3D11_VIDEO_PROCESSOR_FEATURE_CAPS_ROTATION))
                throw std::runtime_error("GPU capture rotation unavailable");
            check(video->CreateVideoProcessor(enumerator.Get(), 0, &processor), "Capture rotation processor");
            video_context->VideoProcessorSetStreamRotation(processor.Get(), 0, TRUE,
                static_cast<D3D11_VIDEO_PROCESSOR_ROTATION>(inverse));
            video_context->VideoProcessorSetStreamFrameFormat(processor.Get(), 0, D3D11_VIDEO_FRAME_FORMAT_PROGRESSIVE);
            video_context->VideoProcessorSetStreamAutoProcessingMode(processor.Get(), 0, FALSE);
            RECT source_rect{0, 0, LONG(desktop_width), LONG(desktop_height)};
            RECT destination{0, 0, LONG(content.OutputWidth), LONG(content.OutputHeight)};
            video_context->VideoProcessorSetStreamSourceRect(processor.Get(), 0, TRUE, &source_rect);
            video_context->VideoProcessorSetStreamDestRect(processor.Get(), 0, TRUE, &destination);
            video_context->VideoProcessorSetOutputTargetRect(processor.Get(), TRUE, &destination);
            D3D11_VIDEO_PROCESSOR_COLOR_SPACE rgb{}; rgb.RGB_Range = 0;
            video_context->VideoProcessorSetStreamColorSpace(processor.Get(), 0, &rgb);
            video_context->VideoProcessorSetOutputColorSpace(processor.Get(), &rgb);
            D3D11_TEXTURE2D_DESC desc{};
            desc.Width = content.OutputWidth; desc.Height = content.OutputHeight;
            desc.MipLevels = desc.ArraySize = desc.SampleDesc.Count = 1;
            desc.Format = DXGI_FORMAT_B8G8R8A8_UNORM;
            desc.BindFlags = D3D11_BIND_RENDER_TARGET | D3D11_BIND_SHADER_RESOURCE;
            check(device->CreateTexture2D(&desc, nullptr, &rotated), "Capture rotation texture");
            D3D11_VIDEO_PROCESSOR_OUTPUT_VIEW_DESC view{};
            view.ViewDimension = D3D11_VPOV_DIMENSION_TEXTURE2D;
            check(video->CreateVideoProcessorOutputView(rotated.Get(), enumerator.Get(), &view, &output), "Capture rotation output");
            D3D11_QUERY_DESC query{D3D11_QUERY_EVENT, 0};
            check(device->CreateQuery(&query, &finished), "Capture rotation completion");
        }
        session.StartCapture();
    }

    ComPtr<ID3D11Texture2D> next(unsigned timeout_ms) {
        if (frame) { frame.Close(); frame = nullptr; }
        const auto deadline = Clock::now() + std::chrono::milliseconds(timeout_ms);
        while (!(frame = pool.TryGetNextFrame())) {
            if (Clock::now() >= deadline) return {};
            Sleep(2);
        }
        // Consume at most the two-slot pool; never grow a queue or spin while
        // the producer keeps running. Prefer the newest available image after
        // the viewer pauses and resumes its consumption credits.
        for (unsigned i = 0; i < 2; ++i) {
            auto newer = pool.TryGetNextFrame();
            if (!newer) break;
            frame.Close(); frame = std::move(newer);
        }
        if (frame.ContentSize().Width != desktop_width || frame.ContentSize().Height != desktop_height)
            throw std::runtime_error("Screen changed; refresh generation");
        const auto access = frame.Surface().as<::Windows::Graphics::DirectX::Direct3D11::IDirect3DDxgiInterfaceAccess>();
        ComPtr<ID3D11Texture2D> texture;
        check(access->GetInterface(IID_PPV_ARGS(&texture)), "Capture frame texture");
        D3D11_TEXTURE2D_DESC desc{}; texture->GetDesc(&desc);
        if (desc.Width != desktop_width || desc.Height != desktop_height || desc.Format != DXGI_FORMAT_B8G8R8A8_UNORM)
            throw std::runtime_error("Capture surface changed; refresh generation");
        if (!processor) return texture; // No copy or pixel conversion for identity.
        D3D11_VIDEO_PROCESSOR_INPUT_VIEW_DESC view{};
        view.ViewDimension = D3D11_VPIV_DIMENSION_TEXTURE2D;
        ComPtr<ID3D11VideoProcessorInputView> input;
        check(video->CreateVideoProcessorInputView(texture.Get(), enumerator.Get(), &view, &input), "Capture rotation input");
        D3D11_VIDEO_PROCESSOR_STREAM stream{};
        stream.Enable = TRUE; stream.pInputSurface = input.Get();
        check(video_context->VideoProcessorBlt(processor.Get(), output.Get(), 0, 1, &stream), "GPU capture rotation");
        context->End(finished.Get()); context->Flush();
        const auto gpu_deadline = Clock::now() + std::chrono::seconds(2);
        for (;;) {
            BOOL complete = FALSE;
            const auto hr = context->GetData(finished.Get(), &complete, sizeof(complete), 0);
            check(hr, "Capture rotation completion");
            if (hr == S_OK && complete) break;
            if (Clock::now() >= gpu_deadline) throw std::runtime_error("GPU capture rotation timed out");
            Sleep(1);
        }
        return rotated;
    }
};

WgcRuntime::WgcRuntime() {
    try { winrt::init_apartment(winrt::apartment_type::multi_threaded); }
    catch (const winrt::hresult_error& error) { capture_error(error); }
}
WgcRuntime::~WgcRuntime() { winrt::uninit_apartment(); }

WgcCapture::WgcCapture(ID3D11Device* device, const Display& display, unsigned fps) {
    try {
        impl_ = std::make_unique<Impl>();
        impl_->initialize(device, display, fps);
    } catch (const winrt::hresult_error& error) { capture_error(error); }
}
WgcCapture::~WgcCapture() = default;
ComPtr<ID3D11Texture2D> WgcCapture::next(unsigned timeout_ms) {
    try { return impl_->next(timeout_ms); }
    catch (const winrt::hresult_error& error) { capture_error(error); }
}
