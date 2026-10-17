// Real packet sockets and SCM_RIGHTS; no display or privileged device access.
#include "../native/linux/dma_stream.h"
#include <cassert>
#include <dirent.h>
#include <future>
#include <sys/mman.h>

using namespace pab_linux;
using namespace pab_linux::dma;
template<class F> void rejects(F action) {
    bool caught=false;
    try { action(); } catch(const std::runtime_error&) {caught=true;}
    assert(caught);
}
unsigned count_fds() {
    auto* dir=opendir("/proc/self/fd");assert(dir);
    unsigned count=0;while(readdir(dir))++count;closedir(dir);return count;
}
struct Pair {
    Fd client,server;
    Pair() {
        int fds[2];assert(!socketpair(AF_UNIX,SOCK_SEQPACKET|SOCK_CLOEXEC,0,fds));
        client=Fd(fds[0]);server=Fd(fds[1]);
    }
};
void malformed(int socket,const CursorMetadata& info,size_t bytes,int image,int render,unsigned descriptors) {
    assert(descriptors<=6 && bytes<=sizeof(info)+4);
    std::array<int,6> fds{};fds.fill(image);fds[1]=render;
    alignas(cmsghdr) std::array<unsigned char,CMSG_SPACE(sizeof(fds))> control{};
    std::array<unsigned char,sizeof(info)+4> payload{};memcpy(payload.data(),&info,sizeof(info));
    iovec data{payload.data(),bytes};msghdr message{};message.msg_iov=&data;message.msg_iovlen=1;
    if(descriptors) {
        message.msg_control=control.data();message.msg_controllen=CMSG_SPACE(descriptors*sizeof(int));
        auto* rights=CMSG_FIRSTHDR(&message);rights->cmsg_level=SOL_SOCKET;rights->cmsg_type=SCM_RIGHTS;
        rights->cmsg_len=CMSG_LEN(descriptors*sizeof(int));memcpy(CMSG_DATA(rights),fds.data(),descriptors*sizeof(int));
    }
    assert(sendmsg(socket,&message,MSG_NOSIGNAL)==ssize_t(bytes));
}
int main() {
    // Keep the worker's parent pipe alive while exercising cancellation.
    int parent[2];assert(!pipe2(parent,O_CLOEXEC));
    Fd parent_read(parent[0]),parent_write(parent[1]);assert(dup2(parent_read.get(),STDIN_FILENO)==STDIN_FILENO);
    Fd image(memfd_create("pab-cursor-test",MFD_CLOEXEC)),render(open("/dev/null",O_RDWR|O_CLOEXEC));
    assert(image.get()>=0 && render.get()>=0 && !ftruncate(image.get(),16384));
    CursorMetadata visible;visible.status=CursorStatus::Visible;
    visible.screen_width=1920;visible.screen_height=1080;visible.crtc=1;visible.connector=3;
    visible.left=-1;visible.top=-2;
    visible.image.width=visible.image.height=1;visible.image.stride=4;
    visible.image.format=DRM_FORMAT_ARGB8888;visible.image.crtc=1;visible.image.connector=3;visible.image.framebuffer=2;
    unsigned checked=0;
    Pair pair;
    const auto baseline=count_fds();
    {
        CursorFrame frame;frame.info=visible;
        frame.image.emplace(Frame{visible.image,Fd(dup(image.get())),Fd(dup(render.get()))});
        send_cursor(pair.server.get(),frame);auto received=receive_cursor(pair.client.get(),0);
        assert(received.image && received.info.left==-1 && received.info.top==-2);
        assert(fcntl(received.image->image.get(),F_GETFD)&FD_CLOEXEC);
        assert(fcntl(received.image->render.get(),F_GETFD)&FD_CLOEXEC);
        ++checked;
    }
    assert(count_fds()==baseline);
    for(auto status:{CursorStatus::Hidden,CursorStatus::Unavailable,CursorStatus::Retry}) {
        CursorFrame frame;frame.info.status=status;
        if(status==CursorStatus::Hidden) {
            frame.info.screen_width=1920;frame.info.screen_height=1080;frame.info.crtc=1;frame.info.connector=3;
        }
        send_cursor(pair.server.get(),frame);auto received=receive_cursor(pair.client.get(),0);
        assert(received.info.status==status && !received.image);++checked;
        malformed(pair.server.get(),frame.info,sizeof(frame.info),image.get(),render.get(),2);
        rejects([&]{receive_cursor(pair.client.get(),0);});assert(count_fds()==baseline);++checked;
    }
    for(auto mutate:std::initializer_list<void(*)(CursorMetadata&)>{
        [](auto& m){m.magic=0;},[](auto& m){m.version=2;},[](auto& m){m.sequence=1;},
        [](auto& m){m.status=CursorStatus(4);},[](auto& m){m.reserved=1;},
        [](auto& m){m.screen_width=1;},[](auto& m){m.screen_height=8193;},
        [](auto& m){m.screen_width=8192;m.screen_height=8192;},[](auto& m){m.crtc=0;},
        [](auto& m){m.image.crtc=2;},[](auto& m){m.image.connector=4;},
        [](auto& m){m.hotspot_x=1;},[](auto& m){m.hotspot_y=1;},
        [](auto& m){m.image.width=513;m.image.stride=2052;},
        [](auto& m){m.image.format=DRM_FORMAT_XRGB8888;},
        [](auto& m){m.image.sequence=1;},[](auto& m){m.image.offset=4*1024*1024;},
        [](auto& m){m.status=CursorStatus::Hidden;},[](auto& m){m.status=CursorStatus::Retry;}}) {
        auto invalid=visible;mutate(invalid);
        malformed(pair.server.get(),invalid,sizeof(invalid),image.get(),render.get(),2);
        rejects([&]{receive_cursor(pair.client.get(),0);});assert(count_fds()==baseline);++checked;
    }
    for(unsigned descriptors:{0u,1u,3u,6u}) {
        malformed(pair.server.get(),visible,sizeof(visible),image.get(),render.get(),descriptors);
        rejects([&]{receive_cursor(pair.client.get(),0);});assert(count_fds()==baseline);++checked;
    }
    for(size_t bytes:{sizeof(visible)-1,sizeof(visible)+4}) {
        malformed(pair.server.get(),visible,bytes,image.get(),render.get(),2);
        rejects([&]{receive_cursor(pair.client.get(),0);});assert(count_fds()==baseline);++checked;
    }
    {
        // Concurrent producers share one exporter; counters and reply types
        // must stay distinct, even when every cursor reply is unavailable.
        Pair concurrent;ExportStream stream(concurrent.client.get());std::atomic<bool> stop{false};
        auto exporter=std::async(std::launch::async,[&]{
            uint64_t video=0,cursor=0;
            for(unsigned i=0;i<160;++i) {
                auto operation=export_request(concurrent.server.get(),video,cursor);
                if(operation==ExportOperation::Video) {
                    auto info=visible.image;info.sequence=video++;info.width=info.height=64;info.stride=256;
                    Frame frame{info,Fd(dup(image.get())),Fd(dup(render.get()))};send_frame(concurrent.server.get(),frame);
                } else {
                    assert(operation==ExportOperation::Cursor);
                    CursorFrame frame;frame.info.sequence=cursor++;frame.info.status=CursorStatus::Unavailable;
                    send_cursor(concurrent.server.get(),frame);
                }
            }
            assert(video==80 && cursor==80);
        });
        auto cursors=std::async(std::launch::async,[&]{
            for(uint64_t i=0;i<80;++i) {auto frame=stream.cursor(stop);assert(frame.info.sequence==i && frame.info.status==CursorStatus::Unavailable);}
        });
        for(uint64_t i=0;i<80;++i)assert(stream.video().info.sequence==i);
        cursors.get();exporter.get();++checked;
    }
    assert(count_fds()==baseline);
    {
        Pair cancelled;ExportStream stream(cancelled.client.get());std::atomic<bool> stop{true};
        rejects([&]{stream.cursor(stop);});
        // Cancellation before sending does not poison the shared connection.
        auto exporter=std::async(std::launch::async,[&]{
            assert(export_request(cancelled.server.get(),0,0)==ExportOperation::Video);
            auto info=visible.image;info.width=info.height=64;info.stride=256;
            Frame frame{info,Fd(dup(image.get())),Fd(dup(render.get()))};send_frame(cancelled.server.get(),frame);
        });
        assert(stream.video().info.sequence==0);exporter.get();++checked;
    }
    {
        Pair cancelled;ExportStream stream(cancelled.client.get());std::atomic<bool> stop{false};
        auto consumer=std::async(std::launch::async,[&]{rejects([&]{stream.cursor(stop);});});
        assert(export_request(cancelled.server.get(),0,0)==ExportOperation::Cursor);
        const auto started=Clock::now();stop.store(true);consumer.get();
        assert(Clock::now()-started<std::chrono::seconds(1));
        // An unanswered transaction cannot safely be reused for video.
        rejects([&]{stream.video();});
        assert(export_request(cancelled.server.get(),0,0)==ExportOperation::Closed);++checked;
    }
    assert(count_fds()==baseline);
    send_cursor_request(pair.client.get(),0);
    assert(export_request(pair.server.get(),7,0)==ExportOperation::Cursor);++checked;
    send_request(pair.client.get(),7);
    assert(export_request(pair.server.get(),7,1)==ExportOperation::Video);++checked;
    send_cursor_request(pair.client.get(),0);
    rejects([&]{export_request(pair.server.get(),7,1);});++checked;
    printf("DMA cursor: %u boundary/concurrency/cancellation checks passed; no fd growth\n",checked);
}
