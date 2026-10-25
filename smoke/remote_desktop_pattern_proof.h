#pragma once
// Explicit smoke-only diagnostic. Reads synthetic pixels only when the caller
// selects desktop-proof/stream-proof; normal capture/render does no readback.
#include <array>
#include <fstream>
#include <iomanip>
#include <sstream>

namespace pattern_proof {
using RGB = std::array<unsigned, 3>;
constexpr unsigned columns = 32, rows = 18, samples = columns * rows;
inline RGB color(unsigned frame, unsigned x, unsigned y) {
    if (y == 0 || y == rows-1) {
        const unsigned bit = ((frame >> (x % 16)) & 1) ^ (x >= 16 ? 1 : 0);
        const unsigned v = bit ? 224 : 32;
        return {v,v,v};
    }
    return {32+(frame*3+x*29+y*17)%192,32+(frame*5+x*11+y*31)%192,32+(frame*7+x*37+y*13)%192};
}
struct Result { unsigned pattern = 0, max_error = 0; };
template<class Pixel> Result inspect(Pixel pixel, const char* stage, unsigned tolerance) {
    Result result;
    for (unsigned x = 0; x < 16; ++x) {
        const auto p = pixel(x,0);
        if ((p[0]+p[1]+p[2])/3 > 128) result.pattern |= 1u << x;
    }
    for (unsigned y = 0; y < rows; ++y) for (unsigned x = 0; x < columns; ++x) {
        const auto actual = pixel(x,y), expected = color(result.pattern,x,y);
        unsigned error = 0;
        for (unsigned c=0; c<3; ++c) error=(std::max)(error, unsigned(std::abs(int(actual[c])-int(expected[c]))));
        result.max_error=(std::max)(result.max_error,error);
        if (error > tolerance) throw std::runtime_error(std::string("Synthetic pattern mismatch stage=")+stage+
            " pattern="+std::to_string(result.pattern)+" cell="+std::to_string(x)+","+std::to_string(y)+" error="+std::to_string(error));
    }
    return result;
}
inline uint64_t fingerprint(const std::vector<uint8_t>& data) {
    uint64_t hash=14695981039346656037ull;
    for (auto b:data) { hash^=b; hash*=1099511628211ull; }
    return hash; // Noncryptographic equality diagnostic, never authentication.
}
inline void fixture() {
    for (unsigned frame : {0u,1u,127u,32768u,65535u}) {
        const auto valid=[&](unsigned x,unsigned y){return color(frame,x,y);};
        if (inspect(valid,"fixture",0).pattern != frame) throw std::runtime_error("Pattern counter mismatch");
        for (unsigned defect=0; defect<3; ++defect) {
            bool rejected=false;
            try { inspect([&](unsigned x,unsigned y){
                if (defect==0 && x==3 && y==7) return RGB{255,0,255};
                if (defect==1 && y>=9) return color(frame^1,x,y);
                if (defect==2 && y==0 && x==0) return color(frame^1,x,y);
                return valid(x,y);
            },"negative-fixture",12); } catch (const std::exception&) { rejected=true; }
            if (!rejected) throw std::runtime_error("Pattern defect was not detected");
        }
    }
}
class GPU {
    ComPtr<ID3D11Device> device_;
    ComPtr<ID3D11DeviceContext> context_;
    ComPtr<ID3D11Texture2D> staging_;
public:
    GPU(ID3D11Device* device, ID3D11DeviceContext* context):device_(device),context_(context) {}
    Result inspect_texture(ID3D11Texture2D* source, UINT slice, RECT viewport, const char* stage, unsigned tolerance) {
        D3D11_TEXTURE2D_DESC desc{}; source->GetDesc(&desc);
        if ((desc.Format != DXGI_FORMAT_NV12 && desc.Format != DXGI_FORMAT_B8G8R8A8_UNORM) || desc.SampleDesc.Count!=1 || desc.MipLevels!=1)
            throw std::runtime_error("Unsupported proof texture");
        D3D11_TEXTURE2D_DESC old{};
        if(staging_) staging_->GetDesc(&old);
        if (!staging_ || old.Width!=desc.Width || old.Height!=desc.Height || old.Format!=desc.Format) {
            staging_.Reset();
            auto staging=desc; staging.ArraySize=1; staging.BindFlags=staging.MiscFlags=0;
            staging.Usage=D3D11_USAGE_STAGING; staging.CPUAccessFlags=D3D11_CPU_ACCESS_READ;
            check(device_->CreateTexture2D(&staging,nullptr,&staging_),"Create synthetic proof readback");
        }
        context_->CopySubresourceRegion(staging_.Get(),0,0,0,0,source,slice,nullptr);
        D3D11_MAPPED_SUBRESOURCE mapped{};
        check(context_->Map(staging_.Get(),0,D3D11_MAP_READ,0,&mapped),"Read synthetic proof texture");
        try {
            auto result=inspect([&](unsigned gx,unsigned gy)->RGB {
                const unsigned x=viewport.left+uint64_t(viewport.right-viewport.left)*(2*gx+1)/(2*columns);
                const unsigned y=viewport.top+uint64_t(viewport.bottom-viewport.top)*(2*gy+1)/(2*rows);
                const auto* bytes=static_cast<const uint8_t*>(mapped.pData);
                if(desc.Format==DXGI_FORMAT_B8G8R8A8_UNORM) {
                    const auto* p=bytes+y*mapped.RowPitch+x*4; return {p[2],p[1],p[0]};
                }
                const double l=1.16438356*(int(bytes[y*mapped.RowPitch+x])-16);
                const auto* uv=bytes+desc.Height*mapped.RowPitch+(y/2)*mapped.RowPitch+(x/2)*2;
                const int u=int(uv[0])-128,v=int(uv[1])-128;
                const auto clip=[](double n){return unsigned((std::max)(0,(std::min)(255,int(n+.5))));};
                return {clip(l+1.79274107*v),clip(l-.21324861*u-.53290933*v),clip(l+2.11240179*u)};
            },stage,tolerance);
            context_->Unmap(staging_.Get(),0); return result;
        } catch (...) { context_->Unmap(staging_.Get(),0); throw; }
    }
};
}
