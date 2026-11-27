//! The headset's picture drawn by us ("victory-vr-scene"): a scene of our
//! own submitted to SteamVR's compositor for each eye at the headset's rate,
//! from the last frame the title presented and, for depth, its depth buffer
//! ([`super::Snaps`]).
//!
//! What a frame of ours is made of, back to front:
//!
//! - **The surroundings**: the title's frame carried on past its edges in
//!   every direction, its own picture mirrored there and blurred, and
//!   further from them the frame's mean colour: no dark around the picture
//!   and no hard edge to it.
//! - **The title's frame**, one of three ways ([`super::Mode`]):
//!   - **3d**: a relief around the player, each texel of the title's depth
//!     buffer put back where the title's camera saw it, shown so large that
//!     it fills the view. The picture's border fades out.
//!   - **cinema**: a flat screen standing in the room.
//!   - **immersive**: the relief fixed to the head (the depth of two eyes,
//!     and the head's turning goes to the title's camera).
//! - **What the player holds and wears**: at each controller the player's
//!   hand ([`super::glove`], its fingers bent by grip and trigger) or the
//!   controller as SteamVR draws it (its own render model, loaded from
//!   SteamVR); a marker at each tracker worn on the body.
//!
//! **Where the relief stands.** The title's camera is taken to stand at the
//! player's eyes, and to stay at them while the head only turns: a head
//! turns on the neck, some 9 cm behind and 7 cm under the eyes, which moves
//! the eyes sideways past everything near, and what was behind a near thing
//! would come into view, which the title's one camera never saw. So the
//! camera goes with the eyes round the neck, and stays behind only by what
//! the neck itself moves (leaning, stepping; `VICTORY_VR_PARALLAX`, 1 all of
//! it, 0 none: the relief then has the depth of two eyes only).
//!
//! **What the camera did not see.** Each eye still sees a little behind the
//! edge of a near thing, and a leaning head more. The relief is one sheet (a
//! grid of 480 by 270 cells, a depth read at each corner), whole over such
//! an edge; what to draw on the sheet where it is pulled wide is decided
//! for each pixel: from how the depth goes across its triangle, whether the
//! triangle lies over an edge between near and far; from where this eye
//! sees the near side and the far side of it, how many cells the near side
//! has moved off the far one. Then the near side's own texels are drawn not
//! stretched, up to the texel where the depth buffer says the near thing
//! ends, and the rest, which the camera did not see, takes the far side
//! carried on: the far side's own picture mirrored at the edge along the way
//! the near side moved, so that the background's texture goes on at its own
//! scale instead of one colour smeared; where the mirror would bring in
//! something near again, the far edge's colour. A triangle drawn as the
//! camera saw it keeps its own texels. Nothing is believed to be nearer
//! than `VICTORY_VR_NEAREST` metres (0.3): closer than that two eyes cannot
//! be brought together on it.
//!
//! **Past the frame's edges** the sheet goes on by `VICTORY_VR_SKIRT` of the
//! frame (0.15), at the edge's own depth and with the frame mirrored: a
//! head that leans or turns a little sees the scene go on at the scene's
//! depth, not the frame's edge. Only the sheet's own border fades out, into
//! the surroundings.
//!
//! The view follows the head lazily (`VICTORY_VR_FOLLOW=0`: not): turned
//! further than 25 degrees from the middle of the picture, or stepped
//! further than 40 cm from where the title's camera stands, the picture
//! comes after, so that the player never looks past it.
//!
//! The title's projection is not known here, and its field of view changes
//! from shot to shot: the frame is shown `VICTORY_VR_FOV` degrees high (not
//! said: a little higher than the headset's own view, so that no edge of it
//! is in sight, and just as high where it is fixed to the head; F8 smaller,
//! F9 larger), its depth taken as reversed with an infinite far plane at
//! `VICTORY_VR_NEAR` metres (0.1), and the whole brought nearer or put
//! further off by the world scale (Ctrl+F8 nearer and smaller, with more
//! depth between the eyes; Ctrl+F9 further and larger).
//!
//! Nothing here waits for the title's queue, nor the title's queue for ours:
//! a snapshot is used once the title's queue has passed its copy (a fence
//! looked at on the CPU), and the title skips writing into one we may still
//! be reading.

use std::collections::HashMap;
use std::ffi::{CString, c_char};
use std::time::{Duration, Instant};

use windows::Win32::Foundation::HANDLE;
use windows::Win32::Graphics::Direct3D::D3D_PRIMITIVE_TOPOLOGY_TRIANGLELIST;
use windows::Win32::Graphics::Direct3D12::*;
use windows::Win32::Graphics::Dxgi::Common::*;
use windows::Win32::System::Threading::{CreateEventW, WaitForSingleObject};
use windows::core::{Interface, s};

use super::glove::{self, Glove, Vertex};
use super::math::{IDENTITY, M34, V3, add, axis, cross, from_axes, length, level_forward, mul, place, point, scale, sub, turn, unit};
use super::{Api, BodyPart, Held, Mode, TEXTURE_DIRECTX12, Texture, TextureData, Views, compositor, models, system};
use crate::host::{Host, transition};

/// `TrackedDevicePose_t`, as in `track`.
#[repr(C)]
#[derive(Clone, Copy)]
struct Pose {
    at: M34,
    velocity: V3,
    angular: V3,
    result: i32,
    valid: bool,
    connected: bool,
}

/// `HmdMatrix34_t` returned by value.
#[repr(C)]
#[derive(Clone, Copy)]
struct Matrix34 {
    m: M34,
}

/// `RenderModel_t` (Windows: the natural layout).
#[repr(C)]
struct RenderModel {
    vertices: *const Vertex,
    vertex_count: u32,
    indices: *const u16,
    triangle_count: u32,
    texture: i32,
}

/// `RenderModel_TextureMap_t`: `width` by `height` texels of four bytes.
#[repr(C)]
struct TextureMap {
    width: u16,
    height: u16,
    data: *const u8,
    format: i32,
    mips: u16,
}

const _: () = assert!(size_of::<RenderModel>() == 32 && size_of::<TextureMap>() == 24 && size_of::<Vertex>() == 32);

type WaitGetPoses = unsafe extern "system" fn(*mut Pose, u32, *mut Pose, u32) -> i32;
type Submit = unsafe extern "system" fn(i32, *const Texture, *const std::ffi::c_void, i32) -> i32;
type RecommendedSize = unsafe extern "system" fn(*mut u32, *mut u32);
type ProjectionRaw = unsafe extern "system" fn(i32, *mut f32, *mut f32, *mut f32, *mut f32);
type EyeToHead = unsafe extern "system" fn(i32) -> Matrix34;
type LoadModel = unsafe extern "system" fn(*const c_char, *mut *mut RenderModel) -> i32;
type FreeModel = unsafe extern "system" fn(*mut RenderModel);
type LoadTexture = unsafe extern "system" fn(i32, *mut *mut TextureMap) -> i32;
type FreeTexture = unsafe extern "system" fn(*mut TextureMap);

/// `EVRRenderModelError`: not there yet, asked for again the next frame.
const LOADING: i32 = 100;
const COLOR_SPACE_GAMMA: i32 = 1;
const EYE_FORMAT: DXGI_FORMAT = DXGI_FORMAT_R8G8B8A8_UNORM;
const DEPTH_FORMAT: DXGI_FORMAT = DXGI_FORMAT_D32_FLOAT;
/// The grid the relief is made of, cells across and down
/// (`VICTORY_VR_GRID=<cells across>`, 480): each cell is two triangles for
/// each eye, and triangles of a pixel or two cost the card far more than
/// the pixels they cover.
/// The frame itself takes that many cells, and the sheet goes on past each
/// of its edges by [`skirt`] of it.
fn grid_size() -> (u32, u32) {
    let across = super::number("VICTORY_VR_GRID", 480.0).clamp(96.0, 1280.0);
    let whole = 1.0 + 2.0 * skirt();
    (((across * whole) as u32) & !1, (across * 9.0 / 16.0 * whole) as u32)
}

/// How far past each edge of the title's frame the relief goes on, as a
/// part of the frame (`VICTORY_VR_SKIRT`, 0.15): there the edge's own depth
/// carried outward and the frame's picture mirrored, so that a head that
/// leans or turns a little sees the scene go on, at the scene's depth, and
/// not the frame's edge.
fn skirt() -> f32 {
    super::number("VICTORY_VR_SKIRT", 0.15).clamp(0.0, 0.5)
}

/// The card's clock is read at so many places of each frame of ours: its
/// start, after the small blurred frame, after the left eye's
/// surroundings, relief and meshes, and at its end.
const STAMPS: usize = 6;
/// The title's frame blurred small, for the surroundings.
const AMBIENT: (u32, u32) = (96, 54);
/// How near our own eyes see, metres.
const EYE_NEAR: f32 = 0.03;
/// Frames of ours in flight.
const FRAMES: usize = 2;
/// Descriptors of a frame: the colour, the depth as float, as uint, the
/// blurred frame; behind those of all frames, the models' textures.
const VIEWS_A_FRAME: usize = 4;
const MODEL_VIEWS: usize = 8;

const SCENE: &str = r#"
#define RS "RootConstants(num32BitConstants=48, b0), DescriptorTable(SRV(t0, numDescriptors=4)), StaticSampler(s0, filter=FILTER_MIN_MAG_MIP_LINEAR, addressU=TEXTURE_ADDRESS_CLAMP, addressV=TEXTURE_ADDRESS_CLAMP)"
cbuffer C : register(b0) {
    float4 vp0, vp1, vp2, vp3;   // the eye's view and projection, rows
    float4 w0, w1, w2;           // the title's camera in the room, rows of 3x4 (the surroundings: the eye's turn into the camera's frame)
    float4 p0;                   // tan x, tan y, near, scale
    float4 p1;                   // grid w, grid h, edge ratio, flat (1) or relief (0)
    float4 p2;                   // flat distance, flat width, depth kind, farthest
    float4 p3;                   // the eye's tangents: left, right, top, bottom
    float4 p4;                   // border fade, surroundings' brightness, blur step x, y (the relief: the nearest depth believed, how far the sheet goes past the frame)
};
Texture2D<float4> colour : register(t0);
Texture2D<float> depth_float : register(t1);
Texture2D<uint> depth_uint : register(t2);
Texture2D<float4> ambient : register(t3);
SamplerState linear_clamp : register(s0);

// uv: the vertex's texel. z: how far from the title's camera, its metres.
struct V { float4 pos : SV_Position; float2 uv : TEXCOORD0; float z : TEXCOORD1; };
struct F { float4 pos : SV_Position; float2 ndc : TEXCOORD0; };

