#pragma once
#include "dxgi_support.h"

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
