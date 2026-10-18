#pragma once
#include "presenter.h"
#include "accessibility.h"

extern "C" bool pab_linux_accessibility_initialize() noexcept {
    return pab_linux::accessibility::initialize();
}

// Private in-process ABI. The host obtains its GtkWindow on the GTK main
// thread; raw pointers are never accepted over IPC or from a webview.
namespace pab_linux {
inline void host_error(char* output, size_t capacity, const char* text) noexcept {
    if (output && capacity) snprintf(output,capacity,"%s",text);
}
template<class F> bool host_call(char* error, size_t capacity, F run) noexcept {
    try { run(); return true; }
    catch (const std::exception& e) { host_error(error,capacity,e.what()); }
    catch (...) { host_error(error,capacity,"Native GTK view failed"); }
    return false;
}
}
extern "C" void* pab_linux_view_new(void* host, uint32_t width, uint32_t height, uint32_t rotation,
                                    char* error, size_t capacity) noexcept {
    void* result = nullptr;
    pab_linux::host_call(error,capacity,[&] {
        if (!host) throw std::runtime_error("Native GTK host is missing");
        result = new pab_linux::Presenter(width,height,rotation,false,false,static_cast<GtkWidget*>(host));
    });
    return result;
}
extern "C" bool pab_linux_view_submit(void* view, const uint8_t* nv12, size_t size, uint64_t index,
                                      char* error, size_t capacity) noexcept {
    return pab_linux::host_call(error,capacity,[&] {
        if (!view) throw std::runtime_error("Native GTK view is missing");
        static_cast<pab_linux::Presenter*>(view)->submit({nv12,size},index);
    });
}
extern "C" bool pab_linux_view_poll(void* view, uint64_t* presented, char* error, size_t capacity) noexcept {
    return pab_linux::host_call(error,capacity,[&] {
        if (!view || !presented) throw std::runtime_error("Native GTK view is missing");
        auto& presenter = *static_cast<pab_linux::Presenter*>(view);
        presenter.poll_events(false);
        *presented = presenter.presented();
    });
}
extern "C" void pab_linux_view_drop(void* view) noexcept {
    delete static_cast<pab_linux::Presenter*>(view);
}
extern "C" bool pab_linux_view_set_input(void* view,void* context,bool(*callback)(void*,const uint8_t*),
                                          void(*drop)(void*),char* error,size_t capacity) noexcept {
    if (!drop) { pab_linux::host_error(error,capacity,"Native input context destructor missing"); return false; }
    const std::shared_ptr<void> owned(context,drop);
    return pab_linux::host_call(error,capacity,[&] {
        if (!view || !callback) throw std::runtime_error("Native input view missing");
        static_cast<pab_linux::Presenter*>(view)->input_sink([owned,callback](const pab_linux::InputPacket& packet) {
            return callback(owned.get(),packet.data());
        });
    });
}
extern "C" bool pab_linux_view_input(void* view,bool enabled,char* error,size_t capacity) noexcept {
    return pab_linux::host_call(error,capacity,[&] {
        if (!view) throw std::runtime_error("Native input view missing");
        static_cast<pab_linux::Presenter*>(view)->input_enabled(enabled);
    });
}

extern "C" bool pab_linux_view_cursor(void* view,const uint8_t* bytes,size_t size,char* error,size_t capacity) noexcept {
    return pab_linux::host_call(error,capacity,[&] {
        if (!view || !bytes || !size) throw std::runtime_error("Native cursor command missing");
        static_cast<pab_linux::Presenter*>(view)->cursor(bytes,size);
    });
}
