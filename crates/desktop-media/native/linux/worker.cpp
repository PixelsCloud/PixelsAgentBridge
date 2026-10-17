// GNOME Wayland and local X11 CPU capture. Uses the existing compressed worker IPC; no
// transport, authentication, session authority or application settings here.
#include "capture.h"
#include "input.h"
#include "render.h"
#include "host.h"
#include "x11_input.h"
#include "drm_export.h"
#include "dma_capture.h"
#include <csignal>

extern "C" int pab_media_main(int argc, const char* const* argv) {
    signal(SIGPIPE, SIG_IGN);
    // Internal inherited-descriptor entry points. Normal capture selection is
    // unchanged until the session supervisor owns and authorizes the channel.
    if (argc == 7 && std::string(argv[1]) == "export-dma") {
        try {
            const std::string mode = argv[2];
            if (mode != "borrowed" && mode != "independent") throw std::runtime_error("Invalid DRM export mode");
            pab_linux::dma::export_loop(pab_linux::number(argv[3], 1048576),
                pab_linux::number(argv[4], 1048576), pab_linux::number(argv[5], 1048576), argv[6], mode == "borrowed");
            return 0;
        } catch (const std::exception& error) { fprintf(stderr, "media: %s\n", error.what()); return 1; }
        catch (...) { return 1; }
    }
    pw_init(nullptr, nullptr);
    int status = 1;
    try {
        if (argc == 6 && std::string(argv[1]) == "capture-dma" && std::string(argv[4]) == "desktop") {
            auto bus = pab_linux::session_bus();
            const auto found = pab_linux::displays(bus.get());
            const auto selected = std::find_if(found.begin(), found.end(), [&](const auto& d) { return d.id() == argv[2]; });
            if (selected == found.end()) throw std::runtime_error("Selected DMA display is unavailable");
            pab_linux::dma::capture(bus.get(), *selected, pab_linux::number(argv[3], 60), pab_linux::number(argv[5], 1048576));
            pw_deinit(); return 0;
        }
        if (argc == 8 && std::string(argv[1]) == "render") {
            if (std::string(argv[6]) != "0") throw std::runtime_error("Linux worker does not accept a foreign window handle");
            const std::string angle = argv[7];
            unsigned rotation = angle == "0" ? 0 : angle == "90" ? 1 : angle == "180" ? 2 : angle == "270" ? 3 : 4;
            pab_linux::render(pab_linux::number(argv[2],8192),pab_linux::number(argv[3],8192),pab_linux::number(argv[4],60),argv[5],rotation);
            pw_deinit(); return 0;
        }
        // GNOME owns per-client devices. XTEST needs the existing supervised
        // recovery pass to release only this X server generation's held input.
        if (argc == 2 && std::string(argv[1]) == "recover-input") {
            if (pab_linux::x11::selected()) pab_linux::x11::recover();
            pw_deinit(); return 0;
        }
        if (argc != 2 && argc != 3 && argc != 5) throw std::runtime_error("usage: media inventory | capture SCREEN_ID FPS desktop | input SCREEN_ID");
        if (argc == 2 && std::string(argv[1]) != "inventory") throw std::runtime_error("Unsupported Linux media operation");
        if (argc == 3 && std::string(argv[1]) != "input") throw std::runtime_error("Unsupported Linux media operation");
        if (argc == 5 && (std::string(argv[1]) != "capture" || std::string(argv[4]) != "desktop"))
            throw std::runtime_error("Unsupported Linux media operation");
        if (pab_linux::x11::selected()) {
            pab_linux::x11::run(argc, argv); pw_deinit(); return 0;
        }
        auto bus = pab_linux::session_bus();
        const auto found = pab_linux::displays(bus.get());
        if (argc == 2) pab_linux::inventory(found);
        else {
            const auto selected = std::find_if(found.begin(), found.end(), [&](const auto& d) { return d.id() == argv[2]; });
            if (selected == found.end()) throw std::runtime_error("Selected display is unavailable");
            if (argc == 3) pab_linux::input(bus.get(), *selected);
            else pab_linux::capture(bus.get(), *selected, pab_linux::number(argv[3], 60));
        }
        status = 0;
    } catch (const std::exception& error) { fprintf(stderr, "media: %s\n", error.what()); }
    catch (...) { fprintf(stderr, "media: unknown native failure\n"); }
    pw_deinit(); return status;
}
#ifdef PAB_MEDIA_STANDALONE
int main(int argc, const char* const* argv) { return pab_media_main(argc, argv); }
#endif
