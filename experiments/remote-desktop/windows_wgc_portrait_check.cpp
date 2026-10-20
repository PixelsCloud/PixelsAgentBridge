// Explicit generated-window diagnostic. The capture surface stays on the GPU;
// only this test maps it to verify four pixels. No desktop image is exported.
#include "../../crates/desktop-media/native/windows/dxgi_support.h"
#include <windows.graphics.capture.interop.h>
#include <windows.graphics.directx.direct3d11.interop.h>
#include <winrt/Windows.Foundation.h>
#include <winrt/Windows.Graphics.Capture.h>
#include <winrt/Windows.Graphics.DirectX.Direct3D11.h>
#include <thread>

namespace capture=winrt::Windows::Graphics::Capture;
struct CaptureLease {
    capture::Direct3D11CaptureFramePool pool{nullptr};
    capture::GraphicsCaptureSession session{nullptr};
    ~CaptureLease() {try {if(session) session.Close();if(pool) pool.Close();}catch(...) {}}
};
int main(int argc,char** argv) {
    if(argc!=2 || std::string(argv[1])!="--check-generated") return 2;
    SetProcessDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2);
    try {
        winrt::init_apartment(winrt::apartment_type::multi_threaded);
        if(!capture::GraphicsCaptureSession::IsSupported()) throw std::runtime_error("WGC not supported");
        auto found=displays();const Display* selected=nullptr;
        for(const auto& d:found) {
            const auto& r=d.screen.DesktopCoordinates;
            if(r.left==-1080 && r.top==0 && r.right==0 && r.bottom==1920) {
                if(selected) throw std::runtime_error("Ambiguous test display");selected=&d;
            }
        }
        if(!selected) throw std::runtime_error("Expected generated portrait display");
        const COLORREF expected[]={RGB(180,40,40),RGB(40,180,40),RGB(40,40,180),RGB(180,180,40)};
        HDC dc=GetDC(nullptr);if(!dc) throw std::runtime_error("GDI DC unavailable");
        bool generated=true;
        for(unsigned i=0;i<4;++i) generated &= GetPixel(dc,-1080+1080*(i%2?3:1)/4,1920*(i/2?3:1)/4)==expected[i];
        ReleaseDC(nullptr,dc);if(!generated) throw std::runtime_error("Generated surface not confirmed");
        ComPtr<ID3D11Device> device;ComPtr<ID3D11DeviceContext> context;
        check(D3D11CreateDevice(selected->adapter.Get(),D3D_DRIVER_TYPE_UNKNOWN,nullptr,D3D11_CREATE_DEVICE_BGRA_SUPPORT,
            nullptr,0,D3D11_SDK_VERSION,&device,nullptr,&context),"device");
        ComPtr<IDXGIDevice> dxgi;check(device.As(&dxgi),"DXGI device");
        winrt::Windows::Graphics::DirectX::Direct3D11::IDirect3DDevice rt_device{nullptr};
        check(CreateDirect3D11DeviceFromDXGIDevice(dxgi.Get(),reinterpret_cast<IInspectable**>(winrt::put_abi(rt_device))),"WinRT device");
        const auto interop=winrt::get_activation_factory<capture::GraphicsCaptureItem,IGraphicsCaptureItemInterop>();
        capture::GraphicsCaptureItem item{nullptr};
        check(interop->CreateForMonitor(selected->screen.Monitor,winrt::guid_of<capture::GraphicsCaptureItem>(),winrt::put_abi(item)),"capture item");
        CaptureLease lease;
        lease.pool=capture::Direct3D11CaptureFramePool::CreateFreeThreaded(rt_device,
            winrt::Windows::Graphics::DirectX::DirectXPixelFormat::B8G8R8A8UIntNormalized,2,item.Size());
        lease.session=lease.pool.CreateCaptureSession(item);lease.session.IsCursorCaptureEnabled(false);lease.session.StartCapture();
        const auto deadline=Clock::now()+std::chrono::seconds(5);
        capture::Direct3D11CaptureFrame frame{nullptr};
        while(!(frame=lease.pool.TryGetNextFrame())) {
            if(Clock::now()>=deadline) throw std::runtime_error("WGC frame timeout");
            std::this_thread::sleep_for(std::chrono::milliseconds(10));
        }
        const auto access=frame.Surface().as<::Windows::Graphics::DirectX::Direct3D11::IDirect3DDxgiInterfaceAccess>();
        ComPtr<ID3D11Texture2D> texture;check(access->GetInterface(IID_PPV_ARGS(&texture)),"WGC texture");
        D3D11_TEXTURE2D_DESC desc{};texture->GetDesc(&desc);
        if(desc.Width!=1080 || desc.Height!=1920 || frame.ContentSize().Width!=1080 || frame.ContentSize().Height!=1920)
            throw std::runtime_error("WGC dimensions changed");
        desc.Usage=D3D11_USAGE_STAGING;desc.BindFlags=0;desc.CPUAccessFlags=D3D11_CPU_ACCESS_READ;desc.MiscFlags=0;
        ComPtr<ID3D11Texture2D> staging;check(device->CreateTexture2D(&desc,nullptr,&staging),"staging");
        context->CopyResource(staging.Get(),texture.Get());
        D3D11_MAPPED_SUBRESOURCE mapped{};check(context->Map(staging.Get(),0,D3D11_MAP_READ,0,&mapped),"map");
        bool correct=true;std::cout<<"{\"width\":"<<desc.Width<<",\"height\":"<<desc.Height<<",\"colors\":[";
        for(unsigned i=0;i<4;++i) {
            const auto* p=static_cast<const uint8_t*>(mapped.pData)+size_t(1920*(i/2?3:1)/4)*mapped.RowPitch+(1080*(i%2?3:1)/4)*4;
            std::cout<<(i?",":"")<<'['<<unsigned(p[2])<<','<<unsigned(p[1])<<','<<unsigned(p[0])<<']';
            correct &= RGB(p[2],p[1],p[0])==expected[i];
        }
        context->Unmap(staging.Get(),0);frame.Close();
        std::cout<<"],\"correct\":"<<(correct?"true":"false")<<"}\n"<<std::flush;
        return correct?0:1;
    } catch(const winrt::hresult_error& e) {
        std::cerr<<"WGC HRESULT=0x"<<std::hex<<uint32_t(e.code().value)<<'\n';return 1;
    } catch(const std::exception& e) {std::cerr<<e.what()<<'\n';return 1;}
}
