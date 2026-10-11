//! Windows usage counters, read straight from the kernel totals that Task
//! Manager reports. The PDH `GPU Engine` counter set holds an instance per
//! process and engine, so collecting it costs far more than these few
//! system calls.

use std::mem::{size_of, zeroed};
use std::sync::Arc;
use std::time::{Duration, Instant};

use windows_sys::Wdk::Graphics::Direct3D::{
    D3DKMT_ADAPTERINFO, D3DKMT_ADAPTERREGISTRYINFO, D3DKMT_ADAPTERTYPE, D3DKMT_CLOSEADAPTER,
    D3DKMT_ENUMADAPTERS2, D3DKMT_QUERYADAPTERINFO, D3DKMT_QUERYSTATISTICS,
    D3DKMT_QUERYSTATISTICS_0, D3DKMT_QUERYSTATISTICS_ADAPTER, D3DKMT_QUERYSTATISTICS_NODE,
    D3DKMT_QUERYSTATISTICS_QUERY_NODE, D3DKMT_QUERYSTATISTICS_QUERY_SEGMENT,
    D3DKMT_QUERYSTATISTICS_RESULT, D3DKMT_QUERYSTATISTICS_SEGMENT, D3DKMT_QUERYSTATISTICS_TYPE,
    D3DKMT_SEGMENTSIZEINFO, D3DKMTCloseAdapter, D3DKMTEnumAdapters2, D3DKMTQueryAdapterInfo,
    D3DKMTQueryStatistics, KMTQAITYPE_ADAPTERREGISTRYINFO, KMTQAITYPE_ADAPTERTYPE,
    KMTQAITYPE_GETSEGMENTSIZE, KMTQUERYADAPTERINFOTYPE,
};
use windows_sys::Win32::Foundation::{FILETIME, HANDLE, LUID, NTSTATUS};
use windows_sys::Win32::System::SystemInformation::{
    GetPhysicallyInstalledSystemMemory, GlobalMemoryStatusEx, MEMORYSTATUSEX,
};
use windows_sys::Win32::System::Threading::GetSystemTimes;

mod processes;

use super::{GpuUsage, MemoryUsage, ProcessRoot, Sample};

/// How often to look for a GPU again when none is open, such as after a
/// driver update or an external GPU being attached.
const GPU_RETRY_INTERVAL: Duration = Duration::from_secs(10);
/// `D3DKMT_ADAPTERTYPE` bits.
const ADAPTER_RENDER_SUPPORTED: u32 = 1 << 0;
const ADAPTER_SOFTWARE_DEVICE: u32 = 1 << 2;

pub(super) struct Sampler {
    cpu: Option<CpuTimes>,
    installed_memory: Option<u64>,
    gpu: Option<Gpu>,
    gpu_retry_at: Instant,
    /// Process trees are sampled only while someone asks for them.
    processes: Option<processes::ProcessSampler>,
}

impl Sampler {
    /// Take the baselines that the first sample measures from.
    pub(super) fn new() -> Self {
        Self {
            cpu: CpuTimes::now(),
            installed_memory: installed_memory(),
            gpu: Gpu::open(),
            gpu_retry_at: Instant::now() + GPU_RETRY_INTERVAL,
            processes: None,
        }
    }

    /// Sample system totals, and the process trees under `roots`.
    pub(super) fn sample(&mut self, roots: &[ProcessRoot]) -> Sample {
        let cpu = CpuTimes::now();
        let cpu_usage = self
            .cpu
            .zip(cpu)
            .and_then(|(earlier, now)| now.busy_since(earlier));
        self.cpu = cpu;
        let gpu = self.sample_gpu();
        let trees = if roots.is_empty() {
            self.processes = None;
            Default::default()
        } else {
            self.processes
                .get_or_insert_with(Default::default)
                .sample(roots, self.gpu.as_ref())
        };
        Sample {
            cpu: cpu_usage,
            memory: memory_usage(self.installed_memory),
            gpu,
            trees,
        }
    }

    fn sample_gpu(&mut self) -> Option<GpuUsage> {
        if self.gpu.is_none() && Instant::now() >= self.gpu_retry_at {
            // The first usage arrives one interval after the adapter opens.
            self.gpu = Gpu::open();
            self.gpu_retry_at = Instant::now() + GPU_RETRY_INTERVAL;
            return None;
        }
        let usage = self.gpu.as_mut()?.sample();
        if usage.is_none() {
            // The adapter was removed or restarted; its LUID is stale.
            self.gpu = None;
            self.gpu_retry_at = Instant::now() + GPU_RETRY_INTERVAL;
        }
        usage
    }
}