float view_z(float2 uv) {
    uint w, h;
    depth_float.GetDimensions(w, h);
    int3 t = int3(min(uint2(saturate(uv) * float2(w, h)), uint2(w - 1, h - 1)), 0);
    float d;
    if (p2.z > 1.5 && p2.z < 2.5) d = float(depth_uint.Load(t) & 0xFFFFFFu) / 16777215.0;
    else d = depth_float.Load(t);
    return clamp(p0.z / max(d, 1e-7), p4.z, p2.w);
}

float4 placed(float3 local) {
    float4 l = float4(local, 1.0);
    float4 world = float4(dot(w0, l), dot(w1, l), dot(w2, l), 1.0);
    return float4(dot(vp0, world), dot(vp1, world), dot(vp2, world), dot(vp3, world));
}

// Where the title's camera saw the texel at `uv`, `z` metres away.
float3 seen(float2 uv, float z) {
    float2 ndc = float2(uv.x * 2.0 - 1.0, 1.0 - uv.y * 2.0);
    return float3(ndc.x * p0.x * z, ndc.y * p0.y * z, -z) * p0.w;
}

// The same, in this eye's picture.
float2 on_eye(float2 uv, float z) {
    float4 c = placed(seen(uv, z));
    return c.xy / max(c.w, 1e-5);
}

// The grid's corners, a row of them one more than its cells across.
[RootSignature(RS)]
V vs(uint id : SV_VertexID) {
    uint across = (uint)p1.x + 1;
    // Past the frame's edges by p4.w of it: the texel is then outside 0..1
    // (its depth is the edge's, its colour the frame's mirrored).
    float2 uv = float2(id % across, id / across) / p1.xy * (1.0 + 2.0 * p4.w) - p4.w;
    V o;
    o.uv = uv;
    o.z = 1.0;
    if (p1.w > 0.5) {
        float width = p2.y;
        o.pos = placed(float3((uv.x - 0.5) * width, (0.5 - uv.y) * width * p0.y / p0.x, -p2.x));
        return o;
    }
    o.z = view_z(uv);
    o.pos = placed(seen(uv, o.z));
    return o;
}

float4 ps(V v) : SV_Target {
    float span = 1.0 + 2.0 * p4.w;
    float2 size = p1.xy / span, cell = span / p1.xy;
    // How the texel and the depth go from pixel to pixel, in cells.
    float2 ux = ddx(v.uv) * size, uy = ddy(v.uv) * size;
    float zx = ddx(v.z), zy = ddy(v.z);
    float2 uv = v.uv;
    float det = ux.x * uy.y - ux.y * uy.x;
    if (p1.w < 0.5 && abs(det) > 1e-12) {
        // The depth's slope over this triangle, metres a cell across and
        // down (exact: depth and texel are spread over it alike), and from
        // it the depth at the triangle's three corners: the cell's upper
        // left half or its lower right one.
        float2 slope = float2(zx * uy.y - ux.y * zy, ux.x * zy - uy.x * zx) / det;
        float2 f = frac((v.uv + p4.w) * size);
        bool upper = f.x + f.y < 1.0;
        float zc = upper ? v.z - slope.x * f.x - slope.y * f.y : v.z + slope.x * (1.0 - f.x) + slope.y * (1.0 - f.y);
        float za = upper ? zc + slope.x : zc - slope.x, zb = upper ? zc + slope.y : zc - slope.y;
        float lo = max(min(zc, min(za, zb)), 1e-3), hi = max(zc, max(za, zb));
        // A triangle over an edge between near and far.
        if (hi > lo * p1.z) {
            // This eye's picture of the far side here, a cell across and a
            // cell down; and how far on it the near side has moved, and
            // this pixel's own place on the sheet, in cells.
            float2 o0 = on_eye(v.uv, hi);
            float2 ex = on_eye(v.uv + float2(cell.x, 0.0), hi) - o0, ey = on_eye(v.uv + float2(0.0, cell.y), hi) - o0;
            float d2 = ex.x * ey.y - ex.y * ey.x;
            if (abs(d2) > 1e-16) {
                float2 m_near = on_eye(v.uv, lo) - o0, m_here = on_eye(v.uv, v.z) - o0;
                float2 g = float2(m_near.x * ey.y - m_near.y * ey.x, ex.x * m_near.y - ex.y * m_near.x) / d2;
                float2 g_here = float2(m_here.x * ey.y - m_here.y * ey.x, ex.x * m_here.y - ex.y * m_here.x) / d2;
                float len = length(g), here = length(g_here);
                // Opened: the near side (against the slope) has moved away
                // from the far one, and not along the edge between them.
                float apart = dot(g, -slope) / max(length(slope) * len, 1e-12);
                float much = saturate(len) * saturate(apart * 4.0);
                if (much > 0.0) {
                    float2 far_way = -g / max(len, 1e-6);
                    float s = saturate((hi - v.z) / max(hi - lo, 1e-6));   // 0 at the far side, 1 at the near
                    float middle = lo * hi;
                    // The near thing's own texel for this pixel (the sheet
                    // is pulled wide here, the near thing is not), as far
                    // as the near thing goes.
                    float2 own = v.uv + far_way * max(len - here, 0.0) * cell;
                    float z_own = view_z(own);
                    if (z_own * z_own < middle) {
                        uv = own;
                    } else {
                        // What the camera did not see. Behind this pixel is
                        // the far side's texel `here` cells under the near
                        // thing: mirrored at the edge, it is as far past
                        // the edge on the far side. Where that is something
                        // near again, the far edge's colour.
                        float2 past = v.uv + far_way * (here + 2.0 * s) * cell;
                        float z_past = view_z(past);
                        if (z_past * z_past < middle) past = v.uv + far_way * (s + 1.5) * cell;
                        uv = lerp(v.uv, past, much);
                    }
                }
            }
        }
    }
    // Past the frame's edges, the frame mirrored.
    float3 c = colour.SampleLevel(linear_clamp, 1.0 - abs(1.0 - abs(uv)), 0).rgb;
    float a = 1.0;
    if (p4.x > 0.0) {
        // The sheet's own border fades out.
        float2 on_sheet = (v.uv + p4.w) / span;
        float2 e = min(on_sheet, 1.0 - on_sheet);
        a = saturate(e.x / p4.x) * saturate(e.y / (p4.x * p0.x / p0.y));
        a = a * a * (3.0 - 2.0 * a);
    }
    return float4(c, a);
}

[RootSignature(RS)]
F vs_full(uint id : SV_VertexID) {
    float2 p = float2((id << 1) & 2, id & 2);
    F o;
    o.ndc = p * 2.0 - 1.0;
    o.pos = float4(o.ndc, 0.0, 1.0);
    return o;
}

// The title's frame, small: each texel the mean of an 8x8 spread of taps.
float4 ps_blur(F f) : SV_Target {
    float2 uv = float2(f.ndc.x * 0.5 + 0.5, 0.5 - f.ndc.y * 0.5);
    float3 sum = float3(0.0, 0.0, 0.0);
    for (int y = 0; y < 8; y++)
        for (int x = 0; x < 8; x++)
            sum += colour.SampleLevel(linear_clamp, uv + (float2(x, y) - 3.5) * p4.zw, 0).rgb;
    return float4(sum / 64.0, 1.0);
}

// The surroundings: the title's frame carried on past its edges, its own
// picture mirrored there and the softer the further from the edge, and far
// from the frame the frame's mean colour. Nothing around the picture is
// dark unless the picture is.
float4 ps_dome(F f) : SV_Target {
    float3 e = float3(((p3.y - p3.x) * f.ndc.x + (p3.y + p3.x)) * 0.5, ((p3.w - p3.z) * f.ndc.y + (p3.w + p3.z)) * 0.5, -1.0);
    float3 r = normalize(float3(dot(w0.xyz, e), dot(w1.xyz, e), dot(w2.xyz, e)));
    float ahead = max(-r.z, 0.05);
    float2 q = float2(r.x / ahead / p0.x, r.y / ahead / p0.y);
    float outside = length(max(abs(q) - 1.0, 0.0));
    // Well inside the frame the picture itself is drawn over this.
    if (-r.z > 0.05 && max(abs(q.x), abs(q.y)) < 0.8)
        return float4(ambient.SampleLevel(linear_clamp, float2(q.x * 0.5 + 0.5, 0.5 - q.y * 0.5), 0).rgb * p4.y, 1.0);
    // Mirrored at the frame's edges, once.
    float2 m = min(abs(q), 3.0);
    m = sign(q) * (m > 1.0 ? 2.0 - m : m);
    float2 uv = clamp(float2(m.x * 0.5 + 0.5, 0.5 - m.y * 0.5), 0.03, 0.97);
    float2 spread = float2(0.02, 0.035) * (1.0 + outside * 6.0);
    float3 c = ambient.SampleLevel(linear_clamp, uv, 0).rgb * 0.2;
    c += ambient.SampleLevel(linear_clamp, uv + float2(spread.x, 0.0), 0).rgb * 0.1;
    c += ambient.SampleLevel(linear_clamp, uv - float2(spread.x, 0.0), 0).rgb * 0.1;
    c += ambient.SampleLevel(linear_clamp, uv + float2(0.0, spread.y), 0).rgb * 0.1;
    c += ambient.SampleLevel(linear_clamp, uv - float2(0.0, spread.y), 0).rgb * 0.1;
    c += ambient.SampleLevel(linear_clamp, uv + spread, 0).rgb * 0.1;
    c += ambient.SampleLevel(linear_clamp, uv - spread, 0).rgb * 0.1;
    c += ambient.SampleLevel(linear_clamp, uv + float2(spread.x, -spread.y), 0).rgb * 0.1;
    c += ambient.SampleLevel(linear_clamp, uv + float2(-spread.x, spread.y), 0).rgb * 0.1;
    float far_off = saturate(outside * 0.5 - 0.15);
    if (far_off > 0.0) {
        float3 mean = float3(0.0, 0.0, 0.0);
        for (int j = 0; j < 3; j++)
            for (int i = 0; i < 5; i++)
                mean += ambient.SampleLevel(linear_clamp, float2((i + 0.5) / 5.0, (j + 0.5) / 3.0), 0).rgb;
        c = lerp(c, mean / 15.0, far_off);
    }
    // As bright as the picture at its edge, less so away from it and behind.
    float away = saturate(r.z * 1.5 + 0.4);
    float dim = p4.y * lerp(1.0, 0.6, saturate(outside * 0.5)) * (1.0 - away * 0.6);
    return float4(c * dim, 1.0);
}
"#;

