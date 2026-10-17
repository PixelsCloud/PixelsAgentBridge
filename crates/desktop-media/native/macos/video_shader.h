#pragma once
#import <Foundation/Foundation.h>

namespace pab_mac {
inline NSString* video_shader() {
    return @R"(
            #include <metal_stdlib>
            using namespace metal;
            struct Vertex { float4 position [[position]]; float2 uv; };
            vertex Vertex video_vertex(uint id [[vertex_id]], constant uint& rotation [[buffer(0)]], constant float2& visible_fraction [[buffer(1)]]) {
                const float2 positions[4] = {float2(-1,-1), float2(1,-1), float2(-1,1), float2(1,1)};
                float2 uv = float2((id & 1) ? 1 : 0, (id & 2) ? 0 : 1);
                if (rotation == 1) uv = float2(uv.y, 1-uv.x);
                else if (rotation == 2) uv = 1-uv;
                else if (rotation == 3) uv = float2(1-uv.y, uv.x);
                return {float4(positions[id],0,1), uv * visible_fraction};
            }
            struct Cursor { float2 origin; float2 inverseScale; uint2 dimensions; uint format; uint visible; };
            fragment float4 video_fragment(Vertex in [[stage_in]], texture2d<float> y [[texture(0)]], texture2d<float> uv [[texture(1)]],
                texture2d<float> shape [[texture(2)]], constant Cursor& cursor [[buffer(0)]]) {
                constexpr sampler filter(coord::normalized, address::clamp_to_edge, filter::linear);
                float luma = (y.sample(filter,in.uv).r*255-16)/219;
                float2 chroma = (uv.sample(filter,in.uv).rg*255-128)/224;
                float3 base = clamp(float3(luma+1.5748*chroma.y, luma-0.187324*chroma.x-0.468124*chroma.y,
                    luma+1.8556*chroma.x),0.0,1.0);
                if (cursor.visible) {
                    float2 position = floor((in.position.xy-cursor.origin)*cursor.inverseScale);
                    if (all(position >= 0) && all(position < float2(cursor.dimensions))) {
                        uint2 p = uint2(position);
                        if (cursor.format == 1) {
                            float4 color = shape.read(p).bgra;
                            base = color.rgb + base*(1-color.a);
                        } else {
                            uint3 mask = uint3(round(shape.read(uint2(p.x*2,p.y)).bgr*255));
                            uint3 value = uint3(round(shape.read(uint2(p.x*2+1,p.y)).bgr*255));
                            base = float3((uint3(round(base*255)) & mask) ^ value)/255;
                        }
                    }
                }
                return float4(base,1);
            }
        )";
}
}
