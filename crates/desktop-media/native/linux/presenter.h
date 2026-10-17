#pragma once
#include "common.h"
#include "../software_codec_ffi.h"
#include <gtk/gtk.h>
#include <epoxy/gl.h>
#include <array>
#include "viewer_input.h"

namespace pab_linux {
class Presenter {
    GtkWidget* window_ = nullptr;
    GtkWidget* area_ = nullptr;
    GtkWidget* hosted_root_ = nullptr;
    GtkWidget* overlay_ = nullptr;
    std::unique_ptr<ViewInput> input_;
    GdkFrameClock* clock_ = nullptr;
    gulong painted_signal_ = 0;
    GThread* thread_ = g_thread_self();
    unsigned width_, height_, rotation_;
    bool verify_, closed_ = false, uploaded_ = false, pending_ = false, drawn_ = false;
    bool verify_redraw_ = false;
    uint64_t draws_ = 0, painted_ = 0;
    GLuint program_ = 0, vao_ = 0, textures_[2]{};
    uint64_t index_ = 0, presented_ = 0, checks_ = 0;
    unsigned max_error_ = 0;
    std::string error_;
    static GLuint shader(GLenum kind, const char* text) {
        auto result = glCreateShader(kind);
        glShaderSource(result, 1, &text, nullptr); glCompileShader(result);
        GLint good = 0; glGetShaderiv(result, GL_COMPILE_STATUS, &good);
        if (!good) { glDeleteShader(result); throw std::runtime_error("Native NV12 shader compilation failed"); }
        return result;
    }
    void current() {
        if (g_thread_self() != thread_) throw std::runtime_error("Native view requires its GTK thread");
        if (closed_ || !gtk_widget_get_realized(area_)) throw std::runtime_error("Native GTK view is closed");
        gtk_gl_area_make_current(GTK_GL_AREA(area_));
        if (gtk_gl_area_get_error(GTK_GL_AREA(area_))) throw std::runtime_error("Native OpenGL context unavailable");
    }
    void initialize() {
        if (program_) return;
        static const char* vertex = R"(#version 150
out vec2 uv;
uniform int rotation;
void main() {
    vec2 p[4] = vec2[4](vec2(-1,-1),vec2(1,-1),vec2(-1,1),vec2(1,1));
    gl_Position = vec4(p[gl_VertexID],0,1);
    vec2 t = vec2((p[gl_VertexID].x+1)*.5,(1-p[gl_VertexID].y)*.5);
    if (rotation == 1) t = vec2(t.y,1-t.x);
    else if (rotation == 2) t = vec2(1-t.x,1-t.y);
    else if (rotation == 3) t = vec2(1-t.y,t.x);
    uv = t;
})";
        static const char* fragment = R"(#version 150
in vec2 uv;
out vec4 color;
uniform sampler2D plane_y;
uniform sampler2D plane_uv;
void main() {
    float y = (texture(plane_y,uv).r*255.0-16.0)/219.0;
    vec2 c = (texture(plane_uv,uv).rg*255.0-vec2(128.0))/224.0;
    color = vec4(y+1.5748*c.y,y-.187324*c.x-.468124*c.y,y+1.8556*c.x,1);
})";
        GLuint vs = shader(GL_VERTEX_SHADER, vertex), fs = 0;
        try { fs = shader(GL_FRAGMENT_SHADER, fragment); }
        catch (...) { glDeleteShader(vs); throw; }
        program_ = glCreateProgram(); glAttachShader(program_, vs); glAttachShader(program_, fs); glLinkProgram(program_);
        glDeleteShader(vs); glDeleteShader(fs);
        GLint good = 0; glGetProgramiv(program_, GL_LINK_STATUS, &good);
        if (!good) throw std::runtime_error("Native NV12 shader linking failed");
        glGenVertexArrays(1, &vao_); glGenTextures(2, textures_);
        glPixelStorei(GL_UNPACK_ALIGNMENT, 1);
        for (unsigned i = 0; i < 2; ++i) {
            glBindTexture(GL_TEXTURE_2D, textures_[i]);
            glTexParameteri(GL_TEXTURE_2D, GL_TEXTURE_MIN_FILTER, GL_LINEAR);
            glTexParameteri(GL_TEXTURE_2D, GL_TEXTURE_MAG_FILTER, GL_LINEAR);
            glTexParameteri(GL_TEXTURE_2D, GL_TEXTURE_WRAP_S, GL_CLAMP_TO_EDGE);
            glTexParameteri(GL_TEXTURE_2D, GL_TEXTURE_WRAP_T, GL_CLAMP_TO_EDGE);
            glTexImage2D(GL_TEXTURE_2D, 0, i ? GL_RG8 : GL_R8, width_ / (i ? 2 : 1), height_ / (i ? 2 : 1),
                0, i ? GL_RG : GL_RED, GL_UNSIGNED_BYTE, nullptr);
        }
        if (glGetError() != GL_NO_ERROR) throw std::runtime_error("Native texture allocation failed");
    }
    void verify_pixels(Viewport v) {
        if (!verify_) return;
        for (unsigned q = 0; q < 4; ++q) {
            std::array<uint8_t, 4> pixel{};
            glReadPixels(v.x + v.width * (q % 2 ? 3 : 1) / 4,
                v.y + v.height * (q / 2 ? 1 : 3) / 4, 1, 1, GL_RGBA, GL_UNSIGNED_BYTE, pixel.data());
            unsigned x = q % 2, y = q / 2;
            if (rotation_ == 1) { auto old = x; x = y; y = 1-old; }
            else if (rotation_ == 2) { x = 1-x; y = 1-y; }
            else if (rotation_ == 3) { auto old = x; x = 1-y; y = old; }
            const unsigned quadrant = y*2+x;
            const unsigned expected[]{unsigned(24+(index_*3+quadrant*43)%208),
                unsigned(24+(index_*7+quadrant*67)%208), unsigned(24+(index_*11+quadrant*97)%208)};
            for (unsigned c = 0; c < 3; ++c) max_error_ = std::max(max_error_, unsigned(std::abs(int(pixel[c])-int(expected[c]))));
            ++checks_;
        }
        if (glGetError() != GL_NO_ERROR || max_error_ > 12) throw std::runtime_error("Native displayed color/rotation verification failed");
    }
    void draw() {
        current(); initialize();
        glDisable(GL_DEPTH_TEST); glDisable(GL_BLEND); glDisable(GL_SCISSOR_TEST); glDisable(GL_FRAMEBUFFER_SRGB);
        glClearColor(0,0,0,1); glClear(GL_COLOR_BUFFER_BIT);
        if (!uploaded_) return;
        const int scale = gtk_widget_get_scale_factor(area_);
        auto v = viewport(width_, height_, rotation_, gtk_widget_get_allocated_width(area_)*scale, gtk_widget_get_allocated_height(area_)*scale);
        glViewport(v.x, v.y, v.width, v.height); glUseProgram(program_); glBindVertexArray(vao_);
        glUniform1i(glGetUniformLocation(program_, "rotation"), rotation_);
        for (unsigned i = 0; i < 2; ++i) {
            glActiveTexture(GL_TEXTURE0+i); glBindTexture(GL_TEXTURE_2D, textures_[i]);
            glUniform1i(glGetUniformLocation(program_, i ? "plane_uv" : "plane_y"), i);
        }
        glDrawArrays(GL_TRIANGLE_STRIP, 0, 4);
        if (pending_ || verify_redraw_) verify_pixels(v);
        verify_redraw_ = false;
        glFlush();
        if (glGetError() != GL_NO_ERROR) throw std::runtime_error("Native frame drawing failed");
        if (pending_) drawn_ = true;
        ++draws_;
    }
    static gboolean draw_callback(GtkGLArea*, GdkGLContext*, gpointer data) noexcept {
        auto& view = *static_cast<Presenter*>(data);
        try { view.draw(); } catch (const std::exception& e) { view.error_ = e.what(); }
        catch (...) { view.error_ = "Native frame drawing failed"; }
        return TRUE;
    }
    static void after_paint(GdkFrameClock*, gpointer data) noexcept {
        auto& view = *static_cast<Presenter*>(data);
        view.painted_ = view.draws_;
        if (view.pending_ && view.drawn_ && view.error_.empty()) {
            view.pending_ = false; view.drawn_ = false; ++view.presented_;
        }
    }
    static void destroyed(GtkWidget*, gpointer data) noexcept { static_cast<Presenter*>(data)->closed_ = true; }
    void drop_gl() noexcept {
        gtk_gl_area_make_current(GTK_GL_AREA(area_));
        if (!gtk_gl_area_get_error(GTK_GL_AREA(area_))) {
            glDeleteTextures(2, textures_); glDeleteVertexArrays(1, &vao_); if (program_) glDeleteProgram(program_);
        }
        textures_[0] = textures_[1] = vao_ = program_ = 0;
    }
    static void unrealized(GtkWidget*, gpointer data) noexcept {
        auto& view = *static_cast<Presenter*>(data);
        view.drop_gl(); view.closed_ = true;
    }
