#pragma once
#include "dma_frame.h"
#include <gbm.h>
#include <epoxy/egl.h>
#include <epoxy/gl.h>
#include <linux/dma-buf.h>
#include <sys/ioctl.h>
#include <sys/mman.h>

namespace pab_linux::dma {
// One persistent context, no surface/context recreation per frame. Only this
// unprivileged consumer initializes graphics; the exporter never creates a GPU context.
class ImageReader {
    Fd render_;
    dev_t render_device_{};
    gbm_device* gbm_ = nullptr;
    EGLDisplay display_ = EGL_NO_DISPLAY;
    EGLContext context_ = EGL_NO_CONTEXT;
    GLuint texture_ = 0, framebuffer_ = 0;
    std::vector<uint8_t> pixels_;
    bool bgra_ = false;
    bool linear_cursor_ = false;
    bool read_linear_cursor(const Frame& frame, uint64_t extent) {
        const auto& m = frame.info;
        if (m.modifier != DRM_FORMAT_MOD_LINEAR) return false;
        struct stat info{};
        if (fstat(frame.image.get(), &info) || info.st_size < 0 ||
            (info.st_size && uint64_t(info.st_size) < extent))
            throw std::runtime_error("DMA cursor descriptor extent mismatch");
        const auto sync = [&](uint64_t flags) {
            dma_buf_sync value{flags}; int status;
            do { status = ioctl(frame.image.get(), DMA_BUF_IOCTL_SYNC, &value); } while (status < 0 && errno == EINTR);
            return status == 0;
        };
        if (!sync(DMA_BUF_SYNC_START | DMA_BUF_SYNC_READ)) return false;
        struct EndSync {
            int fd; bool ended=false;
            bool end() {
                if(ended)return true;
                ended=true; dma_buf_sync value{DMA_BUF_SYNC_END | DMA_BUF_SYNC_READ}; int status;
                do { status=ioctl(fd,DMA_BUF_IOCTL_SYNC,&value); } while(status<0 && errno==EINTR);
                return status==0;
            }
            ~EndSync(){end();}
        } end{frame.image.get()};
        auto* data=mmap(nullptr,size_t(extent),PROT_READ,MAP_SHARED,frame.image.get(),0);
        if(data==MAP_FAILED)return false;
        struct Unmap { void* data;size_t length;~Unmap(){munmap(data,length);} } unmap{data,size_t(extent)};
        pixels_.resize(size_t(m.width)*m.height*4);
        for(unsigned y=0;y<m.height;++y) {
            const auto* source=static_cast<const uint8_t*>(data)+m.offset+size_t(y)*m.stride;
            auto* target=pixels_.data()+size_t(y)*m.width*4;
            memcpy(target,source,size_t(m.width)*4);
        }
        if(m.format==DRM_FORMAT_ABGR8888)
            for(size_t i=0;i<pixels_.size();i+=4)std::swap(pixels_[i],pixels_[i+2]);
        if(!end.end())return false;
        linear_cursor_=true;return true;
    }
    void cleanup() noexcept {
        if (framebuffer_) glDeleteFramebuffers(1, &framebuffer_);
        if (texture_) glDeleteTextures(1, &texture_);
        framebuffer_ = texture_ = 0;
        if (context_ != EGL_NO_CONTEXT) {
            eglMakeCurrent(display_, EGL_NO_SURFACE, EGL_NO_SURFACE, EGL_NO_CONTEXT);
            eglDestroyContext(display_, context_); context_ = EGL_NO_CONTEXT;
        }
        if (display_ != EGL_NO_DISPLAY) { eglTerminate(display_); display_ = EGL_NO_DISPLAY; }
        if (gbm_) { gbm_device_destroy(gbm_); gbm_ = nullptr; }
    }
    void initialize(const Frame& frame) {
        struct stat info{};
        if (fstat(frame.render.get(), &info) || !S_ISCHR(info.st_mode))
            throw std::runtime_error("Invalid DMA render node");
        if (display_ != EGL_NO_DISPLAY) {
            if (info.st_rdev != render_device_) throw std::runtime_error("DMA render device changed");
            return;
        }
        render_device_ = info.st_rdev;
        render_ = Fd(fcntl(frame.render.get(), F_DUPFD_CLOEXEC, 3));
        if (render_.get() < 0) throw std::runtime_error("Cannot retain DMA render node");
        try {
            gbm_ = gbm_create_device(render_.get());
            if (!gbm_) throw std::runtime_error("Cannot create DMA GBM device");
            display_ = eglGetPlatformDisplayEXT(EGL_PLATFORM_GBM_KHR, gbm_, nullptr);
            EGLint major, minor;
            if (display_ == EGL_NO_DISPLAY || !eglInitialize(display_, &major, &minor) || !eglBindAPI(EGL_OPENGL_ES_API))
                throw std::runtime_error("Cannot initialize DMA EGL display");
            if (!epoxy_has_egl_extension(display_, "EGL_EXT_image_dma_buf_import"))
                throw std::runtime_error("EGL DMA import is unavailable");
            EGLConfig config; EGLint count;
            const EGLint attributes[]{EGL_RENDERABLE_TYPE, EGL_OPENGL_ES2_BIT, EGL_SURFACE_TYPE, EGL_WINDOW_BIT, EGL_NONE};
            if (!eglChooseConfig(display_, attributes, &config, 1, &count) || !count)
                throw std::runtime_error("Cannot choose DMA EGL configuration");
            const EGLint version[]{EGL_CONTEXT_CLIENT_VERSION, 2, EGL_NONE};
            context_ = eglCreateContext(display_, config, EGL_NO_CONTEXT, version);
            if (context_ == EGL_NO_CONTEXT || !eglMakeCurrent(display_, EGL_NO_SURFACE, EGL_NO_SURFACE, context_))
                throw std::runtime_error("Cannot create DMA EGL context");
            if (!epoxy_has_gl_extension("GL_OES_EGL_image")) throw std::runtime_error("EGL image textures unavailable");
            bgra_ = epoxy_has_gl_extension("GL_EXT_read_format_bgra");
            glGenTextures(1, &texture_); glGenFramebuffers(1, &framebuffer_);
        } catch (...) { cleanup(); throw; }
    }
public:
    ImageReader() = default;
    ImageReader(const ImageReader&) = delete;
    ImageReader& operator=(const ImageReader&) = delete;
    ~ImageReader() { cleanup(); }
    const std::vector<uint8_t>& read(const Frame& frame, bool cursor = false) {
        const auto& m = frame.info;
        const auto extent = cursor ? validate_cursor_image(m, m.sequence) : validate(m, m.sequence);
        linear_cursor_ = false;
        // Some virtual cursor BOs are linear dumb buffers without an EGL
        // surface. Map only the bounded cursor, with explicit DMA CPU sync.
        if (cursor && read_linear_cursor(frame, extent)) return pixels_;
        initialize(frame);
        std::vector<EGLint> attributes{EGL_WIDTH, EGLint(m.width), EGL_HEIGHT, EGLint(m.height),
            EGL_LINUX_DRM_FOURCC_EXT, EGLint(m.format), EGL_DMA_BUF_PLANE0_FD_EXT, frame.image.get(),
            EGL_DMA_BUF_PLANE0_OFFSET_EXT, EGLint(m.offset), EGL_DMA_BUF_PLANE0_PITCH_EXT, EGLint(m.stride)};
        if (m.flags & 2) {
            if (!epoxy_has_egl_extension(display_, "EGL_EXT_image_dma_buf_import_modifiers"))
                throw std::runtime_error("EGL DMA modifiers unavailable");
            attributes.insert(attributes.end(), {EGL_DMA_BUF_PLANE0_MODIFIER_LO_EXT, EGLint(uint32_t(m.modifier)),
                EGL_DMA_BUF_PLANE0_MODIFIER_HI_EXT, EGLint(uint32_t(m.modifier >> 32))});
        }
        attributes.push_back(EGL_NONE);
        const auto image = eglCreateImageKHR(display_, EGL_NO_CONTEXT, EGL_LINUX_DMA_BUF_EXT, nullptr, attributes.data());
        if (image == EGL_NO_IMAGE_KHR) throw std::runtime_error("Cannot import DMA image");
        struct Drop { EGLDisplay d; EGLImageKHR i; ~Drop() { eglDestroyImageKHR(d, i); } } drop{display_, image};
        glBindTexture(GL_TEXTURE_2D, texture_); glEGLImageTargetTexture2DOES(GL_TEXTURE_2D, image);
        glBindFramebuffer(GL_FRAMEBUFFER, framebuffer_);
        glFramebufferTexture2D(GL_FRAMEBUFFER, GL_COLOR_ATTACHMENT0, GL_TEXTURE_2D, texture_, 0);
        if (glGetError() != GL_NO_ERROR || glCheckFramebufferStatus(GL_FRAMEBUFFER) != GL_FRAMEBUFFER_COMPLETE)
            throw std::runtime_error("DMA image is not readable");
        pixels_.resize(size_t(m.width) * m.height * 4);
        glReadPixels(0, 0, GLsizei(m.width), GLsizei(m.height), bgra_ ? GL_BGRA_EXT : GL_RGBA, GL_UNSIGNED_BYTE, pixels_.data());
        if (glGetError() != GL_NO_ERROR) throw std::runtime_error("DMA pixel readback failed");
        // The existing bundled encoder takes BGRA. Prefer direct BGRA readback;
        // only RGBA-only drivers need this in-place swizzle, without a copy.
        if (!bgra_) for (size_t i = 0; i < pixels_.size(); i += 4) std::swap(pixels_[i], pixels_[i + 2]);
        return pixels_;
    }
    bool direct_bgra() const { return bgra_; }
    bool linear_cursor() const { return linear_cursor_; }
};
}
