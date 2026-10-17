#pragma once
#include "dma_frame.h"
#include <xf86drm.h>
#include <xf86drmMode.h>
#include <optional>

// Linux 6.6 added this UAPI; older libdrm headers can still target that kernel.
#ifndef DRM_CLIENT_CAP_CURSOR_PLANE_HOTSPOT
#define DRM_CLIENT_CAP_CURSOR_PLANE_HOTSPOT 6
#endif

namespace pab_linux::dma {
template<class T, void(*release)(T*)> struct DrmDrop { void operator()(T* p) const { if (p) release(p); } };
using Resources = std::unique_ptr<drmModeRes, DrmDrop<drmModeRes, drmModeFreeResources>>;
using Connector = std::unique_ptr<drmModeConnector, DrmDrop<drmModeConnector, drmModeFreeConnector>>;
using Crtc = std::unique_ptr<drmModeCrtc, DrmDrop<drmModeCrtc, drmModeFreeCrtc>>;
using EncoderInfo = std::unique_ptr<drmModeEncoder, DrmDrop<drmModeEncoder, drmModeFreeEncoder>>;
using Planes = std::unique_ptr<drmModePlaneRes, DrmDrop<drmModePlaneRes, drmModeFreePlaneResources>>;
using Plane = std::unique_ptr<drmModePlane, DrmDrop<drmModePlane, drmModeFreePlane>>;
using Props = std::unique_ptr<drmModeObjectProperties, DrmDrop<drmModeObjectProperties, drmModeFreeObjectProperties>>;
using Prop = std::unique_ptr<drmModePropertyRes, DrmDrop<drmModePropertyRes, drmModeFreeProperty>>;
struct Refresh : std::runtime_error { using std::runtime_error::runtime_error; };
struct CursorScanout {
    uint32_t screen_width = 0, screen_height = 0, crtc = 0, connector = 0;
    int32_t left = 0, top = 0;
    uint32_t hotspot_x = 0, hotspot_y = 0;
    std::optional<Frame> image;
};
inline int32_t cursor_coordinate(uint64_t value) {
    if (value > uint64_t(INT32_MAX) && value < UINT64_MAX - uint64_t(INT32_MAX))
        throw std::runtime_error("DRM cursor coordinate outside bounds");
    return value <= uint64_t(INT32_MAX) ? int32_t(value) : -1 - int32_t(UINT64_MAX - value);
}

inline uint64_t property(int fd, const Props& properties, const char* name, uint64_t fallback) {
    if (!properties || properties->count_props > 128) throw std::runtime_error("Cannot read DRM properties");
    for (uint32_t i = 0; i < properties->count_props; ++i) {
        Prop p(drmModeGetProperty(fd, properties->props[i]));
        if (!p) throw std::runtime_error("DRM property disappeared");
        if (strcmp(p->name, name) == 0) return properties->prop_values[i];
    }
    return fallback;
}
class Framebuffer {
    int fd_;
    drmModeFB2* fb_;
    bool retire() noexcept {
        if (!fb_) return true;
        std::array<uint32_t, 4> handles{}; unsigned count = 0;
        for (auto handle : fb_->handles)
            if (handle && std::find(handles.begin(), handles.begin() + count, handle) == handles.begin() + count)
                handles[count++] = handle;
        bool closed = true;
        for (unsigned i = 0; i < count; ++i) {
            drm_gem_close handle{handles[i], 0};
            if (drmIoctl(fd_, DRM_IOCTL_GEM_CLOSE, &handle)) closed = false;
        }
        drmModeFreeFB2(fb_); fb_ = nullptr;
        return closed;
    }
public:
    Framebuffer(int fd, uint32_t id) : fd_(fd), fb_(drmModeGetFB2(fd, id)) {
        if (!fb_) {
            if (errno == ENOENT || errno == EAGAIN) throw Refresh("DRM framebuffer changed");
            throw std::runtime_error("DRM framebuffer is unavailable");
        }
    }
    Framebuffer(const Framebuffer&) = delete;
    Framebuffer& operator=(const Framebuffer&) = delete;
    ~Framebuffer() {
        // GETFB2 returns fresh GEM handles, including on a borrowed compositor
        // file. Never close a handle observed anywhere except this response.
        if (!retire()) fprintf(stderr, "media: failed to retire new DRM GEM handles\n");
    }
    const drmModeFB2& get() const { return *fb_; }
    void finish() {
        if (!retire()) throw std::runtime_error("Cannot retire new DRM GEM handles");
    }
};
class Exporter {
    int card_, render_;
    Fd observer_;
    std::string connector_;
    Connector selected() const {
        const auto fd = observer_.get();
        Resources resources(drmModeGetResources(fd));
        if (!resources || resources->count_connectors < 1 || resources->count_connectors > 64)
            throw std::runtime_error("DRM connector inventory unavailable");
        Connector result;
        for (int i = 0; i < resources->count_connectors; ++i) {
            Connector c(drmModeGetConnector(fd, resources->connectors[i]));
            if (!c) throw std::runtime_error("DRM connector changed");
            const char* type = drmModeGetConnectorTypeName(c->connector_type);
            if (!type || std::string(type) + "-" + std::to_string(c->connector_type_id) != connector_) continue;
            if (result) throw std::runtime_error("Ambiguous DRM connector");
            result = std::move(c);
        }
        if (!result || result->connection != DRM_MODE_CONNECTED || !result->encoder_id)
            throw std::runtime_error("Selected DRM connector is unavailable");
        Props props(drmModeObjectGetProperties(fd, result->connector_id, DRM_MODE_OBJECT_CONNECTOR));
        if (property(fd, props, "DPMS", 0) != DRM_MODE_DPMS_ON)
            throw std::runtime_error("Selected display is asleep");
        return result;
    }
    uint32_t full_scanout(uint32_t crtc, uint32_t width, uint32_t height) const {
        const auto fd = observer_.get();
        Planes planes(drmModeGetPlaneResources(fd));
        if (!planes || planes->count_planes > 128) throw std::runtime_error("DRM plane inventory unavailable");
        unsigned matches = 0; uint32_t framebuffer = 0;
        for (uint32_t i = 0; i < planes->count_planes; ++i) {
            Plane plane(drmModeGetPlane(fd, planes->planes[i]));
            if (!plane || plane->crtc_id != crtc) continue;
            Props props(drmModeObjectGetProperties(fd, plane->plane_id, DRM_MODE_OBJECT_PLANE));
            if (property(fd, props, "type", UINT64_MAX) != DRM_PLANE_TYPE_PRIMARY) continue;
            ++matches;
            framebuffer = plane->fb_id;
            // Reject unimplemented crop/scale/rotation instead of showing the
            // wrong monitor or accepting incorrect input coordinates.
            if (!framebuffer || property(fd, props, "rotation", DRM_MODE_ROTATE_0) != DRM_MODE_ROTATE_0 ||
                property(fd, props, "SRC_X", UINT64_MAX) || property(fd, props, "SRC_Y", UINT64_MAX) ||
                property(fd, props, "SRC_W", 0) != uint64_t(width) << 16 ||
                property(fd, props, "SRC_H", 0) != uint64_t(height) << 16 ||
                property(fd, props, "CRTC_X", UINT64_MAX) || property(fd, props, "CRTC_Y", UINT64_MAX) ||
                property(fd, props, "CRTC_W", 0) != width || property(fd, props, "CRTC_H", 0) != height)
                throw std::runtime_error("DRM primary plane needs crop, transform or refresh");
        }
        if (matches != 1) throw std::runtime_error("DRM primary plane is unavailable");
        return framebuffer;
    }
public:
    Exporter(int card, int render, std::string connector, bool borrowed)
        : card_(card), render_(render), connector_(std::move(connector)) {
        if (geteuid() != 0 || card_ < 3 || render_ < 3 || connector_.empty() || connector_.size() > 120)
            throw std::runtime_error("DRM exporter requires the privileged session supervisor");
        struct stat a{}, b{};
        if (fstat(card_, &a) || fstat(render_, &b) || !S_ISCHR(a.st_mode) || !S_ISCHR(b.st_mode))
            throw std::runtime_error("Invalid DRM devices");
        drmDevicePtr device = nullptr;
        if (drmGetDevice2(card_, 0, &device) || !device) throw std::runtime_error("Cannot identify DRM device");
        struct DeviceDrop { drmDevicePtr* p; ~DeviceDrop() { drmFreeDevice(p); } } drop{&device};
        struct stat expected{};
        if (!(device->available_nodes & (1 << DRM_NODE_RENDER)) ||
            stat(device->nodes[DRM_NODE_RENDER], &expected) || b.st_rdev != expected.st_rdev)
            throw std::runtime_error("DRM render node belongs to another GPU");
        if (!(device->available_nodes & (1 << DRM_NODE_PRIMARY))) throw std::runtime_error("DRM primary node unavailable");
        observer_ = Fd(open(device->nodes[DRM_NODE_PRIMARY], O_RDONLY | O_CLOEXEC | O_NOFOLLOW));
        struct stat observed{};
        if (observer_.get() < 0 || fstat(observer_.get(), &observed) || observed.st_rdev != a.st_rdev ||
            drmSetClientCap(observer_.get(), DRM_CLIENT_CAP_UNIVERSAL_PLANES, 1) ||
            drmSetClientCap(observer_.get(), DRM_CLIENT_CAP_ATOMIC, 1))
            throw std::runtime_error("Cannot observe DRM atomic plane properties");
        // Para-virtualized drivers hide their cursor planes from atomic clients
        // until this capability is set. Never change the borrowed compositor fd.
        // EOPNOTSUPP on physical drivers is normal; inspect actual planes below.
        (void)drmSetClientCap(observer_.get(), DRM_CLIENT_CAP_CURSOR_PLANE_HOTSPOT, 1);
        if (borrowed) {
            drmVersionPtr version = drmGetVersion(card_);
            const bool vmware = version && version->name_len == 6 && !memcmp(version->name, "vmwgfx", 6);
            if (version) drmFreeVersion(version);
            if (!vmware) throw std::runtime_error("Borrowed descriptors are only supported for vmwgfx");
            // No client capability changes on the compositor's DRM file.
        }
    }
private:
    Frame snapshot(uint64_t sequence) const {
        auto connector = selected();
        EncoderInfo encoder(drmModeGetEncoder(observer_.get(), connector->encoder_id));
        if (!encoder || !encoder->crtc_id) throw std::runtime_error("DRM output is disabled");
        Crtc crtc(drmModeGetCrtc(observer_.get(), encoder->crtc_id));
        if (!crtc || !crtc->mode_valid) throw std::runtime_error("DRM scanout is disabled");
        // Atomic compositors update the primary plane. The legacy CRTC buffer
        // field can remain on an older framebuffer even while scanout changes.
        const auto framebuffer = full_scanout(crtc->crtc_id, crtc->width, crtc->height);
        Framebuffer owned(card_, framebuffer);
        const auto& fb = owned.get();
        if (!fb.handles[0] || fb.handles[1] || fb.handles[2] || fb.handles[3] ||
            crtc->width != fb.width || crtc->height != fb.height)
            throw std::runtime_error("Unsupported DRM scanout layout");
        Frame frame;
        auto& m = frame.info; m.sequence = sequence; m.width = fb.width; m.height = fb.height;
        m.format = fb.pixel_format; m.stride = fb.pitches[0]; m.offset = fb.offsets[0];
        m.modifier = fb.modifier; m.flags = fb.flags; m.crtc = crtc->crtc_id;
        m.framebuffer = fb.fb_id; m.connector = connector->connector_id;
        validate(m, sequence);
        int exported = -1;
        if (drmPrimeHandleToFD(card_, fb.handles[0], DRM_CLOEXEC, &exported))
            throw std::runtime_error("DRM scanout export failed");
        frame.image = Fd(exported);
        frame.render = Fd(fcntl(render_, F_DUPFD_CLOEXEC, 3));
        if (frame.render.get() < 0) throw std::runtime_error("Cannot retain DRM render node");
        // Recheck the output before sending; a hotplug/modeset cannot reuse a
        // descriptor from an earlier CRTC or connector generation.
        auto current = selected();
        if (current->connector_id != m.connector || current->encoder_id != connector->encoder_id)
            throw std::runtime_error("DRM output changed during export");
        owned.finish();
        return frame;
    }
public:
    CursorScanout cursor(uint64_t sequence) const {
        const auto fd = observer_.get();
        auto connector = selected();
        EncoderInfo encoder(drmModeGetEncoder(fd, connector->encoder_id));
        if (!encoder || !encoder->crtc_id) throw std::runtime_error("DRM cursor output is disabled");
        Crtc crtc(drmModeGetCrtc(fd, encoder->crtc_id));
        Resources resources(drmModeGetResources(fd));
        if (!crtc || !crtc->mode_valid || !resources || resources->count_crtcs < 1 || resources->count_crtcs > 32)
            throw std::runtime_error("DRM cursor CRTC is unavailable");
        int index = 0;
        while (index < resources->count_crtcs && resources->crtcs[index] != crtc->crtc_id) ++index;
        if (index == resources->count_crtcs) throw Refresh("DRM cursor CRTC changed");
        CursorScanout result;
        result.screen_width = crtc->width; result.screen_height = crtc->height;
        if (crtc->width < 2 || crtc->height < 2 || crtc->width > 8192 || crtc->height > 8192)
            throw std::runtime_error("DRM cursor screen dimensions outside bounds");
        result.crtc = crtc->crtc_id; result.connector = connector->connector_id;
        Planes planes(drmModeGetPlaneResources(fd));
        if (!planes || planes->count_planes > 128) throw std::runtime_error("DRM cursor plane inventory unavailable");
        bool capable = false; Plane selected_plane;
        Props selected_props;
        for (uint32_t i = 0; i < planes->count_planes; ++i) {
            Plane plane(drmModeGetPlane(fd, planes->planes[i]));
            if (!plane) throw Refresh("DRM cursor plane changed");
            Props props(drmModeObjectGetProperties(fd, plane->plane_id, DRM_MODE_OBJECT_PLANE));
            if (property(fd, props, "type", UINT64_MAX) != DRM_PLANE_TYPE_CURSOR) continue;
            capable |= (plane->possible_crtcs & (uint32_t(1) << index)) != 0;
            if (property(fd, props, "CRTC_ID", UINT64_MAX) != crtc->crtc_id) continue;
            if (selected_plane) throw std::runtime_error("Multiple DRM cursor planes target this output");
            selected_plane = std::move(plane); selected_props = std::move(props);
        }
        if (!capable) throw std::runtime_error("DRM cursor plane is not observable on this output");
        if (!selected_plane) return result; // A known cursor plane is currently unbound.
        const auto framebuffer = property(fd, selected_props, "FB_ID", UINT64_MAX);
        if (!framebuffer) return result;
        if (framebuffer > UINT32_MAX) throw std::runtime_error("Invalid DRM cursor framebuffer");
        Framebuffer owned(card_, uint32_t(framebuffer)); const auto& fb = owned.get();
        if (!fb.handles[0] || fb.handles[1] || fb.handles[2] || fb.handles[3] ||
            !fb.width || !fb.height || fb.width > 512 || fb.height > 512 ||
            (fb.pixel_format != DRM_FORMAT_ARGB8888 && fb.pixel_format != DRM_FORMAT_ABGR8888))
            throw std::runtime_error("Unsupported DRM cursor framebuffer layout");
        if (property(fd, selected_props, "SRC_X", UINT64_MAX) || property(fd, selected_props, "SRC_Y", UINT64_MAX) ||
            property(fd, selected_props, "SRC_W", 0) != uint64_t(fb.width) << 16 ||
            property(fd, selected_props, "SRC_H", 0) != uint64_t(fb.height) << 16 ||
            property(fd, selected_props, "CRTC_W", 0) != fb.width || property(fd, selected_props, "CRTC_H", 0) != fb.height ||
            property(fd, selected_props, "rotation", DRM_MODE_ROTATE_0) != DRM_MODE_ROTATE_0 ||
            property(fd, selected_props, "alpha", 65535) != 65535 ||
            property(fd, selected_props, "pixel blend mode", 0) != 0) // DRM_MODE_BLEND_PREMULTI
            throw std::runtime_error("DRM cursor needs crop, scale, rotation or blending adapter");
        // CRTC_X/Y are signed coordinates represented in a u64 property value.
        result.left = cursor_coordinate(property(fd, selected_props, "CRTC_X", uint64_t(1) << 63));
        result.top = cursor_coordinate(property(fd, selected_props, "CRTC_Y", uint64_t(1) << 63));
        const auto hx = property(fd, selected_props, "HOTSPOT_X", 0), hy = property(fd, selected_props, "HOTSPOT_Y", 0);
        if (hx >= fb.width || hy >= fb.height) throw std::runtime_error("DRM cursor hotspot outside image");
        result.hotspot_x = uint32_t(hx); result.hotspot_y = uint32_t(hy);
        Frame frame; auto& m = frame.info;
        m.sequence = sequence; m.width = fb.width; m.height = fb.height; m.format = fb.pixel_format;
        m.stride = fb.pitches[0]; m.offset = fb.offsets[0]; m.modifier = fb.modifier; m.flags = fb.flags;
        m.crtc = crtc->crtc_id; m.framebuffer = fb.fb_id; m.connector = connector->connector_id;
        validate_cursor_image(m, sequence);
        int exported = -1;
        if (drmPrimeHandleToFD(card_, fb.handles[0], DRM_CLOEXEC, &exported))
            throw std::runtime_error("DRM cursor export failed");
        frame.image = Fd(exported); frame.render = Fd(fcntl(render_, F_DUPFD_CLOEXEC, 3));
        if (frame.render.get() < 0) throw std::runtime_error("Cannot retain cursor render node");
        auto current = selected();
        if (current->connector_id != connector->connector_id || current->encoder_id != connector->encoder_id)
            throw Refresh("DRM cursor output changed during export");
        Props now(drmModeObjectGetProperties(fd, selected_plane->plane_id, DRM_MODE_OBJECT_PLANE));
        for (const auto* name : {"FB_ID", "CRTC_ID", "CRTC_X", "CRTC_Y", "CRTC_W", "CRTC_H", "SRC_X", "SRC_Y", "SRC_W", "SRC_H", "HOTSPOT_X", "HOTSPOT_Y"})
            if (property(fd, now, name, UINT64_MAX) != property(fd, selected_props, name, UINT64_MAX))
                throw Refresh("DRM cursor changed during export");
        owned.finish(); result.image.emplace(std::move(frame)); return result;
    }
    Frame next(uint64_t sequence) const {
        // A modeset/page flip can retire an FB between plane and GETFB2. Each
        // failed attempt releases its own handles; nothing was sent downstream.
        for (unsigned attempt = 0;; ++attempt) {
            try { return snapshot(sequence); }
            catch (const Refresh&) {
                if (attempt == 2) throw;
                std::this_thread::sleep_for(std::chrono::milliseconds(4));
            }
        }
    }
};
inline void export_loop(int card, int render, int socket, const std::string& connector, bool borrowed) {
    channel(socket); Exporter exporter(card, render, connector, borrowed);
    uint64_t sequence = 0;
    while (request(socket, sequence)) {
        auto frame = exporter.next(sequence); send_frame(socket, frame); ++sequence;
    }
}
}
