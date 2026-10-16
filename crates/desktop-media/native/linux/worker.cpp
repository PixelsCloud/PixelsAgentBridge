// GNOME Wayland CPU fallback. Uses the existing compressed worker IPC; no
// transport, authentication, session authority or application settings here.
#include "capture.h"
#include "input.h"
#include <csignal>

extern "C" int pab_media_main(int argc, const char* const* argv) {
    signal(SIGPIPE, SIG_IGN);
    pw_init(nullptr, nullptr);
    int status = 1;
    try {
        // Input state belongs to compositor-owned per-client devices. Reaping
        // the prior process closes its D-Bus peer; there is no global key journal.
        if (argc == 2 && std::string(argv[1]) == "recover-input") { pw_deinit(); return 0; }
        if (argc != 2 && argc != 3 && argc != 5) throw std::runtime_error("usage: media inventory | capture SCREEN_ID FPS desktop | input SCREEN_ID");
        if (argc == 2 && std::string(argv[1]) != "inventory") throw std::runtime_error("Unsupported Linux media operation");
        if (argc == 3 && std::string(argv[1]) != "input") throw std::runtime_error("Unsupported Linux media operation");
        if (argc == 5 && (std::string(argv[1]) != "capture" || std::string(argv[4]) != "desktop"))
            throw std::runtime_error("Unsupported Linux media operation");
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
