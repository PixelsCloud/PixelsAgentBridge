#pragma once
#include "dxgi_support.h"
#include <d3dcompiler.h>
#include <cmath>
#include <cstring>
#include <algorithm>

// Shader compositing uses no desktop CPU readback. Normal shapes use hardware
// alpha blending; XOR copies only the clipped cursor rectangle on the GPU.
class CursorOverlay {
    bool visible_ = false;
    int64_t x_ = 0, y_ = 0;
    uint32_t desktop_width_ = 0, desktop_height_ = 0;
    uint32_t width_ = 0, height_ = 0, hotspot_x_ = 0, hotspot_y_ = 0;
    uint8_t format_ = 0;
    ComPtr<ID3D11ShaderResourceView> shape_;
    ComPtr<ID3D11VertexShader> vertex_;
    ComPtr<ID3D11PixelShader> pixel_;
    ComPtr<ID3D11Buffer> constants_;
    ComPtr<ID3D11RasterizerState> raster_;
    ComPtr<ID3D11BlendState> blend_;
    ComPtr<ID3D11Texture2D> background_;
    ComPtr<ID3D11ShaderResourceView> background_view_;
    UINT background_width_ = 0, background_height_ = 0;
    struct Constants {
        float x, y, inverse_x, inverse_y;
        uint32_t width, height, format, pad;
        int32_t background_x, background_y, pad2, pad3;
    };
    static_assert(sizeof(Constants) == 48);
    static constexpr const char* shader = R"(
cbuffer Parameters : register(b0) {
    float2 origin; float2 inverseScale;
    uint2 dimensions; uint format; uint pad;
    int2 backgroundOrigin; int2 pad2;
};
Texture2D<float4> Shape : register(t0);
Texture2D<float4> Background : register(t1);
float4 vs(uint id : SV_VertexID) : SV_POSITION {
    float2 p = id == 0 ? float2(-1,-1) : id == 1 ? float2(-1,3) : float2(3,-1);
    return float4(p,0,1);
}
float4 ps(float4 position : SV_POSITION) : SV_TARGET {
    int2 p = int2(floor((position.xy-origin)*inverseScale));
    if (any(p < 0) || any(p >= int2(dimensions))) discard;
    if (format == 1) return Shape.Load(int3(p,0)).bgra;
    uint3 mask = uint3(round(Shape.Load(int3(p.x*2,p.y,0)).bgr*255));
    uint3 value = uint3(round(Shape.Load(int3(p.x*2+1,p.y,0)).bgr*255));
    uint3 base = uint3(round(Background.Load(int3(int2(position.xy)-backgroundOrigin,0)).rgb*255));
    return float4(float3((base & mask) ^ value)/255,1);
}
)";
    void initialize(ID3D11Device* device) {
        if (vertex_) return;
        ComPtr<ID3DBlob> vertex, pixel, error;
        check(D3DCompile(shader, std::strlen(shader), nullptr, nullptr, nullptr, "vs", "vs_4_0",
            D3DCOMPILE_ENABLE_STRICTNESS, 0, &vertex, &error), "Compile cursor vertex shader");
        error.Reset();
        check(D3DCompile(shader, std::strlen(shader), nullptr, nullptr, nullptr, "ps", "ps_4_0",
            D3DCOMPILE_ENABLE_STRICTNESS, 0, &pixel, &error), "Compile cursor pixel shader");
        check(device->CreateVertexShader(vertex->GetBufferPointer(), vertex->GetBufferSize(), nullptr, &vertex_), "Create cursor vertex shader");
        check(device->CreatePixelShader(pixel->GetBufferPointer(), pixel->GetBufferSize(), nullptr, &pixel_), "Create cursor pixel shader");
        D3D11_BUFFER_DESC buffer{}; buffer.ByteWidth = sizeof(Constants);
        buffer.Usage = D3D11_USAGE_DEFAULT; buffer.BindFlags = D3D11_BIND_CONSTANT_BUFFER;
        check(device->CreateBuffer(&buffer, nullptr, &constants_), "Create cursor parameters");
        D3D11_RASTERIZER_DESC raster{}; raster.FillMode = D3D11_FILL_SOLID;
        raster.CullMode = D3D11_CULL_NONE; raster.ScissorEnable = TRUE; raster.DepthClipEnable = TRUE;
        check(device->CreateRasterizerState(&raster, &raster_), "Create cursor raster state");
        D3D11_BLEND_DESC blend{}; auto& target = blend.RenderTarget[0];
        target.BlendEnable = TRUE; target.SrcBlend = D3D11_BLEND_ONE;
        target.DestBlend = D3D11_BLEND_INV_SRC_ALPHA; target.BlendOp = D3D11_BLEND_OP_ADD;
        target.SrcBlendAlpha = D3D11_BLEND_ONE; target.DestBlendAlpha = D3D11_BLEND_INV_SRC_ALPHA;
        target.BlendOpAlpha = D3D11_BLEND_OP_ADD; target.RenderTargetWriteMask = D3D11_COLOR_WRITE_ENABLE_ALL;
        check(device->CreateBlendState(&blend, &blend_), "Create cursor alpha blend");
    }
