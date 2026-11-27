//! The headset's picture drawn by us ("victory-vr-scene"): a scene of our
//! own submitted to SteamVR's compositor for each eye at the headset's rate,
//! from the last frame the title presented and, for depth, its depth buffer
//! ([`super::Snaps`]).
//!
//! Three ways to show the frame ([`super::Mode`]):
//!
//! - **3d**: the frame as a relief around the player, each texel of the
//!   title's depth buffer put back where the title's camera saw it. The
//!   title's camera stands where the player's head was when the view was
//!   last centred, looking the way the head looked; the head moves freely
//!   in front of it, with the depth of both eyes and the parallax of the
//!   head's own moves, at the headset's rate however slowly the title's
//!   frames come. Where depth jumps (an edge between near and far) the grid
//!   cell is left out rather than stretched over the gap.
//! - **cinema**: a flat screen standing in the room.
//! - **immersive**: a wide flat screen fixed to the head.
//!
//! The title's projection is not known here: its vertical field of view is
//! `VICTORY_VR_FOV` degrees (50), its depth reversed with an infinite far
//! plane at `VICTORY_VR_NEAR` metres (0.1), and the relief scaled by the
//! world scale (F8 smaller, F9 larger).
//!
//! Nothing here waits for the title's queue, nor the title's queue for ours:
//! a snapshot is used once the title's queue has passed its copy (a fence
//! looked at on the CPU), and the title skips writing into one we may still
//! be reading.

use std::time::{Duration, Instant};

use windows::Win32::Foundation::HANDLE;
use windows::Win32::Graphics::Direct3D::D3D_PRIMITIVE_TOPOLOGY_TRIANGLELIST;
use windows::Win32::Graphics::Direct3D12::*;
use windows::Win32::Graphics::Dxgi::Common::*;
use windows::Win32::System::Threading::{CreateEventW, WaitForSingleObject};
use windows::core::{Interface, s};

use super::math::{IDENTITY, M34, V3, add, axis, cross, from_axes, level_forward, mul, place, scale, turn};
use super::{Api, Mode, TEXTURE_DIRECTX12, Texture, TextureData, compositor, system};
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

type WaitGetPoses = unsafe extern "system" fn(*mut Pose, u32, *mut Pose, u32) -> i32;
type Submit = unsafe extern "system" fn(i32, *const Texture, *const c_void_bounds, i32) -> i32;
type RecommendedSize = unsafe extern "system" fn(*mut u32, *mut u32);
type ProjectionRaw = unsafe extern "system" fn(i32, *mut f32, *mut f32, *mut f32, *mut f32);
type EyeToHead = unsafe extern "system" fn(i32) -> Matrix34;

/// `VRTextureBounds_t` (never passed: the whole texture).
#[allow(non_camel_case_types)]
type c_void_bounds = std::ffi::c_void;

const COLOR_SPACE_GAMMA: i32 = 1;
const EYE_FORMAT: DXGI_FORMAT = DXGI_FORMAT_R8G8B8A8_UNORM;
const DEPTH_FORMAT: DXGI_FORMAT = DXGI_FORMAT_D32_FLOAT;
/// The grid the relief is made of, cells across and down.
const GRID: (u32, u32) = (320, 180);
/// How near our own eyes see, metres.
const EYE_NEAR: f32 = 0.03;
/// Frames of ours in flight.
const FRAMES: usize = 2;
/// Descriptors a frame uses: the colour, the depth as float, as uint.
const VIEWS: usize = 3;

const SOURCE: &str = r#"
#define RS "RootConstants(num32BitConstants=40, b0), DescriptorTable(SRV(t0, numDescriptors=3)), StaticSampler(s0, filter=FILTER_MIN_MAG_MIP_LINEAR, addressU=TEXTURE_ADDRESS_CLAMP, addressV=TEXTURE_ADDRESS_CLAMP)"
cbuffer C : register(b0) {
    float4 vp0, vp1, vp2, vp3;   // the eye's view and projection, rows
    float4 w0, w1, w2;           // the title's camera in the room, rows of 3x4
    float4 p0;                   // tan x, tan y, near, scale
    float4 p1;                   // grid w, grid h, edge ratio, flat (1) or relief (0)
    float4 p2;                   // flat distance, flat width, depth kind, farthest
};
Texture2D<float4> colour : register(t0);
Texture2D<float> depth_float : register(t1);
Texture2D<uint> depth_uint : register(t2);
SamplerState linear_clamp : register(s0);

