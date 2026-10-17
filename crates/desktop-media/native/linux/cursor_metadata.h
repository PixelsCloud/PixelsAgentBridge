#pragma once
#include "common.h"
#include <spa/buffer/buffer.h>
#include <spa/param/video/raw.h>
#include <mutex>

namespace pab_linux {
struct CursorImage {
    uint32_t width=0, height=0, hotspot_x=0, hotspot_y=0;
    std::vector<uint8_t> pixels;
    bool operator==(const CursorImage& other) const {
        return width==other.width && height==other.height && hotspot_x==other.hotspot_x &&
            hotspot_y==other.hotspot_y && pixels==other.pixels;
    }
};
struct CursorSnapshot {
    bool visible=false;
    int32_t x=0, y=0;
    uint64_t image_id=0, revision=0;
    std::shared_ptr<const CursorImage> image;
};
// One immutable image, one latest position. Metadata callbacks never wait for
// pipe/network output and position-only changes never copy the shape pixels.
class CursorState {
    mutable std::mutex mutex_;
    CursorSnapshot latest_;
    bool sprite_present_=false;
    std::string error_;
public:
    CursorSnapshot snapshot() const {
        std::lock_guard<std::mutex> lock(mutex_);
        if(!error_.empty())throw std::runtime_error(error_);
        return latest_;
    }
    void fail(const char* error) {
        std::lock_guard<std::mutex> lock(mutex_); error_=error;
    }
    void update(bool visible,int32_t x,int32_t y,std::shared_ptr<const CursorImage> image=nullptr,bool clear=false) {
        std::lock_guard<std::mutex> lock(mutex_);
        bool changed=latest_.revision==0;
        if(clear && sprite_present_) { sprite_present_=false; changed=true; }
        if(image && !sprite_present_) { sprite_present_=true;changed=true; }
        if(image && (!latest_.image || !(*latest_.image==*image))) {
            if(latest_.image_id==UINT64_MAX)throw std::runtime_error("Cursor image sequence exhausted");
            latest_.image=std::move(image);++latest_.image_id;changed=true;
        }
        visible=visible && sprite_present_ && bool(latest_.image);
        changed=changed || latest_.visible!=visible || (visible && (latest_.x!=x || latest_.y!=y));
        latest_.visible=visible;latest_.x=x;latest_.y=y;
        if(changed) {
            if(latest_.revision==UINT64_MAX)throw std::runtime_error("Cursor revision exhausted");
            ++latest_.revision;
        }
    }
};

inline int32_t cursor_position(int32_t pixel,int origin,unsigned logical,unsigned physical) {
    if(!logical || !physical)throw std::runtime_error("Invalid cursor screen geometry");
    const double position=double(origin)+double(pixel)*logical/physical;
    if(position<INT32_MIN || position>INT32_MAX)throw std::runtime_error("Cursor position overflow");
    return int32_t(std::lround(position));
}
template<class T> inline T cursor_field(const uint8_t* bytes,size_t size,size_t offset) {
    if(offset>size || sizeof(T)>size-offset)throw std::runtime_error("Truncated cursor metadata");
    T value;std::memcpy(&value,bytes+offset,sizeof(value));return value;
}
// GNOME's version-4 adapter uses premultiplied RGBA cursor pixels, already
// oriented and scaled to capture pixels. Position is the hotspot relative to
// that capture. A position-only sample writes zero hotspots: keep the image's
// hotspot rather than resetting it to zero. This is not generic Wayland input.
inline void cursor_metadata(CursorState& state,const Display& display,const spa_meta& meta) {
    constexpr size_t maximum=sizeof(spa_meta_cursor)+sizeof(spa_meta_bitmap)+512*512*4;
    if(meta.type!=SPA_META_Cursor || !meta.data || meta.size>maximum)
        throw std::runtime_error("Invalid cursor metadata extent");
    const auto* bytes=static_cast<const uint8_t*>(meta.data);
    const auto cursor=cursor_field<spa_meta_cursor>(bytes,meta.size,0);
    if(!cursor.id) { state.update(false,0,0);return; }
    const auto x=cursor_position(cursor.position.x,display.x,display.size.logical_width,display.size.width);
    const auto y=cursor_position(cursor.position.y,display.y,display.size.logical_height,display.size.height);
    if(!cursor.bitmap_offset) { state.update(true,x,y);return; }
    if(cursor.bitmap_offset<sizeof(cursor))throw std::runtime_error("Cursor bitmap overlaps header");
    const auto bitmap=cursor_field<spa_meta_bitmap>(bytes,meta.size,cursor.bitmap_offset);
    // Mutter's explicit empty-sprite sample is a fully zeroed bitmap. A later
    // position-only sample must not resurrect the previously visible image.
    if(!bitmap.format && !bitmap.size.width && !bitmap.size.height && !bitmap.stride && !bitmap.offset) {
        state.update(false,x,y,nullptr,true);return;
    }
    if(!bitmap.format) { state.update(true,x,y);return; } // SPA: no new bitmap information.
    const auto width=bitmap.size.width,height=bitmap.size.height;
    if(bitmap.format!=SPA_VIDEO_FORMAT_RGBA || !width || !height || width>512 || height>512 ||
        bitmap.stride<int64_t(width)*4 || bitmap.offset<sizeof(bitmap) ||
        cursor.hotspot.x<0 || cursor.hotspot.y<0 || uint32_t(cursor.hotspot.x)>=width || uint32_t(cursor.hotspot.y)>=height)
        throw std::runtime_error("Invalid cursor bitmap format or geometry");
    const uint64_t offset=uint64_t(cursor.bitmap_offset)+bitmap.offset;
    const uint64_t length=uint64_t(height-1)*bitmap.stride+uint64_t(width)*4;
    if(offset>meta.size || length>meta.size-offset)throw std::runtime_error("Cursor bitmap outside metadata");
    auto image=std::make_shared<CursorImage>();
    image->width=width;image->height=height;image->hotspot_x=cursor.hotspot.x;image->hotspot_y=cursor.hotspot.y;
    image->pixels.resize(size_t(width)*height*4);
    for(unsigned row=0;row<height;++row)for(unsigned column=0;column<width;++column) {
        const auto* source=bytes+offset+size_t(row)*bitmap.stride+column*4;
        auto* target=image->pixels.data()+(size_t(row)*width+column)*4;
        if(source[0]>source[3] || source[1]>source[3] || source[2]>source[3])
            throw std::runtime_error("Cursor color is not premultiplied");
        target[0]=source[2];target[1]=source[1];target[2]=source[0];target[3]=source[3];
    }
    state.update(true,x,y,std::move(image));
}
}
