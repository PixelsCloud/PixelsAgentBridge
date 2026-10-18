// Opt-in, bounded diagnostic for the generated four-quadrant test window on
// a left-side 1080x1920 monitor. Exports four RGB samples, never a screenshot.
#include "../../crates/desktop-media/native/windows/dxgi_support.h"
#include <array>
#include <thread>

using Colors = std::array<std::array<unsigned,3>,4>;
static void colors(const Colors& values) {
    std::cout << '[';
    for (unsigned i=0;i<4;++i) std::cout<<(i?",":"")<<'['<<values[i][0]<<','<<values[i][1]<<','<<values[i][2]<<']';
    std::cout << ']';
}
static Colors gdi(const RECT& r) {
    HDC dc=GetDC(nullptr);if(!dc) throw std::runtime_error("GDI DC unavailable");
    Colors out{};
    for(unsigned i=0;i<4;++i) {
        const int x=r.left+(r.right-r.left)*(i%2?3:1)/4;
        const int y=r.top+(r.bottom-r.top)*(i/2?3:1)/4;
        const auto c=GetPixel(dc,x,y);
        if(c==CLR_INVALID) {ReleaseDC(nullptr,dc);throw std::runtime_error("GDI sample unavailable");}
        out[i]={GetRValue(c),GetGValue(c),GetBValue(c)};
    }
    ReleaseDC(nullptr,dc);return out;
}
static Colors texture_colors(ID3D11Device* device, ID3D11DeviceContext* context, ID3D11Texture2D* texture) {
    D3D11_TEXTURE2D_DESC desc{};texture->GetDesc(&desc);
    if(desc.Width!=1920 || desc.Height!=1080 || desc.Format!=DXGI_FORMAT_B8G8R8A8_UNORM)
        throw std::runtime_error("Unexpected diagnostic texture");
    desc.Usage=D3D11_USAGE_STAGING;desc.BindFlags=0;desc.CPUAccessFlags=D3D11_CPU_ACCESS_READ;desc.MiscFlags=0;
    ComPtr<ID3D11Texture2D> staging;check(device->CreateTexture2D(&desc,nullptr,&staging),"staging");
    context->CopyResource(staging.Get(),texture);
    D3D11_MAPPED_SUBRESOURCE mapped{};check(context->Map(staging.Get(),0,D3D11_MAP_READ,0,&mapped),"map");
    Colors out{};
    for(unsigned i=0;i<4;++i) {
        const unsigned x=desc.Width*(i%2?3:1)/4,y=desc.Height*(i/2?3:1)/4;
        const auto* pixel=static_cast<const uint8_t*>(mapped.pData)+size_t(y)*mapped.RowPitch+x*4;
        out[i]={pixel[2],pixel[1],pixel[0]};
    }
    context->Unmap(staging.Get(),0);return out;
}
int main(int argc,char** argv) {
    if(argc!=2 || std::string(argv[1])!="--check-generated") return 2;
    SetProcessDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2);
    try {
        auto found=displays();const Display* selected=nullptr;
        for(const auto& d:found) {
            const auto& r=d.screen.DesktopCoordinates;
            if(r.left==-1080 && r.top==0 && r.right==0 && r.bottom==1920
                && d.screen.Rotation==DXGI_MODE_ROTATION_ROTATE90) {
                if(selected) throw std::runtime_error("Ambiguous test display");selected=&d;
            }
        }
        if(!selected) throw std::runtime_error("Expected generated portrait display");
        const auto observed=gdi(selected->screen.DesktopCoordinates);
        std::cout<<"{\"backend\":\"gdi\",\"colors\":";colors(observed);std::cout<<"}\n"<<std::flush;
        const Colors expected{{{180,40,40},{40,180,40},{40,40,180},{180,180,40}}};
        if(observed!=expected) throw std::runtime_error("Generated surface not confirmed; stop before GPU readback");
        ComPtr<ID3D11Device> device;ComPtr<ID3D11DeviceContext> context;
        check(D3D11CreateDevice(selected->adapter.Get(),D3D_DRIVER_TYPE_UNKNOWN,nullptr,D3D11_CREATE_DEVICE_BGRA_SUPPORT,
            nullptr,0,D3D11_SDK_VERSION,&device,nullptr,&context),"device");
        for(unsigned attempt=0;attempt<4;++attempt) {
            ComPtr<IDXGIOutputDuplication> duplication;
            check(selected->output->DuplicateOutput(device.Get(),&duplication),"recreate duplication");
            for(unsigned frame=0;frame<3;++frame) {
                DXGI_OUTDUPL_FRAME_INFO info{};ComPtr<IDXGIResource> resource;
                const auto hr=duplication->AcquireNextFrame(500,&info,&resource);
                std::cout<<"{\"attempt\":"<<attempt<<",\"frame\":"<<frame<<",\"hresult\":"<<uint32_t(hr);
                if(SUCCEEDED(hr)) {
                    FrameLease lease{duplication.Get()};ComPtr<ID3D11Texture2D> texture;
                    check(resource.As(&texture),"texture");
                    std::cout<<",\"last_present\":"<<info.LastPresentTime.QuadPart<<",\"colors\":";
                    colors(texture_colors(device.Get(),context.Get(),texture.Get()));
                }
                std::cout<<"}\n"<<std::flush;
                if(hr==DXGI_ERROR_ACCESS_LOST) break;
                if(hr!=DXGI_ERROR_WAIT_TIMEOUT) check(hr,"AcquireNextFrame");
            }
            std::this_thread::sleep_for(std::chrono::milliseconds(250));
        }
        return 0; // Observation result, not a declaration that capture works.
    } catch(const std::exception& e) {std::cerr<<e.what()<<'\n';return 1;}
}