public:
    Presenter(unsigned width, unsigned height, unsigned rotation, bool verify, bool fullscreen = false, GtkWidget* host = nullptr)
        : width_(width), height_(height), rotation_(rotation), verify_(verify) {
        if (!width || !height || width%2 || height%2 || width>8192 || height>8192 || uint64_t(width)*height>33554432 || rotation>3)
            throw std::runtime_error("Invalid native renderer geometry");
        if (host) {
            if (!GTK_IS_WINDOW(host) || !gtk_widget_get_realized(host) || fullscreen || verify)
                throw std::runtime_error("Native host requires a realized GTK window");
            hosted_root_ = gtk_bin_get_child(GTK_BIN(host));
            if (!hosted_root_) throw std::runtime_error("Native host content is missing");
            g_object_ref(hosted_root_);
            window_ = host; g_object_ref(window_);
        } else {
            if (!gtk_init_check(nullptr, nullptr)) throw std::runtime_error("Native viewer requires a graphical GTK session");
            window_ = gtk_window_new(GTK_WINDOW_TOPLEVEL); g_object_ref_sink(window_);
            gtk_window_set_title(GTK_WINDOW(window_), "Pixels remote desktop media test");
            gtk_window_set_default_size(GTK_WINDOW(window_), 960, 600);
        }
        area_ = gtk_gl_area_new(); g_object_ref_sink(area_);
        gtk_gl_area_set_required_version(GTK_GL_AREA(area_), 3, 2);
        gtk_gl_area_set_use_es(GTK_GL_AREA(area_), FALSE);
        gtk_gl_area_set_has_alpha(GTK_GL_AREA(area_), FALSE);
        gtk_gl_area_set_auto_render(GTK_GL_AREA(area_), TRUE);
        g_signal_connect(area_, "render", G_CALLBACK(draw_callback), this);
        g_signal_connect(area_, "destroy", G_CALLBACK(destroyed), this);
        g_signal_connect(area_, "unrealize", G_CALLBACK(unrealized), this);
        input_ = std::make_unique<ViewInput>(area_,window_,width_,height_,rotation_);
        if (hosted_root_) {
            // The WebKit content keeps the toolbar and its existing state.
            // Only this generation's GL overlay is removed on retirement.
            overlay_ = gtk_overlay_new(); g_object_ref_sink(overlay_);
            gtk_container_remove(GTK_CONTAINER(window_), hosted_root_);
            gtk_container_add(GTK_CONTAINER(overlay_), hosted_root_);
            gtk_container_add(GTK_CONTAINER(window_), overlay_);
            gtk_widget_set_margin_top(area_, 72);
            gtk_widget_set_hexpand(area_, TRUE); gtk_widget_set_vexpand(area_, TRUE);
            gtk_overlay_add_overlay(GTK_OVERLAY(overlay_), area_);
            gtk_widget_show(area_); gtk_widget_show(overlay_);
        } else {
            gtk_container_add(GTK_CONTAINER(window_), area_);
            if (fullscreen) gtk_window_fullscreen(GTK_WINDOW(window_));
            gtk_widget_show_all(window_);
        }
        clock_ = gtk_widget_get_frame_clock(area_);
        if (clock_) { g_object_ref(clock_); painted_signal_ = g_signal_connect(clock_, "after-paint", G_CALLBACK(after_paint), this); }
        // Potential GL errors are reported by poll/present after construction,
        // so the fully owned widgets are always cleaned up on initialization failure.
    }
    Presenter(const Presenter&) = delete;
    Presenter& operator=(const Presenter&) = delete;
    ~Presenter() {
        input_.reset();
        if (clock_) {
            if (g_signal_handler_is_connected(clock_, painted_signal_)) g_signal_handler_disconnect(clock_, painted_signal_);
            g_object_unref(clock_);
        }
        if (!closed_ && gtk_widget_get_realized(area_)) {
            drop_gl();
        }
        g_signal_handlers_disconnect_by_data(area_, this);
        if (hosted_root_) {
            gtk_widget_destroy(area_);
            // A host close may already have destroyed this widget tree. Never
            // restore content into a closed window or destroy a host we borrow.
            if (gtk_widget_get_parent(overlay_) == window_ && gtk_widget_get_parent(hosted_root_) == overlay_) {
                gtk_container_remove(GTK_CONTAINER(overlay_), hosted_root_);
                gtk_container_remove(GTK_CONTAINER(window_), overlay_);
                gtk_container_add(GTK_CONTAINER(window_), hosted_root_);
            }
            gtk_widget_destroy(overlay_); g_object_unref(overlay_); g_object_unref(hosted_root_);
        } else gtk_widget_destroy(window_);
        g_object_unref(area_); g_object_unref(window_);
    }
    void poll_events(bool dispatch_events = true) {
        // In-process hosts own their GTK loop. Re-entering it during a render
        // or FFI call could destroy this generation while it is borrowed.
        if (dispatch_events) pump();
        if (closed_) throw std::runtime_error("Native viewer window closed");
        if (!error_.empty()) throw std::runtime_error(error_);
        if (!clock_) throw std::runtime_error("Native viewer frame clock unavailable");
        current();
    }
    void submit(const PabCodecBytes& nv12, uint64_t index) {
        poll_events(false); initialize();
        if (pending_ || index != presented_ || !nv12.data || nv12.size != uint64_t(width_)*height_*3/2)
            throw std::runtime_error("Invalid decoded native frame");
        glPixelStorei(GL_UNPACK_ALIGNMENT, 1);
        glActiveTexture(GL_TEXTURE0); glBindTexture(GL_TEXTURE_2D, textures_[0]);
        glTexSubImage2D(GL_TEXTURE_2D,0,0,0,width_,height_,GL_RED,GL_UNSIGNED_BYTE,nv12.data);
        glActiveTexture(GL_TEXTURE1); glBindTexture(GL_TEXTURE_2D, textures_[1]);
        glTexSubImage2D(GL_TEXTURE_2D,0,0,0,width_/2,height_/2,GL_RG,GL_UNSIGNED_BYTE,nv12.data+size_t(width_)*height_);
        if (glGetError() != GL_NO_ERROR) throw std::runtime_error("Native NV12 upload failed");
        index_ = index; uploaded_ = true; pending_ = true; drawn_ = false;
        gtk_gl_area_queue_render(GTK_GL_AREA(area_));
    }
    void present(const PabCodecBytes& nv12, uint64_t index) {
        poll_events(); submit(nv12,index);
        while (pending_) { parent_alive(); poll_events(); std::this_thread::sleep_for(std::chrono::milliseconds(1)); }
    }
    uint64_t presented() const { return presented_; }
    void input_sink(std::function<bool(const InputPacket&)> sink) { poll_events(false); input_->sink(std::move(sink)); }
    void input_enabled(bool value) { poll_events(false); input_->enable(value); }
    uint64_t checks() const { return checks_; }
    unsigned max_error() const { return max_error_; }
    const char* renderer() { current(); return reinterpret_cast<const char*>(glGetString(GL_RENDERER)); }
    std::pair<int,int> drawable_size() const {
        int scale = gtk_widget_get_scale_factor(area_);
        return {gtk_widget_get_allocated_width(area_)*scale,gtk_widget_get_allocated_height(area_)*scale};
    }
    // Explicit generated-frame diagnostic, never run for a normal remote view.
    void lifecycle_check(bool minimize = true) {
        if (hosted_root_ || !verify_ || presented_ != 1) throw std::runtime_error("Lifecycle check requires one verified standalone frame");
        const auto wait = [&](const char* stage, auto ready) {
            const auto end = Clock::now()+std::chrono::seconds(4);
            while (!ready()) {
                parent_alive(); poll_events();
                if (Clock::now() >= end) throw std::runtime_error(std::string("Native window lifecycle timed out: ")+stage+
                    ", size="+std::to_string(gtk_widget_get_allocated_width(area_))+"x"+std::to_string(gtk_widget_get_allocated_height(area_))+
                    ", state="+std::to_string(gdk_window_get_state(gtk_widget_get_window(window_))));
                std::this_thread::sleep_for(std::chrono::milliseconds(2));
            }
        };
        const auto repaint = [&] {
            const auto previous = painted_;
            verify_redraw_ = true; gtk_gl_area_queue_render(GTK_GL_AREA(area_));
            wait("repaint",[&] { return painted_ > previous && !verify_redraw_; });
        };
        for (const auto& size : {std::pair<int,int>{640,480},{800,400}}) {
            gtk_window_resize(GTK_WINDOW(window_),size.first,size.second);
            wait("resize",[&] { return gtk_widget_get_allocated_width(area_) == size.first && gtk_widget_get_allocated_height(area_) == size.second; });
            repaint();
        }
        if (minimize) {
            // Wayland's xdg_toplevel has no minimized-state event. Do not
            // pretend a request or elapsed delay proves that transition.
            gtk_window_iconify(GTK_WINDOW(window_));
            const auto minimized = [&] { return (gdk_window_get_state(gtk_widget_get_window(window_)) & GDK_WINDOW_STATE_ICONIFIED) != 0; };
            wait("minimize",minimized);
            gtk_window_deiconify(GTK_WINDOW(window_)); gtk_window_present(GTK_WINDOW(window_));
            wait("restore",[&] { return !minimized() && gtk_widget_get_mapped(area_); });
        }
        repaint();
        gtk_widget_destroy(window_);
        if (!closed_) throw std::runtime_error("Native close did not retire the view");
        fprintf(stderr,"media lifecycle=%s\n",minimize ? "resize_minimize_restore_close_passed" : "resize_redraw_close_passed");
    }
};
}