const MESH: &str = r#"
#define RS "RootFlags(ALLOW_INPUT_ASSEMBLER_INPUT_LAYOUT), RootConstants(num32BitConstants=36, b0), DescriptorTable(SRV(t0, numDescriptors=1)), StaticSampler(s0, filter=FILTER_MIN_MAG_MIP_LINEAR)"
cbuffer C : register(b0) {
    float4 vp0, vp1, vp2, vp3;   // the eye's view and projection, rows
    float4 m0, m1, m2;           // the model in the room, rows of 3x4
    float4 tint;                 // its colour; a: 1 with a texture
    float4 light;                // towards the light, in the room
};
Texture2D<float4> skin : register(t0);
SamplerState linear_wrap : register(s0);

struct I { float3 pos : POSITION; float3 normal : NORMAL; float2 uv : TEXCOORD0; };
struct V { float4 pos : SV_Position; float3 normal : NORMAL; float2 uv : TEXCOORD0; };

[RootSignature(RS)]
V vs(I i) {
    float4 l = float4(i.pos, 1.0);
    float4 world = float4(dot(m0, l), dot(m1, l), dot(m2, l), 1.0);
    V o;
    o.pos = float4(dot(vp0, world), dot(vp1, world), dot(vp2, world), dot(vp3, world));
    o.normal = float3(dot(m0.xyz, i.normal), dot(m1.xyz, i.normal), dot(m2.xyz, i.normal));
    o.uv = i.uv;
    return o;
}

float4 ps(V v) : SV_Target {
    float3 base = tint.rgb;
    if (tint.a > 0.5) base *= skin.Sample(linear_wrap, v.uv).rgb;
    float d = dot(normalize(v.normal), light.xyz);
    return float4(base * (0.40 + 0.60 * (d * 0.5 + 0.5)), 1.0);
}
"#;

/// The scene's root constants, as its shaders have them.
#[repr(C)]
#[derive(Clone, Copy, Default)]
struct SceneConstants {
    viewproj: [[f32; 4]; 4],
    world: M34,
    p0: [f32; 4],
    p1: [f32; 4],
    p2: [f32; 4],
    p3: [f32; 4],
    p4: [f32; 4],
}

/// A model's root constants.
#[repr(C)]
#[derive(Clone, Copy, Default)]
struct MeshConstants {
    viewproj: [[f32; 4]; 4],
    model: M34,
    tint: [f32; 4],
    light: [f32; 4],
}

const _: () = assert!(size_of::<SceneConstants>() == 48 * 4 && size_of::<MeshConstants>() == 36 * 4);

/// A rigid transform's inverse.
fn inverse(m: &M34) -> M34 {
    let (x, y, z, at) = (axis(m, 0), axis(m, 1), axis(m, 2), place(m));
    let dot = |a: V3, b: V3| a[0] * b[0] + a[1] * b[1] + a[2] * b[2];
    [[x[0], x[1], x[2], -dot(x, at)], [y[0], y[1], y[2], -dot(y, at)], [z[0], z[1], z[2], -dot(z, at)]]
}

/// An eye's projection from its tangents (OpenVR's raw ones), with depth
/// reversed and no far plane: depth is `near / distance`.
fn projection(left: f32, right: f32, top: f32, bottom: f32, near: f32) -> [[f32; 4]; 4] {
    let (idx, idy) = (1.0 / (right - left), 1.0 / (bottom - top));
    [[2.0 * idx, 0.0, (right + left) * idx, 0.0], [0.0, 2.0 * idy, (bottom + top) * idy, 0.0], [0.0, 0.0, 0.0, near], [0.0, 0.0, -1.0, 0.0]]
}

/// `p * v`, `v` a 3x4 taken as 4x4.
fn times(p: &[[f32; 4]; 4], v: &M34) -> [[f32; 4]; 4] {
    let mut out = [[0.0; 4]; 4];
    for (i, row) in out.iter_mut().enumerate() {
        for (j, cell) in row.iter_mut().enumerate() {
            let w = if j == 3 { 1.0 } else { 0.0 };
            *cell = p[i][0] * v[0][j] + p[i][1] * v[1][j] + p[i][2] * v[2][j] + p[i][3] * w;
        }
    }
    out
}

/// A pose turned about +y only: where the head is, looking level.
fn level(head: &M34) -> M34 {
    let (forward, _) = level_forward(head);
    let up = [0.0, 1.0, 0.0];
    from_axes(cross(forward, up), up, scale(forward, -1.0), place(head))
}

/// `m` standing at `at`.
fn at_place(m: &M34, at: V3) -> M34 {
    from_axes(axis(m, 0), axis(m, 1), axis(m, 2), at)
}

/// From between the eyes to where the head turns on the neck, in the head's
/// own frame: under and behind them.
const NECK: V3 = [0.0, -0.075, 0.09];

/// A head as its neck: standing where the head turns.
fn neck_of(head: &M34) -> M34 {
    at_place(head, point(head, NECK))
}

/// The title's camera in the room for a view anchored at `anchor` (a neck's
/// place) and a head at `head`: at the eyes, less `parallax` of what the
/// neck itself has moved from the anchor.
fn camera_of(anchor: &M34, head: &M34, parallax: f32) -> M34 {
    at_place(anchor, sub(place(head), scale(sub(place(&neck_of(head)), place(anchor)), parallax)))
}

/// `m` with its axes stretched by `by` (a model's size).
fn sized(m: &M34, by: V3) -> M34 {
    from_axes(scale(axis(m, 0), by[0]), scale(axis(m, 1), by[1]), scale(axis(m, 2), by[2]), place(m))
}

/// The view's anchor brought after a head that has turned more than
/// `turn_free` radians from it or stepped more than `step_free` metres away.
fn follow(anchor: &M34, head: &M34, turn_free: f32, step_free: f32, dt: f32) -> M34 {
    let ((_, head_yaw), (_, anchor_yaw)) = (level_forward(head), level_forward(anchor));
    let mut by = head_yaw - anchor_yaw;
    if by > std::f32::consts::PI {
        by -= std::f32::consts::TAU;
    } else if by < -std::f32::consts::PI {
        by += std::f32::consts::TAU;
    }
    let rate = (dt * 4.0).min(1.0);
    let turn = (by.abs() - turn_free).max(0.0) * by.signum() * rate;
    let mut out = *anchor;
    if turn != 0.0 {
        // About the upright line through the head.
        let (c, s) = (turn.cos(), turn.sin());
        let round = from_axes([c, 0.0, -s], [0.0, 1.0, 0.0], [s, 0.0, c], [0.0; 3]);
        let at = place(head);
        let there = from_axes([1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0], at);
        let back = from_axes([1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0], scale(at, -1.0));
        out = mul(&there, &mul(&round, &mul(&back, &out)));
    }
    let off = sub(place(head), place(&out));
    let far = length(off);
    if far > step_free {
        let step = scale(off, (far - step_free) / far * rate);
        out = from_axes(axis(&out, 0), axis(&out, 1), axis(&out, 2), add(place(&out), step));
    }
    out
}

/// A ball of radius one: its vertices and indices.
fn ball() -> (Vec<Vertex>, Vec<u16>) {
    const AROUND: u16 = 14;
    const DOWN: u16 = 8;
    let mut vertices = Vec::new();
    for j in 0..=DOWN {
        let polar = std::f32::consts::PI * f32::from(j) / f32::from(DOWN);
        for i in 0..=AROUND {
            let turn = std::f32::consts::TAU * f32::from(i) / f32::from(AROUND);
            let n = [polar.sin() * turn.cos(), polar.cos(), polar.sin() * turn.sin()];
            vertices.push([n[0], n[1], n[2], n[0], n[1], n[2], 0.0, 0.0]);
        }
    }
    let mut indices = Vec::new();
    for j in 0..DOWN {
        for i in 0..AROUND {
            let a = j * (AROUND + 1) + i;
            let b = a + AROUND + 1;
            indices.extend_from_slice(&[a, b, a + 1, a + 1, b, b + 1]);
        }
    }
    (vertices, indices)
}

/// A static or rewritten buffer in upload memory, drawn from as it is.
struct Buffer {
    resource: ID3D12Resource,
    data: *mut u8,
    bytes: usize,
}

impl Buffer {
    fn new(host: &Host, bytes: usize) -> Option<Self> {
        let (resource, data) = host.upload_buffer(bytes.max(16) as u64)?;
        Some(Self { resource, data, bytes: bytes.max(16) })
    }

    fn holding<T: Copy>(host: &Host, items: &[T]) -> Option<Self> {
        let buffer = Self::new(host, size_of_val(items))?;
        // SAFETY: a mapping of at least that many bytes; plain data.
        unsafe { std::ptr::copy_nonoverlapping(items.as_ptr().cast::<u8>(), buffer.data, size_of_val(items)) };
        Some(buffer)
    }

    fn vertices(&self) -> D3D12_VERTEX_BUFFER_VIEW {
        // SAFETY: a live buffer.
        D3D12_VERTEX_BUFFER_VIEW { BufferLocation: unsafe { self.resource.GetGPUVirtualAddress() }, SizeInBytes: self.bytes as u32, StrideInBytes: size_of::<Vertex>() as u32 }
    }

    fn indices(&self) -> D3D12_INDEX_BUFFER_VIEW {
        // SAFETY: a live buffer.
        D3D12_INDEX_BUFFER_VIEW { BufferLocation: unsafe { self.resource.GetGPUVirtualAddress() }, SizeInBytes: self.bytes as u32, Format: DXGI_FORMAT_R16_UINT }
    }
}

/// The relief's grid: the corners of each cell's two triangles, a row of
/// corners one more than the cells across.
fn grid_indices(cells: (u32, u32)) -> Vec<u32> {
    let across = cells.0 + 1;
    let mut indices = Vec::with_capacity((cells.0 * cells.1 * 6) as usize);
    for y in 0..cells.1 {
        for x in 0..cells.0 {
            let (a, c) = (y * across + x, (y + 1) * across + x);
            indices.extend([a, a + 1, c, c, a + 1, c + 1]);
        }
    }
    indices
}

