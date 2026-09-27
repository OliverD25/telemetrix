//! GPU readings the way Task Manager takes them: the adapter statistics of
//! the Windows display kernel (D3DKMT functions in gdi32.dll). Every GPU
//! with a WDDM driver answers, NVIDIA, AMD and Intel alike, and gdi32 is
//! already loaded, so this costs almost no memory. NVML adds about 24 MB.
//!
//! - Load: each engine (node) reports its running time. The load is the
//!   busiest engine's share of the time between two readings, as in Task
//!   Manager's GPU column.
//! - Memory: bytes resident in the dedicated (non-aperture) segments, of
//!   the dedicated video memory size.
//! - Temperature: the adapter performance data (WDDM 2.4 and newer), in
//!   tenths of a degree. Drivers that do not fill it in report 0.
//! - Power is left out: the performance data gives it only as a share of
//!   the adapter's limit, not in watts.

use std::ffi::c_void;
use std::time::Instant;

use windows_sys::Wdk::Graphics::Direct3D::{
    D3DKMT_ADAPTER_PERFDATA, D3DKMT_ADAPTERINFO, D3DKMT_ADAPTERREGISTRYINFO, D3DKMT_ADAPTERTYPE,
    D3DKMT_CLOSEADAPTER, D3DKMT_ENUMADAPTERS2, D3DKMT_QUERYADAPTERINFO, D3DKMT_QUERYSTATISTICS,
    D3DKMT_QUERYSTATISTICS_ADAPTER, D3DKMT_QUERYSTATISTICS_NODE, D3DKMT_QUERYSTATISTICS_RESULT,
    D3DKMT_QUERYSTATISTICS_SEGMENT, D3DKMT_QUERYSTATISTICS_TYPE, D3DKMT_SEGMENTSIZEINFO,
    D3DKMTCloseAdapter, D3DKMTEnumAdapters2, D3DKMTQueryAdapterInfo, D3DKMTQueryStatistics,
    KMTQAITYPE_ADAPTERPERFDATA, KMTQAITYPE_ADAPTERREGISTRYINFO, KMTQAITYPE_ADAPTERTYPE,
    KMTQAITYPE_GETSEGMENTSIZE, KMTQUERYADAPTERINFOTYPE,
};
use windows_sys::Win32::Foundation::LUID;

use super::GpuMetric;

/// Bits of `D3DKMT_ADAPTERTYPE`.
const RENDER_SUPPORTED: u32 = 1 << 0;
const SOFTWARE_DEVICE: u32 = 1 << 2;
/// Running times count in 100 ns steps.
const TICKS_PER_SECOND: f64 = 10_000_000.0;

struct Adapter {
    handle: u32,
    luid: LUID,
    name: String,
    nodes: u32,
    segments: u32,
    dedicated: u64,
    /// The engines' running times at the last reading.
    last: Option<(Instant, Vec<i64>)>,
}

/// The GPUs Windows knows about, opened once.
pub struct D3dkmt {
    adapters: Vec<Adapter>,
}

/// `D3DKMTQueryAdapterInfo` into a plain struct.
fn query<T: Copy>(handle: u32, kind: KMTQUERYADAPTERINFOTYPE, mut out: T) -> Option<T> {
    let mut q = D3DKMT_QUERYADAPTERINFO {
        hAdapter: handle,
        Type: kind,
        pPrivateDriverData: (&raw mut out).cast::<c_void>(),
        PrivateDriverDataSize: size_of::<T>() as u32,
    };
    // SAFETY: the buffer is `out`, with its size.
    (unsafe { D3DKMTQueryAdapterInfo(&mut q) } == 0).then_some(out)
}

/// `D3DKMTQueryStatistics` for one adapter, node or segment.
fn statistics(
    luid: LUID,
    kind: D3DKMT_QUERYSTATISTICS_TYPE,
    id: u32,
) -> Option<D3DKMT_QUERYSTATISTICS_RESULT> {
    let mut q = D3DKMT_QUERYSTATISTICS {
        Type: kind,
        AdapterLuid: luid,
        ..Default::default()
    };
    // The node and segment queries share this first field.
    q.Anonymous.QueryNode.NodeId = id;
    // SAFETY: a zeroed query with its type, adapter and id set.
    (unsafe { D3DKMTQueryStatistics(&q) } == 0).then_some(q.QueryResult)
}

