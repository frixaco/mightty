//! Usage of each pane's process tree: its shell and every descendant.
//!
//! One `SystemProcessInformation` snapshot gives every process's parent,
//! creation time, CPU time and private working set, which is what Task
//! Manager shows as a process's memory. GPU engine time needs one cheap
//! query per process and engine.

use std::collections::{HashMap, HashSet};
use std::mem::size_of;
use std::os::windows::io::{AsRawHandle, FromRawHandle, OwnedHandle};
use std::time::Instant;

use windows_sys::Wdk::Graphics::Direct3D::{
    D3DKMT_QUERYSTATISTICS_PROCESS_NODE, D3DKMT_QUERYSTATISTICS_QUERY_NODE,
};
use windows_sys::Wdk::System::SystemInformation::{
    NtQuerySystemInformation, SystemProcessInformation,
};
use windows_sys::Win32::Foundation::{HANDLE, STATUS_INFO_LENGTH_MISMATCH};
use windows_sys::Win32::System::Threading::{OpenProcess, PROCESS_QUERY_INFORMATION};
use windows_sys::Win32::System::WindowsProgramming::SYSTEM_PROCESS_INFORMATION;

use super::{CpuTimes, Gpu, nt_success, query_statistics};
use crate::system_usage::{ProcessRoot, TreeUsage};

/// Room for processes that start between sizing the buffer and filling it.
const SNAPSHOT_HEADROOM: usize = 64 * 1024;
const SNAPSHOT_ATTEMPTS: usize = 4;

/// A process ID with its creation time, since IDs are reused.
type ProcessKey = (u32, u64);

#[derive(Default)]
pub(super) struct ProcessSampler {
    /// u64 storage keeps the snapshot 8-byte aligned.
    buffer: Vec<u64>,
    previous: Option<Previous>,
    /// GPU statistics need a process handle; `None` when it cannot be opened.
    handles: HashMap<ProcessKey, Option<OwnedHandle>>,
}

struct Previous {
    at: Instant,
    system_time: u64,
    processes: HashMap<ProcessKey, Times>,
}

struct Times {
    /// Kernel plus user time, in 100ns units.
    cpu: u64,
    /// Running time per GPU engine; `None` where the query failed.
    engines: Vec<Option<i64>>,
}

struct Entry {
    parent: u32,
    created: u64,
    cpu: u64,
    private_bytes: u64,
}

impl ProcessSampler {
    /// Usage of each root's tree since the previous call. The first call
    /// only takes baselines, so its CPU and GPU shares are unknown.
    pub(super) fn sample(
        &mut self,
        roots: &[ProcessRoot],
        gpu: Option<&Gpu>,
    ) -> HashMap<ProcessRoot, TreeUsage> {
        let entries = self.snapshot();
        let system_time = CpuTimes::now().map(|times| times.kernel + times.user);
        let (Some(entries), Some(system_time)) = (entries, system_time) else {
            self.previous = None;
            return HashMap::new();
        };
        let at = Instant::now();
        let previous = self.previous.take();
        let children = children(&entries);
        let engine_count = gpu.map_or(0, |gpu| gpu.engine_running_times.len());
        let mut processes = HashMap::new();
        let mut trees = HashMap::new();
        for &root in roots {
            let Some(members) = tree(root, &entries, &children) else {
                continue;
            };
            let mut usage = TreeUsage {
                cpu: None,
                memory: 0,
                engines: Vec::new(),
                processes: members.len(),
            };
            let mut cpu = 0;
            let mut engines = vec![0i64; engine_count];
            for pid in members {
                let entry = &entries[&pid];
                let key = (pid, entry.created);
                let times = Times {
                    cpu: entry.cpu,
                    engines: gpu.map_or_else(Vec::new, |gpu| self.engine_times(key, gpu)),
                };
                usage.memory += entry.private_bytes;
                // A process missing from the previous sample started since,
                // so all of its time is new.
                let earlier = previous.as_ref().and_then(|p| p.processes.get(&key));
                cpu += times
                    .cpu
                    .saturating_sub(earlier.map_or(0, |earlier| earlier.cpu));
                for (engine, (total, now)) in engines.iter_mut().zip(&times.engines).enumerate() {
                    let before = match earlier {
                        Some(earlier) => earlier.engines.get(engine).copied().flatten(),
                        None => Some(0),
                    };
                    // Skip engines without both readings; some drivers start
                    // a counter below zero, so only increases count.
                    if let (Some(now), Some(before)) = (now, before) {
                        *total += now.saturating_sub(before).max(0);
                    }
                }
                processes.insert(key, times);
            }
            if let Some(previous) = &previous {
                let system = system_time.saturating_sub(previous.system_time);
                let elapsed = at.duration_since(previous.at).as_nanos() as f64 / 100.0;
                usage.cpu = (system > 0).then(|| (cpu as f64 / system as f64).min(1.0) as f32);
                if elapsed > 0.0 {
                    usage.engines = engines
                        .iter()
                        .map(|&time| (time as f64 / elapsed).min(1.0) as f32)
                        .collect();
                }
            }
            trees.insert(root, usage);
        }
        self.handles.retain(|key, _| processes.contains_key(key));
        self.previous = Some(Previous {
            at,
            system_time,
            processes,
        });
        trees
    }

