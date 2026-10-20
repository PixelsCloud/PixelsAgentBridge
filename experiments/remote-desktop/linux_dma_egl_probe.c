/* One-frame DMA-BUF import/readback experiment. The fd is inherited from a
 * separate privileged exporter. No modeset, draw, input, or driver settings. */
#include <errno.h>
#include <fcntl.h>
#include <inttypes.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <unistd.h>
#include <gbm.h>
#include <drm_fourcc.h>
#include <epoxy/egl.h>
#include <epoxy/gl.h>

int main(int argc, char **argv) {
    if (argc != 9) return 2;
    uint64_t args[7];
    for (int i = 0; i < 7; ++i) {
        char *end = NULL; errno = 0;
        args[i] = strtoull(argv[i + 1], &end, 10);
        if (errno || !end || *end || argv[i + 1][0] == '-') return 2;
    }
    int dma = (int)args[0];
    unsigned w = (unsigned)args[1], h = (unsigned)args[2];
    unsigned format = (unsigned)args[3], pitch = (unsigned)args[4], offset = (unsigned)args[5];
    if (args[0] > 1048576 || args[1] > 8192 || args[2] > 8192 || !w || !h ||
        args[3] > UINT32_MAX || args[4] > INT32_MAX || args[5] > INT32_MAX ||
        (format != DRM_FORMAT_XRGB8888 && format != DRM_FORMAT_ARGB8888) ||
        pitch < w * 4 || (uint64_t)pitch * h + offset > 64 * 1024 * 1024 ||
        args[6] != 0 || argv[8][0] != '/') return 2;
    int result = 1, render = -1, output = -1;
    struct gbm_device *gbm = NULL;
    EGLDisplay display = EGL_NO_DISPLAY;
    EGLContext context = EGL_NO_CONTEXT;
    EGLImageKHR image = EGL_NO_IMAGE_KHR;
    GLuint texture = 0, framebuffer = 0;
    unsigned char *pixels = NULL;
    const char *stage = "render_node";
    render = open("/dev/dri/renderD128", O_RDWR | O_CLOEXEC);
    if (render < 0) goto done;
    gbm = gbm_create_device(render);
    if (!gbm) goto done;
    stage = "egl_display";
    display = eglGetPlatformDisplayEXT(EGL_PLATFORM_GBM_KHR, gbm, NULL);
    EGLint major, minor;
    if (display == EGL_NO_DISPLAY || !eglInitialize(display, &major, &minor)) goto done;
    stage = "egl_bind_api";
    if (!eglBindAPI(EGL_OPENGL_ES_API)) goto done;
    EGLConfig config; EGLint count;
    const EGLint config_attrs[] = {EGL_RENDERABLE_TYPE, EGL_OPENGL_ES2_BIT,
                                   EGL_SURFACE_TYPE, EGL_WINDOW_BIT, EGL_NONE};
    stage = "egl_config";
    if (!eglChooseConfig(display, config_attrs, &config, 1, &count) || !count) goto done;
    const EGLint context_attrs[] = {EGL_CONTEXT_CLIENT_VERSION, 2, EGL_NONE};
    stage = "egl_context";
    context = eglCreateContext(display, config, EGL_NO_CONTEXT, context_attrs);
    if (context == EGL_NO_CONTEXT) goto done;
    stage = "egl_make_current";
    if (!eglMakeCurrent(display, EGL_NO_SURFACE, EGL_NO_SURFACE, context)) goto done;
    stage = "dma_import";
    const EGLint image_attrs[] = {EGL_WIDTH, (EGLint)w, EGL_HEIGHT, (EGLint)h,
        EGL_LINUX_DRM_FOURCC_EXT, (EGLint)format, EGL_DMA_BUF_PLANE0_FD_EXT, dma,
        EGL_DMA_BUF_PLANE0_OFFSET_EXT, (EGLint)offset,
        EGL_DMA_BUF_PLANE0_PITCH_EXT, (EGLint)pitch, EGL_NONE};
    /* No explicit modifier: this probe accepts only the current linear/implicit
     * VM scanout. An absent FB_MODIFIERS flag is not proof of linear storage. */
    image = eglCreateImageKHR(display, EGL_NO_CONTEXT, EGL_LINUX_DMA_BUF_EXT,
                              NULL, image_attrs);
    if (image == EGL_NO_IMAGE_KHR) goto done;
    stage = "framebuffer";
    glGenTextures(1, &texture); glBindTexture(GL_TEXTURE_2D, texture);
    glEGLImageTargetTexture2DOES(GL_TEXTURE_2D, image);
    if (glGetError() != GL_NO_ERROR) goto done;
    glGenFramebuffers(1, &framebuffer); glBindFramebuffer(GL_FRAMEBUFFER, framebuffer);
    glFramebufferTexture2D(GL_FRAMEBUFFER, GL_COLOR_ATTACHMENT0, GL_TEXTURE_2D, texture, 0);
    if (glCheckFramebufferStatus(GL_FRAMEBUFFER) != GL_FRAMEBUFFER_COMPLETE) goto done;
    stage = "read_pixels";
    pixels = malloc((size_t)w * h * 4);
    if (!pixels) goto done;
    glReadPixels(0, 0, (GLsizei)w, (GLsizei)h, GL_RGBA, GL_UNSIGNED_BYTE, pixels);
    if (glGetError() != GL_NO_ERROR) goto done;
    stage = "write_ppm";
    output = open(argv[8], O_WRONLY | O_CREAT | O_EXCL | O_CLOEXEC | O_NOFOLLOW, 0600);
    if (output < 0) goto done;
    FILE *file = fdopen(output, "wb");
    if (!file) goto done;
    output = -1;
    fprintf(file, "P6\n%u %u\n255\n", w, h);
    uint64_t nonzero = 0, hash = UINT64_C(14695981039346656037);
    for (size_t i = 0; i < (size_t)w * h; ++i) {
        for (unsigned c = 0; c < 3; ++c) {
            unsigned char value = pixels[i * 4 + c];
            nonzero += value != 0; hash = (hash ^ value) * UINT64_C(1099511628211);
        }
        if (fwrite(pixels + i * 4, 3, 1, file) != 1) { fclose(file); goto done; }
    }
    if (fclose(file)) goto done;
    printf("{\"width\":%u,\"height\":%u,\"nonzero_channels\":%" PRIu64
           ",\"rgb_fnv1a\":\"%016" PRIx64 "\",\"egl_major\":%d,\"egl_minor\":%d}\n",
           w, h, nonzero, hash, major, minor);
    result = 0;
done:
    if (result) fprintf(stderr, "EGL probe failed at %s; EGL=0x%x errno=%d\n", stage, eglGetError(), errno);
    if (framebuffer) glDeleteFramebuffers(1, &framebuffer);
    if (texture) glDeleteTextures(1, &texture);
    if (image != EGL_NO_IMAGE_KHR) eglDestroyImageKHR(display, image);
    if (context != EGL_NO_CONTEXT) {
        eglMakeCurrent(display, EGL_NO_SURFACE, EGL_NO_SURFACE, EGL_NO_CONTEXT);
        eglDestroyContext(display, context);
    }
    if (display != EGL_NO_DISPLAY) eglTerminate(display);
    if (gbm) gbm_device_destroy(gbm);
    if (render >= 0) close(render);
    if (output >= 0) close(output);
    free(pixels);
    return result;
}
