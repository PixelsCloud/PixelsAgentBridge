/* Bounded GNOME/PipeWire feasibility probe. Consumes CPU-mappable frames for
 * eight seconds and emits only dimensions/counts, never pixels or input.
 * API reference: Mutter 42.9 ScreenCast XML and PipeWire 0.3.48 video-play.c.
 * This is a GNOME-specific experiment, not a generic Wayland backend. */
#include <gio/gio.h>
#include <gtk/gtk.h>
#include <pipewire/pipewire.h>
#include <spa/param/video/format-utils.h>
#include <inttypes.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <stdbool.h>

#ifdef PAB_CAPTURE_CODEC_PROBE
struct codec_bytes { const uint8_t *data; size_t len; };
extern void *pab_soft_encoder_new(uint32_t, uint32_t, uint16_t);
extern void pab_soft_encoder_drop(void *);
extern bool pab_soft_encode(void *, const uint8_t *, size_t, size_t, struct codec_bytes *);
extern void *pab_soft_decoder_new(uint32_t, uint32_t);
extern void pab_soft_decoder_drop(void *);
extern bool pab_soft_decode(void *, const uint8_t *, size_t, struct codec_bytes *);
#endif

struct capture {
    GMainLoop *main;
    struct pw_thread_loop *loop;
    struct pw_stream *stream;
    struct spa_video_info_raw format;
    uint32_t node;
    uint64_t frames, changed, previous_hash, invalid;
    gint64 first_us, last_us;
    GtkWidget *fixture;
    uint64_t fixture_ticks;
#ifdef PAB_CAPTURE_CODEC_PROBE
    void *encoder, *decoder;
    uint64_t encoded, decoded, encoded_bytes, encode_us, decode_us, luma_error, luma_samples;
#endif
};

static gboolean draw_fixture(GtkWidget *widget, cairo_t *cr, gpointer opaque) {
    struct capture *c = opaque;
    double width = gtk_widget_get_allocated_width(widget), height = gtk_widget_get_allocated_height(widget);
    for (unsigned q = 0; q < 4; ++q) {
        cairo_set_source_rgb(cr, (32 + (c->fixture_ticks * 3 + q * 43) % 192) / 255.0,
            (32 + (c->fixture_ticks * 7 + q * 67) % 192) / 255.0,
            (32 + (c->fixture_ticks * 11 + q * 97) % 192) / 255.0);
        cairo_rectangle(cr, (q % 2) * width / 2, (q / 2) * height / 2, width / 2, height / 2);
        cairo_fill(cr);
    }
    return TRUE;
}
static gboolean animate(gpointer opaque) {
    struct capture *c = opaque;
    ++c->fixture_ticks; gtk_widget_queue_draw(c->fixture); return G_SOURCE_CONTINUE;
}

static void process_frame(void *opaque) {
    struct capture *c = opaque;
    struct pw_buffer *buffer = pw_stream_dequeue_buffer(c->stream);
    if (!buffer) return;
    struct spa_buffer *frame = buffer->buffer;
    if (!frame || frame->n_datas != 1 || !frame->datas[0].data || !frame->datas[0].chunk) goto invalid;
    struct spa_data *plane = &frame->datas[0];
    struct spa_chunk *chunk = plane->chunk;
    uint64_t w = c->format.size.width, h = c->format.size.height;
    if (!w || !h || w > 8192 || h > 8192 || chunk->stride < (int64_t)(w * 4) ||
        chunk->offset > plane->maxsize || chunk->size > plane->maxsize - chunk->offset ||
        (h - 1) * (uint64_t)chunk->stride + w * 4 > chunk->size) goto invalid;
    const uint8_t *data = (uint8_t *)plane->data + chunk->offset;
#ifdef PAB_CAPTURE_CODEC_PROBE
    if (!c->encoder || !c->decoder) goto invalid;
    struct codec_bytes packet = {0}, decoded = {0};
    gint64 codec_start = g_get_monotonic_time();
    if (!pab_soft_encode(c->encoder, data, chunk->size, chunk->stride, &packet) || !packet.len) goto invalid;
    gint64 encode_end = g_get_monotonic_time();
    c->encode_us += encode_end - codec_start;
    ++c->encoded; c->encoded_bytes += packet.len;
    if (!pab_soft_decode(c->decoder, packet.data, packet.len, &decoded)) goto invalid;
    c->decode_us += g_get_monotonic_time() - encode_end;
    if (decoded.len) {
        if (decoded.len != w * h * 3 / 2) goto invalid;
        ++c->decoded;
        for (uint32_t y = 0; y < h; y += 23)
            for (uint32_t x = 0; x < w; x += 19) {
                const uint8_t *p = data + (uint64_t)y * chunk->stride + x * 4;
                int expected_y = 16 + ((11966 * p[2] + 40254 * p[1] + 4064 * p[0] + 32768) >> 16);
                c->luma_error += abs(expected_y - decoded.data[(uint64_t)y * w + x]);
                ++c->luma_samples;
            }
    }
#endif
    uint64_t hash = 1469598103934665603ull;
    // Sparse diagnostic only: do not retain/export a screen image or its hash.
    for (uint32_t y = 0; y < h; y += 13)
        for (uint32_t x = 0; x < w * 4; x += 29) {
            hash ^= data[(uint64_t)y * chunk->stride + x]; hash *= 1099511628211ull;
        }
    c->last_us = g_get_monotonic_time();
    if (!c->frames) c->first_us = c->last_us;
    if (!c->frames || hash != c->previous_hash) ++c->changed;
    c->previous_hash = hash; ++c->frames;
    pw_stream_queue_buffer(c->stream, buffer);
    return;
invalid:
    ++c->invalid;
    pw_stream_queue_buffer(c->stream, buffer);
}

