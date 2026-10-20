#pragma once

namespace pab_linux {
inline const char* video_vertex() { return R"(#version 150
out vec2 uv;
uniform int rotation;
uniform vec2 visible_fraction;
void main() {
    vec2 p[4] = vec2[4](vec2(-1,-1),vec2(1,-1),vec2(-1,1),vec2(1,1));
    gl_Position = vec4(p[gl_VertexID],0,1);
    vec2 t = vec2((p[gl_VertexID].x+1)*.5,(1-p[gl_VertexID].y)*.5);
    if (rotation == 1) t = vec2(t.y,1-t.x);
    else if (rotation == 2) t = vec2(1-t.x,1-t.y);
    else if (rotation == 3) t = vec2(1-t.y,t.x);
    uv = t * visible_fraction;
})"; }
inline const char* video_fragment() { return R"(#version 150
in vec2 uv;
out vec4 color;
uniform sampler2D plane_y;
uniform sampler2D plane_uv;
uniform sampler2D cursor_shape;
uniform vec4 cursor_geometry;
uniform uvec4 cursor_image;
uniform float drawable_height;
void main() {
    float y = (texture(plane_y,uv).r*255.0-16.0)/219.0;
    vec2 c = (texture(plane_uv,uv).rg*255.0-vec2(128.0))/224.0;
    vec3 base = clamp(vec3(y+1.5748*c.y,y-.187324*c.x-.468124*c.y,y+1.8556*c.x),0.0,1.0);
    if (cursor_image.w != 0u) {
        vec2 position=floor((vec2(gl_FragCoord.x,drawable_height-gl_FragCoord.y)-cursor_geometry.xy)*cursor_geometry.zw);
        if (all(greaterThanEqual(position,vec2(0))) && all(lessThan(position,vec2(cursor_image.xy)))) {
            ivec2 p=ivec2(position);
            if (cursor_image.z==1u) {
                vec4 pixel=texelFetch(cursor_shape,p,0).bgra;
                base=pixel.rgb+base*(1.0-pixel.a);
            } else {
                uvec3 mask=uvec3(round(texelFetch(cursor_shape,ivec2(p.x*2,p.y),0).bgr*255.0));
                uvec3 value=uvec3(round(texelFetch(cursor_shape,ivec2(p.x*2+1,p.y),0).bgr*255.0));
                base=vec3((uvec3(round(base*255.0)) & mask) ^ value)/255.0;
            }
        }
    }
    color=vec4(base,1);
})"; }
}