/// Cumulative processor times in 100ns units, summed over logical processors.
#[derive(Clone, Copy, Debug, PartialEq)]
struct CpuTimes {
    idle: u64,
    kernel: u64,
    user: u64,
}

impl CpuTimes {
    fn now() -> Option<Self> {
        let mut idle = FILETIME::default();
        let mut kernel = FILETIME::default();
        let mut user = FILETIME::default();
        // SAFETY: each pointer is a valid FILETIME for the call to fill.
        let ok = unsafe { GetSystemTimes(&mut idle, &mut kernel, &mut user) } != 0;
        ok.then(|| Self {
            idle: filetime(idle),
            kernel: filetime(kernel),
            user: filetime(user),
        })
    }

    /// Busy share of processor time since `earlier`. Kernel time includes
    /// idle time, so busy time is kernel plus user time, less idle time.
    fn busy_since(self, earlier: Self) -> Option<f32> {
        let total =
            self.kernel.saturating_sub(earlier.kernel) + self.user.saturating_sub(earlier.user);
        let idle = self.idle.saturating_sub(earlier.idle).min(total);
        (total > 0).then(|| ((total - idle) as f64 / total as f64) as f32)
    }
}

fn filetime(time: FILETIME) -> u64 {
    (u64::from(time.dwHighDateTime) << 32) | u64::from(time.dwLowDateTime)
}

fn installed_memory() -> Option<u64> {
    let mut kibibytes = 0;
    // SAFETY: the pointer is a valid u64 for the call to fill.
    let ok = unsafe { GetPhysicallyInstalledSystemMemory(&mut kibibytes) } != 0;
    ok.then_some(kibibytes * 1024)
}

/// Memory in use as Task Manager counts it: usable memory not available.
fn memory_usage(installed: Option<u64>) -> Option<MemoryUsage> {
    let mut status = MEMORYSTATUSEX {
        dwLength: size_of::<MEMORYSTATUSEX>() as u32,
        ..Default::default()
    };
    // SAFETY: the struct is valid for writes and carries its size, as required.
    if unsafe { GlobalMemoryStatusEx(&mut status) } == 0 {
        return None;
    }
    Some(MemoryUsage {
        used: status.ullTotalPhys.saturating_sub(status.ullAvailPhys),
        usable: status.ullTotalPhys,
        // Firmware may not report installed memory, as in some VMs.
        installed: installed.unwrap_or(0).max(status.ullTotalPhys),
    })
}

/// One GPU's engines and dedicated memory segments, by adapter LUID.
struct Gpu {
    luid: LUID,
    name: Arc<str>,
    dedicated_total: u64,
    dedicated_segments: Vec<u32>,
    /// Each engine's cumulative running time, in 100ns units.
    engine_running_times: Vec<i64>,
    sampled_at: Instant,
}

impl Gpu {
    /// Open the hardware render adapter with the most dedicated memory: the
    /// discrete GPU on hybrid laptops. Display-only, compute-only (NPU) and
    /// software adapters are skipped.
    fn open() -> Option<Self> {
        // Ties keep the earlier adapter; the primary GPU enumerates first.
        let adapter = adapters()?.into_iter().reduce(|best, adapter| {
            if adapter.dedicated_total > best.dedicated_total {
                adapter
            } else {
                best
            }
        })?;
        let luid = adapter.luid;
        let result = query_statistics(
            luid,
            std::ptr::null_mut(),
            D3DKMT_QUERYSTATISTICS_ADAPTER,
            |_| {},
        )?;
        // SAFETY: an adapter query fills the adapter information.
        let info = unsafe { result.AdapterInformation };
        let mut dedicated_segments = Vec::new();
        for segment in 0..info.NbSegments {
            let result = query_statistics(
                luid,
                std::ptr::null_mut(),
                D3DKMT_QUERYSTATISTICS_SEGMENT,
                |query| {
                    query.QuerySegment = D3DKMT_QUERYSTATISTICS_QUERY_SEGMENT { SegmentId: segment }
                },
            )?;
            // SAFETY: a segment query fills the segment information.
            // Aperture segments map system memory; the others are on the GPU.
            if unsafe { result.SegmentInformation }.Aperture == 0 {
                dedicated_segments.push(segment);
            }
        }
        let sampled_at = Instant::now();
        let engine_running_times = (0..info.NodeCount)
            .map(|engine| engine_running_time(luid, engine))
            .collect::<Option<_>>()?;
        Some(Self {
            luid,
            name: adapter.name,
            dedicated_total: adapter.dedicated_total,
            dedicated_segments,
            engine_running_times,
            sampled_at,
        })
    }

