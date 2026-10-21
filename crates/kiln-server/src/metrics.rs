use std::{
    collections::HashMap,
    fs,
    path::{Path, PathBuf},
    thread,
    time::{Duration, Instant},
};

#[derive(Clone, Copy, Debug, Default, PartialEq, serde::Deserialize, serde::Serialize)]
pub struct Snapshot {
    pub cpu_percent: Option<f64>,
    pub memory_bytes: Option<u64>,
    pub disk_bytes: Option<u64>,
}

#[derive(Clone, Debug)]
pub struct Paths {
    proc: PathBuf,
}

impl Paths {
    pub fn live() -> Self {
        Self {
            proc: PathBuf::from("/proc"),
        }
    }

    #[cfg(test)]
    pub fn fixture(proc: PathBuf) -> Self {
        Self { proc }
    }
}

#[derive(Clone, Copy, Debug, Default)]
struct CpuSample {
    total: u64,
    idle: u64,
}

pub struct Sampler {
    paths: Paths,
    host_cpu: Option<(Instant, CpuSample)>,
    box_cpu: HashMap<String, (Instant, u64)>,
}

impl Sampler {
    pub fn new(paths: Paths) -> Self {
        Self {
            paths,
            host_cpu: None,
            box_cpu: HashMap::new(),
        }
    }

    pub fn prime(&mut self) {
        if let Some(sample) = read_host_cpu(&self.paths.proc) {
            self.host_cpu = Some((Instant::now(), sample));
        }
    }

    pub fn host(&mut self) -> Snapshot {
        Snapshot {
            cpu_percent: self.host_cpu_percent(),
            memory_bytes: read_host_memory(&self.paths.proc),
            disk_bytes: None,
        }
    }

    pub fn box_metrics(&mut self, cgroup: Option<&Path>, active: bool) -> Snapshot {
        if !active {
            return Snapshot::default();
        }
        if cgroup.is_none() {
            return Snapshot::default();
        }
        let cgroup = cgroup.unwrap();
        Snapshot {
            cpu_percent: self.box_cpu_percent(cgroup),
            memory_bytes: read_cgroup_memory(cgroup),
            disk_bytes: None,
        }
    }

    fn host_cpu_percent(&mut self) -> Option<f64> {
        const WAIT: Duration = Duration::from_millis(10);
        const TRIES: usize = 10;

        for _ in 0..TRIES {
            let sample = read_host_cpu(&self.paths.proc)?;
            let now = Instant::now();
            let Some((previous_at, previous)) = self.host_cpu else {
                self.host_cpu = Some((now, sample));
                return None;
            };
            if let Some(pct) = cpu_percent(
                previous.total,
                previous.idle,
                sample.total,
                sample.idle,
                previous_at,
                now,
            ) {
                self.host_cpu = Some((now, sample));
                return Some(pct);
            }
            thread::sleep(WAIT);
        }
        None
    }

    fn box_cpu_percent(&mut self, cgroup: &Path) -> Option<f64> {
        let usage = read_cpu_usage_usec(cgroup)?;
        let key = cgroup.to_string_lossy().into_owned();
        let now = Instant::now();
        let (previous_at, previous_usec) = self.box_cpu.insert(key, (now, usage))?;
        let elapsed = now.duration_since(previous_at).as_secs_f64();
        if elapsed <= 0.0 {
            return None;
        }
        let delta = usage.saturating_sub(previous_usec);
        Some((delta as f64 / 1_000_000.0) / elapsed * 100.0)
    }
}

fn read_host_cpu(proc: &Path) -> Option<CpuSample> {
    let line = fs::read_to_string(proc.join("stat")).ok()?;
    parse_proc_stat(line.lines().next()?)
}

fn parse_proc_stat(line: &str) -> Option<CpuSample> {
    let mut fields = line.split_whitespace();
    if fields.next()? != "cpu" {
        return None;
    }
    let values = fields
        .map(|value| value.parse::<u64>().ok())
        .collect::<Option<Vec<_>>>()?;
    if values.len() < 4 {
        return None;
    }
    let idle = values[3] + values.get(4).copied().unwrap_or(0);
    Some(CpuSample {
        total: values.iter().sum(),
        idle,
    })
}

fn cpu_percent(
    previous_total: u64,
    previous_idle: u64,
    total: u64,
    idle: u64,
    previous_at: Instant,
    now: Instant,
) -> Option<f64> {
    let elapsed = now.duration_since(previous_at).as_secs_f64();
    if elapsed <= 0.0 {
        return None;
    }
    let total_delta = total.saturating_sub(previous_total);
    if total_delta == 0 {
        return None;
    }
    let idle_delta = idle.saturating_sub(previous_idle);
    let busy = total_delta.saturating_sub(idle_delta);
    Some(busy as f64 * 100.0 / total_delta as f64)
}