/// A buffer of `bytes` in the card's own memory, to be copied into.
fn card_buffer(host: &Host, bytes: usize) -> windows::core::Result<ID3D12Resource> {
    let heap = D3D12_HEAP_PROPERTIES { Type: D3D12_HEAP_TYPE_DEFAULT, ..Default::default() };
    let desc = D3D12_RESOURCE_DESC {
        Dimension: D3D12_RESOURCE_DIMENSION_BUFFER,
        Width: bytes as u64,
        Height: 1,
        DepthOrArraySize: 1,
        MipLevels: 1,
        SampleDesc: DXGI_SAMPLE_DESC { Count: 1, Quality: 0 },
        Layout: D3D12_TEXTURE_LAYOUT_ROW_MAJOR,
        ..Default::default()
    };
    let mut made: Option<ID3D12Resource> = None;
    // SAFETY: plain creation of a buffer of ours.
    unsafe { host.device.CreateCommittedResource(&heap, D3D12_HEAP_FLAG_NONE, &desc, D3D12_RESOURCE_STATE_COMMON, None, &mut made)? };
    made.ok_or_else(windows::core::Error::empty)
}

/// A model: its vertices, its indices and how many of those there are.
struct Mesh {
    vertices: Buffer,
    indices: Buffer,
    count: u32,
}

impl Mesh {
    fn new(host: &Host, vertices: &[Vertex], indices: &[u16]) -> Option<Self> {
        Some(Self { vertices: Buffer::holding(host, vertices)?, indices: Buffer::holding(host, indices)?, count: indices.len() as u32 })
    }
}

/// A hand: its model, and its vertices as last posed for each frame in
/// flight.
struct Hand {
    glove: Glove,
    indices: Buffer,
    posed: [Buffer; FRAMES],
    last: [Option<[i32; 3]>; FRAMES],
    mirrored: bool,
}

/// A controller's model as SteamVR has it.
enum Model {
    /// Asked for; SteamVR is still reading it.
    Loading,
    /// Its mesh, and its texture: asked for, ours with its place among the
    /// views, or none.
    Ready(Mesh, Skin),
    Missing,
}

enum Skin {
    Loading(i32),
    Ready(#[allow(dead_code)] ID3D12Resource, #[allow(dead_code)] Buffer, usize),
    None,
}

/// One thing to draw: which mesh, where, its colour, its texture's place
/// among the views.
struct Draw {
    vertices: D3D12_VERTEX_BUFFER_VIEW,
    indices: D3D12_INDEX_BUFFER_VIEW,
    count: u32,
    at: M34,
    tint: [f32; 4],
    view: Option<usize>,
}

struct Renderer {
    queue: ID3D12CommandQueue,
    allocators: [ID3D12CommandAllocator; FRAMES],
    list: ID3D12GraphicsCommandList,
    fence: ID3D12Fence,
    event: HANDLE,
    values: [u64; FRAMES],
    value: u64,
    scene_root: ID3D12RootSignature,
    relief: ID3D12PipelineState,
    dome: ID3D12PipelineState,
    blur: ID3D12PipelineState,
    mesh_root: ID3D12RootSignature,
    mesh: ID3D12PipelineState,
    views: ID3D12DescriptorHeap,
    view_step: usize,
    rtvs: ID3D12DescriptorHeap,
    rtv_step: usize,
    dsvs: ID3D12DescriptorHeap,
    eyes: [ID3D12Resource; 2],
    _depth: ID3D12Resource,
    ambient: ID3D12Resource,
    /// The grid's indices on the card, and where they come from until the
    /// first frame has copied them there.
    grid: ID3D12Resource,
    grid_from: Option<Buffer>,
    cells: (u32, u32),
    /// The card's clock read through each frame of ours, where the readings
    /// land, and how many of its ticks make a millisecond.
    clock: Option<(ID3D12QueryHeap, ID3D12Resource, *const u64, f64)>,
    size: (u32, u32),
    projections: [[[f32; 4]; 4]; 2],
    tangents: [[f32; 4]; 2],
    eye_to_head: [M34; 2],
    ball: Option<Mesh>,
    hands: [Option<Hand>; 2],
    models: HashMap<String, Model>,
    model_views: usize,
}

// SAFETY: free-threaded D3D12 objects, an event handle and mappings of our
// own buffers, used by the one thread that owns the renderer.
unsafe impl Send for Renderer {}

fn texture2d(host: &Host, size: (u32, u32), format: DXGI_FORMAT, flags: D3D12_RESOURCE_FLAGS, state: D3D12_RESOURCE_STATES, clear: Option<D3D12_CLEAR_VALUE>) -> windows::core::Result<ID3D12Resource> {
    let heap = D3D12_HEAP_PROPERTIES { Type: D3D12_HEAP_TYPE_DEFAULT, ..Default::default() };
    let desc = D3D12_RESOURCE_DESC {
        Dimension: D3D12_RESOURCE_DIMENSION_TEXTURE2D,
        Width: u64::from(size.0),
        Height: size.1,
        DepthOrArraySize: 1,
        MipLevels: 1,
        Format: format,
        SampleDesc: DXGI_SAMPLE_DESC { Count: 1, Quality: 0 },
        Flags: flags,
        ..Default::default()
    };
    let mut made: Option<ID3D12Resource> = None;
    // SAFETY: plain creation of a texture of ours.
    unsafe { host.device.CreateCommittedResource(&heap, D3D12_HEAP_FLAG_NONE, &desc, state, clear.as_ref().map(std::ptr::from_ref), &mut made)? };
    made.ok_or_else(windows::core::Error::empty)
}

fn srv(format: DXGI_FORMAT) -> D3D12_SHADER_RESOURCE_VIEW_DESC {
    D3D12_SHADER_RESOURCE_VIEW_DESC {
        Format: format,
        ViewDimension: D3D12_SRV_DIMENSION_TEXTURE2D,
        Shader4ComponentMapping: D3D12_DEFAULT_SHADER_4_COMPONENT_MAPPING,
        Anonymous: D3D12_SHADER_RESOURCE_VIEW_DESC_0 { Texture2D: D3D12_TEX2D_SRV { MipLevels: 1, ..Default::default() } },
    }
}

impl Renderer {
    fn new(host: &Host, api: &Api) -> windows::core::Result<Self> {
        let device = &host.device;
        let (mut w, mut h) = (0u32, 0u32);
        // SAFETY: the tables' entries by their types; out values ours.
        let (projections, tangents, eye_to_head) = unsafe {
            api.system::<RecommendedSize>(system::RECOMMENDED_SIZE)(&mut w, &mut h);
            let mut projections = [[[0.0; 4]; 4]; 2];
            let mut tangents = [[0.0; 4]; 2];
            let mut eye_to_head = [IDENTITY; 2];
            for eye in 0..2 {
                let (mut l, mut r, mut t, mut b) = (0.0, 0.0, 0.0, 0.0);
                api.system::<ProjectionRaw>(system::PROJECTION_RAW)(eye as i32, &mut l, &mut r, &mut t, &mut b);
                projections[eye] = projection(l, r, t, b, EYE_NEAR);
                tangents[eye] = [l, r, t, b];
                eye_to_head[eye] = api.system::<EyeToHead>(system::EYE_TO_HEAD)(eye as i32).m;
            }
            (projections, tangents, eye_to_head)
        };
        // The frame is shown as high as the headset's own view, and a little
        // over: no edge of it is in sight while looking ahead.
        let up = tangents.iter().flat_map(|t| [t[2].abs(), t[3].abs()]).fold(0.0f32, f32::max);
        super::view_is(up);
        let most = super::number("VICTORY_VR_EYE_MOST", 2400.0).clamp(512.0, 4096.0) as u32;
        let shrink = (w.max(h) as f32 / most as f32).max(1.0);
        let size = (((w as f32 / shrink) as u32).max(256), ((h as f32 / shrink) as u32).max(256));
        let code = |source, entry, target| crate::host::compile(source, entry, target);
        let scene = [code(SCENE, s!("vs"), s!("vs_5_1"))?, code(SCENE, s!("ps"), s!("ps_5_1"))?, code(SCENE, s!("vs_full"), s!("vs_5_1"))?, code(SCENE, s!("ps_dome"), s!("ps_5_1"))?, code(SCENE, s!("ps_blur"), s!("ps_5_1"))?];
        let mesh = [code(MESH, s!("vs"), s!("vs_5_1"))?, code(MESH, s!("ps"), s!("ps_5_1"))?];
        // SAFETY: plain object creation; the blobs and the layout live
        // through the calls.
        unsafe {
            let queue: ID3D12CommandQueue = device.CreateCommandQueue(&D3D12_COMMAND_QUEUE_DESC {
                Type: D3D12_COMMAND_LIST_TYPE_DIRECT,
                Priority: D3D12_COMMAND_QUEUE_PRIORITY_HIGH.0,
                ..Default::default()
            })?;
            let allocators = [device.CreateCommandAllocator(D3D12_COMMAND_LIST_TYPE_DIRECT)?, device.CreateCommandAllocator(D3D12_COMMAND_LIST_TYPE_DIRECT)?];
            let list: ID3D12GraphicsCommandList = device.CreateCommandList(0, D3D12_COMMAND_LIST_TYPE_DIRECT, &allocators[0], None)?;
            list.Close()?;
            let scene_root: ID3D12RootSignature = device.CreateRootSignature(0, &scene[0])?;
            let mesh_root: ID3D12RootSignature = device.CreateRootSignature(0, &mesh[0])?;
            let bytes = |b: &Vec<u8>| D3D12_SHADER_BYTECODE { pShaderBytecode: b.as_ptr().cast(), BytecodeLength: b.len() };
            let layout = [
                D3D12_INPUT_ELEMENT_DESC { SemanticName: s!("POSITION"), Format: DXGI_FORMAT_R32G32B32_FLOAT, AlignedByteOffset: 0, InputSlotClass: D3D12_INPUT_CLASSIFICATION_PER_VERTEX_DATA, ..Default::default() },
                D3D12_INPUT_ELEMENT_DESC { SemanticName: s!("NORMAL"), Format: DXGI_FORMAT_R32G32B32_FLOAT, AlignedByteOffset: 12, InputSlotClass: D3D12_INPUT_CLASSIFICATION_PER_VERTEX_DATA, ..Default::default() },
                D3D12_INPUT_ELEMENT_DESC { SemanticName: s!("TEXCOORD"), Format: DXGI_FORMAT_R32G32_FLOAT, AlignedByteOffset: 24, InputSlotClass: D3D12_INPUT_CLASSIFICATION_PER_VERTEX_DATA, ..Default::default() },
            ];
            // (depth: tested and written, tested for none, or none bound;
            // blended over what is there; with vertex buffers)
            let pipeline = |root: &ID3D12RootSignature, vs: &Vec<u8>, ps: &Vec<u8>, depth: Option<bool>, blended: bool, meshes: bool| -> windows::core::Result<ID3D12PipelineState> {
                let mut desc = D3D12_GRAPHICS_PIPELINE_STATE_DESC {
                    pRootSignature: std::mem::transmute_copy(root),
                    VS: bytes(vs),
                    PS: bytes(ps),
                    SampleMask: u32::MAX,
                    RasterizerState: D3D12_RASTERIZER_DESC { FillMode: D3D12_FILL_MODE_SOLID, CullMode: D3D12_CULL_MODE_NONE, DepthClipEnable: true.into(), ..Default::default() },
                    DepthStencilState: D3D12_DEPTH_STENCIL_DESC {
                        DepthEnable: (depth == Some(true)).into(),
                        DepthWriteMask: if depth == Some(true) { D3D12_DEPTH_WRITE_MASK_ALL } else { D3D12_DEPTH_WRITE_MASK_ZERO },
                        DepthFunc: D3D12_COMPARISON_FUNC_GREATER,
                        ..Default::default()
                    },
                    PrimitiveTopologyType: D3D12_PRIMITIVE_TOPOLOGY_TYPE_TRIANGLE,
                    NumRenderTargets: 1,
                    DSVFormat: if depth.is_some() { DEPTH_FORMAT } else { DXGI_FORMAT_UNKNOWN },
                    SampleDesc: DXGI_SAMPLE_DESC { Count: 1, Quality: 0 },
                    ..Default::default()
                };
                if meshes {
                    desc.InputLayout = D3D12_INPUT_LAYOUT_DESC { pInputElementDescs: layout.as_ptr(), NumElements: layout.len() as u32 };
                }
                desc.RTVFormats[0] = EYE_FORMAT;
                desc.BlendState.RenderTarget[0] = D3D12_RENDER_TARGET_BLEND_DESC {
                    BlendEnable: blended.into(),
                    SrcBlend: D3D12_BLEND_SRC_ALPHA,
                    DestBlend: D3D12_BLEND_INV_SRC_ALPHA,
                    BlendOp: D3D12_BLEND_OP_ADD,
                    SrcBlendAlpha: D3D12_BLEND_ONE,
                    DestBlendAlpha: D3D12_BLEND_ONE,
                    BlendOpAlpha: D3D12_BLEND_OP_MAX,
                    RenderTargetWriteMask: D3D12_COLOR_WRITE_ENABLE_ALL.0 as u8,
                    ..Default::default()
                };
                device.CreateGraphicsPipelineState(&desc)
            };
            let relief = pipeline(&scene_root, &scene[0], &scene[1], Some(true), true, false)?;
            let dome = pipeline(&scene_root, &scene[2], &scene[3], Some(false), false, false)?;
            let blur = pipeline(&scene_root, &scene[2], &scene[4], None, false, false)?;
            let mesh = pipeline(&mesh_root, &mesh[0], &mesh[1], Some(true), false, true)?;
            let heap = |kind, count, flags| device.CreateDescriptorHeap::<ID3D12DescriptorHeap>(&D3D12_DESCRIPTOR_HEAP_DESC { Type: kind, NumDescriptors: count, Flags: flags, NodeMask: 0 });
            let views = heap(D3D12_DESCRIPTOR_HEAP_TYPE_CBV_SRV_UAV, (FRAMES * VIEWS_A_FRAME + MODEL_VIEWS) as u32, D3D12_DESCRIPTOR_HEAP_FLAG_SHADER_VISIBLE)?;
            let view_step = device.GetDescriptorHandleIncrementSize(D3D12_DESCRIPTOR_HEAP_TYPE_CBV_SRV_UAV) as usize;
            let rtvs = heap(D3D12_DESCRIPTOR_HEAP_TYPE_RTV, 3, D3D12_DESCRIPTOR_HEAP_FLAG_NONE)?;
            let dsvs = heap(D3D12_DESCRIPTOR_HEAP_TYPE_DSV, 1, D3D12_DESCRIPTOR_HEAP_FLAG_NONE)?;
            let rtv_step = device.GetDescriptorHandleIncrementSize(D3D12_DESCRIPTOR_HEAP_TYPE_RTV) as usize;
            let colour_clear = D3D12_CLEAR_VALUE { Format: EYE_FORMAT, Anonymous: D3D12_CLEAR_VALUE_0 { Color: [0.01, 0.01, 0.01, 1.0] } };
            let target = |size| texture2d(host, size, EYE_FORMAT, D3D12_RESOURCE_FLAG_ALLOW_RENDER_TARGET, D3D12_RESOURCE_STATE_PIXEL_SHADER_RESOURCE, Some(colour_clear));
            let eyes = [target(size)?, target(size)?];
            let ambient = target(AMBIENT)?;
            let depth_clear = D3D12_CLEAR_VALUE { Format: DEPTH_FORMAT, Anonymous: D3D12_CLEAR_VALUE_0 { DepthStencil: D3D12_DEPTH_STENCIL_VALUE { Depth: 0.0, Stencil: 0 } } };
            let depth = texture2d(host, size, DEPTH_FORMAT, D3D12_RESOURCE_FLAG_ALLOW_DEPTH_STENCIL, D3D12_RESOURCE_STATE_DEPTH_WRITE, Some(depth_clear))?;
            for (i, target) in eyes.iter().chain([&ambient]).enumerate() {
                let mut at = rtvs.GetCPUDescriptorHandleForHeapStart();
                at.ptr += i * rtv_step;
                device.CreateRenderTargetView(target, None, at);
            }
            device.CreateDepthStencilView(&depth, None, dsvs.GetCPUDescriptorHandleForHeapStart());
            // Every view is something from the start: null textures, and the
            // blurred frame in its place of each frame.
            let cpu = views.GetCPUDescriptorHandleForHeapStart();
            for i in 0..FRAMES * VIEWS_A_FRAME + MODEL_VIEWS {
                let at = D3D12_CPU_DESCRIPTOR_HANDLE { ptr: cpu.ptr + i * view_step };
                let is_ambient = i < FRAMES * VIEWS_A_FRAME && i % VIEWS_A_FRAME == 3;
                device.CreateShaderResourceView(is_ambient.then_some(&ambient), Some(&srv(EYE_FORMAT)), at);
            }
            let (ball_vertices, ball_indices) = ball();
            let cells = grid_size();
            let grid_from = Buffer::holding(host, &grid_indices(cells)).ok_or_else(windows::core::Error::empty)?;
            let grid = card_buffer(host, grid_from.bytes)?;
            // The card's clock, where the card has one for this queue.
            let clock = (|| {
                let ticks = queue.GetTimestampFrequency().ok()?;
                let mut heap: Option<ID3D12QueryHeap> = None;
                device.CreateQueryHeap(&D3D12_QUERY_HEAP_DESC { Type: D3D12_QUERY_HEAP_TYPE_TIMESTAMP, Count: (FRAMES * STAMPS) as u32, NodeMask: 0 }, &mut heap).ok()?;
                let memory = D3D12_HEAP_PROPERTIES { Type: D3D12_HEAP_TYPE_READBACK, ..Default::default() };
                let desc = D3D12_RESOURCE_DESC {
                    Dimension: D3D12_RESOURCE_DIMENSION_BUFFER,
                    Width: (FRAMES * STAMPS * 8) as u64,
                    Height: 1,
                    DepthOrArraySize: 1,
                    MipLevels: 1,
                    SampleDesc: DXGI_SAMPLE_DESC { Count: 1, Quality: 0 },
                    Layout: D3D12_TEXTURE_LAYOUT_ROW_MAJOR,
                    ..Default::default()
                };
                let mut out: Option<ID3D12Resource> = None;
                device.CreateCommittedResource(&memory, D3D12_HEAP_FLAG_NONE, &desc, D3D12_RESOURCE_STATE_COPY_DEST, None, &mut out).ok()?;
                let out = out?;
                let mut data = std::ptr::null_mut();
                out.Map(0, None, Some(&mut data)).ok()?;
                Some((heap?, out, data.cast::<u64>().cast_const(), ticks as f64 / 1000.0))
            })();
            tracing::info!(width = size.0, height = size.1, recommended = ?(w, h), view_up = (2.0 * up.atan()).to_degrees(), frame = super::picture(super::mode()).to_degrees(), grid = ?cells, "the headset mode: our eyes' pictures are made");
            Ok(Self {
                queue,
                allocators,
                list,
                fence: device.CreateFence(0, D3D12_FENCE_FLAG_NONE)?,
                event: CreateEventW(None, false, false, None)?,
                values: [0; FRAMES],
                value: 0,
                scene_root,
                relief,
                dome,
                blur,
                mesh_root,
                mesh,
                views,
                view_step,
                rtvs,
                rtv_step,
                dsvs,
                eyes,
                _depth: depth,
                ambient,
                grid,
                grid_from: Some(grid_from),
                cells,
                clock,
                size,
                projections,
                tangents,
                eye_to_head,
                ball: Mesh::new(host, &ball_vertices, &ball_indices),
                hands: [None, None],
                models: HashMap::new(),
                model_views: 0,
            })
        }
    }

