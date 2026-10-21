#pragma once
#include "geometry.h"
#include "keycodes.h"
#include <gtk/gtk.h>
#ifdef GDK_WINDOWING_X11
#include <gdk/gdkx.h>
#endif
#include <array>
#include <functional>

namespace pab_linux {
using InputPacket = std::array<uint8_t,8>;
inline uint16_t windows_key(unsigned hardware_code) {
    // GDK Wayland adds the XKB/evdev offset of eight, matching Linux X11's
    // evdev keymap. Physical keys preserve the remote layout, not local text.
    if (hardware_code < 8 || hardware_code > KEY_MAX+8) return 0;
    static const auto mapping=[] {
        std::array<uint16_t,KEY_MAX+1> result{};
        for (unsigned vk=8;vk<=0xe2;++vk) {
            if (vk==0x10 || vk==0x11 || vk==0x12) continue;
            try { result.at(evdev_key(vk))=uint16_t(vk); } catch (const std::exception&) {}
        }
        result[KEY_KPENTER]=0x0d; return result;
    }();
    return mapping[hardware_code-8];
}
// Coordinates are normalized in the displayed desktop orientation, as on
// Windows/macOS. Do not inverse-rotate them a second time at the input boundary.
inline bool viewer_position(unsigned width,unsigned height,unsigned rotation,int drawable_width,int drawable_height,
                            double px,double py,bool dragging,uint16_t& x,uint16_t& y) {
    if (!std::isfinite(px) || !std::isfinite(py)) return false;
    const auto v=viewport(width,height,rotation,drawable_width,drawable_height);
    const int top=drawable_height-v.y-v.height; // GL viewport origin is bottom-left.
    px-=v.x; py-=top;
    if (!dragging && (px<0 || py<0 || px>=v.width || py>=v.height)) return false;
    x=uint16_t(std::lround(std::clamp(px/std::max(1,v.width-1),0.0,1.0)*65535));
    y=uint16_t(std::lround(std::clamp(py/std::max(1,v.height-1),0.0,1.0)*65535));
    return true;
}
class ViewInput {
    GtkWidget* area_;
    GtkWidget* window_;
    unsigned width_,height_,rotation_;
    bool enabled_=false;
    bool fullscreen_=false, escape_held_=false;
    bool cursor_tracking_=false;
    GdkWindow* cursor_window_=nullptr;
    GdkCursor* blank_cursor_=nullptr;
    GdkCursor* previous_cursor_=nullptr;
    bool cursor_hidden_=false;
    unsigned buttons_=0;
    std::array<bool,256> keys_{};
    double wheel_=0;
    int allocated_width_=0,allocated_height_=0;
    std::function<bool(const InputPacket&)> sink_;
    void pointer_hidden(bool hidden) noexcept {
        if (!cursor_window_ || gdk_window_is_destroyed(cursor_window_)) return;
        if (hidden==cursor_hidden_) return;
        if (hidden && !blank_cursor_)
            blank_cursor_=gdk_cursor_new_for_display(gtk_widget_get_display(area_),GDK_BLANK_CURSOR);
        if (hidden && !blank_cursor_) return;
        gdk_window_set_cursor(cursor_window_,hidden ? blank_cursor_ : previous_cursor_);
        cursor_hidden_=hidden;
    }
    void pointer_at(double px,double py) noexcept {
        uint16_t x=0,y=0; bool inside=false;
        const int scale=gtk_widget_get_scale_factor(area_);
        if (enabled_ && cursor_tracking_ && gtk_widget_get_mapped(area_)) {
            try { inside=viewer_position(width_,height_,rotation_,gtk_widget_get_allocated_width(area_)*scale,
                gtk_widget_get_allocated_height(area_)*scale,px*scale,py*scale,false,x,y); } catch (...) {}
        }
        pointer_hidden(inside);
    }
    void refresh_pointer() noexcept {
        if (!enabled_ || !cursor_tracking_ || !cursor_window_) { pointer_hidden(false); return; }
        auto* seat=gdk_display_get_default_seat(gtk_widget_get_display(area_));
        auto* pointer=seat ? gdk_seat_get_pointer(seat) : nullptr;
        if (!pointer) { pointer_hidden(false); return; }
        int x=0,y=0;
        gdk_window_get_device_position(cursor_window_,pointer,&x,&y,nullptr);
        pointer_at(x,y);
    }
    void release_pointer_window() noexcept {
        pointer_hidden(false);
        if (cursor_window_) g_object_unref(cursor_window_);
        if (previous_cursor_) g_object_unref(previous_cursor_);
        if (blank_cursor_) g_object_unref(blank_cursor_);
        cursor_window_=nullptr; previous_cursor_=blank_cursor_=nullptr; cursor_hidden_=false;
    }
    static void realized(GtkWidget*,gpointer data) noexcept {
        auto& self=*static_cast<ViewInput*>(data);
        // GtkGLArea is windowless for drawing; its registered input-only
        // child belongs to this widget. Never change the shared host cursor.
        auto* parent=gtk_widget_get_parent_window(self.area_);
        if (!parent) return;
        auto* children=gdk_window_get_children(parent);
        for (auto* entry=children;entry;entry=entry->next) {
            auto* window=GDK_WINDOW(entry->data); gpointer owner=nullptr;
            gdk_window_get_user_data(window,&owner);
            if (owner==self.area_) {
                self.cursor_window_=GDK_WINDOW(g_object_ref(window));
                self.previous_cursor_=gdk_window_get_cursor(window);
                if (self.previous_cursor_) g_object_ref(self.previous_cursor_);
                break;
            }
        }
        g_list_free(children); self.refresh_pointer();
    }
    static void unrealized(GtkWidget*,gpointer data) noexcept { static_cast<ViewInput*>(data)->release_pointer_window(); }
    static gboolean crossing(GtkWidget*,GdkEventCrossing* event,gpointer data) noexcept {
        auto& self=*static_cast<ViewInput*>(data);
        if (event->type==GDK_LEAVE_NOTIFY) self.pointer_hidden(false);
        else self.pointer_at(event->x,event->y);
        return FALSE;
    }
    bool emit(uint8_t kind,uint16_t x=0,uint16_t y=0,uint16_t value=0,bool down=false) noexcept {
        const InputPacket packet{kind,uint8_t(x),uint8_t(x>>8),uint8_t(y),uint8_t(y>>8),uint8_t(value),uint8_t(value>>8),uint8_t(down)};
        bool accepted=false;
        try { accepted=sink_ && sink_(packet); } catch (...) {}
        if (!accepted) { enabled_=false; buttons_=0; keys_.fill(false); wheel_=0; pointer_hidden(false); }
        return accepted;
    }
    bool position(double px,double py,uint16_t& x,uint16_t& y) noexcept {
        const int scale=gtk_widget_get_scale_factor(area_);
        try { return viewer_position(width_,height_,rotation_,gtk_widget_get_allocated_width(area_)*scale,
            gtk_widget_get_allocated_height(area_)*scale,px*scale,py*scale,buttons_!=0,x,y); }
        catch (...) { release(); return false; }
    }
    static gboolean motion(GtkWidget*,GdkEventMotion* event,gpointer data) noexcept {
        auto& self=*static_cast<ViewInput*>(data); self.pointer_at(event->x,event->y);
        if (!self.enabled_) return FALSE;
        uint16_t x=0,y=0; if (self.position(event->x,event->y,x,y)) self.emit(1,x,y);
        return TRUE;
    }
    static gboolean button(GtkWidget*,GdkEventButton* event,gpointer data) noexcept {
        auto& self=*static_cast<ViewInput*>(data); if (!self.enabled_) return FALSE;
        if (event->type==GDK_2BUTTON_PRESS || event->type==GDK_3BUTTON_PRESS) return TRUE;
        const unsigned button=event->button==1 ? 0 : event->button==3 ? 1 : event->button==2 ? 2 : 3;
        if (button==3) return TRUE;
        const bool down=event->type==GDK_BUTTON_PRESS;
        uint16_t x=0,y=0; if (!self.position(event->x,event->y,x,y)) return TRUE;
        if (down) { gtk_widget_grab_focus(self.area_); self.buttons_|=1u<<button; }
        else if (!(self.buttons_&(1u<<button))) return TRUE;
        self.emit(2,x,y,uint16_t(button),down);
        if (!down) self.buttons_&=~(1u<<button);
        return TRUE;
    }
    static gboolean key(GtkWidget*,GdkEventKey* event,gpointer data) noexcept {
        auto& self=*static_cast<ViewInput*>(data);
        const bool pressed=event->type==GDK_KEY_PRESS;
        if (event->keyval==GDK_KEY_Escape) {
            if (self.escape_held_) { if (!pressed) self.escape_held_=false; return TRUE; }
            if (self.fullscreen_ && pressed) { self.escape_held_=true; self.release(); self.emit(8); return TRUE; }
        }
        if (!self.enabled_) return FALSE;
        const auto vk=windows_key(event->hardware_keycode);
        if (!vk) return TRUE;
        const bool down=event->type==GDK_KEY_PRESS;
        if (self.keys_[vk]==down) return TRUE;
        self.keys_[vk]=down; self.emit(4,0,0,vk,down); return TRUE;
    }
    static gboolean scroll(GtkWidget*,GdkEventScroll* event,gpointer data) noexcept {
        auto& self=*static_cast<ViewInput*>(data); if (!self.enabled_) return FALSE;
        uint16_t x=0,y=0; if (!self.position(event->x,event->y,x,y)) return TRUE;
        double amount=event->direction==GDK_SCROLL_UP ? 120 : event->direction==GDK_SCROLL_DOWN ? -120 : 0;
        if (event->direction==GDK_SCROLL_SMOOTH && std::isfinite(event->delta_y)) amount=-event->delta_y*120;
        self.wheel_=std::clamp(self.wheel_+amount,-32767.0,32767.0);
        const auto delta=int16_t(std::trunc(self.wheel_));
        if (delta) { self.wheel_-=delta; self.emit(3,x,y,uint16_t(delta)); }
        return TRUE;
    }
    static gboolean focus_out(GtkWidget*,GdkEvent*,gpointer data) noexcept { static_cast<ViewInput*>(data)->release(); return FALSE; }
    static void unmapped(GtkWidget*,gpointer data) noexcept {
        auto& self=*static_cast<ViewInput*>(data); self.release(); self.pointer_hidden(false);
    }
    static void active(GObject*,GParamSpec*,gpointer data) noexcept {
        auto& self=*static_cast<ViewInput*>(data);
        if (!gtk_window_is_active(GTK_WINDOW(self.window_))) self.release();
    }
    static void resized(GtkWidget*,GtkAllocation* size,gpointer data) noexcept {
        auto& self=*static_cast<ViewInput*>(data);
        if (self.allocated_width_ && (self.allocated_width_!=size->width || self.allocated_height_!=size->height)) self.release();
        self.allocated_width_=size->width; self.allocated_height_=size->height;
        self.refresh_pointer();
    }
public:
    ViewInput(GtkWidget* area,GtkWidget* window,unsigned width,unsigned height,unsigned rotation)
        :area_(area),window_(window),width_(width),height_(height),rotation_(rotation) {
        int smooth=GDK_SMOOTH_SCROLL_MASK;
#ifdef GDK_WINDOWING_X11
        // GTK 3's XI2 scroll cache consumes the first valuator as a baseline
        // and suppresses the corresponding emulated wheel button. Request
        // discrete events on X11 so the first notch after entry is preserved.
        // X11 touchpad scrolling is consequently quantized into wheel notches.
        if (GDK_IS_X11_DISPLAY(gtk_widget_get_display(area_))) smooth=0;
#endif
        gtk_widget_add_events(area_,GDK_POINTER_MOTION_MASK|GDK_BUTTON_PRESS_MASK|GDK_BUTTON_RELEASE_MASK|
            GDK_SCROLL_MASK|smooth|GDK_KEY_PRESS_MASK|GDK_KEY_RELEASE_MASK|GDK_FOCUS_CHANGE_MASK|
            GDK_ENTER_NOTIFY_MASK|GDK_LEAVE_NOTIFY_MASK);
        g_signal_connect_after(area_,"realize",G_CALLBACK(realized),this);
        g_signal_connect(area_,"unrealize",G_CALLBACK(unrealized),this);
        g_signal_connect(area_,"enter-notify-event",G_CALLBACK(crossing),this);
        g_signal_connect(area_,"leave-notify-event",G_CALLBACK(crossing),this);
        g_signal_connect(area_,"motion-notify-event",G_CALLBACK(motion),this);
        g_signal_connect(area_,"button-press-event",G_CALLBACK(button),this);
        g_signal_connect(area_,"button-release-event",G_CALLBACK(button),this);
        g_signal_connect(area_,"key-press-event",G_CALLBACK(key),this);
        g_signal_connect(area_,"key-release-event",G_CALLBACK(key),this);
        g_signal_connect(area_,"scroll-event",G_CALLBACK(scroll),this);
        g_signal_connect(area_,"focus-out-event",G_CALLBACK(focus_out),this);
        g_signal_connect(area_,"grab-broken-event",G_CALLBACK(focus_out),this);
        g_signal_connect(area_,"unmap",G_CALLBACK(unmapped),this);
        g_signal_connect(area_,"size-allocate",G_CALLBACK(resized),this);
        g_signal_connect(window_,"notify::is-active",G_CALLBACK(active),this);
    }
    ViewInput(const ViewInput&)=delete;
    ~ViewInput() { release(); release_pointer_window(); g_signal_handlers_disconnect_by_data(area_,this); g_signal_handlers_disconnect_by_data(window_,this); }
    void release() noexcept {
        if (enabled_) emit(5);
        buttons_=0; keys_.fill(false); wheel_=0;
    }
    void fullscreen(bool value) {
        fullscreen_=value; gtk_widget_set_can_focus(area_,value || enabled_);
        if (value) gtk_widget_grab_focus(area_);
    }
    void cursor_tracking(bool value) { cursor_tracking_=value; refresh_pointer(); }
    void sink(std::function<bool(const InputPacket&)> sink) { release(); enabled_=false; sink_=std::move(sink); refresh_pointer(); }
    void enable(bool value) {
        release(); enabled_=value && bool(sink_);
        gtk_widget_set_can_focus(area_,enabled_ || fullscreen_);
        if (enabled_) gtk_widget_grab_focus(area_);
        if (!emit(6,0,0,enabled_ ? 1 : 0)) throw std::runtime_error("Native input queue unavailable");
        refresh_pointer();
    }
};
}
