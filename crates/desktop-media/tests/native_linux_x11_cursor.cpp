#include "../native/linux/x11_cursor.h"
#include <X11/cursorfont.h>
#include <cassert>
#include <cstdio>

// Run only on a disposable X server: this check changes its root cursor and
// pointer, including moving between X screens. It never targets a user's GUI.
int main(int argc, char** argv) {
    using namespace pab_linux;
    unsigned channels = 0, rejected = 0;
    unsigned long pixels[]{0x80402010ul,0xffeeccaaul,0x01010101ul,0ul};
    XFixesCursorImage raw{}; raw.width=2;raw.height=2;raw.xhot=1;raw.yhot=1;raw.pixels=pixels;
    const auto image=x11::cursor_image(raw);
    const uint8_t expected[]{16,32,64,128,170,204,238,255,1,1,1,1,0,0,0,0};
    for(size_t i=0;i<sizeof(expected);++i) { assert(image->pixels[i]==expected[i]);++channels; }
    for(unsigned i=0;i<6;++i) {
        auto invalid=raw;
        if(i==0)invalid.width=0;
        if(i==1)invalid.height=513;
        if(i==2)invalid.xhot=2;
        if(i==3)invalid.yhot=2;
        if(i==4)invalid.pixels=nullptr;
        if(i==5)pixels[0]=0x01020101;
        try { x11::cursor_image(invalid);assert(false); } catch(const std::exception&) { ++rejected; }
        pixels[0]=0x80402010ul;
    }
    assert(argc==2 && std::string(argv[1])=="--isolated-display");
    x11::Connection connection;auto* d=connection.get();
    assert(ScreenCount(d)==2);
    auto found=x11::outputs(connection);
    auto first=std::find_if(found.begin(),found.end(),[](const auto& o){return o.screen==0;});
    assert(first!=found.end());
    const auto root=first->root;
    const auto arrow=XCreateFontCursor(d,XC_left_ptr),cross=XCreateFontCursor(d,XC_crosshair);
    const char zero=0; const auto bitmap=XCreateBitmapFromData(d,root,&zero,1,1);
    XColor black{}; const auto transparent=XCreatePixmapCursor(d,bitmap,bitmap,&black,&black,0,0);
    XDefineCursor(d,root,arrow);XWarpPointer(d,None,root,0,0,0,0,100,100);connection.check();
    x11::CursorSource source(connection,*first);source.sample();auto a=source.state.snapshot();
    assert(a.visible && a.x==100 && a.y==100 && a.image_id==1);
    for(int i=1;i<=3;++i) { XWarpPointer(d,None,root,0,0,0,0,100+i*10,100+i*5);connection.check();source.sample(); }
    auto moved=source.state.snapshot();
    assert(moved.x==130 && moved.y==115 && moved.image==a.image && moved.image_id==a.image_id);
    XDefineCursor(d,root,cross);connection.check();source.sample();auto changed=source.state.snapshot();
    assert(changed.visible && changed.image_id==2 && changed.image!=a.image);
    XDefineCursor(d,root,transparent);connection.check();source.sample();auto hidden=source.state.snapshot();
    assert(!hidden.visible && hidden.image_id==3);
    XWarpPointer(d,None,root,0,0,0,0,140,140);connection.check();source.sample();assert(!source.state.snapshot().visible);
    XDefineCursor(d,root,arrow);connection.check();source.sample();assert(source.state.snapshot().visible);
    XWarpPointer(d,None,RootWindow(d,1),0,0,0,0,50,50);connection.check();source.sample();assert(!source.state.snapshot().visible);
    XWarpPointer(d,None,root,0,0,0,0,150,150);connection.check();source.sample();assert(source.state.snapshot().visible);
    x11::Cursor before(XFixesGetCursorImage(d));
    XFixesHideCursor(d,root);connection.check();source.sample();
    x11::Cursor after(XFixesGetCursorImage(d));
    const bool same=before->cursor_serial==after->cursor_serial && before->width==after->width && before->height==after->height &&
        std::equal(before->pixels,before->pixels+size_t(before->width)*before->height,after->pixels);
    // Record the API limitation as an observed failure to detect visibility,
    // not as a passed hidden-cursor product requirement.
    const bool hide_not_detected=source.state.snapshot().visible;
    XFixesShowCursor(d,root);connection.check();
    XUndefineCursor(d,root);XFreeCursor(d,arrow);XFreeCursor(d,cross);XFreeCursor(d,transparent);XFreePixmap(d,bitmap);
    printf("{\"channels_checked\":%u,\"invalid_shapes_rejected\":%u,\"motion_cached_shape\":true,\"shape_changed\":true,\"transparent_hidden\":true,\"other_root_hidden\":true,\"return_root_restored\":true,\"xfixes_hide_same_image\":%s,\"xfixes_hide_not_detected\":%s}\n",
        channels,rejected,same?"true":"false",hide_not_detected?"true":"false");
}