    /// The frame-in-flight `f`'s last use is over.
    fn wait(&self, f: usize) {
        // SAFETY: our fence and event.
        unsafe {
            if self.fence.GetCompletedValue() < self.values[f] && self.fence.SetEventOnCompletion(self.values[f], self.event).is_ok() {
                WaitForSingleObject(self.event, 1000);
            }
        }
    }

    /// The hands' models, read once from the runtime's folder.
    fn load_hands(&mut self, host: &Host) {
        let Some(folder) = super::runtime_folder() else {
            return;
        };
        let mirror = std::env::var("VICTORY_VR_GLOVE_MIRROR").unwrap_or_default();
        for right in [false, true] {
            let made = glove::load(&folder, right).and_then(|glove| {
                let bytes = glove.vertices() * size_of::<Vertex>();
                let buffers = (Buffer::holding(host, &glove.indices), Buffer::new(host, bytes), Buffer::new(host, bytes));
                match buffers {
                    (Some(indices), Some(a), Some(b)) => {
                        let mirrored = mirror.contains(if right { 'r' } else { 'l' });
                        Ok(Hand { glove, indices, posed: [a, b], last: [None; FRAMES], mirrored })
                    }
                    _ => Err("no memory for its buffers".into()),
                }
            });
            match made {
                Ok(hand) => {
                    tracing::info!(right, vertices = hand.glove.vertices(), "the headset mode: a hand's model is read");
                    self.hands[usize::from(right)] = Some(hand);
                }
                Err(why) => tracing::warn!(right, "the headset mode: no model for a hand (the controller is drawn instead): {why}"),
            }
        }
    }