public:
    void hide() { visible_ = false; }
    // Wire payload is private to a single viewer process; session/generation
    // and cursor image references were validated by the Rust sender.
    void update(ID3D11Device* device, const std::vector<uint8_t>& packet) {
        size_t offset = 0;
        const auto read = [&](auto& value) {
            if (sizeof(value) > packet.size() - offset) throw std::runtime_error("Truncated cursor command");
            std::memcpy(&value, packet.data()+offset, sizeof(value)); offset += sizeof(value);
        };
        uint8_t action = 0; read(action);
        if (!action) {
            if (offset != packet.size()) throw std::runtime_error("Invalid cursor clear");
            hide(); return;
        }
        if (action != 1) throw std::runtime_error("Invalid cursor operation");
        uint8_t visible = 0, format = 0; int64_t x = 0, y = 0;
        uint32_t desktop_width = 0, desktop_height = 0;
        read(visible); read(x); read(y); read(desktop_width); read(desktop_height); read(format);
        if (visible > 1 || !desktop_width || !desktop_height || desktop_width > 8192 || desktop_height > 8192 ||
            x < -INT64_C(4294967295) || x > INT64_C(4294967295) || y < -INT64_C(4294967295) || y > INT64_C(4294967295))
            throw std::runtime_error("Invalid cursor coordinates");
        if (format) {
            uint32_t width, height, hotspot_x, hotspot_y;
            read(width); read(height); read(hotspot_x); read(hotspot_y);
            if (format > 2 || !width || !height || width > 512 || height > 512 || hotspot_x >= width || hotspot_y >= height ||
                packet.size()-offset != size_t(width)*height*(format == 1 ? 4 : 8))
                throw std::runtime_error("Invalid cursor image command");
            D3D11_TEXTURE2D_DESC texture{}; texture.Width = width*(format == 1 ? 1 : 2); texture.Height = height;
            texture.MipLevels = texture.ArraySize = texture.SampleDesc.Count = 1;
            texture.Format = DXGI_FORMAT_R8G8B8A8_UNORM; texture.Usage = D3D11_USAGE_IMMUTABLE;
            texture.BindFlags = D3D11_BIND_SHADER_RESOURCE;
            D3D11_SUBRESOURCE_DATA initial{packet.data()+offset, texture.Width*4, 0};
            ComPtr<ID3D11Texture2D> image; ComPtr<ID3D11ShaderResourceView> view;
            check(device->CreateTexture2D(&texture, &initial, &image), "Upload cursor image");
            check(device->CreateShaderResourceView(image.Get(), nullptr, &view), "Create cursor image view");
            shape_ = view; format_ = format; width_ = width; height_ = height;
            hotspot_x_ = hotspot_x; hotspot_y_ = hotspot_y;
        } else if (offset != packet.size()) throw std::runtime_error("Unexpected cursor image bytes");
        if (visible && !shape_) throw std::runtime_error("Visible cursor missing image");
        x_ = x; y_ = y; desktop_width_ = desktop_width; desktop_height_ = desktop_height; visible_ = visible != 0;
    }
    void draw(ID3D11Device* device, ID3D11DeviceContext* context, ID3D11Texture2D* output,
        const RECT& viewport, UINT display_width, UINT display_height) {
        if (!visible_ || !shape_) return;
        const double scale_x = double(viewport.right-viewport.left)/display_width;
        const double scale_y = double(viewport.bottom-viewport.top)/display_height;
        if (scale_x <= 0 || scale_y <= 0) return;
        const double left = viewport.left + double(x_)*(viewport.right-viewport.left)/desktop_width_ - hotspot_x_*scale_x;
        const double top = viewport.top + double(y_)*(viewport.bottom-viewport.top)/desktop_height_ - hotspot_y_*scale_y;
        const double right = left + width_*scale_x, bottom = top + height_*scale_y;
        // Clip in double precision BEFORE converting possibly off-screen i32
        // desktop coordinates to LONGs; no overflow and no letterbox drawing.
        if (right <= viewport.left || left >= viewport.right || bottom <= viewport.top || top >= viewport.bottom) return;
        RECT clip{static_cast<LONG>((std::max)(double(viewport.left),std::floor(left))),
            static_cast<LONG>((std::max)(double(viewport.top),std::floor(top))),
            static_cast<LONG>((std::min)(double(viewport.right),std::ceil(right))),
            static_cast<LONG>((std::min)(double(viewport.bottom),std::ceil(bottom)))};
        if (clip.right <= clip.left || clip.bottom <= clip.top) return;
        initialize(device);
        D3D11_TEXTURE2D_DESC desc{}; output->GetDesc(&desc);
        if (format_ == 2) {
            const UINT w = clip.right-clip.left, h = clip.bottom-clip.top;
            if (!background_ || w != background_width_ || h != background_height_) {
                background_view_.Reset(); background_.Reset();
                D3D11_TEXTURE2D_DESC copy{}; copy.Width = w; copy.Height = h;
                copy.MipLevels = copy.ArraySize = copy.SampleDesc.Count = 1;
                copy.Format = desc.Format; copy.Usage = D3D11_USAGE_DEFAULT; copy.BindFlags = D3D11_BIND_SHADER_RESOURCE;
                check(device->CreateTexture2D(&copy,nullptr,&background_), "Create cursor background region");
                check(device->CreateShaderResourceView(background_.Get(),nullptr,&background_view_), "Create cursor background view");
                background_width_ = w; background_height_ = h;
            }
            const D3D11_BOX box{UINT(clip.left),UINT(clip.top),0,UINT(clip.right),UINT(clip.bottom),1};
            context->CopySubresourceRegion(background_.Get(),0,0,0,0,output,0,&box);
        }
        const Constants parameters{float(left),float(top),float(1/scale_x),float(1/scale_y),
            width_,height_,format_,0,clip.left,clip.top,0,0};
        context->UpdateSubresource(constants_.Get(),0,nullptr,&parameters,0,0);
        ComPtr<ID3D11RenderTargetView> target;
        check(device->CreateRenderTargetView(output,nullptr,&target), "Create cursor target");
        ID3D11RenderTargetView* targets[]{target.Get()};
        ID3D11ShaderResourceView* textures[]{shape_.Get(),format_ == 2 ? background_view_.Get() : nullptr};
        ID3D11Buffer* constants[]{constants_.Get()};
        const D3D11_VIEWPORT full{0,0,float(desc.Width),float(desc.Height),0,1};
        context->OMSetRenderTargets(1,targets,nullptr);
        context->OMSetBlendState(format_ == 1 ? blend_.Get() : nullptr,nullptr,0xffffffff);
        context->RSSetViewports(1,&full); context->RSSetState(raster_.Get()); context->RSSetScissorRects(1,&clip);
        context->IASetInputLayout(nullptr); context->IASetPrimitiveTopology(D3D11_PRIMITIVE_TOPOLOGY_TRIANGLELIST);
        context->VSSetShader(vertex_.Get(),nullptr,0); context->PSSetShader(pixel_.Get(),nullptr,0);
        context->PSSetConstantBuffers(0,1,constants); context->PSSetShaderResources(0,2,textures);
        context->Draw(3,0);
        // A backbuffer reference must never survive into ResizeBuffers, and
        // the video processor must not encounter a still-bound output/SRV.
        ID3D11ShaderResourceView* empty[]{nullptr,nullptr};
        context->PSSetShaderResources(0,2,empty); context->OMSetRenderTargets(0,nullptr,nullptr);
        context->OMSetBlendState(nullptr,nullptr,0xffffffff); context->RSSetState(nullptr);
        context->VSSetShader(nullptr,nullptr,0); context->PSSetShader(nullptr,nullptr,0);
    }
};