static void format_changed(void *opaque, uint32_t id, const struct spa_pod *param) {
    struct capture *c = opaque;
    if (id != SPA_PARAM_Format || !param) return;
    if (spa_format_video_raw_parse(param, &c->format) < 0 ||
        c->format.format != SPA_VIDEO_FORMAT_BGRx || !c->format.size.width ||
        !c->format.size.height || (uint64_t)c->format.size.width * c->format.size.height > 33554432) {
        pw_stream_set_error(c->stream, -EINVAL, "unsupported bounded BGRx format"); return;
    }
#ifdef PAB_CAPTURE_CODEC_PROBE
    pab_soft_encoder_drop(c->encoder); pab_soft_decoder_drop(c->decoder);
    c->encoder = pab_soft_encoder_new(c->format.size.width, c->format.size.height, 15);
    c->decoder = pab_soft_decoder_new(c->format.size.width, c->format.size.height);
    if (!c->encoder || !c->decoder) {
        pw_stream_set_error(c->stream, -EINVAL, "software codec initialization failed"); return;
    }
#endif
    uint8_t storage[1024];
    struct spa_pod_builder b = SPA_POD_BUILDER_INIT(storage, sizeof(storage));
    const struct spa_pod *buffers = spa_pod_builder_add_object(&b,
        SPA_TYPE_OBJECT_ParamBuffers, SPA_PARAM_Buffers,
        SPA_PARAM_BUFFERS_buffers, SPA_POD_CHOICE_RANGE_Int(4, 2, 8),
        SPA_PARAM_BUFFERS_blocks, SPA_POD_Int(1),
        SPA_PARAM_BUFFERS_size, SPA_POD_Int(c->format.size.width * c->format.size.height * 4),
        SPA_PARAM_BUFFERS_stride, SPA_POD_Int(c->format.size.width * 4),
        SPA_PARAM_BUFFERS_dataType, SPA_POD_CHOICE_FLAGS_Int((1 << SPA_DATA_MemFd) | (1 << SPA_DATA_MemPtr)));
    pw_stream_update_params(c->stream, &buffers, 1);
}

static void state_changed(void *opaque, enum pw_stream_state old, enum pw_stream_state state, const char *error) {
    (void)old;
    if (state == PW_STREAM_STATE_ERROR) {
        fprintf(stderr, "PipeWire error: %s\n", error ? error : "unknown");
        g_main_loop_quit(((struct capture *)opaque)->main);
    }
}
static const struct pw_stream_events events = {
    PW_VERSION_STREAM_EVENTS, .state_changed = state_changed,
    .param_changed = format_changed, .process = process_frame,
};

