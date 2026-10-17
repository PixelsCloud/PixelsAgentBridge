/* Read-only KMS/PRIME capability probe. No modeset, DRM master request, frame
 * mapping, screenshot, input injection, or persistent driver configuration. */
#include <errno.h>
#include <fcntl.h>
#include <inttypes.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <unistd.h>
#include <xf86drm.h>
#include <xf86drmMode.h>
#include <vmwgfx_drm.h>

/* A zero-clip VMW readback performs no frame transfer. It checks whether this
 * independent fd can use the driver fallback, without creating a framebuffer,
 * borrowing the compositor's fd, becoming master, or changing display state. */
static void inspect_vmware_readback(int fd, unsigned card) {
    drmVersionPtr version = drmGetVersion(fd);
    if (!version) return;
    const int vmware = version->name_len == 6 && !memcmp(version->name, "vmwgfx", 6);
    drmFreeVersion(version);
    if (!vmware) return;
    struct drm_vmw_present_readback_arg request = {0};
    errno = 0;
    int result = drmIoctl(fd, DRM_IOW(DRM_COMMAND_BASE + DRM_VMW_PRESENT_READBACK,
                                    struct drm_vmw_present_readback_arg), &request);
    const int saved = result ? errno : 0;
    printf("{\"card\":%u,\"vmwgfx_readback_zero_clips\":true,\"drm_master\":%s,\"readback_errno\":%d}\n",
           card, drmIsMaster(fd) ? "true" : "false", saved);
}

static void inspect_framebuffer(int fd, unsigned card, unsigned plane, unsigned crtc, unsigned fb_id) {
    errno = 0;
    drmModeFB2Ptr fb = drmModeGetFB2(fd, fb_id);
    if (!fb) {
        printf("{\"card\":%u,\"plane\":%u,\"crtc\":%u,\"fb\":%u,\"fb2_errno\":%d}\n",
               card, plane, crtc, fb_id, errno);
        return;
    }
    printf("{\"card\":%u,\"plane\":%u,\"crtc\":%u,\"fb\":%u,\"width\":%u,\"height\":%u,\"format\":%u,\"modifier\":%" PRIu64 ",\"buffers\":[",
           card, plane, crtc, fb_id, fb->width, fb->height, fb->pixel_format, fb->modifier);
    unsigned emitted = 0;
    for (unsigned i = 0; i < 4; ++i) {
        if (!fb->handles[i]) continue;
        int dma = -1;
        errno = 0;
        int result = drmPrimeHandleToFD(fd, fb->handles[i], DRM_CLOEXEC, &dma);
        int saved_errno = result ? errno : 0;
        off_t size = dma >= 0 ? lseek(dma, 0, SEEK_END) : -1;
        /* Offset lookup only: still no mmap or pixels read by this inventory. */
        struct drm_mode_map_dumb mapping = { .handle = fb->handles[i] };
        errno = 0;
        int map_result = drmIoctl(fd, DRM_IOCTL_MODE_MAP_DUMB, &mapping);
        const int map_errno = map_result ? errno : 0;
        printf("%s{\"index\":%u,\"pitch\":%u,\"offset\":%u,\"exported\":%s,\"errno\":%d,\"bytes\":%jd,\"map_dumb_errno\":%d,\"map_dumb_offset\":%" PRIu64 "}",
               emitted++ ? "," : "", i, fb->pitches[i], fb->offsets[i],
               result == 0 ? "true" : "false", saved_errno, (intmax_t)size, map_errno, (uint64_t)mapping.offset);
        if (dma >= 0) close(dma);
    }
    puts("]}");
    // GETFB2 returns handles owned by this DRM file; planes can share one.
    for (unsigned i = 0; i < 4; ++i) {
        if (!fb->handles[i]) continue;
        unsigned j = 0;
        for (; j < i; ++j) if (fb->handles[j] == fb->handles[i]) break;
        if (j == i) {
            struct drm_gem_close handle = { .handle = fb->handles[i] };
            if (drmIoctl(fd, DRM_IOCTL_GEM_CLOSE, &handle))
                fprintf(stderr, "close GEM handle failed: %d\n", errno);
        }
    }
    drmModeFreeFB2(fb);
}

int main(void) {
    unsigned opened = 0;
    for (unsigned card = 0; card < 16; ++card) {
        char path[32];
        snprintf(path, sizeof(path), "/dev/dri/card%u", card);
        int fd = open(path, O_RDONLY | O_CLOEXEC);
        if (fd < 0) {
            if (errno != ENOENT) printf("{\"card\":%u,\"open_errno\":%d}\n", card, errno);
            continue;
        }
        ++opened;
        inspect_vmware_readback(fd, card);
        uint64_t prime = 0;
        int prime_ok = drmGetCap(fd, DRM_CAP_PRIME, &prime) == 0;
        printf("{\"card\":%u,\"prime_cap_available\":%s,\"prime_cap\":%" PRIu64 "}\n", card, prime_ok ? "true" : "false", prime);
        drmModeResPtr resources = drmModeGetResources(fd);
        if (resources) {
            for (int i = 0; i < resources->count_crtcs; ++i) {
                drmModeCrtcPtr crtc = drmModeGetCrtc(fd, resources->crtcs[i]);
                if (!crtc) continue;
                if (crtc->mode_valid) printf("{\"card\":%u,\"crtc\":%u,\"x\":%u,\"y\":%u,\"width\":%u,\"height\":%u,\"fb\":%u}\n",
                    card, crtc->crtc_id, crtc->x, crtc->y, crtc->width, crtc->height, crtc->buffer_id);
                drmModeFreeCrtc(crtc);
            }
            drmModeFreeResources(resources);
        }
        // This changes enumeration on our own fd, not display state.
        drmSetClientCap(fd, DRM_CLIENT_CAP_UNIVERSAL_PLANES, 1);
        drmModePlaneResPtr planes = drmModeGetPlaneResources(fd);
        if (planes) {
            for (uint32_t i = 0; i < planes->count_planes; ++i) {
                drmModePlanePtr plane = drmModeGetPlane(fd, planes->planes[i]);
                if (!plane) continue;
                if (plane->crtc_id && plane->fb_id)
                    inspect_framebuffer(fd, card, plane->plane_id, plane->crtc_id, plane->fb_id);
                drmModeFreePlane(plane);
            }
            drmModeFreePlaneResources(planes);
        } else printf("{\"card\":%u,\"planes_errno\":%d}\n", card, errno);
        close(fd);
    }
    return opened ? 0 : 1;
}
