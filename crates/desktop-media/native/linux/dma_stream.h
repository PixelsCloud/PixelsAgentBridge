#pragma once
#include "dma_cursor_frame.h"
#include <mutex>
#include <atomic>

namespace pab_linux::dma {
// One authenticated exporter connection, one in-flight fd exchange. Video and
// cursor sequence spaces stay independent. Conversion/output never hold this
// lock, so video network credit cannot block cursor sampling.
class ExportStream {
    int socket_;
    std::mutex mutex_;
    uint64_t video_=0,cursor_=0;
    bool failed_=false;
    void alive(const std::atomic<bool>* stopped) {
        if(stopped && stopped->load())throw std::runtime_error("DMA cursor stopped");
        parent_alive();
    }
    std::unique_lock<std::mutex> acquire(const std::atomic<bool>* stopped) {
        std::unique_lock<std::mutex> lock(mutex_,std::defer_lock);
        while(!lock.try_lock()) { alive(stopped);std::this_thread::sleep_for(std::chrono::milliseconds(2)); }
        alive(stopped);
        if(failed_)throw std::runtime_error("DMA export stream failed");
        return lock;
    }
    void await(short events,const std::atomic<bool>* stopped) {
        const auto deadline=Clock::now()+std::chrono::seconds(3);
        for(;;) {
            alive(stopped);pollfd pending{socket_,events,0};const auto status=poll(&pending,1,10);
            if(status<0 && errno==EINTR)continue;
            if(status<0 || (pending.revents&(POLLHUP|POLLERR|POLLNVAL)) || Clock::now()>=deadline)
                throw std::runtime_error("DMA export exchange ended or timed out");
            if(pending.revents&events)return;
        }
    }
    void fail() { failed_=true;shutdown(socket_,SHUT_RDWR); }
public:
    explicit ExportStream(int socket):socket_(socket){channel(socket);}
    Frame video() {
        auto lock=acquire(nullptr);
        try {
            if(video_==UINT64_MAX)throw std::runtime_error("DMA video sequence exhausted");
            await(POLLOUT,nullptr);send_request(socket_,video_);await(POLLIN,nullptr);
            auto frame=receive(socket_,video_);++video_;return frame;
        } catch(...) {fail();throw;}
    }
    CursorFrame cursor(const std::atomic<bool>& stopped) {
        auto lock=acquire(&stopped);
        try {
            if(cursor_==UINT64_MAX)throw std::runtime_error("DMA cursor sequence exhausted");
            await(POLLOUT,&stopped);send_cursor_request(socket_,cursor_);await(POLLIN,&stopped);
            auto frame=receive_cursor(socket_,cursor_);++cursor_;return frame;
        } catch(...) {fail();throw;}
    }
};
}
