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
    unsigned buttons_=0;
    std::array<bool,256> keys_{};
    double wheel_=0;
    int allocated_width_=0,allocated_height_=0;
    std::function<bool(const InputPacket&)> sink_;
    bool emit(uint8_t kind,uint16_t x=0,uint16_t y=0,uint16_t value=0,bool down=false) noexcept {
        const InputPacket packet{kind,uint8_t(x),uint8_t(x>>8),uint8_t(y),uint8_t(y>>8),uint8_t(value),uint8_t(value>>8),uint8_t(down)};
        bool accepted=false;
        try { accepted=sink_ && sink_(packet); } catch (...) {}
        if (!accepted) { enabled_=false; buttons_=0; keys_.fill(false); wheel_=0; }
        return accepted;
    }
    bool position(double px,double py,uint16_t& x,uint16_t& y) noexcept {
        const int scale=gtk_widget_get_scale_factor(area_);
        try { return viewer_position(width_,height_,rotation_,gtk_widget_get_allocated_width(area_)*scale,
            gtk_widget_get_allocated_height(area_)*scale,px*scale,py*scale,buttons_!=0,x,y); }
        catch (...) { release(); return false; }
    }
    static gboolean motion(GtkWidget*,GdkEventMotion* event,gpointer data) noexcept {
        auto& self=*static_cast<ViewInput*>(data); if (!self.enabled_) return FALSE;
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
        auto& self=*static_cast<ViewInput*>(data); if (!self.enabled_) return FALSE;
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
    static void unmapped(GtkWidget*,gpointer data) noexcept { static_cast<ViewInput*>(data)->release(); }
    static void active(GObject*,GParamSpec*,gpointer data) noexcept {
        auto& self=*static_cast<ViewInput*>(data);
        if (!gtk_window_is_active(GTK_WINDOW(self.window_))) self.release();
    }
    static void resized(GtkWidget*,GtkAllocation* size,gpointer data) noexcept {
        auto& self=*static_cast<ViewInput*>(data);
        if (self.allocated_width_ && (self.allocated_width_!=size->width || self.allocated_height_!=size->height)) self.release();
        self.allocated_width_=size->width; self.allocated_height_=size->height;
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
            GDK_SCROLL_MASK|smooth|GDK_KEY_PRESS_MASK|GDK_KEY_RELEASE_MASK|GDK_FOCUS_CHANGE_MASK);
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
    ~ViewInput() { release(); g_signal_handlers_disconnect_by_data(area_,this); g_signal_handlers_disconnect_by_data(window_,this); }
    void release() noexcept {
        if (enabled_) emit(5);
        buttons_=0; keys_.fill(false); wheel_=0;
    }
    void sink(std::function<bool(const InputPacket&)> sink) { release(); enabled_=false; sink_=std::move(sink); }
    void enable(bool value) {
        release(); enabled_=value && bool(sink_);
        gtk_widget_set_can_focus(area_,enabled_);
        if (enabled_) gtk_widget_grab_focus(area_);
        if (!emit(6,0,0,enabled_ ? 1 : 0)) throw std::runtime_error("Native input queue unavailable");
    }
};
}
