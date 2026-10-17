/* Isolated read-only framebuffer experiment. No DRM master, modeset, write
 * mapping, borrowed compositor descriptors, or input. Not a production backend. */
#include <errno.h>
#include <fcntl.h>
#include <inttypes.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <sys/mman.h>
#include <sys/stat.h>
#include <unistd.h>
#include <drm_fourcc.h>
#include <xf86drm.h>
#include <xf86drmMode.h>

int main(int argc, char **argv) {
    if (argc != 4 || strncmp(argv[1], "/dev/dri/card", 13) || argv[3][0] != '/') return 2;
    char *end = NULL; unsigned long plane_id = strtoul(argv[2], &end, 10);
    if (!plane_id || !end || *end || plane_id > UINT32_MAX) return 2;
    int status = 1, output = -1, fd = open(argv[1], O_RDONLY | O_CLOEXEC);
    drmModePlanePtr plane = NULL; drmModeFB2Ptr fb = NULL;
    void *data = MAP_FAILED; size_t bytes = 0;
    if (fd < 0) goto done;
    if (drmSetClientCap(fd, DRM_CLIENT_CAP_UNIVERSAL_PLANES, 1)) goto done;
    plane = drmModeGetPlane(fd, (uint32_t)plane_id);
    if (!plane || !plane->fb_id || !plane->crtc_id) goto done;
    fb = drmModeGetFB2(fd, plane->fb_id);
    if (!fb || !fb->handles[0] || fb->handles[1] || fb->handles[2] || fb->handles[3]
        || fb->pixel_format != DRM_FORMAT_XRGB8888 || fb->modifier != DRM_FORMAT_MOD_LINEAR
        || !fb->width || !fb->height || fb->width > 8192 || fb->height > 8192
        || fb->pitches[0] < fb->width * 4) { errno = ENOTSUP; goto done; }
    uint64_t extent = (uint64_t)fb->offsets[0] + (uint64_t)fb->pitches[0] * fb->height;
    if (extent > 64 * 1024 * 1024) { errno = EOVERFLOW; goto done; }
    bytes = (size_t)extent;
    struct drm_mode_map_dumb mapping = { .handle = fb->handles[0] };
    if (drmIoctl(fd, DRM_IOCTL_MODE_MAP_DUMB, &mapping)) goto done;
    data = mmap(NULL, bytes, PROT_READ, MAP_SHARED, fd, (off_t)mapping.offset);
    if (data == MAP_FAILED) goto done;
    output = open(argv[3], O_WRONLY | O_CREAT | O_EXCL | O_CLOEXEC | O_NOFOLLOW, 0600);
    if (output < 0) goto done;
    FILE *file = fdopen(output, "wb");
    if (!file) goto done;
    output = -1;
    fprintf(file, "P6\n%u %u\n255\n", fb->width, fb->height);
    unsigned char *row = malloc((size_t)fb->width * 3);
    if (!row) { fclose(file); goto done; }
    uint64_t nonzero = 0, hash = UINT64_C(14695981039346656037);
    for (uint32_t y = 0; y < fb->height; ++y) {
        const unsigned char *source = (const unsigned char *)data + fb->offsets[0] + (size_t)y * fb->pitches[0];
        for (uint32_t x = 0; x < fb->width; ++x) {
            for (unsigned c = 0; c < 3; ++c) {
                unsigned char value = source[x * 4 + (2 - c)];
                row[x * 3 + c] = value; nonzero += value != 0;
                hash = (hash ^ value) * UINT64_C(1099511628211);
            }
        }
        if (fwrite(row, 3, fb->width, file) != fb->width) { free(row); fclose(file); goto done; }
    }
    free(row);
    if (fclose(file)) goto done;
    printf("{\"width\":%u,\"height\":%u,\"nonzero_channels\":%" PRIu64 ",\"rgb_fnv1a\":\"%016" PRIx64 "\",\"mapped_read_only\":true,\"drm_master\":%s}\n",
           fb->width, fb->height, nonzero, hash, drmIsMaster(fd) ? "true" : "false");
    status = 0;
done:
    if (status) fprintf(stderr, "mapped probe failed: %s\n", strerror(errno));
    if (output >= 0) close(output);
    if (data != MAP_FAILED) munmap(data, bytes);
    if (fb) {
        for (unsigned i = 0; i < 4; ++i) {
            if (!fb->handles[i]) continue;
            unsigned j = 0; for (; j < i; ++j) if (fb->handles[j] == fb->handles[i]) break;
            if (j == i) { struct drm_gem_close handle = { .handle = fb->handles[i] }; drmIoctl(fd, DRM_IOCTL_GEM_CLOSE, &handle); }
        }
        drmModeFreeFB2(fb);
    }
    if (plane) drmModeFreePlane(plane);
    if (fd >= 0) close(fd);
    return status;
}