    fn engine_times(&mut self, key: ProcessKey, gpu: &Gpu) -> Vec<Option<i64>> {
        let handle = self.handles.entry(key).or_insert_with(|| {
            // SAFETY: plain call; a null result means the process is gone or
            // belongs to another user or an elevated session.
            let raw = unsafe { OpenProcess(PROCESS_QUERY_INFORMATION, 0, key.0) };
            // SAFETY: a non-null result is a process handle that we own.
            (!raw.is_null()).then(|| unsafe { OwnedHandle::from_raw_handle(raw.cast()) })
        });
        let Some(handle) = handle else {
            return Vec::new();
        };
        let process = handle.as_raw_handle() as HANDLE;
        (0..gpu.engine_running_times.len() as u32)
            .map(|engine| {
                // Processes that never used this GPU have no statistics.
                let result = query_statistics(
                    gpu.luid,
                    process,
                    D3DKMT_QUERYSTATISTICS_PROCESS_NODE,
                    |query| {
                        query.QueryProcessNode =
                            D3DKMT_QUERYSTATISTICS_QUERY_NODE { NodeId: engine }
                    },
                );
                // SAFETY: a process node query fills the process node information.
                result.map(|result| unsafe { result.ProcessNodeInformation }.RunningTime)
            })
            .collect()
    }

    fn snapshot(&mut self) -> Option<HashMap<u32, Entry>> {
        for _ in 0..SNAPSHOT_ATTEMPTS {
            let capacity = self.buffer.len() * size_of::<u64>();
            let mut written = 0;
            // SAFETY: the buffer is writable for `capacity` bytes.
            let status = unsafe {
                NtQuerySystemInformation(
                    SystemProcessInformation,
                    self.buffer.as_mut_ptr().cast(),
                    capacity as u32,
                    &mut written,
                )
            };
            if status == STATUS_INFO_LENGTH_MISMATCH {
                let bytes = written as usize + SNAPSHOT_HEADROOM;
                self.buffer.resize(bytes.div_ceil(size_of::<u64>()), 0);
                continue;
            }
            return nt_success(status).then(|| parse(&self.buffer, written as usize));
        }
        None
    }
}

