// Explicit metadata-only diagnostic: no pixel readback, encoding or input.
#include "../../crates/desktop-media/native/windows/dxgi_support.h"
int main(int argc, char** argv) {
    const bool video = argc==2 && std::string(argv[1])=="--probe-video";
    const bool sequence = argc==2 && std::string(argv[1])=="--probe-sequence";
    if (argc!=2 || (std::string(argv[1])!="--probe" && !video && !sequence)) return 2;
    SetProcessDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2);
    try {
        for (const auto& display : displays()) {
            if (sequence && display.screen.DesktopCoordinates.left>=0) continue;
            ComPtr<ID3D11Device> device; ComPtr<ID3D11DeviceContext> context;
            check(D3D11CreateDevice(display.adapter.Get(),D3D_DRIVER_TYPE_UNKNOWN,nullptr,
                D3D11_CREATE_DEVICE_BGRA_SUPPORT | (video ? D3D11_CREATE_DEVICE_VIDEO_SUPPORT : 0),
                nullptr,0,D3D11_SDK_VERSION,&device,nullptr,&context),"device");
            ComPtr<IDXGIOutputDuplication> duplication;
            check(display.output->DuplicateOutput(device.Get(),&duplication),"DuplicateOutput");
            DXGI_OUTDUPL_DESC desc{};duplication->GetDesc(&desc);
            for (unsigned sample=0;sample<(sequence?3u:1u);++sample) {
            DXGI_OUTDUPL_FRAME_INFO info{};ComPtr<IDXGIResource> resource;
            check(duplication->AcquireNextFrame(3000,&info,&resource),"AcquireNextFrame");
            FrameLease lease{duplication.Get()};ComPtr<ID3D11Texture2D> texture;
            check(resource.As(&texture),"texture");D3D11_TEXTURE2D_DESC frame{};texture->GetDesc(&frame);
            const auto& r=display.screen.DesktopCoordinates;
            std::cout<<"{\"sample\":"<<sample<<",\"name\":"<<quoted(display.screen.DeviceName)
                <<",\"x\":"<<r.left<<",\"y\":"<<r.top<<",\"desktop_width\":"<<r.right-r.left
                <<",\"desktop_height\":"<<r.bottom-r.top<<",\"output_rotation\":"<<display.screen.Rotation
                <<",\"mode_width\":"<<desc.ModeDesc.Width<<",\"mode_height\":"<<desc.ModeDesc.Height
                <<",\"duplication_rotation\":"<<desc.Rotation<<",\"texture_width\":"<<frame.Width
                <<",\"texture_height\":"<<frame.Height<<",\"last_present\":"<<info.LastPresentTime.QuadPart
                <<",\"last_mouse_update\":"<<info.LastMouseUpdateTime.QuadPart
                <<",\"accumulated_frames\":"<<info.AccumulatedFrames<<"}\n"<<std::flush;
            }
        }
        return 0;
    } catch (const std::exception& e) {std::cerr<<e.what()<<'\n';return 1;}
}
