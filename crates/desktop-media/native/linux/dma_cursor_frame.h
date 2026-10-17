#pragma once
#include "dma_frame.h"
#include <optional>

namespace pab_linux::dma {
constexpr uint32_t cursor_magic=0x32525543;
enum class ExportOperation { Closed, Video, Cursor };
inline ExportOperation export_request(int socket,uint64_t video_sequence,uint64_t cursor_sequence) {
    Request r{};ssize_t count;
    do { count=recv(socket,&r,sizeof(r),MSG_TRUNC); } while(count<0 && errno==EINTR);
    if(!count)return ExportOperation::Closed;
    if(count!=sizeof(r) || r.version!=wire_version ||
        (r.magic!=wire_magic && r.magic!=cursor_magic) ||
        r.sequence!=(r.magic==wire_magic?video_sequence:cursor_sequence))
        throw std::runtime_error("Invalid DMA export request");
    return r.magic==wire_magic?ExportOperation::Video:ExportOperation::Cursor;
}
inline void send_cursor_request(int socket,uint64_t sequence) {
    Request r;r.magic=cursor_magic;r.sequence=sequence;
    if(send(socket,&r,sizeof(r),MSG_NOSIGNAL)!=sizeof(r))throw std::runtime_error("DMA cursor request failed");
}
enum class CursorStatus:uint32_t { Hidden=0, Visible=1, Unavailable=2, Retry=3 };
struct CursorMetadata {
    uint32_t magic=cursor_magic,version=wire_version;
    uint64_t sequence=0;
    CursorStatus status=CursorStatus::Hidden;
    uint32_t screen_width=0,screen_height=0,crtc=0,connector=0;
    int32_t left=0,top=0;
    uint32_t hotspot_x=0,hotspot_y=0,reserved=0;
    Metadata image;
};
static_assert(sizeof(CursorMetadata)==120);
struct CursorFrame { CursorMetadata info;std::optional<Frame> image; };
inline uint64_t validate_cursor(const CursorMetadata& m,uint64_t sequence) {
    if(m.magic!=cursor_magic || m.version!=wire_version || m.sequence!=sequence || m.reserved ||
        uint32_t(m.status)>uint32_t(CursorStatus::Retry))throw std::runtime_error("Invalid DMA cursor header");
    if(m.status==CursorStatus::Unavailable || m.status==CursorStatus::Retry) {
        if(m.screen_width || m.screen_height || m.crtc || m.connector)
            throw std::runtime_error("Invalid DMA cursor status fields");
    } else if(m.screen_width<2 || m.screen_height<2 || m.screen_width>8192 || m.screen_height>8192 ||
              uint64_t(m.screen_width)*m.screen_height>33554432 || !m.crtc || !m.connector)
        throw std::runtime_error("Invalid DMA cursor output geometry");
    if(m.status==CursorStatus::Visible) {
        const auto extent=validate_cursor_image(m.image,sequence);
        if(m.image.crtc!=m.crtc || m.image.connector!=m.connector ||
            m.hotspot_x>=m.image.width || m.hotspot_y>=m.image.height)
            throw std::runtime_error("Invalid DMA cursor image geometry");
        return extent;
    }
    const Metadata empty{};
    if(m.left || m.top || m.hotspot_x || m.hotspot_y || memcmp(&m.image,&empty,sizeof(empty)))
        throw std::runtime_error("Non-visible DMA cursor contains image fields");
    return 0;
}
inline void send_cursor(int socket,const CursorFrame& frame) {
    const auto extent=validate_cursor(frame.info,frame.info.sequence);
    if(bool(extent)!=bool(frame.image))throw std::runtime_error("DMA cursor image presence mismatch");
    if(frame.image) {
        if(memcmp(&frame.info.image,&frame.image->info,sizeof(Metadata)))
            throw std::runtime_error("DMA cursor image metadata mismatch");
        send_descriptors(socket,frame.info,{frame.image->image.get(),frame.image->render.get()});
    } else send_descriptors(socket,frame.info,{});
}
inline CursorFrame receive_cursor(int socket,uint64_t sequence) {
    auto [info,fds]=receive_descriptors<CursorMetadata>(socket);
    const auto extent=validate_cursor(info,sequence);
    CursorFrame frame;frame.info=info;
    if(extent) {
        validate_descriptors(fds,extent);
        frame.image.emplace(Frame{info.image,std::move(fds[0]),std::move(fds[1])});
    } else if(!fds.empty())throw std::runtime_error("Unexpected DMA cursor descriptors");
    return frame;
}
}
