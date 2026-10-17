// GNOME Wayland and local X11 CPU capture. Uses the existing compressed worker IPC; no
// transport, authentication, session authority or application settings here.
#include "capture.h"
#include "input.h"
#include "render.h"
#include "host.h"
#include "x11_input.h"
#include "drm_export.h"
#include "dma_capture.h"
#include "dma_broker.h"
#include <csignal>

extern "C" int pab_media_main(int argc, const char* const* argv) {
    signal(SIGPIPE, SIG_IGN);
    if (argc == 5 && std::string(argv[1]) == "input-session") {
        try {
            pab_linux::virtual_input::serve(pab_linux::number(argv[2], INT32_MAX),
                pab_linux::number(argv[3], 1048576), pab_linux::number(argv[4], 1048576));
            return 0;
        } catch (const std::exception& error) {
            if (pab_linux::virtual_input::stopping) return 0;
            fprintf(stderr, "media: %s\n", error.what()); return 1;
        }
        catch (...) { return 1; }
    }
    if (argc == 5 && std::string(argv[1]) == "export-session") {
        try {
            pab_linux::dma::export_session(pab_linux::number(argv[2], INT32_MAX),
                pab_linux::number(argv[3], 1048576), pab_linux::number(argv[4], 1048576));
            return 0;
        } catch (const std::exception& error) { fprintf(stderr, "media: %s\n", error.what()); return 1; }
        catch (...) { return 1; }
    }
    // Explicit inherited-descriptor entry point retained for native checks.
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
            const bool wake = std::string(argv[5]) == "broker" && pab_linux::dma::asleep(bus.get());
            auto channel = std::string(argv[5]) == "broker" ? pab_linux::dma::connect_broker(selected->connector, wake) : pab_linux::dma::Fd();
            if (wake) pab_linux::dma::await_awake(bus.get());
            const auto fd = channel.get() >= 0 ? channel.get() : pab_linux::number(argv[5], 1048576);
            pab_linux::dma::capture(bus.get(), *selected, pab_linux::number(argv[3], 60), fd);
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
        const bool x11 = pab_linux::x11::selected();
        const bool separate=(argc==6 || (x11 && argc==5)) && std::string(argv[1])=="capture" && std::string(argv[4])=="desktop-separate-cursor";
        if (argc != 2 && argc != 3 && argc != 5 && !separate) throw std::runtime_error("usage: media inventory | capture SCREEN_ID FPS desktop | input SCREEN_ID");
        if (argc == 2 && std::string(argv[1]) != "inventory") throw std::runtime_error("Unsupported Linux media operation");
        if (argc == 3 && std::string(argv[1]) != "input" && std::string(argv[1]) != "input-uinput" &&
            !(x11 && std::string(argv[1]) == "cursor")) throw std::runtime_error("Unsupported Linux media operation");
        if (argc == 5 && !separate && (std::string(argv[1]) != "capture" || std::string(argv[4]) != "desktop"))
            throw std::runtime_error("Unsupported Linux media operation");
        if (x11) {
            if(argc==6)throw std::runtime_error("X11 cursor uses its own worker");
            pab_linux::x11::run(argc, argv); pw_deinit(); return 0;
        }
        auto bus = pab_linux::session_bus();
        const auto found = pab_linux::displays(bus.get());
        if (argc == 2) pab_linux::inventory(found);
        else {
            const auto selected = std::find_if(found.begin(), found.end(), [&](const auto& d) { return d.id() == argv[2]; });
            if (selected == found.end()) throw std::runtime_error("Selected display is unavailable");
            if (argc == 3) pab_linux::input(bus.get(), *selected, std::string(argv[1]) == "input-uinput");
            else {
                const auto fps = pab_linux::number(argv[3], 60);
                std::unique_ptr<pab_linux::CursorChannel> cursor;
                if(separate)cursor=std::make_unique<pab_linux::CursorChannel>(argv[5],*selected);
                const bool sleeping = pab_linux::dma::asleep(bus.get());
                if (sleeping || pab_linux::dma::protected_screen(bus.get())) {
                    if(cursor)cursor->state.fail("Independent cursor unavailable for DRM capture");
                    auto channel = pab_linux::dma::connect_broker(selected->connector, sleeping);
                    if (sleeping) pab_linux::dma::await_awake(bus.get());
                    pab_linux::dma::capture(bus.get(), *selected, fps, channel.get());
                    pw_deinit(); return 0;
                }
                auto source = std::make_unique<pab_linux::Capture>(bus.get(), *selected, fps,cursor?&cursor->state:nullptr);
                try { source->start(); }
                catch (const std::exception& error) {
                    // No compressed header/frame has been published yet. Close
                    // the denied PipeWire session before choosing the fallback.
                    fprintf(stderr, "media: PipeWire start failed: %s; trying supervised DMA\n", error.what());
                    source.reset();
                    const bool wake = pab_linux::dma::asleep(bus.get());
                    auto channel = pab_linux::dma::connect_broker(selected->connector, wake);
                    if (wake) pab_linux::dma::await_awake(bus.get());
                    pab_linux::dma::capture(bus.get(), *selected, fps, channel.get());
                }
                if (source) pab_linux::capture(*source, *selected, fps);
            }
        }
        status = 0;
    } catch (const std::exception& error) { fprintf(stderr, "media: %s\n", error.what()); }
    catch (...) { fprintf(stderr, "media: unknown native failure\n"); }
    pw_deinit(); return status;
}
#ifdef PAB_MEDIA_STANDALONE
int main(int argc, const char* const* argv) { return pab_media_main(argc, argv); }
#endif