/// Read a `SystemProcessInformation` snapshot of `len` bytes.
///
/// `Reserved1` holds, in order, the private working set (u64), hard fault
/// count and peak thread count (u32 each), cycle time, creation time, user
/// time and kernel time (u64 each); `Reserved2` holds the parent process ID.
/// This layout has not changed since Windows 7.
fn parse(buffer: &[u64], len: usize) -> HashMap<u32, Entry> {
    let len = len.min(std::mem::size_of_val(buffer));
    let base = buffer.as_ptr().cast::<u8>();
    let mut entries = HashMap::new();
    let mut offset = 0;
    while offset + size_of::<SYSTEM_PROCESS_INFORMATION>() <= len {
        // SAFETY: the entry lies within the written part of the buffer, and
        // an unaligned read of plain C data is always valid.
        let info = unsafe {
            base.add(offset)
                .cast::<SYSTEM_PROCESS_INFORMATION>()
                .read_unaligned()
        };
        let field = |at: usize| {
            u64::from_le_bytes(info.Reserved1[at..at + 8].try_into().expect("8-byte field"))
        };
        entries.insert(
            info.UniqueProcessId as usize as u32,
            Entry {
                parent: info.Reserved2 as usize as u32,
                created: field(24),
                cpu: field(32) + field(40),
                private_bytes: field(0),
            },
        );
        if info.NextEntryOffset == 0 {
            break;
        }
        offset += info.NextEntryOffset as usize;
    }
    entries
}

fn children(entries: &HashMap<u32, Entry>) -> HashMap<u32, Vec<u32>> {
    let mut children: HashMap<u32, Vec<u32>> = HashMap::new();
    for (&pid, entry) in entries {
        if pid != entry.parent {
            children.entry(entry.parent).or_default().push(pid);
        }
    }
    children
}

/// The root and its descendants, or `None` when the root has exited.
fn tree(
    root: ProcessRoot,
    entries: &HashMap<u32, Entry>,
    children: &HashMap<u32, Vec<u32>>,
) -> Option<Vec<u32>> {
    entries
        .get(&root.pid)
        .filter(|entry| entry.created == root.created)?;
    let mut members = vec![root.pid];
    let mut seen = HashSet::from([root.pid]);
    let mut index = 0;
    while let Some(&pid) = members.get(index) {
        index += 1;
        let created = entries[&pid].created;
        for &child in children.get(&pid).into_iter().flatten() {
            // A parent's ID can be reused after it exits; real children
            // start after their parent.
            if entries[&child].created >= created && seen.insert(child) {
                members.push(child);
            }
        }
    }
    Some(members)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(parent: u32, created: u64) -> Entry {
        Entry {
            parent,
            created,
            cpu: 0,
            private_bytes: 0,
        }
    }

    #[test]
    fn trees_follow_parents_but_not_reused_ids() {
        let entries = HashMap::from([
            (10, entry(1, 100)),
            (11, entry(10, 110)),
            (12, entry(11, 120)),
            // Started before process 10, so its parent was an earlier process 10.
            (13, entry(10, 50)),
            (20, entry(1, 100)),
        ]);
        let children = children(&entries);
        let root = |pid, created| ProcessRoot { pid, created };
        let mut members = tree(root(10, 100), &entries, &children).unwrap();
        members.sort();
        assert_eq!(members, [10, 11, 12]);
        assert_eq!(tree(root(10, 99), &entries, &children), None);
        assert_eq!(tree(root(30, 100), &entries, &children), None);
    }

    #[test]
    fn samples_this_process_tree() {
        let mut sampler = ProcessSampler::default();
        let entries = sampler.snapshot().expect("process snapshot");
        let me = &entries[&std::process::id()];
        let root = ProcessRoot {
            pid: std::process::id(),
            created: me.created,
        };
        assert!(me.private_bytes > 0);
        let gpu = Gpu::open();
        let first = sampler.sample(&[root], gpu.as_ref());
        assert_eq!(first[&root].cpu, None);
        let started = Instant::now();
        while started.elapsed().as_millis() < 200 {
            std::hint::black_box(0u64.wrapping_add(1));
        }
        let second = sampler.sample(&[root], gpu.as_ref());
        let cpu = second[&root].cpu.expect("CPU share after a baseline");
        assert!(cpu > 0.0 && cpu <= 1.0, "{cpu}");
        assert!(second[&root].memory > 0);
    }
}
