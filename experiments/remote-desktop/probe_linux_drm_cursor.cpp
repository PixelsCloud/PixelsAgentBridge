// Read-only KMS inventory on an independently opened DRM file. No master,
// modeset, compositor-fd change, framebuffer mapping, or pointer injection.
#include <xf86drm.h>
#include <xf86drmMode.h>
#include <fcntl.h>
#include <unistd.h>
#include <cstdio>
#include <cerrno>
#include <cstring>
#include <string>
#include <set>

#ifndef DRM_CLIENT_CAP_CURSOR_PLANE_HOTSPOT
#define DRM_CLIENT_CAP_CURSOR_PLANE_HOTSPOT 6
#endif

static void scan(const char* path, bool hotspot) {
    const int fd = open(path, O_RDONLY | O_CLOEXEC | O_NOFOLLOW);
    if (fd < 0) { printf("{\"open_errno\":%d}",errno);return; }
    const auto universal = drmSetClientCap(fd, DRM_CLIENT_CAP_UNIVERSAL_PLANES, 1);
    const auto atomic = drmSetClientCap(fd, DRM_CLIENT_CAP_ATOMIC, 1);
    errno=0;
    const auto cap = hotspot ? drmSetClientCap(fd, DRM_CLIENT_CAP_CURSOR_PLANE_HOTSPOT, 1) : 0;
    const int cap_error=errno;
    printf("{\"universal_result\":%d,\"atomic_result\":%d,\"hotspot_requested\":%s,\"hotspot_result\":%d,\"hotspot_errno\":%d,\"planes\":[",
        universal,atomic,hotspot?"true":"false",cap,cap_error);
    drmModePlaneRes* planes=drmModeGetPlaneResources(fd);
    if (planes && planes->count_planes <= 128) for(uint32_t i=0;i<planes->count_planes;++i) {
        if(i)printf(",");
        auto* p=drmModeGetPlane(fd,planes->planes[i]);
        if(!p) { printf("{\"plane_errno\":%d}",errno);continue; }
        printf("{\"id\":%u,\"crtc_id\":%u,\"fb_id\":%u,\"possible_crtcs\":%u,\"properties\":{",
            p->plane_id,p->crtc_id,p->fb_id,p->possible_crtcs);
        auto* props=drmModeObjectGetProperties(fd,p->plane_id,DRM_MODE_OBJECT_PLANE);
        bool comma=false, cursor=false;
        if(props && props->count_props<=128) for(uint32_t j=0;j<props->count_props;++j) {
            auto* prop=drmModeGetProperty(fd,props->props[j]);if(!prop)continue;
            const std::string name(prop->name,strnlen(prop->name,sizeof(prop->name)));
            if(name.find_first_not_of("abcdefghijklmnopqrstuvwxyzABCDEFGHIJKLMNOPQRSTUVWXYZ0123456789_ -")==std::string::npos) {
                if(comma)printf(",");
                comma=true;
                printf("\"%s\":%llu",name.c_str(),static_cast<unsigned long long>(props->prop_values[j]));
            }
            if(name=="type")cursor=props->prop_values[j]==DRM_PLANE_TYPE_CURSOR;
            drmModeFreeProperty(prop);
        }
        if(props)drmModeFreeObjectProperties(props);
        printf("}");
        if(cursor && p->fb_id) {
            auto* fb=drmModeGetFB2(fd,p->fb_id);
            if(!fb)printf(",\"getfb2_errno\":%d",errno);
            else {
                printf(",\"framebuffer\":{\"width\":%u,\"height\":%u,\"format\":%u,\"pitch\":%u,\"offset\":%u,\"flags\":%u,\"modifier\":%llu,\"handle_available\":%s}",
                    fb->width,fb->height,fb->pixel_format,fb->pitches[0],fb->offsets[0],fb->flags,
                    static_cast<unsigned long long>(fb->modifier),fb->handles[0]?"true":"false");
                std::set<uint32_t> handles;
                for(auto handle:fb->handles)if(handle)handles.insert(handle);
                for(auto handle:handles) { drm_gem_close value{handle,0};if(drmIoctl(fd,DRM_IOCTL_GEM_CLOSE,&value))fprintf(stderr,"GEM close failed\n"); }
                drmModeFreeFB2(fb);
            }
        }
        printf("}");drmModeFreePlane(p);
    }
    printf("]}");if(planes)drmModeFreePlaneResources(planes);close(fd);
}
int main(int argc,char** argv) {
    if(argc!=2)return 2;
    const std::string path=argv[1],prefix="/dev/dri/card";
    if(path.rfind(prefix,0)!=0 || path.size()==prefix.size() ||
        path.substr(prefix.size()).find_first_not_of("0123456789")!=std::string::npos)return 2;
    printf("{\"scans\":[");scan(argv[1],false);printf(",");scan(argv[1],true);printf("]}\n");
}