static void node_added(GDBusConnection *bus, const gchar *sender, const gchar *path,
                      const gchar *interface, const gchar *signal, GVariant *parameters, gpointer opaque) {
    (void)bus; (void)sender; (void)path; (void)interface; (void)signal;
    struct capture *c = opaque;
    if (c->stream) return;
    g_variant_get(parameters, "(u)", &c->node);
    pw_thread_loop_lock(c->loop);
    c->stream = pw_stream_new_simple(pw_thread_loop_get_loop(c->loop), "Pixels bounded capture probe",
        pw_properties_new(PW_KEY_MEDIA_TYPE, "Video", PW_KEY_MEDIA_CATEGORY, "Capture", PW_KEY_MEDIA_ROLE, "Screen", NULL),
        &events, c);
    if (c->stream) {
        uint8_t storage[1024];
        struct spa_pod_builder b = SPA_POD_BUILDER_INIT(storage, sizeof(storage));
        const struct spa_pod *format = spa_pod_builder_add_object(&b,
            SPA_TYPE_OBJECT_Format, SPA_PARAM_EnumFormat,
            SPA_FORMAT_mediaType, SPA_POD_Id(SPA_MEDIA_TYPE_video),
            SPA_FORMAT_mediaSubtype, SPA_POD_Id(SPA_MEDIA_SUBTYPE_raw),
            SPA_FORMAT_VIDEO_format, SPA_POD_Id(SPA_VIDEO_FORMAT_BGRx),
            SPA_FORMAT_VIDEO_size, SPA_POD_CHOICE_RANGE_Rectangle(&SPA_RECTANGLE(1920,1080), &SPA_RECTANGLE(1,1), &SPA_RECTANGLE(8192,8192)),
            SPA_FORMAT_VIDEO_framerate, SPA_POD_Fraction(&SPA_FRACTION(0,1)),
            SPA_FORMAT_VIDEO_maxFramerate, SPA_POD_Fraction(&SPA_FRACTION(15,1)));
        int result = pw_stream_connect(c->stream, PW_DIRECTION_INPUT, c->node,
            PW_STREAM_FLAG_AUTOCONNECT | PW_STREAM_FLAG_MAP_BUFFERS, &format, 1);
        if (result < 0) { fprintf(stderr, "PipeWire connect failed: %d\n", result); g_main_loop_quit(c->main); }
    } else { fprintf(stderr, "Cannot create PipeWire stream\n"); g_main_loop_quit(c->main); }
    pw_thread_loop_unlock(c->loop);
}

static gboolean expire(gpointer opaque) { g_main_loop_quit(opaque); return G_SOURCE_REMOVE; }
static GVariant *call(GDBusConnection *bus, const char *path, const char *interface, const char *method, GVariant *args) {
    GError *error = NULL;
    GVariant *result = g_dbus_connection_call_sync(bus, "org.gnome.Mutter.ScreenCast", path, interface, method,
        args, NULL, G_DBUS_CALL_FLAGS_NONE, 5000, NULL, &error);
    if (error) { fprintf(stderr, "%s: %s\n", method, error->message); g_error_free(error); }
    return result;
}

