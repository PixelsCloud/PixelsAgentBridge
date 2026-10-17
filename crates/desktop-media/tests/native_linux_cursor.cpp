// Generated NV12/cursor pixels only. Reuses the product OpenGL shader and
// command parser; the CPU reference is independent of its parameter builder.
#include "../native/linux/cursor_overlay.h"
#include "../native/linux/video_shader.h"
#include "../native/linux/viewer_input.h"
#include <gtk/gtk.h>
#include <array>
#include <iostream>

static void require(bool value,const char* message) { if (!value) throw std::runtime_error(message); }
template<class T> static void append(std::vector<uint8_t>& out,T value) {
    const auto* bytes=reinterpret_cast<const uint8_t*>(&value);out.insert(out.end(),bytes,bytes+sizeof(value));
}
static std::vector<uint8_t> packet(int64_t x,int64_t y,unsigned format,bool visible,bool image) {
    std::vector<uint8_t> out{1,uint8_t(visible)};
    append(out,x);append(out,y);append(out,uint32_t(320));append(out,uint32_t(240));out.push_back(image ? format : 0);
    if (!image) return out;
    for (uint32_t v : {17,13,5,7}) append(out,v);
    for (unsigned y=0;y<13;++y) for (unsigned x=0;x<17;++x) {
        if (format==1) {
            const auto alpha=std::array<uint8_t,4>{0,80,160,255}[(x+y)%4];
            for (uint8_t v : {uint8_t(alpha/4),uint8_t(alpha/2),alpha,alpha}) out.push_back(v);
        } else {
            const uint8_t mask=(x+y)%4<2 ? 0 : 255,value=(x+y)%2 ? 255 : 0;
            for (uint8_t v : {mask,mask,mask,uint8_t(0),value,value,value,uint8_t(0)}) out.push_back(v);
        }
    }
    return out;
}
static GLuint shader(GLenum kind,const char* source) {
    auto result=glCreateShader(kind);glShaderSource(result,1,&source,nullptr);glCompileShader(result);
    GLint good=0;glGetShaderiv(result,GL_COMPILE_STATUS,&good);
    if (!good) { char error[1024]{};glGetShaderInfoLog(result,sizeof(error),nullptr,error);throw std::runtime_error(error); }
    return result;
}
int main() {
    try {
        require(gtk_init_check(nullptr,nullptr),"GTK unavailable");
        auto* window=gtk_window_new(GTK_WINDOW_TOPLEVEL);auto* area=gtk_gl_area_new();
        gtk_gl_area_set_required_version(GTK_GL_AREA(area),3,2);
        gtk_gl_area_set_use_es(GTK_GL_AREA(area),FALSE);
        // This bridge is attached before realization, as in the product.
        auto input=std::make_unique<pab_linux::ViewInput>(area,window,320,240,0);
        input->sink([](const pab_linux::InputPacket&) { return true; });
        gtk_window_set_default_size(GTK_WINDOW(window),384,256);
        gtk_container_add(GTK_CONTAINER(window),area);gtk_widget_show_all(window);
        gtk_gl_area_make_current(GTK_GL_AREA(area));require(!gtk_gl_area_get_error(GTK_GL_AREA(area)),"GL unavailable");
        auto vs=shader(GL_VERTEX_SHADER,pab_linux::video_vertex()),fs=shader(GL_FRAGMENT_SHADER,pab_linux::video_fragment());
        auto program=glCreateProgram();glAttachShader(program,vs);glAttachShader(program,fs);glLinkProgram(program);
        GLint good=0;glGetProgramiv(program,GL_LINK_STATUS,&good);require(good,"GL link failed");
        glDeleteShader(vs);glDeleteShader(fs);glUseProgram(program);
        GLuint vao=0;glGenVertexArrays(1,&vao);glBindVertexArray(vao);
        const auto texture=[](GLint internal,GLenum format,int w,int h,const uint8_t* data) {
            GLuint t=0;glGenTextures(1,&t);glBindTexture(GL_TEXTURE_2D,t);
            glTexParameteri(GL_TEXTURE_2D,GL_TEXTURE_MIN_FILTER,GL_LINEAR);glTexParameteri(GL_TEXTURE_2D,GL_TEXTURE_MAG_FILTER,GL_LINEAR);
            glTexParameteri(GL_TEXTURE_2D,GL_TEXTURE_WRAP_S,GL_CLAMP_TO_EDGE);glTexParameteri(GL_TEXTURE_2D,GL_TEXTURE_WRAP_T,GL_CLAMP_TO_EDGE);
            glTexImage2D(GL_TEXTURE_2D,0,internal,w,h,0,format,GL_UNSIGNED_BYTE,data);return t;
        };
        glPixelStorei(GL_UNPACK_ALIGNMENT,1);glPixelStorei(GL_PACK_ALIGNMENT,1);
        std::vector<uint8_t> y(320*240,100),uv(160*120*2,128);
        auto luma=texture(GL_R8,GL_RED,320,240,y.data()),chroma=texture(GL_RG8,GL_RG,160,120,uv.data());
        auto target=texture(GL_RGBA8,GL_RGBA,384,256,nullptr);
        GLuint fbo=0;glGenFramebuffers(1,&fbo);glBindFramebuffer(GL_FRAMEBUFFER,fbo);
        glFramebufferTexture2D(GL_FRAMEBUFFER,GL_COLOR_ATTACHMENT0,GL_TEXTURE_2D,target,0);
        require(glCheckFramebufferStatus(GL_FRAMEBUFFER)==GL_FRAMEBUFFER_COMPLETE,"Framebuffer incomplete");
        glDisable(GL_DEPTH_TEST);glDisable(GL_BLEND);glDisable(GL_SCISSOR_TEST);glDisable(GL_FRAMEBUFFER_SRGB);glDisable(GL_DITHER);
        uint64_t pixels=0;unsigned cases=0,rejected=0,max_error=0;
        for (unsigned rotation=0;rotation<4;++rotation) for (unsigned format : {1,2}) for (double scale : {.5,1.0}) {
            const int w=rotation%2 ? 240 : 320,h=rotation%2 ? 320 : 240;
            const pab_linux::Viewport viewport{int((384-w*scale)/2),int((256-h*scale)/2),int(w*scale),int(h*scale)};
            const double origin_y=256-viewport.y-viewport.height;
            pab_linux::CursorOverlay cursor;
            const auto draw=[&] {
                glUseProgram(program);glClearColor(0,0,0,1);glClear(GL_COLOR_BUFFER_BIT);
                glViewport(viewport.x,viewport.y,viewport.width,viewport.height);
                glUniform1i(glGetUniformLocation(program,"rotation"),rotation);
                glUniform2f(glGetUniformLocation(program,"visible_fraction"),1,1);
                glActiveTexture(GL_TEXTURE0);glBindTexture(GL_TEXTURE_2D,luma);glUniform1i(glGetUniformLocation(program,"plane_y"),0);
                glActiveTexture(GL_TEXTURE1);glBindTexture(GL_TEXTURE_2D,chroma);glUniform1i(glGetUniformLocation(program,"plane_uv"),1);
                cursor.bind(program,cursor.parameters(viewport,256,w,h),256);glDrawArrays(GL_TRIANGLE_STRIP,0,4);
                std::vector<uint8_t> pixels(384*256*4);glReadPixels(0,0,384,256,GL_RGBA,GL_UNSIGNED_BYTE,pixels.data());
                require(glGetError()==GL_NO_ERROR,"GL drawing/readback failed");return pixels;
            };
            auto base=draw();bool first=true;
            for (const auto position : std::array<std::array<int64_t,2>,5>{{{123,111},{0,0},{319,239},{-4294967295LL,0},{4294967295LL,0}}}) {
                auto bytes=packet(position[0],position[1],format,true,first);first=false;cursor.update(bytes.data(),bytes.size());
                auto actual=draw();
                const double left=viewport.x+position[0]*double(viewport.width)/320-5*scale;
                const double top=origin_y+position[1]*double(viewport.height)/240-7*scale;
                for (unsigned py=0;py<256;++py) for (unsigned px=0;px<384;++px) {
                    const double sx=std::floor((px+.5-left)/scale),sy=std::floor((py+.5-top)/scale);
                    const bool inside=px+.5>=viewport.x && px+.5<viewport.x+viewport.width &&
                        py+.5>=origin_y && py+.5<origin_y+viewport.height && sx>=0 && sx<17 && sy>=0 && sy<13;
                    const auto offset=((255-py)*384+px)*4; // GL readback is bottom-up.
                    for (unsigned channel=0;channel<3;++channel) {
                        unsigned expected=base[offset+channel];
                        if (inside) {
                            const unsigned index=unsigned(sx)+unsigned(sy);
                            if (format==1) {
                                const auto alpha=std::array<unsigned,4>{0,80,160,255}[index%4];
                                const auto color=channel==2 ? alpha/4 : channel==1 ? alpha/2 : alpha;
                                expected=unsigned(std::lround(color+expected*(1-alpha/255.0)));
                            } else expected=(expected & (index%4<2 ? 0 : 255)) ^ (index%2 ? 255 : 0);
                        }
                        const auto error=unsigned(std::abs(int(actual[offset+channel])-int(expected)));
                        max_error=std::max(max_error,error);
                        require(error<=(format==1 && inside ? 1U : 0U),"Cursor reference pixel mismatch");
                    }
                    ++pixels;
                }
                ++cases;
            }
            auto hidden=packet(123,111,format,false,false);cursor.update(hidden.data(),hidden.size());
            require(draw()==base && cursor.tracking(),"Hidden changed background/tracking");++cases;
            const uint8_t clear=0;cursor.update(&clear,1);require(draw()==base && !cursor.tracking(),"Clear failed");++cases;
            auto valid=packet(123,111,format,true,true);
            for (size_t size : {size_t(0),size_t(1),size_t(26),size_t(42),valid.size()-1}) {
                bool refused=false;try { cursor.update(valid.data(),size); } catch (const std::runtime_error&) { refused=true; }
                require(refused,"Truncated command accepted");++rejected;
            }
            cursor.release();
        }
        // Deliver events only to this test widget; no global pointer warps or
        // input into another application. Check window ownership as well as
        // restoration when control/channel state changes without more motion.
        GdkWindow* input_window=nullptr;
        auto* children=gdk_window_get_children(gtk_widget_get_parent_window(area));
        for (auto* entry=children;entry;entry=entry->next) {
            gpointer owner=nullptr;gdk_window_get_user_data(GDK_WINDOW(entry->data),&owner);
            if (owner==area) input_window=GDK_WINDOW(entry->data);
        }
        g_list_free(children);require(input_window,"GL input window missing");
        auto* host_window=gtk_widget_get_window(window);
        auto* original_host_cursor=gdk_window_get_cursor(host_window);
        const auto crossing=[&](bool inside) {
            GdkEventCrossing event{};event.type=inside ? GDK_ENTER_NOTIFY : GDK_LEAVE_NOTIFY;
            event.x=gtk_widget_get_allocated_width(area)/2;event.y=gtk_widget_get_allocated_height(area)/2;
            gboolean handled=FALSE;g_signal_emit_by_name(area,inside ? "enter-notify-event" : "leave-notify-event",&event,&handled);
        };
        auto* initial_cursor=gdk_window_get_cursor(input_window);
        auto* blank_reference=gdk_cursor_new_for_display(gtk_widget_get_display(area),GDK_BLANK_CURSOR);
        require(blank_reference,"Blank reference cursor unavailable");
        const auto blank=[&] { auto* c=gdk_window_get_cursor(input_window);return c && c!=initial_cursor &&
            gdk_cursor_get_cursor_type(c)==gdk_cursor_get_cursor_type(blank_reference); };
        input->cursor_tracking(true);crossing(true);require(!blank(),"View-only pointer hidden");
        input->enable(true);crossing(true);require(blank(),"Control pointer not hidden");
        require(gdk_window_get_cursor(host_window)==original_host_cursor,"Shared host cursor changed");
        GdkEventMotion motion{};motion.type=GDK_MOTION_NOTIFY;motion.x=0;motion.y=gtk_widget_get_allocated_height(area)/2;
        gboolean handled=FALSE;g_signal_emit_by_name(area,"motion-notify-event",&motion,&handled);
        require(!blank(),"Letterbox pointer hidden");crossing(true);require(blank(),"Image reentry failed");
        crossing(false);require(!blank(),"Leave did not restore pointer");
        crossing(true);require(blank(),"Reentry did not hide pointer");
        input->enable(false);require(!blank(),"Stop control did not restore pointer");
        input->enable(true);crossing(true);require(blank(),"Control reenable failed");
        input->cursor_tracking(false);require(!blank(),"Cursor failure did not restore pointer");
        input->cursor_tracking(true);crossing(true);require(blank(),"Tracking restart failed");
        input.reset();require(!blank(),"Bridge drop did not restore pointer");
        std::cout<<"{\"cases\":"<<cases<<",\"pixels\":"<<pixels<<",\"max_error\":"<<max_error
            <<",\"truncated_rejected\":"<<rejected<<",\"local_widget_cursor_checks\":true,\"blank_reference_type\":"
            <<gdk_cursor_get_cursor_type(blank_reference)<<",\"renderer\":\""<<glGetString(GL_RENDERER)<<"\"}\n";
        g_object_unref(blank_reference);
        glDeleteTextures(1,&luma);glDeleteTextures(1,&chroma);glDeleteTextures(1,&target);
        glDeleteFramebuffers(1,&fbo);glDeleteVertexArrays(1,&vao);glDeleteProgram(program);
        gtk_widget_destroy(window);return 0;
    } catch (const std::exception& e) { std::cerr<<e.what()<<'\n';return 1; }
}
