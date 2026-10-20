#pragma once
#include "dxgi_support.h"
#include <memory>

// Must outlive the D3D device shared with capture. In particular a rejected
// capture must not tear down COM while the caller still owns that device.
class WgcRuntime {
public:
    WgcRuntime();
    ~WgcRuntime();
    WgcRuntime(const WgcRuntime&) = delete;
    WgcRuntime& operator=(const WgcRuntime&) = delete;
};

// Optional GPU capture after DXGI loses an otherwise unchanged output. WinRT
// remains in a separate translation unit; the worker keeps its C++17 ABI.
class WgcCapture {
    struct Impl;
    std::unique_ptr<Impl> impl_;
public:
    // Caller holds WgcRuntime on this thread through device destruction.
    WgcCapture(ID3D11Device* device, const Display& display, unsigned fps);
    ~WgcCapture();
    WgcCapture(const WgcCapture&) = delete;
    WgcCapture& operator=(const WgcCapture&) = delete;
    // Null on timeout. The frame remains leased through the encoder's use,
    // until the next call or destruction. Output uses DXGI's native orientation.
    ComPtr<ID3D11Texture2D> next(unsigned timeout_ms);
};
