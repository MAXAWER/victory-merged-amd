//! (Merged runtime) The title's buffer copies done on the card
//! (`VICTORY_GPU_COPY=1`; off by default).
//!
//! A title buffer copy (`CopyBufferRegion` between two title buffers) is
//! done on the CPU when its list is submitted, from title memory into title
//! memory (`HostList::defer_copy`). With submissions sent ahead of the wait
//! for the card (`crate::host::late`), a copy whose source or destination
//! lies in memory that shaders of a submission in the air may write has to
//! wait for those submissions first: the bytes it reads are not in title
//! memory yet. In the merged runtime's census these copies settled the
//! flights twice a frame (some 16 ms a frame of the submitting thread),
//! every one of them 2 KiB out of a buffer the card had just written.
//!
//! With the switch such a copy is a dispatch of ours in the list, at the
//! title's place for it, between the host copies of the two buffers: the
//! card copies the bytes after the shaders before it wrote them, as on the
//! console. The destination is declared written, as a shader's unordered
//! access is (`Shadow::gpu_writes`): the bytes reach title memory when the
//! submission has run, by the compare that finds what shaders wrote. A copy
//! that does not fit (unaligned, over 16 MiB, a buffer without a host copy
//! that shaders can write) is done on the CPU as before.

use std::sync::atomic::{AtomicU64, Ordering::Relaxed};

use windows::Win32::Graphics::Direct3D::Fxc::D3DCompile;
use windows::Win32::Graphics::Direct3D::ID3DBlob;
use windows::Win32::Graphics::Direct3D12::*;
use windows::core::s;

use crate::host::{HostList, host};

const SOURCE: &str = r#"
#define RS "RootConstants(num32BitConstants=1, b0), UAV(u0), SRV(t0)"
cbuffer Copy : register(b0) {
    uint words;
};
RWByteAddressBuffer dst : register(u0);
ByteAddressBuffer src : register(t0);
[RootSignature(RS)]
[numthreads(64, 1, 1)]
void main(uint3 id : SV_DispatchThreadID) {
    if (id.x < words) {
        dst.Store(id.x * 4, src.Load(id.x * 4));
    }
}
"#;

/// The most bytes one copy dispatch moves (65,535 groups of 64 words).
const MOST: u64 = 16 << 20;

/// Whether title buffer copies go to the card (`VICTORY_GPU_COPY=1`, all
/// that fit; `pending`, only those whose source shaders in the air write).
pub(crate) fn on() -> bool {
    static ON: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *ON.get_or_init(|| std::env::var("VICTORY_GPU_COPY").is_ok_and(|v| v == "1" || v == "pending"))
}

/// `VICTORY_GPU_COPY=pending` ([`on`]).
fn pending_only() -> bool {
    static ON: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *ON.get_or_init(|| std::env::var("VICTORY_GPU_COPY").is_ok_and(|v| v == "pending"))
}

/// Since the last [`take_stats`]: copies done on the card, their bytes;
/// copies left to the CPU.
static STATS: [AtomicU64; 3] = [const { AtomicU64::new(0) }; 3];

/// [`STATS`], taken.
pub(crate) fn take_stats() -> [u64; 3] {
    [0, 1, 2].map(|i| STATS[i].swap(0, Relaxed))
}

struct Copier {
    root: ID3D12RootSignature,
    pso: ID3D12PipelineState,
}

// SAFETY: D3D12 objects are free-threaded.
unsafe impl Send for Copier {}
// SAFETY: as above.
unsafe impl Sync for Copier {}

fn copier() -> Option<&'static Copier> {
    static COPIER: std::sync::OnceLock<Option<Copier>> = std::sync::OnceLock::new();
    COPIER
        .get_or_init(|| match make() {
            Ok(c) => Some(c),
            Err(e) => {
                tracing::warn!(error = %e, "no buffer copy on the card: title buffer copies stay on the CPU");
                None
            }
        })
        .as_ref()
}

fn make() -> windows::core::Result<Copier> {
    let device = &host().ok_or_else(windows::core::Error::empty)?.device;
    let mut code: Option<ID3DBlob> = None;
    let mut errors: Option<ID3DBlob> = None;
    // SAFETY: the source and names live through the call; out pointers ours.
    let compiled = unsafe {
        D3DCompile(
            SOURCE.as_ptr().cast(),
            SOURCE.len(),
            s!("gpucopy"),
            None,
            None,
            s!("main"),
            s!("cs_5_1"),
            0,
            0,
            &mut code,
            Some(&mut errors),
        )
    };
    if let Err(e) = compiled {
        if let Some(errors) = errors {
            // SAFETY: the blob's own bytes.
            let text = unsafe { std::slice::from_raw_parts(errors.GetBufferPointer().cast::<u8>(), errors.GetBufferSize()) };
            tracing::warn!("buffer copy on the card: {}", String::from_utf8_lossy(text));
        }
        return Err(e);
    }
    let code = code.ok_or_else(windows::core::Error::empty)?;
    // SAFETY: the blob's own bytes; the root signature is inside the shader.
    unsafe {
        let bytes = std::slice::from_raw_parts(code.GetBufferPointer().cast::<u8>(), code.GetBufferSize());
        let root: ID3D12RootSignature = device.CreateRootSignature(0, bytes)?;
        let pso = device.CreateComputePipelineState(&D3D12_COMPUTE_PIPELINE_STATE_DESC {
            pRootSignature: std::mem::transmute_copy(&root),
            CS: D3D12_SHADER_BYTECODE {
                pShaderBytecode: bytes.as_ptr().cast(),
                BytecodeLength: bytes.len(),
            },
            ..Default::default()
        })?;
        Ok(Copier { root, pso })
    }
}

