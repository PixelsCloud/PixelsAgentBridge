// Bounded own-window diagnostic. No remote connection, capture or input.
// Compare baseline WebKit against the production native presenter while an
// external AT-SPI client traverses only this process's accessibility tree.
#include "../../crates/desktop-media/native/linux/host.h"
#include <webkit2/webkit2.h>
#include <cstdio>

using namespace pab_linux;
int main(int argc, char** argv) {
    if (argc != 2 || (std::string(argv[1]) != "baseline" && std::string(argv[1]) != "host" && std::string(argv[1]) != "cycle" && std::string(argv[1]) != "retired-button")) return 2;
    if (!gtk_init_check(nullptr, nullptr)) return 3;
    auto* window = gtk_window_new(GTK_WINDOW_TOPLEVEL);
    g_object_ref_sink(window);
    gtk_window_set_title(GTK_WINDOW(window), "Pixels accessibility diagnostic");
    gtk_window_set_default_size(GTK_WINDOW(window), 640, 400);
    auto* root = gtk_box_new(GTK_ORIENTATION_VERTICAL, 0);
    GtkWidget* retired_button = nullptr;
    AtkObject* retained_accessible = nullptr;
    if (std::string(argv[1]) == "retired-button") {
        retired_button = gtk_button_new_with_label("Owned temporary button");
        gtk_box_pack_start(GTK_BOX(root), retired_button, FALSE, FALSE, 0);
        // Accessibility clients can retain a reference beyond widget lifetime.
        retained_accessible = ATK_OBJECT(g_object_ref(gtk_widget_get_accessible(retired_button)));
    }
    auto* web = webkit_web_view_new();
    gtk_box_pack_start(GTK_BOX(root), web, TRUE, TRUE, 0);
    gtk_container_add(GTK_CONTAINER(window), root);
    bool loaded = false;
    g_signal_connect(web, "load-changed", G_CALLBACK(+[](WebKitWebView*, WebKitLoadEvent event, gpointer data) {
        if (event == WEBKIT_LOAD_FINISHED) *static_cast<bool*>(data) = true;
    }), &loaded);
    webkit_web_view_load_html(WEBKIT_WEB_VIEW(web),
        "<!doctype html><title>Owned probe</title><button>Owned toolbar</button><input aria-label='Owned field'>", nullptr);
    gtk_widget_show_all(window);
    const auto deadline = Clock::now() + std::chrono::seconds(15);
    while (!loaded && Clock::now() < deadline) { pump(); std::this_thread::sleep_for(std::chrono::milliseconds(5)); }
    if (!loaded) { gtk_widget_destroy(window); g_object_unref(window); return 4; }
    char error[512]{};
    void* view = nullptr;
    if (std::string(argv[1]) == "host") {
        view = pab_linux_view_new(window, 64, 32, 0, error, sizeof(error));
        if (!view) { fprintf(stderr, "%s\n", error); gtk_widget_destroy(window); g_object_unref(window); return 5; }
        std::vector<uint8_t> pixels(64 * 32 * 3 / 2, 128);
        if (!pab_linux_view_submit(view, pixels.data(), pixels.size(), 0, error, sizeof(error))) {
            pab_linux_view_drop(view); gtk_widget_destroy(window); g_object_unref(window); return 6;
        }
    }
    printf("{\"ready\":true,\"pid\":%d,\"mode\":\"%s\"}\n", int(getpid()), argv[1]); fflush(stdout);
    const auto end = Clock::now() + std::chrono::seconds(8);
    const auto retire_at = Clock::now() + std::chrono::seconds(2);
    auto transition = Clock::now() + std::chrono::milliseconds(500);
    unsigned cycles = 0;
    int exit_code = 0;
    while (Clock::now() < end) {
        pump();
        if (retired_button && Clock::now() >= retire_at) {
            gtk_widget_destroy(retired_button); retired_button = nullptr;
            auto* states = atk_object_ref_state_set(retained_accessible);
            printf("{\"retired_button\":true,\"widget_null\":%s,\"state_null\":%s}\n",
                gtk_accessible_get_widget(GTK_ACCESSIBLE(retained_accessible)) ? "false" : "true", states ? "false" : "true");
            fflush(stdout);
            if (states) g_object_unref(states);
        }
        if (std::string(argv[1]) == "cycle" && Clock::now() >= transition) {
            if (view) { pab_linux_view_drop(view); view = nullptr; }
            else {
                view = pab_linux_view_new(window, 64, 32, 0, error, sizeof(error));
                if (!view) { fprintf(stderr, "%s\n", error); exit_code = 7; break; }
                ++cycles;
            }
            transition = Clock::now() + std::chrono::milliseconds(500);
        }
        std::this_thread::sleep_for(std::chrono::milliseconds(5));
    }
    if (view) pab_linux_view_drop(view);
    if (retained_accessible) g_object_unref(retained_accessible);
    gtk_widget_destroy(window); g_object_unref(window);
    printf("{\"finished\":%s,\"cycles\":%u}\n", exit_code ? "false" : "true", cycles);
    return exit_code;
}
