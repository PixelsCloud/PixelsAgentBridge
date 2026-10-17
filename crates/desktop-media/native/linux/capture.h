#pragma once
#include "gnome.h"
#include "cursor_channel.h"
#include "../software_codec_ffi.h"
#include <pipewire/pipewire.h>
#include <spa/param/video/format-utils.h>

namespace pab_linux {
struct FrameView { const uint8_t* data; size_t size, stride; };
inline FrameView frame_view(const spa_buffer* frame, unsigned width, unsigned height) {
    if (!frame || frame->n_datas != 1 || !frame->datas || !width || !height ||
        width > 8192 || height > 8192 || uint64_t(width) * height > 33554432)
        throw std::runtime_error("Invalid capture buffer");
    const auto& plane = frame->datas[0];
    const auto* chunk = plane.chunk;
    if (!plane.data || !chunk || (plane.type != SPA_DATA_MemFd && plane.type != SPA_DATA_MemPtr) ||
        (chunk->flags & SPA_CHUNK_FLAG_CORRUPTED) || chunk->stride < int64_t(width) * 4 ||
        chunk->offset > plane.maxsize || chunk->size > plane.maxsize - chunk->offset ||
        uint64_t(height - 1) * chunk->stride + uint64_t(width) * 4 > chunk->size ||
        chunk->size > 256 * 1024 * 1024)
        throw std::runtime_error("Capture stride, offset or extent outside mapped buffer");
    return {static_cast<const uint8_t*>(plane.data) + chunk->offset, chunk->size, size_t(chunk->stride)};
}
struct EncoderDrop { void operator()(void* p) const { pab_soft_encoder_drop(p); } };
using Encoder = std::unique_ptr<void, EncoderDrop>;
struct PwLock {
    pw_thread_loop* loop;
    explicit PwLock(pw_thread_loop* v) : loop(v) { pw_thread_loop_lock(loop); }
    ~PwLock() { pw_thread_loop_unlock(loop); }
};
struct Encoded { PabCodecBytes bytes; Clock::time_point sampled; };
class Capture {
    GDBusConnection* bus_;
    Display display_;
    unsigned fps_;
    CursorState* cursor_;
    std::string session_;
    std::vector<guint> subscriptions_;
    pw_thread_loop* loop_ = nullptr;
    pw_stream* stream_ = nullptr;
    pw_buffer* pending_ = nullptr;
    Clock::time_point sampled_{};
    const char* error_ = nullptr;
    bool started_ = false, formatted_ = false;
    // Except for setup/teardown, these fields are protected by the PipeWire
    // loop lock. The main thread owns D-Bus callbacks and the codec.
    static void state_changed(void* opaque, pw_stream_state, pw_stream_state state, const char*) noexcept {
        auto& c = *static_cast<Capture*>(opaque);
        if (state == PW_STREAM_STATE_ERROR || state == PW_STREAM_STATE_UNCONNECTED) {
            c.error_ = "PipeWire capture disconnected";
            if(c.cursor_)c.cursor_->fail(c.error_);
        }
    }
    static void removed(void* opaque, pw_buffer* buffer) noexcept {
        auto& c = *static_cast<Capture*>(opaque);
        if (c.pending_ == buffer) c.pending_ = nullptr;
    }
    static void process(void* opaque) noexcept {
        auto& c = *static_cast<Capture*>(opaque);
        // Keep one most recent pool buffer. No queue of CPU pixel copies, and
        // no encoded packets are generated while reliable transport is busy.
        for (unsigned i = 0; i < 8; ++i) {
            auto* next = pw_stream_dequeue_buffer(c.stream_);
            if (!next) break;
            if(c.cursor_) {
                try {
                    const auto* meta=spa_buffer_find_meta(next->buffer,SPA_META_Cursor);
                    if(!meta)throw std::runtime_error("PipeWire cursor metadata missing");
                    cursor_metadata(*c.cursor_,c.display_,*meta);
                } catch(const std::exception& error) { c.cursor_->fail(error.what()); }
            }
            // Mutter emits cursor-only buffers with a zero-sized pixel chunk.
            // Return them immediately without replacing the pending video frame.
            if(c.cursor_ && next->buffer && next->buffer->n_datas==1 && next->buffer->datas &&
                next->buffer->datas[0].chunk && next->buffer->datas[0].chunk->size==0) {
                pw_stream_queue_buffer(c.stream_,next);continue;
            }
            if (c.pending_) pw_stream_queue_buffer(c.stream_, c.pending_);
            c.pending_ = next; c.sampled_ = Clock::now();
        }
    }
    static void format_changed(void* opaque, uint32_t id, const spa_pod* param) noexcept {
        auto& c = *static_cast<Capture*>(opaque);
        if (id != SPA_PARAM_Format) return;
        if (!param) { if (c.formatted_) c.error_ = "Capture format retired"; return; }
        spa_video_info_raw format{};
        if (spa_format_video_raw_parse(param, &format) < 0 || format.format != SPA_VIDEO_FORMAT_BGRx ||
            format.size.width != c.display_.size.width || format.size.height != c.display_.size.height) {
            c.error_ = "Display geometry changed; refresh capture generation"; return;
        }
        uint8_t storage[1024];
        spa_pod_builder builder = SPA_POD_BUILDER_INIT(storage, sizeof(storage));
        const auto* buffers = static_cast<const spa_pod*>(spa_pod_builder_add_object(&builder,
            SPA_TYPE_OBJECT_ParamBuffers, SPA_PARAM_Buffers,
            SPA_PARAM_BUFFERS_buffers, SPA_POD_CHOICE_RANGE_Int(4, 2, 8),
            SPA_PARAM_BUFFERS_blocks, SPA_POD_Int(1),
            SPA_PARAM_BUFFERS_size, SPA_POD_Int(format.size.width * format.size.height * 4),
            SPA_PARAM_BUFFERS_stride, SPA_POD_Int(format.size.width * 4),
            SPA_PARAM_BUFFERS_dataType, SPA_POD_CHOICE_FLAGS_Int((1 << SPA_DATA_MemFd) | (1 << SPA_DATA_MemPtr))));
        const spa_pod* params[2]={buffers,nullptr};unsigned count=1;
        if(c.cursor_) {
            params[count++]=static_cast<const spa_pod*>(spa_pod_builder_add_object(&builder,
                SPA_TYPE_OBJECT_ParamMeta,SPA_PARAM_Meta,SPA_PARAM_META_type,SPA_POD_Id(SPA_META_Cursor),
                SPA_PARAM_META_size,SPA_POD_CHOICE_RANGE_Int(
                    int(sizeof(spa_meta_cursor)+sizeof(spa_meta_bitmap)+384*384*4),
                    int(sizeof(spa_meta_cursor)+sizeof(spa_meta_bitmap)),
                    int(sizeof(spa_meta_cursor)+sizeof(spa_meta_bitmap)+512*512*4))));
        }
        if (pw_stream_update_params(c.stream_, params, count) < 0) c.error_ = "PipeWire buffer negotiation failed";
        else c.formatted_ = true;
    }
    static const pw_stream_events& events() {
        static const auto value = [] {
            pw_stream_events e{}; e.version = PW_VERSION_STREAM_EVENTS;
            e.state_changed = state_changed; e.param_changed = format_changed;
            e.remove_buffer = removed; e.process = process; return e;
        }();
        return value;
    }
    static void invalidated(GDBusConnection*, const char*, const char*, const char*, const char*, GVariant*, void* opaque) noexcept {
        auto& c = *static_cast<Capture*>(opaque);
        PwLock lock(c.loop_); c.error_ = "Desktop session or display layout changed; refresh capture generation";
        if(c.cursor_)c.cursor_->fail(c.error_);
    }
    static void node_added(GDBusConnection*, const char*, const char*, const char*, const char*, GVariant* params, void* opaque) noexcept {
        auto& c = *static_cast<Capture*>(opaque);
        PwLock lock(c.loop_);
        if (c.stream_) { c.error_ = "PipeWire node changed; refresh capture generation"; return; }
        uint32_t node = 0; g_variant_get(params, "(u)", &node);
        c.stream_ = pw_stream_new_simple(pw_thread_loop_get_loop(c.loop_), "Pixels remote desktop",
            pw_properties_new(PW_KEY_MEDIA_TYPE, "Video", PW_KEY_MEDIA_CATEGORY, "Capture", PW_KEY_MEDIA_ROLE, "Screen", nullptr),
            &events(), &c);
        if (!c.stream_) { c.error_ = "Cannot create PipeWire capture stream"; return; }
        uint8_t storage[1024];
        spa_pod_builder builder = SPA_POD_BUILDER_INIT(storage, sizeof(storage));
        const spa_rectangle size{c.display_.size.width, c.display_.size.height};
        const spa_fraction variable{0, 1}, maximum{c.fps_, 1};
        const auto* format = static_cast<const spa_pod*>(spa_pod_builder_add_object(&builder,
            SPA_TYPE_OBJECT_Format, SPA_PARAM_EnumFormat,
            SPA_FORMAT_mediaType, SPA_POD_Id(SPA_MEDIA_TYPE_video),
            SPA_FORMAT_mediaSubtype, SPA_POD_Id(SPA_MEDIA_SUBTYPE_raw),
            SPA_FORMAT_VIDEO_format, SPA_POD_Id(SPA_VIDEO_FORMAT_BGRx),
            SPA_FORMAT_VIDEO_size, SPA_POD_Rectangle(&size),
            SPA_FORMAT_VIDEO_framerate, SPA_POD_Fraction(&variable),
            SPA_FORMAT_VIDEO_maxFramerate, SPA_POD_Fraction(&maximum)));
        if (pw_stream_connect(c.stream_, PW_DIRECTION_INPUT, node,
            pw_stream_flags(PW_STREAM_FLAG_AUTOCONNECT | PW_STREAM_FLAG_MAP_BUFFERS), &format, 1) < 0)
            c.error_ = "Cannot connect PipeWire capture stream";
    }
    void subscribe(const char* service, const char* interface, const char* signal,
                   const char* path, GDBusSignalCallback callback, const char* arg0 = nullptr) {
        subscriptions_.push_back(g_dbus_connection_signal_subscribe(bus_, service, interface, signal,
            path, arg0, G_DBUS_SIGNAL_FLAGS_NONE, callback, this, nullptr));
    }
    Variant session_call(const char* method, GVariant* args = nullptr, const GVariantType* type = nullptr) {
        return call(bus_, "org.gnome.Mutter.ScreenCast", session_.c_str(),
            "org.gnome.Mutter.ScreenCast.Session", method, args, type);
    }
    void ensure_valid() const {
        if (error_) throw std::runtime_error(error_);
        if (g_dbus_connection_is_closed(bus_)) throw std::runtime_error("Desktop session bus closed");
    }
public:
    Capture(GDBusConnection* bus, Display display, unsigned fps,CursorState* cursor=nullptr)
        : bus_(bus), display_(std::move(display)), fps_(fps),cursor_(cursor) {}
    Capture(const Capture&) = delete;
    Capture& operator=(const Capture&) = delete;
    ~Capture() {
        if(cursor_)cursor_->fail("Video capture ended");
        for (auto id : subscriptions_) g_dbus_connection_signal_unsubscribe(bus_, id);
        if (loop_) {
            if (started_) pw_thread_loop_stop(loop_);
            if (stream_) pw_stream_destroy(stream_);
            pw_thread_loop_destroy(loop_);
        }
        if (!session_.empty()) { try { session_call("Stop"); } catch (...) {} }
    }
    void start() {
        const PabVideoGeometry geometry(display_.size.width, display_.size.height);
        auto version = call(bus_, "org.gnome.Mutter.ScreenCast", "/org/gnome/Mutter/ScreenCast",
            "org.freedesktop.DBus.Properties", "Get", g_variant_new("(ss)", "org.gnome.Mutter.ScreenCast", "Version"), G_VARIANT_TYPE("(v)"));
        auto boxed = child(version.get(), 0); Variant value(g_variant_get_variant(boxed.get()));
        if (!g_variant_is_of_type(value.get(), G_VARIANT_TYPE_INT32) || g_variant_get_int32(value.get()) != 4)
            throw std::runtime_error("Unsupported GNOME ScreenCast interface version (expected 4)");
        loop_ = pw_thread_loop_new("pixels-screen-capture", nullptr);
        if (!loop_ || pw_thread_loop_start(loop_) < 0) throw std::runtime_error("Cannot start PipeWire event loop");
        started_ = true;
        subscribe("org.gnome.Mutter.DisplayConfig", "org.gnome.Mutter.DisplayConfig", "MonitorsChanged",
            "/org/gnome/Mutter/DisplayConfig", invalidated);
        subscribe("org.freedesktop.DBus", "org.freedesktop.DBus", "NameOwnerChanged",
            "/org/freedesktop/DBus", invalidated, "org.gnome.Mutter.ScreenCast");
        const auto current = displays(bus_);
        if (current.front().layout_serial != display_.layout_serial)
            throw std::runtime_error("Display layout changed before capture startup");
        auto result = call(bus_, "org.gnome.Mutter.ScreenCast", "/org/gnome/Mutter/ScreenCast",
            "org.gnome.Mutter.ScreenCast", "CreateSession", g_variant_new("(a{sv})", nullptr), G_VARIANT_TYPE("(o)"));
        session_ = text(child(result.get(), 0).get());
        subscribe("org.gnome.Mutter.ScreenCast", "org.gnome.Mutter.ScreenCast.Session", "Closed", session_.c_str(), invalidated);
        GVariantBuilder options;
        g_variant_builder_init(&options, G_VARIANT_TYPE_VARDICT);
        g_variant_builder_add(&options, "{sv}", "cursor-mode", g_variant_new_uint32(cursor_?2:1));
        result = session_call("RecordMonitor", g_variant_new("(sa{sv})", display_.connector.c_str(), &options), G_VARIANT_TYPE("(o)"));
        auto path = text(child(result.get(), 0).get());
        subscribe("org.gnome.Mutter.ScreenCast", "org.gnome.Mutter.ScreenCast.Stream", "PipeWireStreamAdded", path.c_str(), node_added);
        session_call("Start");
        const auto deadline = Clock::now() + std::chrono::seconds(3);
        for (;;) {
            parent_alive(); pump();
            { PwLock lock(loop_); ensure_valid(); if (formatted_ && pending_) return; }
            if (Clock::now() >= deadline) throw std::runtime_error("No initial GNOME desktop frame");
            std::this_thread::sleep_for(std::chrono::milliseconds(2));
        }
    }
    Encoded next(void* encoder) {
        for (;;) {
            parent_alive(); pump();
            {
                PwLock lock(loop_); ensure_valid();
                if (pending_) {
                    // Keep the PW pool buffer owned and the loop locked during
                    // CPU conversion/encoding. remove_buffer cannot unmap it.
                    // Release before any IPC write so network backpressure can
                    // never retain a queue of old raw frames or block PW cleanup.
                    auto view = frame_view(pending_->buffer, display_.size.width, display_.size.height);
                    Encoded result{{nullptr, 0}, sampled_};
                    const bool ok = pab_soft_encode(encoder, view.data, view.size, view.stride, &result.bytes);
                    pw_stream_queue_buffer(stream_, pending_); pending_ = nullptr;
                    if (!ok || !result.bytes.size || result.bytes.size > max_packet) throw std::runtime_error("Software encoding failed");
                    return result;
                }
            }
            // A static desktop does not synthesize duplicate frames.
            std::this_thread::sleep_for(std::chrono::milliseconds(2));
        }
    }
};
inline bool keyframe(const PabCodecBytes& packet) {
    for (size_t i = 0; i + 3 < packet.size; ++i) {
        if (packet.data[i] || packet.data[i + 1]) continue;
        size_t prefix = packet.data[i + 2] == 1 ? 3 : (packet.data[i + 2] == 0 && packet.data[i + 3] == 1 ? 4 : 0);
        if (prefix && i + prefix < packet.size && (packet.data[i + prefix] & 31) == 5) return true;
    }
    return false;
}
inline void capture(Capture& source, const Display& display, unsigned fps) {
    Encoder encoder(pab_soft_encoder_new(display.size.width, display.size.height, fps));
    if (!encoder) throw std::runtime_error("Cannot initialize bundled software encoder");
    fprintf(stderr, "media source=gnome_pipewire encoder=openh264_software pixel_format=bgrx cpu_conversion=true\n");
    write_value(magic); write_value(display.size.width); write_value(display.size.height);
    const auto start = Clock::now(); auto next = start;
    uint64_t index = 0;
    for (;;) {
        auto command = read_value<uint8_t>();
        if (!command) return;
        if (command != 1) throw std::runtime_error("Invalid capture command");
        while (Clock::now() < next) {
            parent_alive(); pump(); std::this_thread::sleep_for(std::chrono::milliseconds(1));
        }
        const auto frame = source.next(encoder.get());
        const bool key = keyframe(frame.bytes);
        if (!index && !key) throw std::runtime_error("First software frame is not IDR");
        const auto sampled = std::max(frame.sampled, start);
        write_value(uint32_t(frame.bytes.size)); write_value(index);
        write_value(uint64_t(std::chrono::duration_cast<std::chrono::microseconds>(sampled - start).count()));
        write_value(uint32_t(key)); write_bytes(frame.bytes.data, frame.bytes.size);
        ++index; next = std::max(next + std::chrono::microseconds(1000000 / fps), Clock::now());
    }
}
inline void capture(GDBusConnection* bus, const Display& display, unsigned fps) {
    Capture source(bus, display, fps); source.start();
    capture(source, display, fps);
}
}