fn close(handle: u32) {
    let c = D3DKMT_CLOSEADAPTER { hAdapter: handle };
    // SAFETY: a handle from D3DKMTEnumAdapters2, closed once.
    unsafe { D3DKMTCloseAdapter(&c) };
}

fn wide_text(w: &[u16]) -> String {
    let end = w.iter().position(|&c| c == 0).unwrap_or(w.len());
    String::from_utf16_lossy(&w[..end]).trim().to_string()
}

/// The load from two readings of the engines' running times: the busiest
/// engine's share of the elapsed time, 0..100.
pub fn load_pct(before: &[i64], after: &[i64], seconds: f64) -> Option<f32> {
    if seconds <= 0.0 || before.len() != after.len() || after.is_empty() {
        return None;
    }
    let busiest = before
        .iter()
        .zip(after)
        .map(|(b, a)| (a - b).max(0))
        .max()
        .unwrap_or(0);
    Some((busiest as f64 / (seconds * TICKS_PER_SECOND) * 100.0).clamp(0.0, 100.0) as f32)
}

impl D3dkmt {
    /// Opens every hardware GPU that renders; the software renderer
    /// ("Microsoft Basic Render Driver") and display-only adapters are left
    /// out. `Err` when there is none.
    pub fn load() -> Result<Self, String> {
        let mut e = D3DKMT_ENUMADAPTERS2 {
            NumAdapters: 0,
            pAdapters: std::ptr::null_mut(),
        };
        // SAFETY: a null list asks for the count.
        if unsafe { D3DKMTEnumAdapters2(&mut e) } != 0 {
            return Err("D3DKMTEnumAdapters2 failed".into());
        }
        let mut list = vec![D3DKMT_ADAPTERINFO::default(); e.NumAdapters as usize];
        e.pAdapters = list.as_mut_ptr();
        // SAFETY: the list holds NumAdapters entries.
        if unsafe { D3DKMTEnumAdapters2(&mut e) } != 0 {
            return Err("D3DKMTEnumAdapters2 failed".into());
        }
        list.truncate(e.NumAdapters as usize);
        let mut adapters = Vec::new();
        for info in list {
            let h = info.hAdapter;
            let kind = query(h, KMTQAITYPE_ADAPTERTYPE, D3DKMT_ADAPTERTYPE::default())
                // SAFETY: every bit pattern is a valid u32.
                .map_or(0, |t| unsafe { t.Anonymous.Value });
            let stats = statistics(info.AdapterLuid, D3DKMT_QUERYSTATISTICS_ADAPTER, 0);
            let (Some(stats), true) = (
                stats,
                kind & RENDER_SUPPORTED != 0 && kind & SOFTWARE_DEVICE == 0,
            ) else {
                close(h);
                continue;
            };
            // SAFETY: the adapter query fills AdapterInformation.
            let a = unsafe { stats.AdapterInformation };
            // SAFETY: plain data; all zeros is a valid start.
            let registry: D3DKMT_ADAPTERREGISTRYINFO = unsafe { std::mem::zeroed() };
            let name = query(h, KMTQAITYPE_ADAPTERREGISTRYINFO, registry)
                .map(|r| wide_text(&r.AdapterString))
                .filter(|n| !n.is_empty())
                .unwrap_or_else(|| format!("GPU {}", adapters.len()));
            let dedicated = query(
                h,
                KMTQAITYPE_GETSEGMENTSIZE,
                D3DKMT_SEGMENTSIZEINFO::default(),
            )
            .map_or(0, |s| s.DedicatedVideoMemorySize);
            adapters.push(Adapter {
                handle: h,
                luid: info.AdapterLuid,
                name,
                nodes: a.NodeCount,
                segments: a.NbSegments,
                dedicated,
                last: None,
            });
        }
        if adapters.is_empty() {
            return Err("Windows reports no hardware GPU".into());
        }
        // The GPU with the most video memory is the main one: it comes first.
        adapters.sort_by_key(|a| std::cmp::Reverse(a.dedicated));
        Ok(Self { adapters })
    }

