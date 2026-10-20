// Isolated diagnostic: production PipeWire frame extraction, optional codec, no input.
// Samples generated GTK colors; optional packets stay in private test artifacts.
#include "../../crates/desktop-media/native/linux/capture.h"
#include <gtk/gtk.h>
#include <array>
#include <dlfcn.h>
#include <fstream>

static unsigned width, height, ticks;
static std::vector<std::array<unsigned, 3>> observed;
static std::array<unsigned, 3> expected;
static GtkWidget* fixture;
static bool history;
static bool fading;
static bool (*encode_actual)(void*, const uint8_t*, size_t, size_t, PabCodecBytes*);
static bool (*decode_actual)(void*, const uint8_t*, size_t, PabCodecBytes*);
static void* decoder;
static std::vector<std::array<unsigned, 3>> decoded;

extern "C" bool pab_soft_encode(void* encoder, const uint8_t* data, size_t size, size_t stride, PabCodecBytes* output) {
    static const uint8_t sentinel = 1;
    if (!data || stride < width * 4 || size < (height - 1) * stride + width * 4) return false;
    observed.clear();
    for (unsigned y : {height / 3, height / 2, height * 3 / 4}) {
        for (unsigned x : {width / 4, width / 2, width * 3 / 4}) {
            const auto* p = data + y * stride + x * 4;
            observed.push_back({p[2], p[1], p[0]});
        }
    }
    if (encode_actual) {
        if (!encode_actual(encoder, data, size, stride, output)) return false;
        PabCodecBytes frame{};
        if (!decode_actual(decoder, output->data, output->size, &frame) || frame.size != width * height * 3 / 2) return false;
        decoded.clear();
        for (unsigned y : {height / 3, height / 2, height * 3 / 4}) {
            for (unsigned x : {width / 4, width / 2, width * 3 / 4}) {
                const size_t uv = width * height + y / 2 * width + x / 2 * 2;
                decoded.push_back({frame.data[y * width + x], frame.data[uv], frame.data[uv + 1]});
            }
        }
    } else { *output = {&sentinel, 1}; }
    return true;
}
extern "C" void pab_soft_encoder_drop(void*) {}