    /// The controller model `name`, asked for from SteamVR until it is
    /// there; its texture is copied by `list` the frame it arrives.
    fn model(&mut self, host: &Host, api: &Api, list: &ID3D12GraphicsCommandList, name: &str) {
        if name.is_empty() {
            return;
        }
        let state = self.models.entry(name.to_owned()).or_insert(Model::Loading);
        // SAFETY: the tables' entries by their types; what SteamVR hands
        // out is read before it is freed; an open list and live resources.
        unsafe {
            if matches!(state, Model::Loading) {
                let Ok(spelled) = CString::new(name) else {
                    *state = Model::Missing;
                    return;
                };
                let mut loaded: *mut RenderModel = std::ptr::null_mut();
                match api.models::<LoadModel>(models::LOAD_MODEL)(spelled.as_ptr(), &mut loaded) {
                    LOADING => return,
                    0 if !loaded.is_null() => {
                        let m = &*loaded;
                        let vertices = std::slice::from_raw_parts(m.vertices, m.vertex_count as usize);
                        let indices = std::slice::from_raw_parts(m.indices, m.triangle_count as usize * 3);
                        let skin = if m.texture >= 0 { Skin::Loading(m.texture) } else { Skin::None };
                        *state = match Mesh::new(host, vertices, indices) {
                            Some(mesh) => {
                                tracing::info!(name, vertices = vertices.len(), triangles = m.triangle_count, "the headset mode: a controller's model is read");
                                Model::Ready(mesh, skin)
                            }
                            None => Model::Missing,
                        };
                        api.models::<FreeModel>(models::FREE_MODEL)(loaded);
                    }
                    error => {
                        tracing::warn!(name, error, "the headset mode: SteamVR has no such model (nothing is drawn for it)");
                        *state = Model::Missing;
                    }
                }
            }
            let Model::Ready(_, skin) = state else {
                return;
            };
            let Skin::Loading(id) = *skin else {
                return;
            };
            let mut map: *mut TextureMap = std::ptr::null_mut();
            match api.models::<LoadTexture>(models::LOAD_TEXTURE)(id, &mut map) {
                LOADING => {}
                0 if !map.is_null() && self.model_views < MODEL_VIEWS => {
                    let t = &*map;
                    let (width, height) = (u32::from(t.width), u32::from(t.height));
                    let made = texture2d(host, (width, height), EYE_FORMAT, D3D12_RESOURCE_FLAG_NONE, D3D12_RESOURCE_STATE_COPY_DEST, None).ok().and_then(|texture| {
                        let desc = texture.GetDesc();
                        let mut footprint = D3D12_PLACED_SUBRESOURCE_FOOTPRINT::default();
                        let mut total = 0u64;
                        host.device.GetCopyableFootprints(&desc, 0, 1, 0, Some(&mut footprint), None, None, Some(&mut total));
                        let upload = Buffer::new(host, total as usize)?;
                        let pitch = footprint.Footprint.RowPitch as usize;
                        for row in 0..height as usize {
                            std::ptr::copy_nonoverlapping(t.data.add(row * width as usize * 4), upload.data.add(footprint.Offset as usize + row * pitch), width as usize * 4);
                        }
                        let to = D3D12_TEXTURE_COPY_LOCATION { pResource: std::mem::transmute_copy(&texture), Type: D3D12_TEXTURE_COPY_TYPE_SUBRESOURCE_INDEX, Anonymous: D3D12_TEXTURE_COPY_LOCATION_0 { SubresourceIndex: 0 } };
                        let from = D3D12_TEXTURE_COPY_LOCATION { pResource: std::mem::transmute_copy(&upload.resource), Type: D3D12_TEXTURE_COPY_TYPE_PLACED_FOOTPRINT, Anonymous: D3D12_TEXTURE_COPY_LOCATION_0 { PlacedFootprint: footprint } };
                        list.CopyTextureRegion(&to, 0, 0, 0, &from, None);
                        list.ResourceBarrier(&[transition(&texture, D3D12_RESOURCE_STATE_COPY_DEST, D3D12_RESOURCE_STATE_PIXEL_SHADER_RESOURCE)]);
                        Some((texture, upload))
                    });
                    *skin = match made {
                        Some((texture, upload)) => {
                            let view = FRAMES * VIEWS_A_FRAME + self.model_views;
                            self.model_views += 1;
                            let cpu = self.views.GetCPUDescriptorHandleForHeapStart();
                            host.device.CreateShaderResourceView(&texture, Some(&srv(EYE_FORMAT)), D3D12_CPU_DESCRIPTOR_HANDLE { ptr: cpu.ptr + view * self.view_step });
                            Skin::Ready(texture, upload, view)
                        }
                        None => Skin::None,
                    };
                    api.models::<FreeTexture>(models::FREE_TEXTURE)(map);
                }
                _ => *skin = Skin::None,
            }
        }
    }
}

/// What tells trackers apart at a glance.
fn tracker_colour(part: BodyPart) -> [f32; 4] {
    match part {
        BodyPart::Waist | BodyPart::Chest => [0.95, 0.75, 0.2, 0.0],
        BodyPart::LeftFoot | BodyPart::LeftKnee | BodyPart::LeftElbow | BodyPart::LeftShoulder => [0.3, 0.6, 1.0, 0.0],
        BodyPart::RightFoot | BodyPart::RightKnee | BodyPart::RightElbow | BodyPart::RightShoulder => [1.0, 0.4, 0.35, 0.0],
        BodyPart::Other => [0.8, 0.8, 0.8, 0.0],
    }
}

/// The thread's body: never returns unless the renderer cannot be made.
pub(super) fn run(api: &'static Api, host: &'static Host) {
    let mut r = match Renderer::new(host, api) {
        Ok(r) => r,
        Err(e) => {
            tracing::warn!(error = %e, "the headset mode: our picture for the headset could not be made; nothing is shown in it");
            return;
        }
    };
    r.load_hands(host);
    let near = super::number("VICTORY_VR_NEAR", 0.1).clamp(0.001, 10.0);
    let edge = super::number("VICTORY_VR_EDGE", 1.08).max(1.001);
    let farthest = super::number("VICTORY_VR_FARTHEST", 400.0).clamp(10.0, 10_000.0);
    let fade = super::number("VICTORY_VR_FADE", 0.02).clamp(0.0, 0.3);
    let glow = super::number("VICTORY_VR_GLOW", 1.0).clamp(0.0, 2.0);
    let nearest = super::number("VICTORY_VR_NEAREST", 0.3).clamp(0.0, 5.0);
    let skirt = skirt();
    let parallax = super::number("VICTORY_VR_PARALLAX", 1.0).clamp(0.0, 1.0);
    // The hand on the controller: its fingers lifted by so many degrees, its
    // wrist so far (right, up, back) from the controller's own origin.
    let hand_on = {
        let asked: Vec<f32> = std::env::var("VICTORY_VR_HAND_AT").unwrap_or_default().split(',').filter_map(|v| v.trim().parse().ok()).collect();
        let wrist = if asked.len() == 3 { [asked[0], asked[1], asked[2]] } else { glove::WRIST };
        let tilt = super::number("VICTORY_VR_HAND_TILT", 0.0).clamp(-90.0, 90.0);
        [glove::on_controller(false, tilt, wrist), glove::on_controller(true, tilt, wrist)]
    };
    let nod = super::number("VICTORY_VR_EMU_PITCH", 0.0).clamp(-89.0, 89.0).to_radians();
    let (flat_width, flat_distance) = (super::number("VICTORY_VR_WIDTH", 4.0).clamp(0.5, 20.0), super::number("VICTORY_VR_DISTANCE", 3.0).clamp(0.5, 20.0));
    let follows = std::env::var_os("VICTORY_VR_FOLLOW").is_none_or(|v| v != "0");
    let skin_tone = {
        let asked: Vec<f32> = std::env::var("VICTORY_VR_SKIN").unwrap_or_default().split(',').filter_map(|v| v.trim().parse().ok()).collect();
        if asked.len() == 3 { [asked[0], asked[1], asked[2], 0.0] } else { [0.86, 0.66, 0.54, 0.0] }
    };
    let sway = std::env::var_os("VICTORY_VR_EMU").is_some_and(|v| v != "0") && std::env::var_os("VICTORY_VR_EMU_SWAY").is_none_or(|v| v != "0");
    // `calm`: as a seated player's head moves, not from side to side.
    let calm = std::env::var("VICTORY_VR_EMU_SWAY").is_ok_and(|v| v == "calm");
    let started = Instant::now();
    let mut anchor: Option<M34> = None;
    let mut frames = 0u64;
    let mut missed = 0u64;
    let mut said = Instant::now();
    let mut last = Instant::now();
    let mut failures = 0u32;
    // The frame of ours that copied the grid's indices to the card.
    let mut grid_sent: Option<u64> = None;
    // What our frames took on the card since the last word of it: the
    // whole, the small blurred frame, the left eye's surroundings, relief
    // and meshes, and the right eye; and of how many frames.
    let mut took = [0.0f64; 6];
    let mut timed = 0u32;
    loop {
        // SAFETY: plain data the call fills, of the count said.
        let mut poses: [Pose; 64] = unsafe { std::mem::zeroed() };
        // SAFETY: the table's entry by its type; the array lives through the call.
        let waited = unsafe { api.compositor::<WaitGetPoses>(compositor::WAIT_GET_POSES)(poses.as_mut_ptr(), 64, std::ptr::null_mut(), 0) };
        if waited != 0 {
            failures += 1;
            if failures == 1 || failures.is_power_of_two() {
                tracing::warn!(error = waited, failures, "the headset mode: the compositor gave no poses (not the scene application, or SteamVR is busy)");
            }
            std::thread::sleep(Duration::from_millis(20));
            continue;
        }
        let dt = last.elapsed().as_secs_f32().clamp(0.001, 0.1);
        last = Instant::now();
        let mut head = if poses[0].valid { poses[0].at } else { IDENTITY };
        if sway {
            // With emulated controllers, a head that moves by itself: a
            // little to the sides and turning, to see the relief's depth.
            let t = started.elapsed().as_secs_f32();
            let (wide, turned, slow) = if calm { (0.03, 0.07, 0.5) } else { (0.25, 0.12, 1.0) };
            let yaw = turned * (t * 0.7 * slow).sin();
            let (c, s) = (yaw.cos(), yaw.sin());
            let at = add(place(&head), [wide * (t * 0.9 * slow).sin(), wide.min(0.04) * (t * 1.3 * slow).sin(), 0.0]);
            head = from_axes([c, 0.0, -s], [0.0, 1.0, 0.0], [s, 0.0, c], at);
            if nod != 0.0 {
                // Looking up (or, below zero, down) by so much, to see what is worn.
                let (pc, ps) = (nod.cos(), nod.sin());
                head = mul(&head, &from_axes([1.0, 0.0, 0.0], [0.0, pc, ps], [0.0, -ps, pc], [0.0; 3]));
            }
        }
        let mode = super::mode();
        // Where the head turns on the neck: the view is anchored by this,
        // which a head that only turns leaves in place.
        let neck = neck_of(&head);
        if anchor.is_none() || super::RECENTRE.swap(false, std::sync::atomic::Ordering::Relaxed) {
            anchor = Some(level(&neck));
            tracing::info!(?mode, head = ?place(&head), "the headset mode: the view is centred");
        }
        let Some(mut anchor_at) = anchor else { continue };
        if follows {
            anchor_at = follow(&anchor_at, &neck, 25f32.to_radians(), 0.4, dt);
            anchor = Some(anchor_at);
        }
        // The title's camera in the room: at the eyes, less what the neck
        // itself has moved from the anchor.
        let camera = camera_of(&anchor_at, &head, parallax);
        // The eyes of a head upright over the anchor: a screen stands in
        // front of these.
        let home = at_place(&anchor_at, sub(place(&anchor_at), turn(&anchor_at, NECK)));
        let f = (frames as usize) % FRAMES;
        r.wait(f);
        if let Some((_, _, data, per_ms)) = &r.clock
            && frames >= FRAMES as u64
        {
            // What this slot's last frame took on the card (it is over:
            // waited for just now).
            // SAFETY: a mapping of FRAMES * STAMPS readings, written by a
            // resolve that is over.
            let t: [u64; STAMPS] = std::array::from_fn(|i| unsafe { data.add(f * STAMPS + i).read_volatile() });
            if t.windows(2).all(|w| w[1] >= w[0]) {
                for (k, (from, to)) in [(0, 5), (0, 1), (1, 2), (2, 3), (3, 4), (4, 5)].into_iter().enumerate() {
                    took[k] += (t[to] - t[from]) as f64 / per_ms;
                }
                timed += 1;
            }
        }
        if grid_sent.is_some_and(|at| frames >= at + FRAMES as u64) {
            // The frame that copied the grid is over (this is its slot's
            // next turn, waited for just now).
            r.grid_from = None;
            grid_sent = None;
        }
        let value = r.value + 1;
        let snap = super::snap_for_drawing(value, &r.fence);
        if snap.is_none() {
            missed += 1;
        }
        let world_scale = super::world_scale();
        let held = super::held();
        let views: Views = super::VIEWS.lock().unwrap_or_else(|e| e.into_inner()).clone().unwrap_or_default();
        let (head_forward, _) = level_forward(&head);
        let light = unit(add(add([0.0, 0.85, 0.0], scale(head_forward, -0.45)), scale(cross(head_forward, [0.0, 1.0, 0.0]), 0.25)), [0.0, 1.0, 0.0]);
        // SAFETY: an allocator whose frame is over; an open list of ours;
        // live resources (the snapshot's are kept until our fence passes
        // `value`, see `snap_for_drawing`); buffers of ours written only
        // for a frame whose last use is over.
        unsafe {
            let _ = r.allocators[f].Reset();
            let _ = r.list.Reset(&r.allocators[f], &r.relief);
            let list = r.list.clone();
            let cpu = r.views.GetCPUDescriptorHandleForHeapStart();
            let gpu = r.views.GetGPUDescriptorHandleForHeapStart();
            let base = f * VIEWS_A_FRAME;
            let view_step = r.view_step;
            let view_at = |i: usize| D3D12_GPU_DESCRIPTOR_HANDLE { ptr: gpu.ptr + (i * view_step) as u64 };

            // What is held and worn: the meshes of this frame.
            let mut draws: Vec<Draw> = Vec::new();
            for (i, hand) in views.hands.iter().enumerate() {
                if !hand.present {
                    continue;
                }
                let at = match hand.device {
                    Some(device) if poses.get(device as usize).is_some_and(|p| p.valid) => poses[device as usize].at,
                    _ => hand.pose,
                };
                // A closing hand is a hand, whatever is drawn otherwise.
                let as_hand = (held == Held::Hand || hand.grip > 0.25) && r.hands[i].is_some();
                if as_hand && let Some(shown) = r.hands[i].as_mut() {
                    let key = hand.curl.map(|c| (c * 60.0).round() as i32);
                    if shown.last[f] != Some(key) {
                        let out = std::slice::from_raw_parts_mut(shown.posed[f].data.cast::<Vertex>(), shown.glove.vertices());
                        shown.glove.posed(hand.curl, shown.mirrored, out);
                        shown.last[f] = Some(key);
                    }
                    draws.push(Draw { vertices: shown.posed[f].vertices(), indices: shown.indices.indices(), count: shown.glove.indices.len() as u32, at: mul(&at, &hand_on[i]), tint: skin_tone, view: None });
                } else {
                    r.model(host, api, &list, &hand.model);
                    if let Some(Model::Ready(mesh, skin)) = r.models.get(&hand.model) {
                        let view = match skin {
                            Skin::Ready(_, _, view) => Some(*view),
                            _ => None,
                        };
                        let tint = if view.is_some() { [1.0, 1.0, 1.0, 1.0] } else { [0.55, 0.57, 0.6, 0.0] };
                        draws.push(Draw { vertices: mesh.vertices.vertices(), indices: mesh.indices.indices(), count: mesh.count, at, tint, view });
                    }
                }
            }
            if let Some(ball) = &r.ball {
                for tracker in &views.trackers {
                    let at = match tracker.device {
                        Some(device) if poses.get(device as usize).is_some_and(|p| p.valid) => poses[device as usize].at,
                        _ => tracker.pose,
                    };
                    draws.push(Draw { vertices: ball.vertices.vertices(), indices: ball.indices.indices(), count: ball.count, at: sized(&at, [0.045, 0.03, 0.045]), tint: tracker_colour(tracker.part), view: None });
                }
            }

            if let Some(snap) = &snap {
                let at = |i: usize| D3D12_CPU_DESCRIPTOR_HANDLE { ptr: cpu.ptr + (base + i) * view_step };
                host.device.CreateShaderResourceView(&snap.colour, Some(&srv(snap.colour_format)), at(0));
                let (depth, float_format, uint_format) = match &snap.depth {
                    Some((depth, kind)) => (Some(depth), kind.float_view(), kind.uint_view()),
                    None => (None, DXGI_FORMAT_R32_FLOAT, DXGI_FORMAT_R32_UINT),
                };
                host.device.CreateShaderResourceView(depth, Some(&srv(float_format)), at(1));
                host.device.CreateShaderResourceView(depth, Some(&srv(uint_format)), at(2));
            }
            if let Some(from) = &r.grid_from {
                // Once: the grid's indices into the card's memory. The
                // buffer is ours to drop when this frame's last use is over.
                list.CopyBufferRegion(&r.grid, 0, &from.resource, 0, from.bytes as u64);
                list.ResourceBarrier(&[transition(&r.grid, D3D12_RESOURCE_STATE_COPY_DEST, D3D12_RESOURCE_STATE_INDEX_BUFFER)]);
                grid_sent = Some(frames);
            }
            let cells = r.cells;
            let grid_view = D3D12_INDEX_BUFFER_VIEW { BufferLocation: r.grid.GetGPUVirtualAddress(), SizeInBytes: cells.0 * cells.1 * 6 * 4, Format: DXGI_FORMAT_R32_UINT };
            // The card's clock here, the `i`th reading of this frame.
            let clock = r.clock.as_ref().map(|(heap, ..)| heap.clone());
            let stamp = |i: usize| {
                if let Some(heap) = &clock {
                    list.EndQuery(heap, D3D12_QUERY_TYPE_TIMESTAMP, (f * STAMPS + i) as u32);
                }
            };
            stamp(0);
            list.SetDescriptorHeaps(&[Some(r.views.clone())]);
            list.SetGraphicsRootSignature(&r.scene_root);
            list.SetGraphicsRootDescriptorTable(1, view_at(base));
            list.IASetPrimitiveTopology(D3D_PRIMITIVE_TOPOLOGY_TRIANGLELIST);
            let whole = |size: (u32, u32)| {
                list.RSSetViewports(&[D3D12_VIEWPORT { Width: size.0 as f32, Height: size.1 as f32, MaxDepth: 1.0, ..Default::default() }]);
                list.RSSetScissorRects(&[windows::Win32::Foundation::RECT { left: 0, top: 0, right: size.0 as i32, bottom: size.1 as i32 }]);
            };
            let rtv = |i: usize| {
                let mut at = r.rtvs.GetCPUDescriptorHandleForHeapStart();
                at.ptr += i * r.rtv_step;
                at
            };
            // The title's frame, small.
            if snap.is_some() {
                list.ResourceBarrier(&[transition(&r.ambient, D3D12_RESOURCE_STATE_PIXEL_SHADER_RESOURCE, D3D12_RESOURCE_STATE_RENDER_TARGET)]);
                list.SetPipelineState(&r.blur);
                list.OMSetRenderTargets(1, Some(&rtv(2)), false, None);
                whole(AMBIENT);
                let constants = SceneConstants { p4: [0.0, 0.0, 1.0 / (AMBIENT.0 * 8) as f32, 1.0 / (AMBIENT.1 * 8) as f32], ..Default::default() };
                list.SetGraphicsRoot32BitConstants(0, 48, std::ptr::from_ref(&constants).cast(), 0);
                list.DrawInstanced(3, 1, 0, 0);
                list.ResourceBarrier(&[transition(&r.ambient, D3D12_RESOURCE_STATE_RENDER_TARGET, D3D12_RESOURCE_STATE_PIXEL_SHADER_RESOURCE)]);
            }
            list.ResourceBarrier(&[
                transition(&r.eyes[0], D3D12_RESOURCE_STATE_PIXEL_SHADER_RESOURCE, D3D12_RESOURCE_STATE_RENDER_TARGET),
                transition(&r.eyes[1], D3D12_RESOURCE_STATE_PIXEL_SHADER_RESOURCE, D3D12_RESOURCE_STATE_RENDER_TARGET),
            ]);
            whole(r.size);
            let dsv = r.dsvs.GetCPUDescriptorHandleForHeapStart();
            for eye in 0..2 {
                list.OMSetRenderTargets(1, Some(&rtv(eye)), false, Some(&dsv));
                list.ClearRenderTargetView(rtv(eye), &[0.01, 0.01, 0.01, 1.0], None);
                list.ClearDepthStencilView(dsv, D3D12_CLEAR_FLAG_DEPTH, 0.0, 0, None);
                let eye_at = mul(&head, &r.eye_to_head[eye]);
                let viewproj = times(&r.projections[eye], &inverse(&eye_at));
                if let Some(snap) = &snap {
                    let tan_y = (super::picture(mode) / 2.0).tan();
                    let aspect = snap.aspect;
                    let relief = mode != Mode::Cinema && snap.depth.is_some();
                    let (world, width, distance) = match mode {
                        Mode::Relief if relief => (camera, flat_width, flat_distance),
                        Mode::Relief | Mode::Cinema => (home, flat_width, flat_distance),
                        // Fixed to the head; with no depth, a screen 110
                        // degrees wide a little past arm's length.
                        Mode::Immersive => (head, 2.0 * 1.2 * 55f32.to_radians().tan(), 1.2),
                    };
                    // The frame's half size as the camera sees it.
                    let tangents = if relief { [tan_y * aspect, tan_y] } else { [width / 2.0 / distance, width / 2.0 / distance / aspect] };
                    let mut constants = SceneConstants {
                        viewproj,
                        world: mul(&inverse(&world), &eye_at),
                        p0: [tangents[0], tangents[1], near, world_scale],
                        p3: r.tangents[eye],
                        p4: [0.0, glow * if relief { 1.0 } else { 0.7 }, 0.0, 0.0],
                        ..Default::default()
                    };
                    list.SetGraphicsRootSignature(&r.scene_root);
                    list.SetGraphicsRootDescriptorTable(1, view_at(base));
                    list.SetPipelineState(&r.dome);
                    list.SetGraphicsRoot32BitConstants(0, 48, std::ptr::from_ref(&constants).cast(), 0);
                    if eye == 0 {
                        stamp(1);
                    }
                    list.DrawInstanced(3, 1, 0, 0);
                    constants.world = world;
                    constants.p0 = [tan_y * aspect, tan_y, near, world_scale];
                    constants.p1 = [cells.0 as f32, cells.1 as f32, edge, if relief { 0.0 } else { 1.0 }];
                    constants.p2 = [distance, width, snap.depth.as_ref().map_or(0.0, |(_, kind)| kind.number()), farthest];
                    constants.p4[0] = if relief { fade } else { 0.0 };
                    constants.p4[2] = nearest / world_scale;
                    constants.p4[3] = if relief { skirt } else { 0.0 };
                    list.SetPipelineState(&r.relief);
                    list.SetGraphicsRoot32BitConstants(0, 48, std::ptr::from_ref(&constants).cast(), 0);
                    list.IASetIndexBuffer(Some(&grid_view));
                    if eye == 0 {
                        stamp(2);
                    }
                    list.DrawIndexedInstanced(cells.0 * cells.1 * 6, 1, 0, 0, 0);
                }
                if eye == 0 {
                    stamp(3);
                }
                if !draws.is_empty() {
                    list.SetGraphicsRootSignature(&r.mesh_root);
                    list.SetPipelineState(&r.mesh);
                    for draw in &draws {
                        let constants = MeshConstants { viewproj, model: draw.at, tint: draw.tint, light: [light[0], light[1], light[2], 0.0] };
                        list.SetGraphicsRootDescriptorTable(1, view_at(draw.view.unwrap_or(base + 3)));
                        list.SetGraphicsRoot32BitConstants(0, 36, std::ptr::from_ref(&constants).cast(), 0);
                        list.IASetVertexBuffers(0, Some(&[draw.vertices]));
                        list.IASetIndexBuffer(Some(&draw.indices));
                        list.DrawIndexedInstanced(draw.count, 1, 0, 0, 0);
                    }
                }
                if eye == 0 {
                    stamp(4);
                }
            }
            list.ResourceBarrier(&[
                transition(&r.eyes[0], D3D12_RESOURCE_STATE_RENDER_TARGET, D3D12_RESOURCE_STATE_PIXEL_SHADER_RESOURCE),
                transition(&r.eyes[1], D3D12_RESOURCE_STATE_RENDER_TARGET, D3D12_RESOURCE_STATE_PIXEL_SHADER_RESOURCE),
            ]);
            stamp(5);
            if let Some((heap, out, ..)) = &r.clock {
                list.ResolveQueryData(heap, D3D12_QUERY_TYPE_TIMESTAMP, (f * STAMPS) as u32, STAMPS as u32, out, (f * STAMPS * 8) as u64);
            }
            let _ = list.Close();
            r.queue.ExecuteCommandLists(&[Some(list.cast().expect("a graphics list is a command list"))]);
            r.value = value;
            let _ = r.queue.Signal(&r.fence, value);
            r.values[f] = value;
            for eye in 0..2 {
                let mut data = TextureData { resource: r.eyes[eye].as_raw(), queue: r.queue.as_raw(), node_mask: 0 };
                let texture = Texture { handle: std::ptr::from_mut(&mut data).cast(), kind: TEXTURE_DIRECTX12, color_space: COLOR_SPACE_GAMMA };
                let submitted = api.compositor::<Submit>(compositor::SUBMIT)(eye as i32, &texture, std::ptr::null(), 0);
                if submitted != 0 && frames < 4 {
                    tracing::warn!(eye, error = submitted, "the headset mode: the compositor did not take an eye's picture");
                }
            }
        }
        frames += 1;
        if said.elapsed() >= Duration::from_secs(5) {
            let ms = took.map(|t| (t / f64::from(timed.max(1)) * 100.0).round() / 100.0);
            tracing::info!(frames, without_picture = missed, ?mode, ?held, world_scale, head = ?place(&head).map(|v| (v * 100.0).round() / 100.0), card_ms = ms[0], blur = ms[1], surroundings = ms[2], relief = ms[3], meshes = ms[4], right_eye = ms[5], "the headset mode: our frames");
            (took, timed) = ([0.0; 6], 0);
            said = Instant::now();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_point_in_front_of_an_eye_lands_in_its_picture() {
        let p = projection(-1.0, 1.0, -1.0, 1.0, 0.03);
        // Two metres straight ahead: the middle of the picture, depth near/2.
        let clip = times(&p, &IDENTITY);
        let at = [0.0f32, 0.0, -2.0, 1.0];
        let c: Vec<f32> = clip.iter().map(|row| row.iter().zip(at).map(|(a, b)| a * b).sum()).collect();
        assert!((c[0] / c[3]).abs() < 1e-6 && (c[1] / c[3]).abs() < 1e-6 && (c[2] / c[3] - 0.015).abs() < 1e-6);
        // Up and to the right at 45 degrees: the top right corner.
        let at = [2.0f32, 2.0, -2.0, 1.0];
        let c: Vec<f32> = clip.iter().map(|row| row.iter().zip(at).map(|(a, b)| a * b).sum()).collect();
        assert!((c[0] / c[3] - 1.0).abs() < 1e-6 && (c[1] / c[3] - 1.0).abs() < 1e-6);
        // A rigid inverse undoes its transform.
        let m = from_axes([0.0, 0.0, -1.0], [0.0, 1.0, 0.0], [1.0, 0.0, 0.0], [1.0, 2.0, 3.0]);
        let back = mul(&inverse(&m), &m);
        for (i, row) in back.iter().enumerate() {
            for (j, v) in row.iter().enumerate() {
                assert!((v - IDENTITY[i][j]).abs() < 1e-5);
            }
        }
    }

    #[test]
    fn the_view_comes_after_a_head_that_turns_or_walks_away() {
        let turned = |degrees: f32, at: V3| {
            let (c, s) = (degrees.to_radians().cos(), degrees.to_radians().sin());
            from_axes([c, 0.0, -s], [0.0, 1.0, 0.0], [s, 0.0, c], at)
        };
        let free = 25f32.to_radians();
        // Within the free angle and step nothing moves.
        let still = follow(&IDENTITY, &turned(20.0, [0.2, 0.0, 0.1]), free, 0.4, 0.011);
        assert_eq!(still, IDENTITY);
        // Past them, held long enough, the anchor ends at the free angle
        // from the head and the free step away.
        let head = turned(60.0, [1.0, 0.0, 0.0]);
        let mut anchor = IDENTITY;
        for _ in 0..400 {
            anchor = follow(&anchor, &head, free, 0.4, 0.011);
        }
        let (_, yaw) = level_forward(&anchor);
        assert!((yaw.to_degrees() - 35.0).abs() < 0.5, "{}", yaw.to_degrees());
        assert!((length(sub(place(&head), place(&anchor))) - 0.4).abs() < 0.01);
    }

    #[test]
    fn a_turning_head_keeps_the_camera_at_its_eyes() {
        // A head turned by `degrees` on a neck standing at `neck`.
        let head = |degrees: f32, neck: V3| {
            let (c, s) = (degrees.to_radians().cos(), degrees.to_radians().sin());
            let turned = from_axes([c, 0.0, -s], [0.0, 1.0, 0.0], [s, 0.0, c], [0.0; 3]);
            at_place(&turned, sub(neck, turn(&turned, NECK)))
        };
        let anchor = level(&neck_of(&head(0.0, [0.0, 1.6, 0.0])));
        assert!(length(sub(place(&anchor), [0.0, 1.6, 0.0])) < 1e-6);
        // Turned on the neck, the eyes have moved some centimetres, and the
        // camera with them: nothing near moves against what is far.
        let turned = head(40.0, [0.0, 1.6, 0.0]);
        assert!(length(sub(place(&turned), place(&head(0.0, [0.0, 1.6, 0.0])))) > 0.05);
        assert!(length(sub(place(&camera_of(&anchor, &turned, 1.0)), place(&turned))) < 1e-6);
        // Leaning ten centimetres to the right, the camera stays behind by
        // those, by half of them, or not at all.
        let leaned = head(40.0, [0.1, 1.6, 0.0]);
        for (parallax, behind) in [(1.0, 0.1), (0.5, 0.05), (0.0, 0.0)] {
            let off = sub(place(&leaned), place(&camera_of(&anchor, &leaned, parallax)));
            assert!((off[0] - behind).abs() < 1e-6 && off[1].abs() < 1e-6 && off[2].abs() < 1e-6, "{off:?}");
        }
        // The camera looks where the anchor does, whatever the head does.
        assert_eq!(axis(&camera_of(&anchor, &leaned, 1.0), 2), axis(&anchor, 2));
    }

    #[test]
    fn a_ball_is_closed_and_of_radius_one() {
        let (vertices, indices) = ball();
        assert!(indices.len() % 3 == 0 && indices.iter().all(|&i| usize::from(i) < vertices.len()));
        assert!(vertices.iter().all(|v| ((v[0] * v[0] + v[1] * v[1] + v[2] * v[2]).sqrt() - 1.0).abs() < 1e-5));
    }
}
