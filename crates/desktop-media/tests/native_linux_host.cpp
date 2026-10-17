// Opt-in GUI test: own GTK/WebKit window only. No screen capture or input into
// other applications. Use G_DEBUG=fatal-warnings to catch stale GTK ownership.
#include "../native/linux/host.h"
#include <webkit2/webkit2.h>
#include <cassert>
#include <functional>

using namespace pab_linux;
static unsigned checks = 0, retired = 0;
struct InputLog { std::vector<InputPacket> packets; bool dropped=false; unsigned raw_scroll=0; double raw_scroll_y=0; };
static void expect(bool value,const char* detail="unspecified") { if (!value) throw std::runtime_error(std::string("Native GTK host assertion failed: ")+detail); ++checks; }
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
        if (argc==3 && std::string(argv[1])=="--input-probe") {
            const auto rotation=std::stoul(argv[2]); expect(rotation<=3);
            gtk_window_fullscreen(GTK_WINDOW(window));
            until([&] { return (gdk_window_get_state(gtk_widget_get_window(window))&GDK_WINDOW_STATE_FULLSCREEN)!=0; });
            settle(); char error[512]{};
            auto* view=pab_linux_view_new(window,1920,1080,rotation,error,sizeof(error)); expect(view!=nullptr);
            InputLog input;
            expect(pab_linux_view_set_input(view,&input,+[](void* context,const uint8_t* packet) {
                auto& log=*static_cast<InputLog*>(context); InputPacket bytes{};
                std::copy(packet,packet+8,bytes.begin()); log.packets.push_back(bytes); return true;
            },+[](void* context) { static_cast<InputLog*>(context)->dropped=true; },error,sizeof(error)));
            std::vector<uint8_t> pixels(1920*1080*3/2,128); uint64_t displayed=0;
            expect(pab_linux_view_submit(view,pixels.data(),pixels.size(),0,error,sizeof(error)));
            until([&] { return pab_linux_view_poll(view,&displayed,error,sizeof(error)) && displayed==1; });
            auto* overlay=gtk_bin_get_child(GTK_BIN(window)); auto* children=gtk_container_get_children(GTK_CONTAINER(overlay));
            GtkWidget* area=nullptr;
            for (auto* p=children;p;p=p->next) if (GTK_IS_GL_AREA(p->data)) area=GTK_WIDGET(p->data);
            g_list_free(children); expect(area!=nullptr && gtk_widget_get_scale_factor(area)==1);
            g_signal_connect(area,"event",G_CALLBACK(+[](GtkWidget*,GdkEvent* event,gpointer data)->gboolean {
                if (event->type==GDK_SCROLL) {
                    auto& log=*static_cast<InputLog*>(data); ++log.raw_scroll;
                    log.raw_scroll_y+=event->scroll.direction==GDK_SCROLL_SMOOTH ? event->scroll.delta_y : event->scroll.direction==GDK_SCROLL_DOWN ? 1 : -1;
                }
                return FALSE;
            }),&input);
            int ax=0,ay=0; expect(gtk_widget_translate_coordinates(area,window,0,0,&ax,&ay));
            const int w=gtk_widget_get_allocated_width(area),h=gtk_widget_get_allocated_height(area);
            const auto v=viewport(1920,1080,rotation,w,h); const int top=h-v.y-v.height;
            expect(pab_linux_view_input(view,true,error,sizeof(error))); settle(); input.packets.clear();
            printf("{\"ready\":true,\"rotation\":%lu,\"window\":[%d,%d],\"points\":[[%f,%f],[%f,%f]]}\n",rotation,
                gtk_widget_get_allocated_width(window),gtk_widget_get_allocated_height(window),
                ax+v.x+(v.width-1)*.25,ay+top+(v.height-1)*.25,ax+v.x+(v.width-1)*.75,ay+top+(v.height-1)*.75); fflush(stdout);
            until([&] { pollfd pipe{STDIN_FILENO,POLLIN,0}; return poll(&pipe,1,0)>0; });
            char command=0; expect(read(STDIN_FILENO,&command,1)==1 && command=='v'); settle();
            gtk_widget_hide(window); settle();
            const auto contains=[&](uint8_t kind,uint16_t value,bool down) {
                return std::any_of(input.packets.begin(),input.packets.end(),[&](const auto& p) {
                    return p[0]==kind && uint16_t(p[5]|(p[6]<<8))==value && p[7]==uint8_t(down);
                });
            };
            const auto click=[&](bool down,double expected) {
                return std::any_of(input.packets.begin(),input.packets.end(),[&](const auto& p) {
                    return p[0]==2 && p[5]==0 && p[7]==uint8_t(down) &&
                        std::abs(double(p[1]|(p[2]<<8))-65535*expected)<400 &&
                        std::abs(double(p[3]|(p[4]<<8))-65535*expected)<400;
                });
            };
            expect(click(true,.25),"quarter click down"); expect(click(false,.25),"quarter click up"); expect(click(false,.75),"drag release at three quarters");
            expect(contains(4,65,true) && contains(4,65,false),"physical letter down/up");
            expect(contains(4,160,true) && contains(4,160,false),"physical shift down/up");
            expect(contains(4,162,true),"physical control down");
            if (!contains(3,uint16_t(-120),false)) {
                unsigned samples=0; int total=0;
                for (const auto& packet : input.packets) if (packet[0]==3) { ++samples; total+=int16_t(packet[5]|(packet[6]<<8)); }
                fprintf(stderr,"generated wheel samples=%u total=%d raw_samples=%u raw_y=%f\n",samples,total,input.raw_scroll,input.raw_scroll_y);
            }
            expect(contains(3,uint16_t(-120),false),"wheel direction/units");
            expect(!input.packets.empty() && input.packets.back()[0]==5,"unmap release");
            pab_linux_view_drop(view); expect(input.dropped); gtk_widget_destroy(window); g_object_unref(window);
            printf("{\"passed\":true,\"rotation\":%lu,\"real_input\":true,\"click_drag_coordinates\":true,\"physical_keys\":true,\"wheel\":true,\"unmap_release\":true,\"input_contents_saved\":false}\n",rotation);
            return 0;
        }
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
            InputLog input;
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
            if (cycle<4) {
                expect(pab_linux_view_set_input(view,&input,+[](void* context,const uint8_t* packet) {
                    auto& log=*static_cast<InputLog*>(context); InputPacket bytes{};
                    std::copy(packet,packet+8,bytes.begin()); log.packets.push_back(bytes); return true;
                },+[](void* context) { static_cast<InputLog*>(context)->dropped=true; },error,sizeof(error)));
                GdkEventKey key{}; key.type=GDK_KEY_PRESS; key.hardware_keycode=KEY_A+8; gboolean handled=FALSE;
                g_signal_emit_by_name(area,"key-press-event",&key,&handled); expect(input.packets.empty());
                expect(pab_linux_view_input(view,true,error,sizeof(error)));
                expect(input.packets.back()[0]==6 && input.packets.back()[5]==1);
                g_signal_emit_by_name(area,"key-press-event",&key,&handled);
                expect(input.packets.back()==InputPacket({4,0,0,0,0,65,0,1}));
                auto size=input.packets.size(); g_signal_emit_by_name(area,"key-press-event",&key,&handled);
                expect(input.packets.size()==size);
                key.type=GDK_KEY_RELEASE; g_signal_emit_by_name(area,"key-release-event",&key,&handled);
                expect(input.packets.back()==InputPacket({4,0,0,0,0,65,0,0}));
                const int scale=gtk_widget_get_scale_factor(area),w=gtk_widget_get_allocated_width(area)*scale,h=gtk_widget_get_allocated_height(area)*scale;
                const auto v=viewport(64,32,cycle%4,w,h); const int top=h-v.y-v.height;
                GdkEventButton button{}; button.type=GDK_BUTTON_PRESS; button.button=2;
                button.x=double(v.x+v.width-1)/scale; button.y=double(top+v.height-1)/scale;
                g_signal_emit_by_name(area,"button-press-event",&button,&handled);
                expect(input.packets.back()==InputPacket({2,255,255,255,255,2,0,1}));
                GdkEventMotion move{}; move.type=GDK_MOTION_NOTIFY; move.x=-10; move.y=-10;
                g_signal_emit_by_name(area,"motion-notify-event",&move,&handled);
                expect(input.packets.back()==InputPacket({1,0,0,0,0,0,0,0}));
                button.type=GDK_BUTTON_RELEASE; button.x=-10; button.y=-10;
                g_signal_emit_by_name(area,"button-release-event",&button,&handled);
                expect(input.packets.back()==InputPacket({2,0,0,0,0,2,0,0}));
                size=input.packets.size(); g_signal_emit_by_name(area,"motion-notify-event",&move,&handled);
                expect(input.packets.size()==size);
                GdkEventScroll scroll{}; scroll.type=GDK_SCROLL; scroll.direction=GDK_SCROLL_DOWN;
                scroll.x=double(v.x)/scale; scroll.y=double(top)/scale;
                g_signal_emit_by_name(area,"scroll-event",&scroll,&handled);
                expect(input.packets.back()==InputPacket({3,0,0,0,0,136,255,0}));
                GdkEventFocus focus{}; focus.type=GDK_FOCUS_CHANGE; focus.in=FALSE;
                g_signal_emit_by_name(area,"focus-out-event",&focus,&handled);
                expect(input.packets.back()[0]==5);
                expect(pab_linux_view_input(view,false,error,sizeof(error)));
                expect(input.packets.back()[0]==6 && input.packets.back()[5]==0);
                size=input.packets.size(); key.type=GDK_KEY_PRESS;
                g_signal_emit_by_name(area,"key-press-event",&key,&handled); expect(input.packets.size()==size);
            }
            if (cycle == 0) {
                gtk_widget_hide(window); settle();
                expect(pab_linux_view_submit(view,frame.data(),frame.size(),1,error,sizeof(error)));
                settle(); expect(pab_linux_view_poll(view,&count,error,sizeof(error)) && count == 1);
                gtk_widget_show(window);
                until([&] { return pab_linux_view_poll(view,&count,error,sizeof(error)) && count == 2; });
                expect(count == 2);
            }
            pab_linux_view_drop(view);
            if (cycle<4) expect(input.dropped);
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
