// Generated 1080p quadrants only; measures CPU codec cost, not network/display FPS.
#include "../../crates/desktop-media/native/software_codec_ffi.h"
#include <chrono>
#include <cstdio>
#include <dlfcn.h>
#include <stdexcept>
#include <vector>

int main(int argc,char**argv) {
    try {
        if(argc!=2)throw std::runtime_error("usage: codec-color-bench CODEC_LIBRARY");
        void* lib=dlopen(argv[1],RTLD_NOW|RTLD_LOCAL);if(!lib)throw std::runtime_error("load codec");
        auto new_e=reinterpret_cast<void*(*)(uint32_t,uint32_t,uint16_t)>(dlsym(lib,"pab_soft_encoder_new"));
        auto new_d=reinterpret_cast<void*(*)(uint32_t,uint32_t)>(dlsym(lib,"pab_soft_decoder_new"));
        auto encode=reinterpret_cast<decltype(&pab_soft_encode)>(dlsym(lib,"pab_soft_encode"));
        auto decode=reinterpret_cast<decltype(&pab_soft_decode)>(dlsym(lib,"pab_soft_decode"));
        auto drop_e=reinterpret_cast<decltype(&pab_soft_encoder_drop)>(dlsym(lib,"pab_soft_encoder_drop"));
        auto drop_d=reinterpret_cast<decltype(&pab_soft_decoder_drop)>(dlsym(lib,"pab_soft_decoder_drop"));
        if(!new_e||!new_d||!encode||!decode||!drop_e||!drop_d)throw std::runtime_error("codec API missing");
        constexpr unsigned w=1920,h=1080,n=70,warmup=10;
        void* e=new_e(w,h,15);void* d=new_d(w,h);if(!e||!d)throw std::runtime_error("create codec");
        std::vector<uint8_t> bgra(w*h*4);double enc=0,dec=0;size_t bytes=0;
        for(unsigned i=0;i<n;++i) {
            for(unsigned y=0;y<h;++y)for(unsigned x=0;x<w;++x) {
                const unsigned q=(y>=h/2)*2+(x>=w/2),p=(y*w+x)*4;
                bgra[p]=32+(i*11+q*97)%192;bgra[p+1]=32+(i*7+q*67)%192;bgra[p+2]=32+(i*3+q*43)%192;bgra[p+3]=255;
            }
            PabCodecBytes packet{},frame{};
            const auto before=std::chrono::steady_clock::now();
            if(!encode(e,bgra.data(),bgra.size(),w*4,&packet))throw std::runtime_error("encode");
            const auto between=std::chrono::steady_clock::now();
            if(!decode(d,packet.data,packet.size,&frame)||frame.size!=w*h*3/2)throw std::runtime_error("decode");
            const auto after=std::chrono::steady_clock::now();
            if(i>=warmup) {
                enc+=std::chrono::duration<double>(between-before).count();
                dec+=std::chrono::duration<double>(after-between).count();bytes+=packet.size;
            }
        }
        printf("{\"dimensions\":[%u,%u],\"frames\":%u,\"encode_ms\":%.6f,\"decode_ms\":%.6f,\"codec_fps\":%.6f,\"bytes\":%zu}\n",w,h,n-warmup,enc*1000/(n-warmup),dec*1000/(n-warmup),(n-warmup)/(enc+dec),bytes);
        drop_d(d);drop_e(e);dlclose(lib);return 0;
    }catch(const std::exception&e){fprintf(stderr,"codec bench: %s\n",e.what());return 1;}
}
