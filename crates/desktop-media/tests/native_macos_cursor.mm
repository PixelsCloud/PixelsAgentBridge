// Opt-in real Metal pixel comparison; generated textures only, no capture,
// window, input injection, permissions or changes to installed applications.
#include <cursor_overlay.h>
#include <video_shader.h>
#include <array>

static void require(bool value, const char* message) {
    if (!value) throw std::runtime_error(message);
}
template<class T> static void append(std::vector<uint8_t>& out, T value) {
    const auto* bytes = reinterpret_cast<const uint8_t*>(&value);
    out.insert(out.end(),bytes,bytes+sizeof(value));
}
static std::vector<uint8_t> packet(int64_t x, int64_t y, unsigned format, bool visible, bool image) {
    std::vector<uint8_t> out{1,uint8_t(visible)};
    append(out,x); append(out,y); append(out,uint32_t(320)); append(out,uint32_t(240));
    out.push_back(image ? format : 0);
    if (!image) return out;
    for (uint32_t value : {17,13,5,7}) append(out,value);
    for (unsigned y=0;y<13;++y) for (unsigned x=0;x<17;++x) {
        if (format == 1) {
            const uint8_t alpha = std::array<uint8_t,4>{0,80,160,255}[(x+y)%4];
            for (uint8_t value : {uint8_t(alpha/4),uint8_t(alpha/2),alpha,alpha}) out.push_back(value);
        } else {
            const uint8_t mask = ((x+y)%4 < 2) ? 0 : 255;
            const uint8_t value = ((x+y)%2) ? 255 : 0;
            for (uint8_t v : {mask,mask,mask,uint8_t(0),value,value,value,uint8_t(0)}) out.push_back(v);
        }
    }
    return out;
}
int main() {
    @autoreleasepool {
        try {
            auto device=MTLCreateSystemDefaultDevice(); auto queue=[device newCommandQueue];
            require(device && queue,"Metal unavailable");
            NSError* error=nil;
            auto library=[device newLibraryWithSource:pab_mac::video_shader() options:nil error:&error];
            if (!library) throw std::runtime_error(error.localizedDescription.UTF8String);
            auto descriptor=[[MTLRenderPipelineDescriptor alloc] init];
            descriptor.vertexFunction=[library newFunctionWithName:@"video_vertex"];
            descriptor.fragmentFunction=[library newFunctionWithName:@"video_fragment"];
            descriptor.colorAttachments[0].pixelFormat=MTLPixelFormatBGRA8Unorm;
            auto pipeline=[device newRenderPipelineStateWithDescriptor:descriptor error:&error];
            require(pipeline!=nil,"Cursor pipeline unavailable");
            auto make=[&](MTLPixelFormat format,unsigned w,unsigned h,const uint8_t* bytes,unsigned stride) {
                auto d=[MTLTextureDescriptor texture2DDescriptorWithPixelFormat:format width:w height:h mipmapped:NO];
                d.usage=MTLTextureUsageShaderRead;
                auto t=[device newTextureWithDescriptor:d];require(t!=nil,"Texture unavailable");
                [t replaceRegion:MTLRegionMake2D(0,0,w,h) mipmapLevel:0 withBytes:bytes bytesPerRow:stride]; return t;
            };
            std::vector<uint8_t> luma(320*240,100), chroma(160*120*2,128);
            auto y=make(MTLPixelFormatR8Unorm,320,240,luma.data(),320);
            auto uv=make(MTLPixelFormatRG8Unorm,160,120,chroma.data(),320);
            auto target_descriptor=[MTLTextureDescriptor texture2DDescriptorWithPixelFormat:MTLPixelFormatBGRA8Unorm width:384 height:256 mipmapped:NO];
            target_descriptor.usage=MTLTextureUsageRenderTarget;
            auto target=[device newTextureWithDescriptor:target_descriptor];
            const unsigned stride=384*4;
            auto readback=[device newBufferWithLength:stride*256 options:MTLResourceStorageModeShared];
            require(target && readback,"Readback resources unavailable");
            uint64_t pixels=0; unsigned cases=0,max_error=0,rejected=0;
            for (unsigned rotation=0;rotation<4;++rotation) for (unsigned format : {1,2}) {
                const double w=rotation%2 ? 240 : 320,h=rotation%2 ? 320 : 240;
                for (double scale : {.5,1.0}) {
                    const MTLViewport viewport{(384-w*scale)/2,(256-h*scale)/2,w*scale,h*scale,0,1};
                    // At scale 1 a portrait viewport is partly outside the
                    // render target: exercise raster clipping as well as bars.
                    pab_mac::CursorOverlay cursor;
                    const auto draw=[&] {
                        auto pass=[MTLRenderPassDescriptor renderPassDescriptor];
                        pass.colorAttachments[0].texture=target;
                        pass.colorAttachments[0].loadAction=MTLLoadActionClear;
                        pass.colorAttachments[0].storeAction=MTLStoreActionStore;
                        pass.colorAttachments[0].clearColor=MTLClearColorMake(0,0,0,1);
                        auto command=[queue commandBuffer];auto render=[command renderCommandEncoderWithDescriptor:pass];
                        [render setRenderPipelineState:pipeline];[render setViewport:viewport];
                        [render setVertexBytes:&rotation length:sizeof(rotation) atIndex:0];
                        const float fraction[2]{1,1};[render setVertexBytes:fraction length:sizeof(fraction) atIndex:1];
                        [render setFragmentTexture:y atIndex:0];[render setFragmentTexture:uv atIndex:1];
                        [render setFragmentTexture:cursor.image(device) atIndex:2];
                        const auto parameters=cursor.parameters(viewport,w,h);
                        [render setFragmentBytes:&parameters length:sizeof(parameters) atIndex:0];
                        [render drawPrimitives:MTLPrimitiveTypeTriangleStrip vertexStart:0 vertexCount:4];[render endEncoding];
                        auto blit=[command blitCommandEncoder];
                        [blit copyFromTexture:target sourceSlice:0 sourceLevel:0 sourceOrigin:MTLOriginMake(0,0,0)
                            sourceSize:MTLSizeMake(384,256,1) toBuffer:readback destinationOffset:0 destinationBytesPerRow:stride destinationBytesPerImage:stride*256];
                        [blit endEncoding];[command commit];[command waitUntilCompleted];
                        require(command.status==MTLCommandBufferStatusCompleted,"Metal command failed");
                        const auto* bytes=static_cast<const uint8_t*>(readback.contents);
                        return std::vector<uint8_t>(bytes,bytes+stride*256);
                    };
                    const auto base=draw(); bool first=true;
                    for (const auto position : std::array<std::array<int64_t,2>,5>{{{123,111},{0,0},{319,239},{-4294967295LL,0},{4294967295LL,0}}}) {
                        auto command=packet(position[0],position[1],format,true,first);first=false;
                        cursor.update(device,command.data(),command.size());
                        const auto actual=draw();
                        // Independent CPU reference in top-left drawable pixel
                        // coordinates, including half-pixel sample positions.
                        const double left=viewport.originX+position[0]*viewport.width/320-5*scale;
                        const double top=viewport.originY+position[1]*viewport.height/240-7*scale;
                        for (unsigned py=0;py<256;++py) for (unsigned px=0;px<384;++px) {
                            const double sx=std::floor((px+.5-left)/scale), sy=std::floor((py+.5-top)/scale);
                            const bool inside=px+.5>=viewport.originX && px+.5<viewport.originX+viewport.width &&
                                py+.5>=viewport.originY && py+.5<viewport.originY+viewport.height && sx>=0 && sx<17 && sy>=0 && sy<13;
                            const auto offset=py*stride+px*4;
                            for (unsigned channel=0;channel<3;++channel) {
                                unsigned expected=base[offset+channel];
                                if (inside) {
                                    const unsigned index=unsigned(sx)+unsigned(sy);
                                    if (format==1) {
                                        const unsigned alpha=std::array<unsigned,4>{0,80,160,255}[index%4];
                                        const unsigned color=channel==0 ? alpha/4 : channel==1 ? alpha/2 : alpha;
                                        expected=unsigned(std::lround(color+expected*(1-alpha/255.0)));
                                    } else expected=(expected & (index%4<2 ? 0 : 255)) ^ (index%2 ? 255 : 0);
                                }
                                const auto difference=unsigned(std::abs(int(actual[offset+channel])-int(expected)));
                                max_error=std::max(max_error,difference);
                                require(difference <= (format==1 && inside ? 1U : 0U),"Cursor pixel mismatch");
                            }
                            ++pixels;
                        }
                        ++cases;
                    }
                    auto hidden=packet(123,111,format,false,false);cursor.update(device,hidden.data(),hidden.size());
                    require(draw()==base && cursor.tracking(),"Hidden cursor changed pixels or tracking");++cases;
                    const uint8_t clear=0;cursor.update(device,&clear,1);
                    require(draw()==base && !cursor.tracking(),"Clear did not restore background/tracking");++cases;
                    auto valid=packet(123,111,format,true,true);
                    for (size_t size : {size_t(0),size_t(1),size_t(26),size_t(42),valid.size()-1}) {
                        try { cursor.update(device,valid.data(),size);throw std::logic_error("Truncated command accepted"); }
                        catch (const std::runtime_error&) { ++rejected; }
                    }
                }
            }
            printf("{\"cases\":%u,\"pixels\":%llu,\"max_error\":%u,\"truncated_rejected\":%u}\n",cases,pixels,max_error,rejected);
            return 0;
        } catch (const std::exception& error) { fprintf(stderr,"Metal cursor check: %s\n",error.what());return 1; }
    }
}