fn uav_barrier() -> D3D12_RESOURCE_BARRIER {
    D3D12_RESOURCE_BARRIER {
        Type: D3D12_RESOURCE_BARRIER_TYPE_UAV,
        Flags: D3D12_RESOURCE_BARRIER_FLAG_NONE,
        Anonymous: D3D12_RESOURCE_BARRIER_0 {
            UAV: std::mem::ManuallyDrop::new(D3D12_RESOURCE_UAV_BARRIER {
                pResource: std::mem::ManuallyDrop::new(None),
            }),
        },
    }
}

/// Records the title's copy of `len` bytes from title address `src` to
/// `dst` into `list` at this point, on the card. Whether it was recorded;
/// if not, nothing was, and the caller copies on the CPU as before.
pub(crate) fn record(list: &HostList, state: &mut crate::draw::State, dst: u64, src: u64, len: u64) -> bool {
    const UNORDERED: i32 = D3D12_RESOURCE_FLAG_ALLOW_UNORDERED_ACCESS.0;
    let fits = len > 0 && len <= MOST && len.is_multiple_of(4) && dst.is_multiple_of(4) && src.is_multiple_of(4);
    // Overlapping ranges: the CPU's copy (a memmove) is what the title means.
    let overlap = dst < src + len && src < dst + len;
    let copied = fits && !overlap && (|| {
        let copier = copier()?;
        let d = crate::shadow::find(dst, len)?;
        let s = crate::shadow::find(src, len)?;
        // Both whole in their copies, the destination one shaders can
        // write (a transient window cannot take shader writes).
        if d.left(dst) < len || s.left(src) < len || d.transient() {
            return None;
        }
        // Only a copy whose source shaders of a submission in the air may
        // be writing (`VICTORY_GPU_COPY=pending`): the copies that settled
        // the flights. Any other is done on the CPU at submission, which
        // keeps title memory as fills of upload memory read it (a fill read
        // a destination of a copy on the card before its flight had
        // landed, and fills waited from then on).
        if pending_only() && !s.pending_overlaps(src, len) {
            return None;
        }
        // SAFETY: a live buffer.
        if unsafe { d.buffer.GetDesc() }.Flags.0 & UNORDERED == 0 {
            return None;
        }
        let (dst_gpu, src_gpu) = (d.gpu(dst), s.gpu(src));
        if !dst_gpu.is_multiple_of(4) || !src_gpu.is_multiple_of(4) {
            return None;
        }
        // As a dispatch reads and writes them (`draw.rs` `readable`): the
        // source brought up to date where the title wrote it, the
        // destination too (its snapshot must hold title memory), then
        // declared written.
        list.begin_command();
        s.refresh(list, src, len);
        d.refresh(list, dst, len);
        list.buffer_state(&s.buffer, crate::host::BUFFER_READ);
        list.buffer_state(&d.buffer, crate::host::BUFFER_WRITE);
        d.gpu_writes(list, dst, len);
        let words = (len / 4) as u32;
        // SAFETY: an open list; our root signature and pipeline; live host
        // copies in the states set just above, the list keeps them (the
        // refreshes and the declared write hold them until it has run).
        unsafe {
            // What earlier shaders wrote into the source (or the
            // destination) first, and the copy's bytes before what follows.
            list.list.ResourceBarrier(&[uav_barrier()]);
            list.list.SetComputeRootSignature(&copier.root);
            list.list.SetPipelineState(&copier.pso);
            list.list.SetComputeRoot32BitConstants(0, 1, std::ptr::from_ref(&words).cast(), 0);
            list.list.SetComputeRootUnorderedAccessView(1, dst_gpu);
            list.list.SetComputeRootShaderResourceView(2, src_gpu);
            list.list.Dispatch(words.div_ceil(64), 1, 1);
            list.list.ResourceBarrier(&[uav_barrier()]);
        }
        list.keep(&s.buffer);
        list.keep(&d.buffer);
        // Our pipeline and root signature are bound now: the title's go
        // back before its next command.
        state.forget_host();
        Some(())
    })()
    .is_some();
    if copied {
        STATS[0].fetch_add(1, Relaxed);
        STATS[1].fetch_add(len, Relaxed);
    } else {
        STATS[2].fetch_add(1, Relaxed);
    }
    copied
}