    pub fn names(&self) -> Vec<&str> {
        self.adapters.iter().map(|a| a.name.as_str()).collect()
    }

    /// One reading per GPU. The load needs two readings, so the first one
    /// after `load` has none.
    pub fn read(&mut self) -> Vec<GpuMetric> {
        let now = Instant::now();
        self.adapters
            .iter_mut()
            .map(|a| {
                let times: Vec<i64> = (0..a.nodes)
                    .map(|n| {
                        statistics(a.luid, D3DKMT_QUERYSTATISTICS_NODE, n)
                            // SAFETY: the node query fills NodeInformation.
                            .map_or(0, |r| unsafe {
                                r.NodeInformation.GlobalInformation.RunningTime
                            })
                    })
                    .collect();
                let usage = a.last.as_ref().and_then(|(at, before)| {
                    load_pct(before, &times, now.duration_since(*at).as_secs_f64())
                });
                a.last = Some((now, times));
                let used: u64 = (0..a.segments)
                    .filter_map(|s| statistics(a.luid, D3DKMT_QUERYSTATISTICS_SEGMENT, s))
                    // SAFETY: the segment query fills SegmentInformation.
                    .map(|r| unsafe { r.SegmentInformation })
                    .filter(|s| s.Aperture == 0)
                    .map(|s| s.BytesResident)
                    .sum();
                let perf = query(
                    a.handle,
                    KMTQAITYPE_ADAPTERPERFDATA,
                    D3DKMT_ADAPTER_PERFDATA::default(),
                );
                let temp = perf
                    .map(|p| p.Temperature)
                    .filter(|t| *t > 0)
                    .map(|t| t as f32 / 10.0);
                GpuMetric {
                    name: a.name.clone(),
                    usage_pct: usage,
                    mem_used_bytes: (a.dedicated > 0).then_some(used),
                    mem_total_bytes: (a.dedicated > 0).then_some(a.dedicated),
                    temp_c: temp,
                    power_w: None,
                }
            })
            .collect()
    }
}

impl Drop for D3dkmt {
    fn drop(&mut self) {
        for a in &self.adapters {
            close(a.handle);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn load_is_the_busiest_engine() {
        // 1 s apart: engine 2 ran 0.25 s, engine 1 0.05 s.
        let before = [0, 1_000_000, 5_000_000];
        let after = [0, 1_500_000, 7_500_000];
        assert_eq!(load_pct(&before, &after, 1.0), Some(25.0));
        assert_eq!(load_pct(&before, &after, 0.5), Some(50.0));
        assert_eq!(load_pct(&[0], &[30_000_000], 1.0), Some(100.0), "capped");
        assert_eq!(load_pct(&[9], &[3], 1.0), Some(0.0), "a counter reset");
        assert_eq!(load_pct(&[], &[], 1.0), None);
        assert_eq!(load_pct(&[0], &[0, 1], 1.0), None);
        assert_eq!(load_pct(&[0], &[1], 0.0), None);
    }

    #[test]
    fn reading_never_panics() {
        // A machine without a GPU driver (a CI runner) has at least the
        // software renderer, which is left out, so both answers are fine.
        match D3dkmt::load() {
            Ok(mut gpus) => {
                let first = gpus.read();
                assert_eq!(first.len(), gpus.names().len());
                assert!(first.iter().all(|g| g.usage_pct.is_none()));
                std::thread::sleep(std::time::Duration::from_millis(100));
                for g in gpus.read() {
                    assert!(!g.name.is_empty());
                    assert!(g.usage_pct.is_some_and(|u| (0.0..=100.0).contains(&u)));
                }
            }
            Err(reason) => assert!(!reason.is_empty()),
        }
    }
}
