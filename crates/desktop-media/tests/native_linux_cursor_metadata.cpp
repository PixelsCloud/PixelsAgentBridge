#include "../native/linux/cursor_metadata.h"
#include <cassert>
#include <cstdio>
using namespace pab_linux;
template<class T> static void put(std::vector<uint8_t>& data,size_t offset,const T& value) {
    assert(offset+sizeof(value)<=data.size());std::memcpy(data.data()+offset,&value,sizeof(value));
}
int main() {
    Display screen{"test","test",-1920,-100,{3840,2160,1920,1080,2000},true,1};
    spa_meta_cursor cursor{};cursor.id=1;cursor.position={200,100};cursor.hotspot={2,1};cursor.bitmap_offset=sizeof(cursor);
    spa_meta_bitmap bitmap{};bitmap.format=SPA_VIDEO_FORMAT_RGBA;bitmap.size={4,3};bitmap.stride=20;bitmap.offset=sizeof(bitmap);
    const size_t base=sizeof(cursor)+sizeof(bitmap);
    std::vector<uint8_t> bytes(base+56,0xcc);put(bytes,0,cursor);put(bytes,sizeof(cursor),bitmap);
    for(unsigned y=0;y<3;++y)for(unsigned x=0;x<4;++x) {
        auto* p=bytes.data()+base+y*20+x*4;p[0]=x*16;p[1]=y*24;p[2]=64;p[3]=128;
    }
    const auto apply=[&](CursorState& state,const std::vector<uint8_t>& data) {
        spa_meta meta{SPA_META_Cursor,uint32_t(data.size()),const_cast<uint8_t*>(data.data())};cursor_metadata(state,screen,meta);
    };
    CursorState state;apply(state,bytes);auto first=state.snapshot();
    assert(first.visible && first.x==-1820 && first.y==-50 && first.image_id==1 && first.revision==1);
    assert(first.image->hotspot_x==2 && first.image->hotspot_y==1 && first.image->pixels.size()==48);
    unsigned checked=0;
    for(unsigned y=0;y<3;++y)for(unsigned x=0;x<4;++x) {
        const auto* p=first.image->pixels.data()+(y*4+x)*4;
        assert(p[0]==64 && p[1]==y*24 && p[2]==x*16 && p[3]==128);checked+=4;
    }
    apply(state,bytes);assert(state.snapshot().revision==1);
    auto position=cursor;position.position={-20,80};position.hotspot={0,0};position.bitmap_offset=0;
    std::vector<uint8_t> motion(sizeof(position));put(motion,0,position);apply(state,motion);
    auto moved=state.snapshot();assert(moved.visible && moved.x==-1930 && moved.y==-60);
    assert(moved.image==first.image && moved.image_id==1 && moved.image->hotspot_x==2);
    position.id=0;put(motion,0,position);apply(state,motion);assert(!state.snapshot().visible);
    position.id=1;put(motion,0,position);apply(state,motion);assert(state.snapshot().visible);
    auto empty=bytes;spa_meta_bitmap blank{};put(empty,sizeof(cursor),blank);apply(state,empty);
    assert(!state.snapshot().visible);apply(state,motion);assert(!state.snapshot().visible);
    assert(state.snapshot().image==first.image); // A reopening reader can still receive the known cached image.
    apply(state,bytes);assert(state.snapshot().visible && state.snapshot().image_id==1);
    auto changed=bytes;changed[base]=1;apply(state,changed);assert(state.snapshot().image_id==2);
    // Unaligned metadata is copied into aligned local structs before access.
    std::vector<uint8_t> unaligned(1+bytes.size());std::copy(bytes.begin(),bytes.end(),unaligned.begin()+1);
    spa_meta meta{SPA_META_Cursor,uint32_t(bytes.size()),unaligned.data()+1};CursorState unaligned_state;
    cursor_metadata(unaligned_state,screen,meta);assert(unaligned_state.snapshot().image->pixels==first.image->pixels);
    unsigned rejected=0;
    for(size_t n=0;n<bytes.size();++n) {
        auto short_packet=std::vector<uint8_t>(bytes.begin(),bytes.begin()+n);CursorState candidate;
        try { apply(candidate,short_packet);assert(false); } catch(const std::exception&) { ++rejected; }
        assert(candidate.snapshot().revision==0);
    }
    for(unsigned check=0;check<9;++check) {
        auto bad=bytes;auto c=cursor;auto b=bitmap;
        if(check==0)c.bitmap_offset=1;
        if(check==1)c.bitmap_offset=UINT32_MAX;
        if(check==2)b.offset=UINT32_MAX;
        if(check==3)b.stride=-1;
        if(check==4)b.size.width=513;
        if(check==5)c.hotspot.x=4;
        if(check==6)b.format=SPA_VIDEO_FORMAT_NV12;
        if(check==7)b.stride=INT32_MAX;
        if(check==8)bad[base]=255;
        put(bad,0,c);put(bad,sizeof(c),b);CursorState candidate;
        try { apply(candidate,bad);assert(false); } catch(const std::exception&) { ++rejected; }
        assert(candidate.snapshot().revision==0);
    }
    CursorState absent;apply(absent,motion);assert(!absent.snapshot().visible && !absent.snapshot().image_id);
    state.fail("generated failure");try { state.snapshot();assert(false); } catch(const std::exception&) {}
    printf("{\"color_channels\":%u,\"malformed_rejected\":%u,\"hotspot_cache\":true,\"empty_does_not_resurrect\":true,\"unaligned_safe\":true}\n",checked,rejected);
}
