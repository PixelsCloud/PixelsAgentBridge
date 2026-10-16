// Opt-in GUI test: own GTK/WebKit window only. No screen capture or input into
// other applications. Use G_DEBUG=fatal-warnings to catch stale GTK ownership.
#include "../native/linux/host.h"
#include <webkit2/webkit2.h>
#include <cassert>
#include <functional>

using namespace pab_linux;
static unsigned checks = 0, retired = 0;
static void expect(bool value) { if (!value) throw std::runtime_error("Native GTK host assertion failed"); ++checks; }
static void until(const std::function<bool()>& ready) {
    auto deadline = Clock::now()+std::chrono::seconds(5);
    while (!ready()) {
        pump(); std::this_thread::sleep_for(std::chrono::milliseconds(2));
        if (Clock::now() >= deadline) throw std::runtime_error("Native GTK host test timed out");
    }
}
static void settle() {
    auto deadline = Clock::now()+std::chrono::milliseconds(100);
    do { pump(); std::this_thread::sleep_for(std::chrono::milliseconds(2)); } while (Clock::now()<deadline);
}
static int javascript(WebKitWebView* web, const char* script) {
    struct Result { bool done = false; int value = -1; } result;
    webkit_web_view_evaluate_javascript(web,script,-1,nullptr,nullptr,nullptr,
        [](GObject* web, GAsyncResult* async, gpointer context) {
            auto& result = *static_cast<Result*>(context); GError* error = nullptr;
            auto* value = webkit_web_view_evaluate_javascript_finish(WEBKIT_WEB_VIEW(web),async,&error);
            if (value) { result.value = jsc_value_to_int32(value); g_object_unref(value); }
            if (error) g_error_free(error);
            result.done = true;
        },&result);
    until([&] { return result.done; }); return result.value;
}
int main(int argc, char** argv) {
    try {
        expect(gtk_init_check(nullptr,nullptr));
        auto* window = gtk_window_new(GTK_WINDOW_TOPLEVEL); g_object_ref_sink(window);
        gtk_window_set_title(GTK_WINDOW(window),"Pixels native host test");
        gtk_window_set_default_size(GTK_WINDOW(window),960,600);
        auto* root = gtk_box_new(GTK_ORIENTATION_VERTICAL,0);
        auto* web = webkit_web_view_new();
        gtk_box_pack_start(GTK_BOX(root),web,TRUE,TRUE,0);
        gtk_container_add(GTK_CONTAINER(window),root);
        bool loaded = false;
        g_signal_connect(web,"load-changed",G_CALLBACK(+[](WebKitWebView*,WebKitLoadEvent event,gpointer data) {
            if (event == WEBKIT_LOAD_FINISHED) *static_cast<bool*>(data)=true;
        }),&loaded);
        webkit_web_view_load_html(WEBKIT_WEB_VIEW(web),
            "<!doctype html><style>body{margin:0}button{height:72px}</style><button>Host toolbar test</button><script>window.pabTest=73</script>",nullptr);
        gtk_widget_show_all(window); until([&] { return loaded; });
        expect(javascript(WEBKIT_WEB_VIEW(web),"window.pabTest") == 73);
        if (argc == 2 && std::string(argv[1]) == "--gtk-baseline") {
            // Isolate GTK/driver lifetime diagnostics without our presenter,
            // shaders, textures, overlay, reparenting or native host ABI.
            for (unsigned cycle=0;cycle<3;++cycle) {
                auto* area = gtk_gl_area_new();
                gtk_gl_area_set_required_version(GTK_GL_AREA(area),3,2);
                gtk_widget_set_size_request(area,200,200);
                g_signal_connect(area,"render",G_CALLBACK(+[](GtkGLArea*,GdkGLContext*,gpointer) -> gboolean {
                    glClearColor(.2,.3,.4,1); glClear(GL_COLOR_BUFFER_BIT); glFlush(); return TRUE;
                }),nullptr);
                gtk_box_pack_start(GTK_BOX(root),area,TRUE,TRUE,0); gtk_widget_show(area);
                settle(); gtk_widget_destroy(area); settle();
            }
            gtk_widget_destroy(window); g_object_unref(window);
            printf("{\"gtk_baseline\":true,\"cycles\":3}\n"); return 0;
        }
        std::vector<uint8_t> frame(64*32*3/2,128);
        std::fill(frame.begin(),frame.begin()+64*32,72);
        char error[512]{}; uint64_t count = 0;
        expect(!pab_linux_view_new(window,63,32,0,error,sizeof(error)));
        expect(gtk_bin_get_child(GTK_BIN(window)) == root);
        for (unsigned cycle=0; cycle<20; ++cycle) {
            auto* view = pab_linux_view_new(window,64,32,cycle%4,error,sizeof(error));
            expect(view != nullptr);
            auto* overlay = gtk_bin_get_child(GTK_BIN(window));
            expect(GTK_IS_OVERLAY(overlay) && gtk_bin_get_child(GTK_BIN(overlay)) == root);
            auto* children = gtk_container_get_children(GTK_CONTAINER(overlay)); GtkWidget* area = nullptr;
            for (auto* p=children;p;p=p->next) if (GTK_IS_GL_AREA(p->data)) area=GTK_WIDGET(p->data);
            g_list_free(children); expect(area != nullptr);
            g_object_weak_ref(G_OBJECT(area),+[](gpointer,GObject*) { ++retired; },nullptr);
            expect(gtk_widget_get_margin_top(area) == 72);
            expect(pab_linux_view_submit(view,frame.data(),frame.size(),0,error,sizeof(error)));
            expect(pab_linux_view_poll(view,&count,error,sizeof(error)) && count == 0);
            expect(!pab_linux_view_submit(view,frame.data(),frame.size(),0,error,sizeof(error)));
            until([&] { return pab_linux_view_poll(view,&count,error,sizeof(error)) && count == 1; });
            expect(count == 1);
            gtk_gl_area_queue_render(GTK_GL_AREA(area)); settle();
            expect(pab_linux_view_poll(view,&count,error,sizeof(error)) && count == 1);
            expect(!pab_linux_view_submit(view,frame.data(),frame.size()-1,1,error,sizeof(error)));
            if (cycle == 0) {
                gtk_widget_hide(window); settle();
                expect(pab_linux_view_submit(view,frame.data(),frame.size(),1,error,sizeof(error)));
                settle(); expect(pab_linux_view_poll(view,&count,error,sizeof(error)) && count == 1);
                gtk_widget_show(window);
                until([&] { return pab_linux_view_poll(view,&count,error,sizeof(error)) && count == 2; });
                expect(count == 2);
            }
            pab_linux_view_drop(view);
            expect(gtk_bin_get_child(GTK_BIN(window)) == root && gtk_widget_get_realized(window));
            expect(retired == cycle+1);
            expect(javascript(WEBKIT_WEB_VIEW(web),"++window.pabTest") == int(74+cycle));
        }
        auto* view = pab_linux_view_new(window,64,32,0,error,sizeof(error)); expect(view != nullptr);
        expect(pab_linux_view_submit(view,frame.data(),frame.size(),0,error,sizeof(error)));
        gtk_widget_destroy(window);
        expect(!pab_linux_view_poll(view,&count,error,sizeof(error)));
        pab_linux_view_drop(view); g_object_unref(window);
        printf("{\"passed\":true,\"checks\":%u,\"cycles\":20,\"retired_areas\":%u,\"webkit_state_preserved\":true,\"hidden_frame_ack_withheld\":true}\n",checks,retired);
        return 0;
    } catch (const std::exception& error) { fprintf(stderr,"%s\n",error.what()); return 1; }
}
