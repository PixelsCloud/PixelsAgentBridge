#pragma once
#include "presenter.h"

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