struct V { float4 pos : SV_Position; float2 uv : TEXCOORD0; };

float view_z(float2 uv) {
    uint w, h;
    depth_float.GetDimensions(w, h);
    int3 t = int3(min(uint2(uv * float2(w, h)), uint2(w - 1, h - 1)), 0);
    float d;
    if (p2.z > 1.5 && p2.z < 2.5) d = float(depth_uint.Load(t) & 0xFFFFFFu) / 16777215.0;
    else d = depth_float.Load(t);
    return min(p0.z / max(d, 1e-7), p2.w);
}

float4 placed(float3 local) {
    float4 l = float4(local, 1.0);
    float4 world = float4(dot(w0, l), dot(w1, l), dot(w2, l), 1.0);
    return float4(dot(vp0, world), dot(vp1, world), dot(vp2, world), dot(vp3, world));
}

[RootSignature(RS)]
V vs(uint id : SV_VertexID) {
    static const uint2 corners[6] = { uint2(0, 0), uint2(1, 0), uint2(0, 1), uint2(0, 1), uint2(1, 0), uint2(1, 1) };
    uint gw = (uint)p1.x, gh = (uint)p1.y;
    uint cell = id / 6;
    uint2 at = uint2(cell % gw, cell / gw);
    float2 size = float2(gw, gh);
    float2 uv = float2(at + corners[id % 6]) / size;
    V o;
    o.uv = uv;
    float aspect = p0.x / p0.y;
    if (p1.w > 0.5) {
        float width = p2.y;
        o.pos = placed(float3((uv.x - 0.5) * width, (0.5 - uv.y) * width / aspect, -p2.x));
        return o;
    }
    // A cell over an edge between near and far is not stretched from the
    // near thing to the far one: it lies flat at the far depth, behind the
    // near thing's outline (all its corners decide alike).
    float z00 = view_z(float2(at) / size);
    float z10 = view_z(float2(at + uint2(1, 0)) / size);
    float z01 = view_z(float2(at + uint2(0, 1)) / size);
    float z11 = view_z(float2(at + uint2(1, 1)) / size);
    float lo = min(min(z00, z10), min(z01, z11)), hi = max(max(z00, z10), max(z01, z11));
    float z = hi > lo * p1.z ? hi : view_z(uv);
    float2 ndc = float2(uv.x * 2.0 - 1.0, 1.0 - uv.y * 2.0);
    o.pos = placed(float3(ndc.x * p0.x * z, ndc.y * p0.y * z, -z) * p0.w);
    return o;
}

float4 ps(V v) : SV_Target {
    return float4(colour.Sample(linear_clamp, v.uv).rgb, 1.0);
}
"#;

/// The root constants, as the shader has them.
#[repr(C)]
#[derive(Clone, Copy, Default)]
struct Constants {
    viewproj: [[f32; 4]; 4],
    world: M34,
    p0: [f32; 4],
    p1: [f32; 4],
    p2: [f32; 4],
}

const _: () = assert!(size_of::<Constants>() == 40 * 4);

