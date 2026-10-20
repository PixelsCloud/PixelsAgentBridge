#pragma once
#include "geometry.h"
#include <epoxy/gl.h>

namespace pab_linux {
// The only upload is a bounded cursor shape. Motion updates uniforms; the
// existing NV12 shader composes it without reading back the desktop.
class CursorOverlay {
    bool visible_ = false, tracking_ = false;
    int64_t x_ = 0, y_ = 0;
    uint32_t desktop_width_ = 0, desktop_height_ = 0;
    uint32_t width_ = 0, height_ = 0, hotspot_x_ = 0, hotspot_y_ = 0, format_ = 0;
    GLuint shape_ = 0, empty_ = 0;
    static GLuint texture(unsigned width, unsigned height, const uint8_t* bytes) {
        GLuint result = 0; glGenTextures(1,&result);
        glActiveTexture(GL_TEXTURE2); glBindTexture(GL_TEXTURE_2D,result);
        glTexParameteri(GL_TEXTURE_2D,GL_TEXTURE_MIN_FILTER,GL_NEAREST);
        glTexParameteri(GL_TEXTURE_2D,GL_TEXTURE_MAG_FILTER,GL_NEAREST);
        glTexParameteri(GL_TEXTURE_2D,GL_TEXTURE_WRAP_S,GL_CLAMP_TO_EDGE);
        glTexParameteri(GL_TEXTURE_2D,GL_TEXTURE_WRAP_T,GL_CLAMP_TO_EDGE);
        glPixelStorei(GL_UNPACK_ALIGNMENT,1);
        glTexImage2D(GL_TEXTURE_2D,0,GL_RGBA8,width,height,0,GL_RGBA,GL_UNSIGNED_BYTE,bytes);
        glActiveTexture(GL_TEXTURE0);
        if (!result || glGetError()!=GL_NO_ERROR) {
            glDeleteTextures(1,&result); throw std::runtime_error("Native cursor texture upload failed");
        }
        return result;
    }
public:
    struct Parameters {
        float x=0,y=0,inverse_x=0,inverse_y=0;
        uint32_t width=0,height=0,format=0,visible=0;
    };
    CursorOverlay() = default;
    CursorOverlay(const CursorOverlay&) = delete;
    CursorOverlay& operator=(const CursorOverlay&) = delete;
    // Caller owns the GL context and must release before it is destroyed.
    void release() noexcept {
        glDeleteTextures(1,&shape_); glDeleteTextures(1,&empty_);
        shape_=empty_=0; visible_=tracking_=false;
    }
    bool tracking() const { return tracking_; }
    void update(const uint8_t* bytes, size_t size) {
        size_t offset=0;
        const auto read=[&](auto& value) {
            if (sizeof(value)>size-offset) throw std::runtime_error("Truncated cursor command");
            std::memcpy(&value,bytes+offset,sizeof(value)); offset+=sizeof(value);
        };
        uint8_t action=0; read(action);
        if (!action) {
            if (offset!=size) throw std::runtime_error("Invalid cursor clear");
            visible_=tracking_=false; return;
        }
        if (action!=1) throw std::runtime_error("Invalid cursor operation");
        uint8_t visible=0,format=0; int64_t x=0,y=0;
        uint32_t desktop_width=0,desktop_height=0;
        read(visible); read(x); read(y); read(desktop_width); read(desktop_height); read(format);
        if (visible>1 || !desktop_width || !desktop_height || desktop_width>8192 || desktop_height>8192 ||
            x < -INT64_C(4294967295) || x > INT64_C(4294967295) || y < -INT64_C(4294967295) || y > INT64_C(4294967295))
            throw std::runtime_error("Invalid cursor coordinates");
        if (format) {
            uint32_t width,height,hotspot_x,hotspot_y;
            read(width); read(height); read(hotspot_x); read(hotspot_y);
            if (format>2 || !width || !height || width>512 || height>512 || hotspot_x>=width || hotspot_y>=height ||
                size-offset!=size_t(width)*height*(format==1 ? 4 : 8))
                throw std::runtime_error("Invalid cursor image command");
            const auto shape=texture(width*(format==1 ? 1 : 2),height,bytes+offset);
            glDeleteTextures(1,&shape_); shape_=shape;
            format_=format; width_=width; height_=height; hotspot_x_=hotspot_x; hotspot_y_=hotspot_y;
        } else if (offset!=size) throw std::runtime_error("Unexpected cursor image bytes");
        if (visible && !shape_) throw std::runtime_error("Visible cursor missing image");
        x_=x; y_=y; desktop_width_=desktop_width; desktop_height_=desktop_height;
        visible_=visible!=0; tracking_=true;
    }
    Parameters parameters(Viewport viewport, int drawable_height, double display_width, double display_height) const {
        Parameters p{};
        if (!visible_ || !shape_) return p;
        const double sx=viewport.width/display_width,sy=viewport.height/display_height;
        if (sx<=0 || sy<=0) return p;
        const double origin_y=drawable_height-viewport.y-viewport.height;
        const double left=viewport.x+double(x_)*viewport.width/desktop_width_-hotspot_x_*sx;
        const double top=origin_y+double(y_)*viewport.height/desktop_height_-hotspot_y_*sy;
        if (left>=viewport.x+viewport.width || top>=origin_y+viewport.height ||
            left+width_*sx<=viewport.x || top+height_*sy<=origin_y) return p;
        return {float(left),float(top),float(1/sx),float(1/sy),width_,height_,format_,1};
    }
    GLuint image() {
        if (shape_) return shape_;
        if (!empty_) { const uint8_t zero[4]{}; empty_=texture(1,1,zero); }
        return empty_;
    }
    void bind(GLuint program, const Parameters& p, int drawable_height) {
        const auto shape=image();
        glActiveTexture(GL_TEXTURE2); glBindTexture(GL_TEXTURE_2D,shape);
        glUniform1i(glGetUniformLocation(program,"cursor_shape"),2);
        glUniform4f(glGetUniformLocation(program,"cursor_geometry"),p.x,p.y,p.inverse_x,p.inverse_y);
        glUniform4ui(glGetUniformLocation(program,"cursor_image"),p.width,p.height,p.format,p.visible);
        glUniform1f(glGetUniformLocation(program,"drawable_height"),float(drawable_height));
        glActiveTexture(GL_TEXTURE0);
    }
};
}