    /// GPU usage is the busiest engine's share of the elapsed time, as Task
    /// Manager reports it; dedicated memory in use is what is resident in the
    /// GPU's own segments.
    fn sample(&mut self) -> Option<GpuUsage> {
        let now = Instant::now();
        let elapsed = now.duration_since(self.sampled_at).as_nanos() as f64 / 100.0;
        self.sampled_at = now;
        let mut busiest = 0.0f64;
        // Some drivers start a counter below zero; only its increase is meaningful.
        for (engine, previous) in self.engine_running_times.iter_mut().enumerate() {
            let running = engine_running_time(self.luid, engine as u32)?;
            if elapsed > 0.0 {
                busiest = busiest.max(running.saturating_sub(*previous).max(0) as f64 / elapsed);
            }
            *previous = running;
        }
        let mut dedicated_used = 0;
        for &segment in &self.dedicated_segments {
            let result = query_statistics(
                self.luid,
                std::ptr::null_mut(),
                D3DKMT_QUERYSTATISTICS_SEGMENT,
                |query| {
                    query.QuerySegment = D3DKMT_QUERYSTATISTICS_QUERY_SEGMENT { SegmentId: segment }
                },
            )?;
            // SAFETY: a segment query fills the segment information.
            dedicated_used += unsafe { result.SegmentInformation }.BytesResident;
        }
        Some(GpuUsage {
            name: Arc::clone(&self.name),
            utilization: busiest.min(1.0) as f32,
            dedicated_used,
            dedicated_total: self.dedicated_total,
        })
    }
}

fn engine_running_time(luid: LUID, engine: u32) -> Option<i64> {
    let result = query_statistics(
        luid,
        std::ptr::null_mut(),
        D3DKMT_QUERYSTATISTICS_NODE,
        |query| query.QueryNode = D3DKMT_QUERYSTATISTICS_QUERY_NODE { NodeId: engine },
    )?;
    // SAFETY: a node query fills the node information.
    Some(
        unsafe { result.NodeInformation }
            .GlobalInformation
            .RunningTime,
    )
}

/// Query adapter statistics, or one process's when `process` is not null.
fn query_statistics(
    luid: LUID,
    process: HANDLE,
    kind: D3DKMT_QUERYSTATISTICS_TYPE,
    select: impl FnOnce(&mut D3DKMT_QUERYSTATISTICS_0),
) -> Option<D3DKMT_QUERYSTATISTICS_RESULT> {
    // SAFETY: the query is plain C data, for which all zeroes is valid.
    let mut query: D3DKMT_QUERYSTATISTICS = unsafe { zeroed() };
    query.Type = kind;
    query.AdapterLuid = luid;
    query.hProcess = process;
    select(&mut query.Anonymous);
    // The API takes a const pointer but writes the result into the query, so
    // pass a pointer derived from a mutable borrow.
    // SAFETY: the query is initialized and outlives the call.
    let status = unsafe { D3DKMTQueryStatistics((&raw mut query).cast_const()) };
    nt_success(status).then_some(query.QueryResult)
}

struct Adapter {
    luid: LUID,
    name: Arc<str>,
    dedicated_total: u64,
}

/// Hardware render adapters, in the kernel's enumeration order.
fn adapters() -> Option<Vec<Adapter>> {
    let mut enumeration = D3DKMT_ENUMADAPTERS2 {
        NumAdapters: 0,
        pAdapters: std::ptr::null_mut(),
    };
    // SAFETY: with no array, the call only reports the adapter count.
    if !nt_success(unsafe { D3DKMTEnumAdapters2(&mut enumeration) }) {
        return None;
    }
    let mut handles = vec![D3DKMT_ADAPTERINFO::default(); enumeration.NumAdapters as usize];
    enumeration.pAdapters = handles.as_mut_ptr();
    // SAFETY: the array holds NumAdapters entries for the call to fill. An
    // adapter added since the count fails the call; the caller retries later.
    if !nt_success(unsafe { D3DKMTEnumAdapters2(&mut enumeration) }) {
        return None;
    }
    handles.truncate(enumeration.NumAdapters as usize);
    let adapters = handles
        .iter()
        .filter_map(|handle| {
            let adapter = describe_adapter(handle);
            // SAFETY: the enumeration opened this handle, and it is closed once.
            unsafe {
                D3DKMTCloseAdapter(&D3DKMT_CLOSEADAPTER {
                    hAdapter: handle.hAdapter,
                })
            };
            adapter
        })
        .collect();
    Some(adapters)
}