fn read_host_memory(proc: &Path) -> Option<u64> {
    let mut total_kb = None;
    let mut available_kb = None;
    for line in fs::read_to_string(proc.join("meminfo")).ok()?.lines() {
        if let Some(value) = line.strip_prefix("MemTotal:") {
            total_kb = Some(parse_kb(value)?);
        } else if let Some(value) = line.strip_prefix("MemAvailable:") {
            available_kb = Some(parse_kb(value)?);
        }
    }
    let total = total_kb?;
    let available = available_kb?;
    (total - available).checked_mul(1024)
}

fn parse_kb(value: &str) -> Option<u64> {
    value.split_whitespace().next()?.parse().ok()
}

fn read_cpu_usage_usec(cgroup: &Path) -> Option<u64> {
    for line in fs::read_to_string(cgroup.join("cpu.stat")).ok()?.lines() {
        let (key, value) = line.split_once(' ')?;
        if key == "usage_usec" {
            return value.parse().ok();
        }
    }
    None
}

fn read_cgroup_memory(cgroup: &Path) -> Option<u64> {
    fs::read_to_string(cgroup.join("memory.current"))
        .ok()?
        .trim()
        .parse()
        .ok()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{thread, time::Duration};

    fn write_fixture(root: &Path) {
        fs::create_dir_all(root.join("proc")).unwrap();
        fs::create_dir_all(root.join("cgroup/kiln/run1")).unwrap();
        fs::write(root.join("proc/stat"), "cpu  100 0 100 1000 0 0 0 0 0 0\n").unwrap();
        fs::write(
            root.join("proc/meminfo"),
            "MemTotal:       1000 kB\nMemAvailable:    400 kB\n",
        )
        .unwrap();
        fs::write(
            root.join("cgroup/kiln/run1/cpu.stat"),
            "usage_usec 1000000\n",
        )
        .unwrap();
        fs::write(root.join("cgroup/kiln/run1/memory.current"), "52428800\n").unwrap();
    }

    #[test]
    fn first_host_snapshot_after_prime_has_cpu() {
        let mut sampler = Sampler::new(Paths::live());
        sampler.prime();
        assert!(sampler.host().cpu_percent.is_some());
    }

    #[test]
    fn host_cpu_waits_for_proc_stat_delta() {
        let root = tempfile::tempdir().unwrap();
        write_fixture(root.path());
        let stat_path = root.path().join("proc/stat");
        let delayed = stat_path.clone();
        let waiter = thread::spawn(move || {
            thread::sleep(Duration::from_millis(15));
            fs::write(&delayed, "cpu  200 0 200 1100 0 0 0 0 0 0\n").unwrap();
        });
        let paths = Paths::fixture(root.path().join("proc"));
        let mut sampler = Sampler::new(paths);
        sampler.prime();
        let host = sampler.host();
        waiter.join().unwrap();
        assert!(host.cpu_percent.is_some(), "{host:?}");
    }

    #[test]
    fn host_memory_and_cpu_use_fixture_proc() {
        let root = tempfile::tempdir().unwrap();
        write_fixture(root.path());
        let paths = Paths::fixture(root.path().join("proc"));
        let mut sampler = Sampler::new(paths);
        sampler.prime();
        thread::sleep(Duration::from_millis(5));
        fs::write(
            root.path().join("proc/stat"),
            "cpu  200 0 200 1100 0 0 0 0 0 0\n",
        )
        .unwrap();
        let host = sampler.host();
        assert_eq!(host.memory_bytes, Some(614_400));
        let cpu = host.cpu_percent.unwrap();
        assert!(cpu > 0.0 && cpu <= 100.0, "{cpu}");

        let cgroup = root.path().join("cgroup/kiln/run1");
        let idle = sampler.box_metrics(Some(&cgroup), false);
        assert_eq!(idle.cpu_percent, None);
        assert_eq!(idle.memory_bytes, None);

        sampler.box_metrics(Some(&cgroup), true);
        thread::sleep(Duration::from_millis(5));
        fs::write(
            root.path().join("cgroup/kiln/run1/cpu.stat"),
            "usage_usec 2000000\n",
        )
        .unwrap();
        let active = sampler.box_metrics(Some(&cgroup), true);
        assert_eq!(active.memory_bytes, Some(52_428_800));
        assert!(active.cpu_percent.unwrap() > 0.0);
    }
}
