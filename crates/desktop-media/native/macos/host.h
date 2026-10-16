#pragma once
#include "decoder.h"
#include "presenter.h"

// Private in-process ABI. UI handles are created/used/dropped on AppKit's main
// thread; decoder and surface ownership is transferred on a serial background
// thread. These pointers never occur in remote messages or webview arguments.
namespace pab_mac {
template<class F> bool host_call(char* error, size_t capacity, F&& call) noexcept {
    @autoreleasepool {
        try { call(); return true; }
        catch (const std::exception& failure) { if (error && capacity) snprintf(error,capacity,"%s",failure.what()); }
        catch (...) { if (error && capacity) snprintf(error,capacity,"Unknown native media failure"); }
    }
    return false;
}
inline void host_main() { if (![NSThread isMainThread]) throw std::runtime_error("Native host view requires the main thread"); }
}
extern "C" {
using PabInputCallback = bool(*)(void*,const uint8_t*);
using PabContextDrop = void(*)(void*);
void* pab_mac_view_new(void* window, unsigned width, unsigned height, unsigned rotation,
    void* context, PabInputCallback callback, PabContextDrop drop, char* error, size_t capacity) noexcept {
    if (!drop) { if (error && capacity) snprintf(error,capacity,"Missing native context destructor"); return nullptr; }
    // Take ownership even if view creation fails. A retained NSView may outlive
    // its presenter; clearing its sink before removal retires this context.
    const std::shared_ptr<void> owned(context,drop);
    void* result = nullptr;
    pab_mac::host_call(error,capacity,[&] {
        pab_mac::host_main();
        NSWindow* parent = (__bridge NSWindow*)window;
        if (!parent || ![parent isKindOfClass:NSWindow.class] || !parent.contentView || !callback || !drop)
            throw std::runtime_error("Invalid native view host");
        auto view = std::make_unique<pab_mac::NativePresenter>(width,height,rotation,false,parent.contentView);
        view->input_sink([owned,callback](const pab_mac::InputPacket& packet) { return callback(owned.get(),packet.data()); });
        result = view.release();
    });
    return result;
}
bool pab_mac_view_poll(void* view, char* error, size_t capacity) noexcept {
    return pab_mac::host_call(error,capacity,[&] { pab_mac::host_main(); static_cast<pab_mac::NativePresenter*>(view)->pump_events(); });
}
bool pab_mac_view_input(void* view, bool enabled, char* error, size_t capacity) noexcept {
    return pab_mac::host_call(error,capacity,[&] { pab_mac::host_main(); static_cast<pab_mac::NativePresenter*>(view)->input_enabled(enabled); });
}
bool pab_mac_view_render(void* view, void* surface, uint64_t index, char* error, size_t capacity) noexcept {
    return pab_mac::host_call(error,capacity,[&] { pab_mac::host_main();
        static_cast<pab_mac::NativePresenter*>(view)->render(static_cast<CVPixelBufferRef>(surface),index); });
}
void pab_mac_view_drop(void* view) noexcept {
    // A wrong-thread call is a host programming error. Do not call AppKit from
    // that thread even during shutdown; normal Rust ownership schedules main.
    if (![NSThread isMainThread]) { fprintf(stderr,"media view cleanup requires main thread\n"); return; }
    @autoreleasepool { delete static_cast<pab_mac::NativePresenter*>(view); }
}
void* pab_mac_decoder_new(unsigned width, unsigned height, unsigned fps, bool software, char* error, size_t capacity) noexcept {
    void* result = nullptr;
    pab_mac::host_call(error,capacity,[&] {
        if (!width || !height || width%2 || height%2 || width>8192 || height>8192 || uint64_t(width)*height>33554432 || !fps || fps>60)
            throw std::runtime_error("Invalid decoder geometry");
        result = new pab_mac::VideoDecoder(width,height,fps,software);
    });
    return result;
}
void* pab_mac_decoder_frame(void* decoder, const uint8_t* bytes, size_t size, uint64_t index,
    bool* software, char* error, size_t capacity) noexcept {
    void* result = nullptr;
    pab_mac::host_call(error,capacity,[&] {
        if (!decoder || !bytes || !size || size>pab_mac::max_packet || !software) throw std::runtime_error("Invalid native frame");
        auto* instance = static_cast<pab_mac::VideoDecoder*>(decoder);
        auto surface = instance->decode(std::vector<uint8_t>(bytes,bytes+size),index);
        *software = instance->software(); result = surface.release();
    });
    return result;
}
void pab_mac_decoder_drop(void* decoder) noexcept { delete static_cast<pab_mac::VideoDecoder*>(decoder); }
void pab_mac_surface_drop(void* surface) noexcept { if (surface) CVPixelBufferRelease(static_cast<CVPixelBufferRef>(surface)); }
}
