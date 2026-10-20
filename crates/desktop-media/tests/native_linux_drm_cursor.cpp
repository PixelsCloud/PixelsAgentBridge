// Opt-in real GPU check: root exports only cursor DMA-BUF + render fd, then a
// child drops all control fds and privileges before initializing EGL/readback.
// No session/input/lock changes. Arguments: CARD RENDER CONNECTOR USER_UID.
#include "../native/linux/drm_export.h"
#include "../native/linux/dma_image.h"
#include <dirent.h>
#include <sys/wait.h>
#include <grp.h>
#include <cassert>
#include <cstdio>
using namespace pab_linux::dma;
static unsigned descriptors() {
    auto* d=opendir("/proc/self/fd");assert(d);unsigned n=0;
    while(readdir(d))++n;
    closedir(d);return n;
}
static int consume(const CursorScanout& cursor,unsigned uid) {
    const auto& frame=*cursor.image;
    // Close inherited control/observer descriptors before dropping root. Only
    // the exported image, render node and output streams reach the GPU reader.
    auto* directory=opendir("/proc/self/fd");if(!directory)return 2;
    const auto listing=dirfd(directory);std::vector<int> retire;
    while(auto* entry=readdir(directory)) {
        char* end=nullptr;long number=strtol(entry->d_name,&end,10);
        if(end==entry->d_name || *end || number<3 || number==listing ||
            number==frame.image.get() || number==frame.render.get())continue;
        retire.push_back(int(number));
    }
    closedir(directory);for(auto fd:retire)close(fd);
    if(!uid || setgroups(0,nullptr) || setgid(uid) || setuid(uid) || geteuid()!=uid)return 3;
    try {
        ImageReader reader;const auto& pixels=reader.read(frame,true);
        uint64_t hash=14695981039346656037ull;unsigned opaque=0,max_premult_error=0;
        unsigned left=frame.info.width,top=frame.info.height,right=0,bottom=0;
        for(size_t i=0;i<pixels.size();++i) { hash^=pixels[i];hash*=1099511628211ull; }
        for(unsigned y=0;y<frame.info.height;++y)for(unsigned x=0;x<frame.info.width;++x) {
            const auto* pixel=pixels.data()+(size_t(y)*frame.info.width+x)*4;
            for(unsigned c=0;c<3;++c)if(pixel[c]>pixel[3])max_premult_error=std::max(max_premult_error,unsigned(pixel[c]-pixel[3]));
            if(pixel[3]) { ++opaque;left=std::min(left,x);right=std::max(right,x);top=std::min(top,y);bottom=std::max(bottom,y); }
        }
        printf("{\"consumer_uid\":%u,\"width\":%u,\"height\":%u,\"bytes\":%zu,\"nontransparent_pixels\":%u,\"max_premult_error\":%u,\"nontransparent_bounds\":[%u,%u,%u,%u],\"fnv64\":\"%016llx\",\"linear_cursor\":%s}\n",
            unsigned(geteuid()),frame.info.width,frame.info.height,pixels.size(),opaque,max_premult_error,left,top,right,bottom,
            static_cast<unsigned long long>(hash),reader.linear_cursor()?"true":"false");
        fflush(stdout);return max_premult_error>1?4:0;
    } catch(const std::exception& e) { fprintf(stderr,"cursor consumer: %s\n",e.what());return 5; }
}
int main(int argc,char** argv) {
    if(argc!=5 || geteuid()!=0)return 2;
    try {
        Fd card(open(argv[1],O_RDONLY|O_CLOEXEC|O_NOFOLLOW)),render(open(argv[2],O_RDWR|O_CLOEXEC|O_NOFOLLOW));
        Exporter exporter(card.get(),render.get(),argv[3],false);
        const auto before=descriptors();unsigned visible=0,hidden=0;
        for(unsigned i=0;i<32;++i) {
            auto sample=exporter.cursor(i);
            if(sample.image)++visible;else ++hidden;
        }
        assert(descriptors()==before);
        auto sample=exporter.cursor(32);
        printf("{\"root_samples\":32,\"visible\":%u,\"hidden\":%u,\"no_fd_growth\":true,\"screen\":[%u,%u],\"position\":[%d,%d],\"hotspot\":[%u,%u]}\n",
            visible,hidden,sample.screen_width,sample.screen_height,sample.left,sample.top,sample.hotspot_x,sample.hotspot_y);
        fflush(stdout);
        if(!sample.image)return 0;
        const auto pid=fork();if(pid<0)return 3;
        if(!pid)_exit(consume(sample,pab_linux::number(argv[4],INT32_MAX)));
        int status;while(waitpid(pid,&status,0)<0)if(errno!=EINTR)return 4;
        return WIFEXITED(status)?WEXITSTATUS(status):5;
    } catch(const std::exception& e) { fprintf(stderr,"cursor exporter: %s\n",e.what());return 1; }
}
