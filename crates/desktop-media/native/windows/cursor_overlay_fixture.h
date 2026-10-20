#pragma once
// Generated GPU pixels only; no desktop capture or OS input.
static void cursor_overlay_check() {
    ComPtr<ID3D11Device> device; ComPtr<ID3D11DeviceContext> context;
    check(D3D11CreateDevice(nullptr,D3D_DRIVER_TYPE_HARDWARE,nullptr,D3D11_CREATE_DEVICE_BGRA_SUPPORT,
        nullptr,0,D3D11_SDK_VERSION,&device,nullptr,&context), "Cursor test device");
    constexpr UINT side = 256;
    const std::array<uint8_t,4> base{60,90,120,255};
    std::vector<uint8_t> background(side*side*4);
    for (size_t i=0;i<background.size();++i) background[i]=base[i%4];
    D3D11_TEXTURE2D_DESC desc{}; desc.Width=desc.Height=side;
    desc.MipLevels=desc.ArraySize=desc.SampleDesc.Count=1;
    desc.Format=DXGI_FORMAT_B8G8R8A8_UNORM; desc.Usage=D3D11_USAGE_DEFAULT; desc.BindFlags=D3D11_BIND_RENDER_TARGET;
    ComPtr<ID3D11Texture2D> output,readback;
    check(device->CreateTexture2D(&desc,nullptr,&output), "Cursor test target");
    desc.Usage=D3D11_USAGE_STAGING; desc.BindFlags=0; desc.CPUAccessFlags=D3D11_CPU_ACCESS_READ;
    check(device->CreateTexture2D(&desc,nullptr,&readback), "Cursor test readback");
    uint64_t pixel_checks=0; unsigned cases=0,maximum_error=0;
    for (uint8_t format : {uint8_t(1),uint8_t(2)}) {
        std::vector<uint8_t> shape;
        const uint8_t colors[]{0,0,128,128, 0,255,0,255, 0,0,0,0, 255,0,0,255,
            255,0,0,255, 0,0,0,0, 0,255,0,255, 0,0,128,128};
        if (format==1) shape.assign(std::begin(colors),std::end(colors));
        else for (unsigned p=0;p<8;++p) {
            const uint8_t a=p%4>=2?255:0, b=p%2?255:0;
            shape.insert(shape.end(),{a,a,a,0,b,b,b,0});
        }
        for (unsigned geometry=0;geometry<3;++geometry) {
            const UINT w=geometry==2?48:64,h=geometry==2?64:48;
            const double scale=geometry?1.5:1.0;
            const RECT viewport{16,24,16+LONG(w*scale),24+LONG(h*scale)};
            CursorOverlay overlay;
            for (unsigned phase=0;phase<5;++phase) {
                const int64_t x=phase==1?0:phase==2?INT32_MIN:int64_t(w/4), y=h/4;
                const bool visible=phase<3;
                std::vector<uint8_t> packet;
                const auto append=[&](auto value) {
                    const auto* bytes=reinterpret_cast<const uint8_t*>(&value);
                    packet.insert(packet.end(),bytes,bytes+sizeof(value));
                };
                if (phase==4) append(uint8_t(0));
                else {
                    append(uint8_t(1));append(uint8_t(visible));append(x);append(y);
                    append(uint32_t(w/2));append(uint32_t(h/2));append(uint8_t(phase==0?format:0));
                    if (!phase) {
                        for (uint32_t value : {4u,2u,1u,1u}) append(value);
                        packet.insert(packet.end(),shape.begin(),shape.end());
                    }
                }
                overlay.update(device.Get(),packet);
                context->UpdateSubresource(output.Get(),0,nullptr,background.data(),side*4,0);
                overlay.draw(device.Get(),context.Get(),output.Get(),viewport,w,h);
                context->CopyResource(readback.Get(),output.Get());
                D3D11_MAPPED_SUBRESOURCE mapped{};
                check(context->Map(readback.Get(),0,D3D11_MAP_READ,0,&mapped), "Read generated cursor pixels");
                const double left=viewport.left+double(x)*2*scale-scale,top=viewport.top+double(y)*2*scale-scale;
                unsigned errors=0;
                for (UINT py=0;py<side;++py) for(UINT px=0;px<side;++px) {
                    auto expected=base;
                    if (visible && px>=UINT(viewport.left) && px<UINT(viewport.right) && py>=UINT(viewport.top) && py<UINT(viewport.bottom)) {
                        const double sx=std::floor((px+.5-left)/scale),sy=std::floor((py+.5-top)/scale);
                        if (sx>=0 && sx<4 && sy>=0 && sy<2) {
                            const size_t offset=(size_t(sy)*4+size_t(sx))*(format==1?4:8);
                            for (unsigned c=0;c<3;++c) expected[c]=format==1?
                                uint8_t(shape[offset+c]+(unsigned(base[c])*(255-shape[offset+3])+127)/255):
                                uint8_t((base[c]&shape[offset+c])^shape[offset+4+c]);
                        }
                    }
                    const auto* actual=static_cast<const uint8_t*>(mapped.pData)+py*mapped.RowPitch+px*4;
                    for(unsigned c=0;c<4;++c) {
                        const auto difference=unsigned(std::abs(int(actual[c])-int(expected[c])));
                        maximum_error=(std::max)(maximum_error,difference);
                        errors+=difference>(format==1?1u:0u);
                    }
                    ++pixel_checks;
                }
                context->Unmap(readback.Get(),0);
                if(errors) throw std::runtime_error("Cursor GPU pixels mismatch: format="+std::to_string(format)+
                    " geometry="+std::to_string(geometry)+" phase="+std::to_string(phase)+" channels="+std::to_string(errors));
                ++cases;
            }
        }
    }
    std::cout << "cursor overlay cases=" << cases << " pixels=" << pixel_checks << " max_error=" << maximum_error << '\n';
}