int main(int argc, char **argv) {
    bool fullscreen = argc == 3 && strcmp(argv[2], "--fullscreen") == 0;
    if ((argc != 2 && !(argc == 3 && (strcmp(argv[2], "--animate") == 0 || fullscreen))) || !argv[1][0]) {
        fprintf(stderr, "usage: capture-probe CONNECTOR [--animate|--fullscreen]\n"); return 2;
    }
    pw_init(NULL, NULL);
    struct capture c = {0};
    GError *error = NULL;
    GDBusConnection *bus = g_bus_get_sync(G_BUS_TYPE_SESSION, NULL, &error);
    if (!bus) { fprintf(stderr, "Session bus: %s\n", error->message); g_error_free(error); return 1; }
    gchar *session = NULL, *stream = NULL;
    guint subscription = 0, deadline = 0, animation = 0;
    gboolean loop_started = FALSE;
    c.main = g_main_loop_new(NULL, FALSE);
    if (argc == 3) {
        if (!gtk_init_check(NULL, NULL)) goto cleanup;
        c.fixture = gtk_window_new(GTK_WINDOW_TOPLEVEL);
        g_object_ref_sink(c.fixture);
        gtk_window_set_title(GTK_WINDOW(c.fixture), "Pixels remote desktop capture test");
        gtk_window_set_default_size(GTK_WINDOW(c.fixture), 800, 450);
        g_signal_connect(c.fixture, "draw", G_CALLBACK(draw_fixture), &c);
        g_signal_connect_swapped(c.fixture, "destroy", G_CALLBACK(g_main_loop_quit), c.main);
        gtk_widget_show_all(c.fixture);
        if (fullscreen) gtk_window_fullscreen(GTK_WINDOW(c.fixture));
        animation = g_timeout_add(33, animate, &c);
    }
    c.loop = pw_thread_loop_new("pixels-capture-probe", NULL);
    if (!c.loop || pw_thread_loop_start(c.loop) < 0) goto cleanup;
    loop_started = TRUE;
    GVariantBuilder props;
    g_variant_builder_init(&props, G_VARIANT_TYPE_VARDICT);
    GVariant *result = call(bus, "/org/gnome/Mutter/ScreenCast", "org.gnome.Mutter.ScreenCast", "CreateSession", g_variant_new("(a{sv})", &props));
    if (!result) goto cleanup;
    g_variant_get(result, "(o)", &session); g_variant_unref(result);
    g_variant_builder_init(&props, G_VARIANT_TYPE_VARDICT);
    g_variant_builder_add(&props, "{sv}", "cursor-mode", g_variant_new_uint32(1));
    result = call(bus, session, "org.gnome.Mutter.ScreenCast.Session", "RecordMonitor", g_variant_new("(sa{sv})", argv[1], &props));
    if (!result) goto cleanup;
    g_variant_get(result, "(o)", &stream); g_variant_unref(result);
    subscription = g_dbus_connection_signal_subscribe(bus, "org.gnome.Mutter.ScreenCast", "org.gnome.Mutter.ScreenCast.Stream",
        "PipeWireStreamAdded", stream, NULL, G_DBUS_SIGNAL_FLAGS_NONE, node_added, &c, NULL);
    result = call(bus, session, "org.gnome.Mutter.ScreenCast.Session", "Start", NULL);
    if (!result) goto cleanup;
    g_variant_unref(result);
    deadline = g_timeout_add_seconds(8, expire, c.main);
    g_main_loop_run(c.main);
cleanup:
    if (animation) g_source_remove(animation);
    if (deadline && g_main_context_find_source_by_id(NULL, deadline)) g_source_remove(deadline);
    if (subscription) g_dbus_connection_signal_unsubscribe(bus, subscription);
    if (c.loop) {
        if (loop_started) pw_thread_loop_stop(c.loop);
        if (c.stream) pw_stream_destroy(c.stream);
        pw_thread_loop_destroy(c.loop);
    }
    if (session) {
        GVariant *stopped = call(bus, session, "org.gnome.Mutter.ScreenCast.Session", "Stop", NULL);
        if (stopped) g_variant_unref(stopped);
    }
    printf("{\"node_created\":%s,\"width\":%u,\"height\":%u,\"frames\":%" PRIu64 ",\"changed_samples\":%" PRIu64 ",\"invalid_buffers\":%" PRIu64 ",\"sample_seconds\":%.6f,\"fixture_ticks\":%" PRIu64 ",\"max_fps_num\":%u,\"max_fps_den\":%u,\"pixel_bytes_saved\":0",
        c.node ? "true" : "false", c.format.size.width, c.format.size.height, c.frames, c.changed, c.invalid,
        c.frames > 1 ? (c.last_us - c.first_us) / 1000000.0 : 0.0, c.fixture_ticks,
        c.format.max_framerate.num, c.format.max_framerate.denom);
#ifdef PAB_CAPTURE_CODEC_PROBE
    printf(",\"encoded_frames\":%" PRIu64 ",\"decoded_frames\":%" PRIu64 ",\"encoded_bytes\":%" PRIu64 ",\"encode_average_ms\":%.6f,\"decode_average_ms\":%.6f,\"luma_mean_error\":%.6f",
        c.encoded, c.decoded, c.encoded_bytes, c.encoded ? (double)c.encode_us / c.encoded / 1000 : 0,
        c.decoded ? (double)c.decode_us / c.decoded / 1000 : 0,
        c.luma_samples ? (double)c.luma_error / c.luma_samples : 0);
    pab_soft_encoder_drop(c.encoder); pab_soft_decoder_drop(c.decoder);
#endif
    puts("}");
    if (c.fixture) { gtk_widget_destroy(c.fixture); g_object_unref(c.fixture); }
    g_free(session); g_free(stream); g_main_loop_unref(c.main); g_object_unref(bus); pw_deinit();
    bool success = c.frames && !c.invalid;
#ifdef PAB_CAPTURE_CODEC_PROBE
    success = success && c.encoded == c.frames && c.decoded == c.frames;
#endif
    return success ? 0 : 1;
}
