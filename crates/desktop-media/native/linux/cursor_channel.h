#pragma once
#include "cursor_metadata.h"
#include <atomic>
#include <sys/socket.h>
#include <sys/stat.h>
#include <sys/un.h>

namespace pab_linux {
// The capture's parent creates a private directory. Only that same parent PID
// and UID may read this channel; no network listener, account or new credential.
// The thread owns bounded cursor output, never video/PipeWire buffers.
class CursorChannel {
    std::string path_;
    Display display_;
    int listener_=-1;
    pid_t parent_=getppid();
    std::atomic<bool> stopped_{false};
    std::thread thread_;
    struct Fd { int value; ~Fd(){if(value>=0)close(value);} };
    void transfer(int fd,void* bytes,size_t size,bool output) {
        auto* data=static_cast<uint8_t*>(bytes);
        const auto deadline=Clock::now()+std::chrono::seconds(5);
        while(size) {
            if(stopped_.load() || Clock::now()>=deadline)throw std::runtime_error("Cursor channel ended or timed out");
            pollfd p{fd,short(output?POLLOUT:POLLIN),0};
            const int status=poll(&p,1,10);
            if(status<0 && errno==EINTR)continue;
            if(status<0 || (p.revents&(POLLERR|POLLHUP|POLLNVAL)))throw std::runtime_error("Cursor channel closed");
            if(!status)continue;
            const auto count=output?send(fd,data,size,MSG_NOSIGNAL):recv(fd,data,size,0);
            if(count<0 && (errno==EINTR || errno==EAGAIN))continue;
            if(count<=0)throw std::runtime_error("Cursor channel transfer failed");
            data+=count;size-=size_t(count);
        }
    }
    template<class T> T receive(int fd) { T value;transfer(fd,&value,sizeof(value),false);return value; }
    template<class T> static void append(std::vector<uint8_t>& bytes,T value) {
        const auto* data=reinterpret_cast<const uint8_t*>(&value);bytes.insert(bytes.end(),data,data+sizeof(value));
    }
    void serve(int fd) {
        const auto x=receive<int32_t>(fd),y=receive<int32_t>(fd);
        const auto width=receive<uint32_t>(fd),height=receive<uint32_t>(fd),rotation=receive<uint32_t>(fd);
        if(x!=display_.x || y!=display_.y || width!=display_.size.logical_width || height!=display_.size.logical_height || rotation)
            throw std::runtime_error("Cursor layout changed");
        uint32_t ready[]={magic,1};transfer(fd,ready,sizeof(ready),true);
        bool requested=false;uint64_t revision=0,sequence=0,sent_image=0;
        while(!stopped_.load()) {
            pollfd p{fd,POLLIN,0};const int status=poll(&p,1,8);
            if(status<0 && errno==EINTR)continue;
            if(status<0 || (p.revents&(POLLERR|POLLHUP|POLLNVAL)))return;
            if(p.revents&POLLIN) {
                const auto command=receive<uint8_t>(fd);
                if(!command)return;
                if(command!=1 || requested)throw std::runtime_error("Invalid cursor request");
                requested=true;
            }
            const auto snapshot=state.snapshot();
            if(!requested || !snapshot.revision || snapshot.revision==revision)continue;
            if(sequence==UINT64_MAX)throw std::runtime_error("Cursor sequence exhausted");
            const bool image=snapshot.image_id!=sent_image;
            std::vector<uint8_t> packet;packet.reserve(45+(image?snapshot.image->pixels.size():0));
            append(packet,uint8_t(1));append(packet,uint16_t(1));append(packet,++sequence);
            append(packet,uint8_t(snapshot.visible));append(packet,snapshot.x);append(packet,snapshot.y);
            append(packet,snapshot.image_id);append(packet,uint8_t(image?1:0));
            if(image) {
                append(packet,snapshot.image->width);append(packet,snapshot.image->height);
                append(packet,snapshot.image->hotspot_x);append(packet,snapshot.image->hotspot_y);
                packet.insert(packet.end(),snapshot.image->pixels.begin(),snapshot.image->pixels.end());
            }
            transfer(fd,packet.data(),packet.size(),true);
            revision=snapshot.revision;sent_image=snapshot.image_id;requested=false;
        }
    }
    void run() noexcept {
        while(!stopped_.load()) {
            pollfd p{listener_,POLLIN,0};const auto status=poll(&p,1,10);
            if(status<=0)continue;
            Fd peer{accept4(listener_,nullptr,nullptr,SOCK_CLOEXEC|SOCK_NONBLOCK)};
            if(peer.value<0)continue;
            ucred identity{};socklen_t size=sizeof(identity);
            if(getsockopt(peer.value,SOL_SOCKET,SO_PEERCRED,&identity,&size) ||
                size!=sizeof(identity) || identity.uid!=geteuid() || identity.pid!=parent_)continue;
            try { serve(peer.value); } catch(const std::exception&) {} // Cursor failure never retires video.
        }
    }
public:
    CursorState state;
    CursorChannel(const std::string& path,Display display):path_(path),display_(std::move(display)) {
        const auto slash=path.find_last_of('/');struct stat info{};
        sockaddr_un address{};address.sun_family=AF_UNIX;
        if(slash==std::string::npos || path.size()>=sizeof(address.sun_path) ||
            lstat(path.substr(0,slash).c_str(),&info) || !S_ISDIR(info.st_mode) ||
            info.st_uid!=geteuid() || (info.st_mode&0777)!=0700)
            throw std::runtime_error("Cursor channel requires a private owned directory");
        std::memcpy(address.sun_path,path.c_str(),path.size()+1);
        listener_=socket(AF_UNIX,SOCK_STREAM|SOCK_CLOEXEC|SOCK_NONBLOCK,0);
        if(listener_<0)throw std::runtime_error("Cannot create cursor channel");
        if(bind(listener_,reinterpret_cast<sockaddr*>(&address),sizeof(address))) {
            close(listener_);listener_=-1;throw std::runtime_error("Cannot bind cursor channel");
        }
        try {
            if(chmod(path.c_str(),0600) || listen(listener_,1))throw std::runtime_error("Cannot listen on cursor channel");
            thread_=std::thread([this]{run();});
        } catch(...) { close(listener_);listener_=-1;unlink(path.c_str());throw; }
    }
    ~CursorChannel() {
        stopped_.store(true);if(thread_.joinable())thread_.join();
        if(listener_>=0)close(listener_);
        unlink(path_.c_str());
    }
    CursorChannel(const CursorChannel&)=delete;
};
}