/// A rigid transform's inverse.
fn inverse(m: &M34) -> M34 {
    let (x, y, z, at) = (axis(m, 0), axis(m, 1), axis(m, 2), place(m));
    let r = [[x[0], x[1], x[2]], [y[0], y[1], y[2]], [z[0], z[1], z[2]]];
    let t = [-(r[0][0] * at[0] + r[0][1] * at[1] + r[0][2] * at[2]), -(r[1][0] * at[0] + r[1][1] * at[1] + r[1][2] * at[2]), -(r[2][0] * at[0] + r[2][1] * at[1] + r[2][2] * at[2])];
    [[r[0][0], r[0][1], r[0][2], t[0]], [r[1][0], r[1][1], r[1][2], t[1]], [r[2][0], r[2][1], r[2][2], t[2]]]
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

struct Renderer {
    queue: ID3D12CommandQueue,
    allocators: [ID3D12CommandAllocator; FRAMES],
    list: ID3D12GraphicsCommandList,
    fence: ID3D12Fence,
    event: HANDLE,
    values: [u64; FRAMES],
    value: u64,
    root: ID3D12RootSignature,
    pso: ID3D12PipelineState,
    views: ID3D12DescriptorHeap,
    view_step: usize,
    rtvs: ID3D12DescriptorHeap,
    rtv_step: usize,
    dsvs: ID3D12DescriptorHeap,
    eyes: [ID3D12Resource; 2],
    _depth: ID3D12Resource,
    size: (u32, u32),
    projections: [[[f32; 4]; 4]; 2],
    eye_to_head: [M34; 2],
}

// SAFETY: free-threaded D3D12 objects and an event handle, used by the one
// thread that owns the renderer.
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

impl Renderer {
    fn new(host: &Host, api: &Api) -> windows::core::Result<Self> {
        let device = &host.device;
        let (mut w, mut h) = (0u32, 0u32);
        // SAFETY: the tables' entries by their types; out values ours.
        let (projections, eye_to_head) = unsafe {
            api.system::<RecommendedSize>(system::RECOMMENDED_SIZE)(&mut w, &mut h);
            let mut projections = [[[0.0; 4]; 4]; 2];
            let mut eye_to_head = [IDENTITY; 2];
            for eye in 0..2 {
                let (mut l, mut r, mut t, mut b) = (0.0, 0.0, 0.0, 0.0);
                api.system::<ProjectionRaw>(system::PROJECTION_RAW)(eye as i32, &mut l, &mut r, &mut t, &mut b);
                projections[eye] = projection(l, r, t, b, EYE_NEAR);
                eye_to_head[eye] = api.system::<EyeToHead>(system::EYE_TO_HEAD)(eye as i32).m;
            }
            (projections, eye_to_head)
        };
        let most = super::number("VICTORY_VR_EYE_MOST", 2400.0).clamp(512.0, 4096.0) as u32;
        let shrink = (w.max(h) as f32 / most as f32).max(1.0);
        let size = (((w as f32 / shrink) as u32).max(256), ((h as f32 / shrink) as u32).max(256));
        let code = |entry, target| crate::host::compile(SOURCE, entry, target);
        let (vs, ps) = (code(s!("vs"), s!("vs_5_1"))?, code(s!("ps"), s!("ps_5_1"))?);
        // SAFETY: plain object creation; the blobs live through the calls.
        unsafe {
            let queue: ID3D12CommandQueue = device.CreateCommandQueue(&D3D12_COMMAND_QUEUE_DESC {
                Type: D3D12_COMMAND_LIST_TYPE_DIRECT,
                Priority: D3D12_COMMAND_QUEUE_PRIORITY_HIGH.0,
                ..Default::default()
            })?;
            let allocators = [device.CreateCommandAllocator(D3D12_COMMAND_LIST_TYPE_DIRECT)?, device.CreateCommandAllocator(D3D12_COMMAND_LIST_TYPE_DIRECT)?];
            let list: ID3D12GraphicsCommandList = device.CreateCommandList(0, D3D12_COMMAND_LIST_TYPE_DIRECT, &allocators[0], None)?;
            list.Close()?;
            let root: ID3D12RootSignature = device.CreateRootSignature(0, &vs)?;
            let mut desc = D3D12_GRAPHICS_PIPELINE_STATE_DESC {
                pRootSignature: std::mem::transmute_copy(&root),
                VS: D3D12_SHADER_BYTECODE { pShaderBytecode: vs.as_ptr().cast(), BytecodeLength: vs.len() },
                PS: D3D12_SHADER_BYTECODE { pShaderBytecode: ps.as_ptr().cast(), BytecodeLength: ps.len() },
                SampleMask: u32::MAX,
                RasterizerState: D3D12_RASTERIZER_DESC { FillMode: D3D12_FILL_MODE_SOLID, CullMode: D3D12_CULL_MODE_NONE, DepthClipEnable: true.into(), ..Default::default() },
                DepthStencilState: D3D12_DEPTH_STENCIL_DESC {
                    DepthEnable: true.into(),
                    DepthWriteMask: D3D12_DEPTH_WRITE_MASK_ALL,
                    DepthFunc: D3D12_COMPARISON_FUNC_GREATER,
                    ..Default::default()
                },
                PrimitiveTopologyType: D3D12_PRIMITIVE_TOPOLOGY_TYPE_TRIANGLE,
                NumRenderTargets: 1,
                DSVFormat: DEPTH_FORMAT,
                SampleDesc: DXGI_SAMPLE_DESC { Count: 1, Quality: 0 },
                ..Default::default()
            };
            desc.RTVFormats[0] = EYE_FORMAT;
            desc.BlendState.RenderTarget[0].RenderTargetWriteMask = D3D12_COLOR_WRITE_ENABLE_ALL.0 as u8;
            let pso: ID3D12PipelineState = device.CreateGraphicsPipelineState(&desc)?;
            let heap = |kind, count, flags| device.CreateDescriptorHeap::<ID3D12DescriptorHeap>(&D3D12_DESCRIPTOR_HEAP_DESC { Type: kind, NumDescriptors: count, Flags: flags, NodeMask: 0 });
            let views = heap(D3D12_DESCRIPTOR_HEAP_TYPE_CBV_SRV_UAV, (FRAMES * VIEWS) as u32, D3D12_DESCRIPTOR_HEAP_FLAG_SHADER_VISIBLE)?;
            let rtvs = heap(D3D12_DESCRIPTOR_HEAP_TYPE_RTV, 2, D3D12_DESCRIPTOR_HEAP_FLAG_NONE)?;
            let dsvs = heap(D3D12_DESCRIPTOR_HEAP_TYPE_DSV, 1, D3D12_DESCRIPTOR_HEAP_FLAG_NONE)?;
            let rtv_step = device.GetDescriptorHandleIncrementSize(D3D12_DESCRIPTOR_HEAP_TYPE_RTV) as usize;
            let colour_clear = D3D12_CLEAR_VALUE { Format: EYE_FORMAT, Anonymous: D3D12_CLEAR_VALUE_0 { Color: [0.02, 0.02, 0.025, 1.0] } };
            let eyes = [
                texture2d(host, size, EYE_FORMAT, D3D12_RESOURCE_FLAG_ALLOW_RENDER_TARGET, D3D12_RESOURCE_STATE_PIXEL_SHADER_RESOURCE, Some(colour_clear))?,
                texture2d(host, size, EYE_FORMAT, D3D12_RESOURCE_FLAG_ALLOW_RENDER_TARGET, D3D12_RESOURCE_STATE_PIXEL_SHADER_RESOURCE, Some(colour_clear))?,
            ];
            let depth_clear = D3D12_CLEAR_VALUE { Format: DEPTH_FORMAT, Anonymous: D3D12_CLEAR_VALUE_0 { DepthStencil: D3D12_DEPTH_STENCIL_VALUE { Depth: 0.0, Stencil: 0 } } };
            let depth = texture2d(host, size, DEPTH_FORMAT, D3D12_RESOURCE_FLAG_ALLOW_DEPTH_STENCIL, D3D12_RESOURCE_STATE_DEPTH_WRITE, Some(depth_clear))?;
            for (i, eye) in eyes.iter().enumerate() {
                let mut at = rtvs.GetCPUDescriptorHandleForHeapStart();
                at.ptr += i * rtv_step;
                device.CreateRenderTargetView(eye, None, at);
            }
            device.CreateDepthStencilView(&depth, None, dsvs.GetCPUDescriptorHandleForHeapStart());
            tracing::info!(width = size.0, height = size.1, recommended = ?(w, h), "the headset mode: our eyes' pictures are made");
            Ok(Self {
                queue,
                allocators,
                list,
                fence: device.CreateFence(0, D3D12_FENCE_FLAG_NONE)?,
                event: CreateEventW(None, false, false, None)?,
                values: [0; FRAMES],
                value: 0,
                root,
                pso,
                view_step: device.GetDescriptorHandleIncrementSize(D3D12_DESCRIPTOR_HEAP_TYPE_CBV_SRV_UAV) as usize,
                views,
                rtvs,
                rtv_step,
                dsvs,
                eyes,
                _depth: depth,
                size,
                projections,
                eye_to_head,
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
    let fov = super::number("VICTORY_VR_FOV", 50.0).clamp(10.0, 150.0).to_radians();
    let near = super::number("VICTORY_VR_NEAR", 0.1).clamp(0.001, 10.0);
    let edge = super::number("VICTORY_VR_EDGE", 1.08).max(1.001);
    let farthest = super::number("VICTORY_VR_FARTHEST", 400.0).clamp(10.0, 10_000.0);
    let (flat_width, flat_distance) = (super::number("VICTORY_VR_WIDTH", 4.0).clamp(0.5, 20.0), super::number("VICTORY_VR_DISTANCE", 3.0).clamp(0.5, 20.0));
    let mut anchor: Option<M34> = None;
    let sway = std::env::var_os("VICTORY_VR_EMU").is_some_and(|v| v != "0") && std::env::var_os("VICTORY_VR_EMU_SWAY").is_none_or(|v| v != "0");
    let started = Instant::now();
    let mut frames = 0u64;
    let mut missed = 0u64;
    let mut said = Instant::now();
    let mut failures = 0u32;
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
        let mut head = if poses[0].valid { poses[0].at } else { IDENTITY };
        if sway {
            // With emulated controllers, a head that moves by itself: a
            // little to the sides and turning, to see the relief's depth.
            let t = started.elapsed().as_secs_f32();
            let yaw = 0.12 * (t * 0.7).sin();
            let (c, s) = (yaw.cos(), yaw.sin());
            let at = add(place(&head), [0.25 * (t * 0.9).sin(), 0.04 * (t * 1.3).sin(), 0.0]);
            head = from_axes([c, 0.0, -s], [0.0, 1.0, 0.0], [s, 0.0, c], at);
        }
        let mode = super::mode();
        if anchor.is_none() || super::RECENTRE.swap(false, std::sync::atomic::Ordering::Relaxed) {
            anchor = Some(level(&head));
            tracing::info!(?mode, head = ?place(&head), "the headset mode: the view is centred");
        }
        let Some(anchor_at) = anchor else { continue };
        let f = (frames as usize) % FRAMES;
        r.wait(f);
        let value = r.value + 1;
        let snap = super::snap_for_drawing(value, &r.fence);
        if snap.is_none() {
            missed += 1;
        }
        let world_scale = super::world_scale();
        // SAFETY: an allocator whose frame is over; an open list of ours;
        // live resources (the snapshot's are kept until our fence passes
        // `value`, see `snap_for_drawing`).
        unsafe {
            let _ = r.allocators[f].Reset();
            let _ = r.list.Reset(&r.allocators[f], &r.pso);
            let list = &r.list;
            let cpu = r.views.GetCPUDescriptorHandleForHeapStart();
            let gpu = r.views.GetGPUDescriptorHandleForHeapStart();
            let base = f * VIEWS;
            if let Some(snap) = &snap {
                let at = |i: usize| D3D12_CPU_DESCRIPTOR_HANDLE { ptr: cpu.ptr + (base + i) * r.view_step };
                let srv = |format: DXGI_FORMAT| D3D12_SHADER_RESOURCE_VIEW_DESC {
                    Format: format,
                    ViewDimension: D3D12_SRV_DIMENSION_TEXTURE2D,
                    Shader4ComponentMapping: D3D12_DEFAULT_SHADER_4_COMPONENT_MAPPING,
                    Anonymous: D3D12_SHADER_RESOURCE_VIEW_DESC_0 { Texture2D: D3D12_TEX2D_SRV { MipLevels: 1, ..Default::default() } },
                };
                host.device.CreateShaderResourceView(&snap.colour, Some(&srv(snap.colour_format)), at(0));
                let (depth, float_format, uint_format) = match &snap.depth {
                    Some((depth, kind)) => (Some(depth), kind.float_view(), kind.uint_view()),
                    None => (None, DXGI_FORMAT_R32_FLOAT, DXGI_FORMAT_R32_UINT),
                };
                host.device.CreateShaderResourceView(depth, Some(&srv(float_format)), at(1));
                host.device.CreateShaderResourceView(depth, Some(&srv(uint_format)), at(2));
            }
            list.ResourceBarrier(&[
                transition(&r.eyes[0], D3D12_RESOURCE_STATE_PIXEL_SHADER_RESOURCE, D3D12_RESOURCE_STATE_RENDER_TARGET),
                transition(&r.eyes[1], D3D12_RESOURCE_STATE_PIXEL_SHADER_RESOURCE, D3D12_RESOURCE_STATE_RENDER_TARGET),
            ]);
            list.SetGraphicsRootSignature(&r.root);
            list.SetDescriptorHeaps(&[Some(r.views.clone())]);
            list.SetGraphicsRootDescriptorTable(1, D3D12_GPU_DESCRIPTOR_HANDLE { ptr: gpu.ptr + (base * r.view_step) as u64 });
            list.IASetPrimitiveTopology(D3D_PRIMITIVE_TOPOLOGY_TRIANGLELIST);
            list.RSSetViewports(&[D3D12_VIEWPORT { Width: r.size.0 as f32, Height: r.size.1 as f32, MaxDepth: 1.0, ..Default::default() }]);
            list.RSSetScissorRects(&[windows::Win32::Foundation::RECT { left: 0, top: 0, right: r.size.0 as i32, bottom: r.size.1 as i32 }]);
            let dsv = r.dsvs.GetCPUDescriptorHandleForHeapStart();
            for eye in 0..2 {
                let mut rtv = r.rtvs.GetCPUDescriptorHandleForHeapStart();
                rtv.ptr += eye * r.rtv_step;
                list.OMSetRenderTargets(1, Some(&rtv), false, Some(&dsv));
                list.ClearRenderTargetView(rtv, &[0.02, 0.02, 0.025, 1.0], None);
                list.ClearDepthStencilView(dsv, D3D12_CLEAR_FLAG_DEPTH, 0.0, 0, None);
                let Some(snap) = &snap else { continue };
                let view = inverse(&mul(&head, &r.eye_to_head[eye]));
                let tan_y = (fov / 2.0).tan();
                let aspect = snap.aspect;
                let (world, flat, width, distance) = match mode {
                    Mode::Relief => (anchor_at, snap.depth.is_none(), flat_width, flat_distance),
                    Mode::Cinema => (anchor_at, true, flat_width, flat_distance),
                    // Fixed to the head: 110 degrees wide, a little past arm's length.
                    Mode::Immersive => (head, true, 2.0 * 1.2 * 55f32.to_radians().tan(), 1.2),
                };
                let constants = Constants {
                    viewproj: times(&r.projections[eye], &view),
                    world,
                    p0: [tan_y * aspect, tan_y, near, world_scale],
                    p1: [GRID.0 as f32, GRID.1 as f32, edge, if flat { 1.0 } else { 0.0 }],
                    p2: [distance, width, snap.depth.as_ref().map_or(0.0, |(_, kind)| kind.number()), farthest],
                };
                list.SetGraphicsRoot32BitConstants(0, 40, std::ptr::from_ref(&constants).cast(), 0);
                list.DrawInstanced(GRID.0 * GRID.1 * 6, 1, 0, 0);
            }
            list.ResourceBarrier(&[
                transition(&r.eyes[0], D3D12_RESOURCE_STATE_RENDER_TARGET, D3D12_RESOURCE_STATE_PIXEL_SHADER_RESOURCE),
                transition(&r.eyes[1], D3D12_RESOURCE_STATE_RENDER_TARGET, D3D12_RESOURCE_STATE_PIXEL_SHADER_RESOURCE),
            ]);
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
            tracing::info!(frames, without_picture = missed, ?mode, world_scale, head = ?place(&head).map(|v| (v * 100.0).round() / 100.0), forward = ?turn(&head, [0.0, 0.0, -1.0]).map(|v| (v * 100.0).round() / 100.0), "the headset mode: our frames");
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
}