fn describe_adapter(handle: &D3DKMT_ADAPTERINFO) -> Option<Adapter> {
    // SAFETY: each query type fills the struct it is paired with.
    let kind: D3DKMT_ADAPTERTYPE =
        unsafe { query_adapter_info(handle.hAdapter, KMTQAITYPE_ADAPTERTYPE) }?;
    // SAFETY: the union is a bitfield over one u32.
    let flags = unsafe { kind.Anonymous.Value };
    if flags & ADAPTER_RENDER_SUPPORTED == 0 || flags & ADAPTER_SOFTWARE_DEVICE != 0 {
        return None;
    }
    // SAFETY: as above.
    let sizes: D3DKMT_SEGMENTSIZEINFO =
        unsafe { query_adapter_info(handle.hAdapter, KMTQAITYPE_GETSEGMENTSIZE) }?;
    // SAFETY: as above.
    let registry: Option<D3DKMT_ADAPTERREGISTRYINFO> =
        unsafe { query_adapter_info(handle.hAdapter, KMTQAITYPE_ADAPTERREGISTRYINFO) };
    let name = registry
        .map(|registry| utf16_until_nul(&registry.AdapterString))
        .filter(|name| !name.is_empty())
        .unwrap_or_else(|| "GPU".to_string());
    Some(Adapter {
        luid: handle.AdapterLuid,
        name: name.into(),
        dedicated_total: sizes.DedicatedVideoMemorySize,
    })
}

/// # Safety
///
/// `T` must be the plain C struct that `kind` fills.
unsafe fn query_adapter_info<T>(adapter: u32, kind: KMTQUERYADAPTERINFOTYPE) -> Option<T> {
    // SAFETY: the caller guarantees T is plain C data, valid as all zeroes.
    let mut value: T = unsafe { zeroed() };
    let mut query = D3DKMT_QUERYADAPTERINFO {
        hAdapter: adapter,
        Type: kind,
        pPrivateDriverData: (&raw mut value).cast(),
        PrivateDriverDataSize: size_of::<T>() as u32,
    };
    // SAFETY: the buffer is a T of the size passed, and outlives the call.
    nt_success(unsafe { D3DKMTQueryAdapterInfo(&mut query) }).then_some(value)
}

fn utf16_until_nul(text: &[u16]) -> String {
    let len = text
        .iter()
        .position(|&unit| unit == 0)
        .unwrap_or(text.len());
    String::from_utf16_lossy(&text[..len]).trim().to_string()
}

fn nt_success(status: NTSTATUS) -> bool {
    status >= 0
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cpu_busy_share_excludes_idle_time_counted_as_kernel_time() {
        let earlier = CpuTimes {
            idle: 100,
            kernel: 300,
            user: 100,
        };
        let now = CpuTimes {
            idle: 160,
            kernel: 400,
            user: 150,
        };
        // 150 units passed, 60 of them idle.
        assert_eq!(now.busy_since(earlier), Some(0.6));
        assert_eq!(now.busy_since(now), None);
    }

    #[test]
    fn samples_this_machine() {
        let mut sampler = Sampler::new();
        std::thread::sleep(Duration::from_millis(250));
        let started = Instant::now();
        let sample = sampler.sample(&[]);
        let cost = started.elapsed();
        let cpu = sample.cpu.expect("system times are available");
        assert!((0.0..=1.0).contains(&cpu), "{cpu}");
        let memory = sample.memory.expect("memory status is available");
        assert!(
            0 < memory.used && memory.used <= memory.usable && memory.usable <= memory.installed
        );
        // CI machines may have no hardware GPU.
        if let Some(gpu) = &sample.gpu {
            assert!((0.0..=1.0).contains(&gpu.utilization));
        }
        eprintln!("sample took {cost:?}: {sample:?}");
    }
}