static gboolean draw(GtkWidget*, cairo_t* cr, gpointer) {
    cairo_set_source_rgb(cr, .08, .32, .29); cairo_paint(cr);
    if (history && ticks < 20) {
        const int w=gtk_widget_get_allocated_width(fixture), h=gtk_widget_get_allocated_height(fixture);
        for (int y=0;y<h;y+=8) for (int x=0;x<w;x+=8) {
            const int col=x/8,row=y/8;
            cairo_set_source_rgb(cr,(col*17+row*31)&1,((col*3^row*13)&2)/2.,((col*7+row*11)&4)/4.);
            cairo_rectangle(cr,x,y,8,8);cairo_fill(cr);
        }
        if (fading) {
            cairo_set_source_rgba(cr,.08,.32,.29,ticks/20.);cairo_paint(cr);
        }
    }
    cairo_set_source_rgb(cr, 1, 1, 1); cairo_set_font_size(cr, 26);
    cairo_move_to(cr, 36, 72); cairo_show_text(cr, "Pixels isolated remote desktop acceptance");
    cairo_set_source_rgb(cr, ticks % 2, 0, 0);
    cairo_rectangle(cr, 0, 0, 16, 16); cairo_fill(cr);
    return TRUE;
}
static gboolean tick(gpointer) { ++ticks; gtk_widget_queue_draw(fixture); return G_SOURCE_CONTINUE; }
int main(int argc, char** argv) {
    try {
        fading = argc == 4 && std::string(argv[3]) == "fade";
        history = fading || (argc == 4 && std::string(argv[3]) == "history");
        gtk_init(&argc, &argv); pw_init(nullptr, nullptr);
        auto* surface = cairo_image_surface_create(CAIRO_FORMAT_ARGB32, 2, 2);
        auto* cr = cairo_create(surface);
        cairo_set_source_rgb(cr, .08, .32, .29); cairo_paint(cr); cairo_surface_flush(surface);
        const auto* p = cairo_image_surface_get_data(surface); expected = {p[2], p[1], p[0]};
        cairo_destroy(cr); cairo_surface_destroy(surface);
        fixture = gtk_window_new(GTK_WINDOW_TOPLEVEL);
        gtk_window_set_title(GTK_WINDOW(fixture), "Pixels source color diagnostic");
        gtk_widget_add_events(fixture, GDK_STRUCTURE_MASK);
        g_signal_connect(fixture, "draw", G_CALLBACK(draw), nullptr);
        gtk_widget_show_all(fixture); gtk_window_fullscreen(GTK_WINDOW(fixture));
        // Hide the pointer only inside this fixture: embedded cursor pixels are
        // not source-color samples and must not be mistaken for conversion drift.
        auto* cursor = gdk_cursor_new_for_display(gtk_widget_get_display(fixture), GDK_BLANK_CURSOR);
        gdk_window_set_cursor(gtk_widget_get_window(fixture), cursor); g_object_unref(cursor);
        const auto timer = g_timeout_add(100, tick, nullptr);
        for (unsigned n = 0; n < 40; ++n) { pab_linux::pump(); g_usleep(25000); }
        auto bus = pab_linux::session_bus(); const auto screens = pab_linux::displays(bus.get());
        const auto selected = std::find_if(screens.begin(), screens.end(), [](const auto& d) { return d.primary; });
        if (selected == screens.end()) throw std::runtime_error("No primary screen");
        width = selected->size.width; height = selected->size.height;
        void* library = nullptr; void* encoder = nullptr;
        if (argc >= 2) {
            library = dlopen(argv[1], RTLD_NOW | RTLD_LOCAL);
            if (!library) throw std::runtime_error("Cannot load diagnostic codec library");
            encode_actual = reinterpret_cast<decltype(encode_actual)>(dlsym(library, "pab_soft_encode"));
            decode_actual = reinterpret_cast<decltype(decode_actual)>(dlsym(library, "pab_soft_decode"));
            auto create_encoder = reinterpret_cast<void*(*)(uint32_t,uint32_t,uint16_t)>(dlsym(library,"pab_soft_encoder_new"));
            auto create_decoder = reinterpret_cast<void*(*)(uint32_t,uint32_t)>(dlsym(library,"pab_soft_decoder_new"));
            if (!encode_actual || !decode_actual || !create_encoder || !create_decoder) throw std::runtime_error("Missing codec entry point");
            encoder=create_encoder(width,height,15); decoder=create_decoder(width,height);
            if (!encoder || !decoder) throw std::runtime_error("Cannot create diagnostic codec");
        }
        printf("{\"dimensions\":[%u,%u],\"cairo_rgb\":[%u,%u,%u],\"frames\":[", width,height,expected[0],expected[1],expected[2]);
        std::ofstream packets;
        if (argc >= 3) { packets.open(argv[2], std::ios::binary | std::ios::trunc); if (!packets) throw std::runtime_error("Cannot open generated packet output"); }
        size_t saved_bytes = 0;
        {
            pab_linux::Capture capture(bus.get(), *selected, 15); capture.start();
            for (unsigned index=0; index<30; ++index) {
                const auto encoded = capture.next(encoder);
                if (packets.is_open()) {
                    if (!gtk_window_is_active(GTK_WINDOW(fixture)) ||
                        !std::all_of(observed.begin(), observed.end(), [](const auto& v) {
                            if (v==expected || (history && std::all_of(v.begin(),v.end(),[](unsigned c){return c==0||c==255;}))) return true;
                            if (fading) for (unsigned phase=0;phase<=20;++phase) {
                                bool valid=true;
                                for (unsigned c=0;c<3;++c) {
                                    const double alpha=phase/20.;
                                    valid=valid && (std::abs(v[c]-expected[c]*alpha)<=2 || std::abs(v[c]-(expected[c]*alpha+255*(1-alpha)))<=2);
                                }
                                if(valid)return true;
                            }
                            return false;
                        }))
                        throw std::runtime_error("Generated fixture not active/visible; no packet saved");
                    saved_bytes += encoded.bytes.size;
                    if (saved_bytes > 16 * 1024 * 1024) throw std::runtime_error("Generated packet bound exceeded");
                    const uint32_t size=encoded.bytes.size;
                    const uint8_t length[4]={uint8_t(size),uint8_t(size>>8),uint8_t(size>>16),uint8_t(size>>24)};
                    packets.write(reinterpret_cast<const char*>(length),4);
                    packets.write(reinterpret_cast<const char*>(encoded.bytes.data),size);
                    if (!packets) throw std::runtime_error("Generated packet output failed");
                }
                if (index) printf(","); if (library) printf("{\"rgb\":"); printf("[");
                for (size_t i=0; i<observed.size(); ++i) {
                    if (i) printf(","); const auto& v=observed[i]; printf("[%u,%u,%u]",v[0],v[1],v[2]);
                }
                printf("]");
                if (library) {
                    printf(",\"yuv\":[");
                    for (size_t i=0;i<decoded.size();++i) { if(i)printf(",");const auto&v=decoded[i];printf("[%u,%u,%u]",v[0],v[1],v[2]); }
                    printf("]}");
                }
            }
        }
        if (library) {
            reinterpret_cast<void(*)(void*)>(dlsym(library,"pab_soft_encoder_drop"))(encoder);
            reinterpret_cast<void(*)(void*)>(dlsym(library,"pab_soft_decoder_drop"))(decoder);
            dlclose(library);
        }
        printf("]}\n"); g_source_remove(timer); gtk_widget_destroy(fixture); pab_linux::pump(); pw_deinit();
        return 0;
    } catch (const std::exception& e) { fprintf(stderr,"source color: %s\n",e.what()); return 1; }
}
