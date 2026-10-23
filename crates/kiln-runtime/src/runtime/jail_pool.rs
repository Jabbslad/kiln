//! Unclaimed jailer generations live under `{state}/jail-pool/{pending,ready}/{run}`.
//! Claim renames `ready/{run}` into `boxes/{box_id}/{run}`. Shutdown and the next
//! arm reap only generations this module can prove it created. Claimed rows have
//! no `slot.json` and are not signaled.
//!
//! Jailer argv is frozen at exec. A rename of `--chroot-base-dir` keeps the
//! directory inode, which is what `process::Identity` compares. If jailer 1.17
//! re-opens that path after the API socket exists, this shape is wrong.

use super::{BoxRecord, Runtime, id};
use crate::{
    Error, Result,
    firecracker::Client,
    isolation::{self, Config, JailIdentity},
    network::Topology,
    process::{self, FileIdentity, Identity},
    storage,
};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::{
    fs::{self, File},
    os::fd::AsRawFd,
    os::unix::{fs::OpenOptionsExt, process::CommandExt},
    path::{Path, PathBuf},
    process::{Child, Command, Stdio},
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
    },
    thread::JoinHandle,
    time::{Duration, Instant},
};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Shape {
    pub memory_mib: u32,
    pub vcpus: u8,
    pub networked: bool,
}

impl Shape {
    pub fn new(memory_mib: u32, vcpus: u8, networked: bool) -> Result<Self> {
        if !isolation::launch_limits(memory_mib, vcpus) {
            return Err(Error::Invalid(
                "invalid jail path or resource limits".into(),
            ));
        }
        Ok(Self {
            memory_mib,
            vcpus,
            networked,
        })
    }
}

#[derive(Debug)]
#[must_use]
pub(super) enum Launch {
    Cold,
    Pooled(PooledLaunch),
}

#[derive(Debug)]
#[must_use]
pub(super) struct PooledLaunch {
    run: String,
    process: Identity,
    uid: u32,
    gid: u32,
}

impl PooledLaunch {
    pub(super) fn matches(&self, record: &BoxRecord) -> Result<()> {
        let process = record
            .process
            .as_ref()
            .ok_or_else(|| Error::Invalid("pooled launch is missing its process".into()))?;
        let jail = record
            .jail
            .as_ref()
            .ok_or_else(|| Error::Invalid("pooled launch is missing its jail identity".into()))?;
        if record.run != self.run
            || process.pid != self.process.pid
            || process.start_ticks != self.process.start_ticks
            || jail.uid != self.uid
            || jail.gid != self.gid
        {
            return Err(Error::Invalid(
                "pooled launch does not match the reserved box".into(),
            ));
        }
        Ok(())
    }

    pub(super) fn pid(&self) -> u32 {
        self.process.pid
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum LauncherKind {
    Jailer,
    #[cfg(test)]
    Stub,
}

fn stub_launcher(launcher: LauncherKind) -> bool {
    match launcher {
        LauncherKind::Jailer => false,
        #[cfg(test)]
        LauncherKind::Stub => true,
    }
}

struct Control {
    size: u8,
    shape: Option<Shape>,
    isolation: Option<Config>,
    launcher: LauncherKind,
    refill: Option<JoinHandle<()>>,
}

pub(super) struct Pool {
    root: PathBuf,
    stop: AtomicBool,
    control: Mutex<Control>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct SlotFile {
    schema_version: u32,
    run: String,
    uid: u32,
    gid: u32,
    memory_mib: u32,
    vcpus: u8,
    network_slot: Option<u8>,
    process: Option<Identity>,
}

impl SlotFile {
    fn identity(&self) -> JailIdentity {
        JailIdentity {
            uid: self.uid,
            gid: self.gid,
        }
    }
}

pub(super) struct Claimed {
    armed: bool,
    run: String,
    slot: SlotFile,
    generation: PathBuf,
    process: Identity,
}

impl Drop for Claimed {
    fn drop(&mut self) {
        if !self.armed {
            return;
        }
        self.armed = false;
        let _ = process::terminate_with_successor(&self.process, None);
        let _ = fs::remove_dir_all(&self.generation);
    }
}

struct Holds {
    idents: Vec<JailIdentity>,
    slots: Vec<u8>,
}

pub(super) fn in_child_cgroup(text: &str, parent: &str, run: &str) -> bool {
    let expected = format!("0::/{parent}/{run}");
    text.lines().any(|line| line.trim() == expected)
}

impl Pool {
    pub(super) fn new(root: PathBuf) -> Self {
        Self {
            root,
            stop: AtomicBool::new(false),
            control: Mutex::new(Control {
                size: 0,
                shape: None,
                isolation: None,
                launcher: LauncherKind::Jailer,
                refill: None,
            }),
        }
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, Control> {
        self.control
            .lock()
            .unwrap_or_else(|poison| poison.into_inner())
    }

    fn arm(
        self: &Arc<Self>,
        isolation: Option<Config>,
        size: u8,
        shape: Option<Shape>,
        launcher: LauncherKind,
        handle: Option<tokio::runtime::Handle>,
    ) -> Result<()> {
        if size > 8 {
            return Err(Error::Invalid("jail pool size must be 0..=8".into()));
        }
        if size > 0 && isolation.is_none() {
            return Err(Error::Invalid(
                "jail pool requires an isolated runtime".into(),
            ));
        }
        if size > 0
            && shape.is_some_and(|shape| shape.networked)
            && isolation
                .as_ref()
                .and_then(|config| config.network.as_ref())
                .is_none()
        {
            return Err(Error::Invalid(
                "jail pool network shape requires an isolation network helper".into(),
            ));
        }
        if launcher == LauncherKind::Jailer && size > 0 && handle.is_none() {
            return Err(Error::Invalid(
                "jail pool refill must start on the async runtime".into(),
            ));
        }
        self.stop.store(true, Ordering::SeqCst);
        let previous = {
            let mut control = self.lock();
            control.refill.take()
        };
        if let Some(thread) = previous {
            let _ = thread.join();
        }
        self.reap(isolation.as_ref())?;
        self.stop.store(false, Ordering::SeqCst);
        let mut control = self.lock();
        control.size = size;
        control.shape = shape.filter(|_| size > 0);
        control.isolation = isolation;
        control.launcher = launcher;
        if size == 0 || shape.is_none() {
            return Ok(());
        }
        let pool = Arc::clone(self);
        control.refill = Some(
            std::thread::Builder::new()
                .name("jail-pool".into())
                .spawn(move || pool.refill_loop(handle))?,
        );
        Ok(())
    }

    pub(super) fn shutdown(&self) -> Result<()> {
        self.stop.store(true, Ordering::SeqCst);
        let previous = {
            let mut control = self.lock();
            control.size = 0;
            control.refill.take()
        };
        if let Some(thread) = previous {
            let _ = thread.join();
        }
        let isolation = self.lock().isolation.clone();
        self.reap(isolation.as_ref())
    }

    fn held(&self) -> Result<Holds> {
        let mut holds = Holds {
            idents: Vec::new(),
            slots: Vec::new(),
        };
        for area in ["pending", "ready"] {
            self.collect_area(&self.root.join("jail-pool").join(area), &mut holds)?;
        }
        let boxes = self.root.join("boxes");
        if boxes.exists() {
            for entry in fs::read_dir(&boxes)? {
                let box_dir = entry?.path();
                if !is_private_dir(&box_dir) {
                    continue;
                }
                for generation in fs::read_dir(&box_dir)? {
                    let generation = generation?.path();
                    if generation.join("slot.json").exists() {
                        let slot = read_slot(&generation)?;
                        holds.idents.push(slot.identity());
                        if let Some(network) = slot.network_slot {
                            holds.slots.push(network);
                        }
                    }
                }
            }
        }
        Ok(holds)
    }

    fn collect_area(&self, dir: &Path, holds: &mut Holds) -> Result<()> {
        if !dir.exists() {
            return Ok(());
        }
        for entry in fs::read_dir(dir)? {
            let path = entry?.path();
            if path.join("slot.json").exists() {
                let slot = read_slot(&path)?;
                holds.idents.push(slot.identity());
                if let Some(network) = slot.network_slot {
                    holds.slots.push(network);
                }
            }
        }
        Ok(())
    }

    pub(super) fn claim(
        &self,
        box_dir: &Path,
        memory_mib: u32,
        vcpus: u8,
        networked: bool,
    ) -> Result<Option<Claimed>> {
        // The caller already holds the state-directory flock. This mutex is
        // the inner lock, so claim must not take the flock itself.
        let guard = self.lock();
        let isolation = guard.isolation.clone();
        let ready = self.root.join("jail-pool").join("ready");
        if !ready.exists() {
            return Ok(None);
        }
        let mut runs = Vec::new();
        for entry in fs::read_dir(&ready)? {
            let path = entry?.path();
            if let Some(name) = path.file_name().and_then(|name| name.to_str())
                && storage::valid_id(name)
            {
                runs.push(name.to_string());
            }
        }
        runs.sort();
        for run in runs {
            let generation = ready.join(&run);
            if !generation.join("slot.json").exists() {
                continue;
            }
            let slot = match read_slot(&generation) {
                Ok(slot) => slot,
                Err(_) => {
                    destroy_generation(isolation.as_ref(), &generation)?;
                    continue;
                }
            };
            if slot.memory_mib != memory_mib
                || slot.vcpus != vcpus
                || slot.network_slot.is_some() != networked
                || slot.run != run
            {
                continue;
            }
            let Some(process) = slot.process.clone() else {
                destroy_generation(isolation.as_ref(), &generation)?;
                continue;
            };
            if !still_same(&process, &generation) {
                destroy_generation(isolation.as_ref(), &generation)?;
                continue;
            }
            let dest = box_dir.join(&run);
            if dest.exists() {
                return Err(Error::Invalid("jail generation destination exists".into()));
            }
            fs::rename(&generation, &dest)?;
            if !still_same(&process, &dest) {
                destroy_generation(isolation.as_ref(), &dest)?;
                continue;
            }
            return Ok(Some(Claimed {
                armed: true,
                run,
                slot,
                generation: dest,
                process,
            }));
        }
        drop(guard);
        Ok(None)
    }

    /// Destroy one unclaimed generation so a reservation that would have fit
    /// with the pool off still fits. Pending before ready. At most the caller
    /// retries once.
    pub(super) fn release_one(&self) -> Result<bool> {
        let isolation = self.lock().isolation.clone();
        for area in ["pending", "ready"] {
            let dir = self.root.join("jail-pool").join(area);
            if !dir.exists() {
                continue;
            }
            let mut runs = Vec::new();
            for entry in fs::read_dir(&dir)? {
                if let Some(name) = entry?.file_name().to_str()
                    && storage::valid_id(name)
                {
                    runs.push(name.to_string());
                }
            }
            runs.sort();
            if let Some(run) = runs.into_iter().next() {
                destroy_generation(isolation.as_ref(), &dir.join(run))?;
                return Ok(true);
            }
        }
        Ok(false)
    }

    fn reap(&self, isolation: Option<&Config>) -> Result<()> {
        for area in ["pending", "ready"] {
            let dir = self.root.join("jail-pool").join(area);
            if !dir.exists() {
                continue;
            }
            let mut paths = Vec::new();
            for entry in fs::read_dir(&dir)? {
                paths.push(entry?.path());
            }
            for path in paths {
                if is_private_dir(&path) {
                    destroy_generation(isolation, &path)?;
                }
            }
        }
        let boxes = self.root.join("boxes");
        if !boxes.exists() {
            return Ok(());
        }
        for entry in fs::read_dir(&boxes)? {
            let box_dir = entry?.path();
            if !is_private_dir(&box_dir) {
                continue;
            }
            let mut generations = Vec::new();
            for generation in fs::read_dir(&box_dir)? {
                generations.push(generation?.path());
            }
            for generation in generations {
                if !generation.join("slot.json").exists() {
                    continue;
                }
                let slot = read_slot(&generation)?;
                if record_owns(&box_dir.join("box.json"), &slot)? {
                    fs::remove_file(generation.join("slot.json"))?;
                } else {
                    destroy_generation(isolation, &generation)?;
                }
            }
        }
        Ok(())
    }

    fn refill_loop(self: &Arc<Self>, handle: Option<tokio::runtime::Handle>) {
        while !self.stop.load(Ordering::SeqCst) {
            if self.refill_once(handle.as_ref()).is_err() {
                std::thread::sleep(Duration::from_millis(50));
            }
        }
    }

    fn refill_once(self: &Arc<Self>, handle: Option<&tokio::runtime::Handle>) -> Result<()> {
        if self.stop.load(Ordering::SeqCst) {
            return Ok(());
        }
        let Some(file_lock) = try_lock(&self.root) else {
            std::thread::sleep(Duration::from_millis(20));
            return Ok(());
        };
        let decision = {
            let control = self.lock();
            if self.stop.load(Ordering::SeqCst) {
                None
            } else if let Some(shape) = control.shape {
                let ready = count_area(&self.root.join("jail-pool").join("ready"))?;
                let pending = count_area(&self.root.join("jail-pool").join("pending"))?;
                if ready + pending >= usize::from(control.size) {
                    None
                } else {
                    Some((shape, control.launcher, control.isolation.clone()))
                }
            } else {
                None
            }
        };
        let Some((shape, launcher, isolation)) = decision else {
            drop(file_lock);
            std::thread::sleep(Duration::from_millis(20));
            return Ok(());
        };
        let Some(config) = isolation else {
            return Ok(());
        };
        let reserved = match reserve_pending(&self.root, &config, shape) {
            Ok(reserved) => reserved,
            Err(_) => {
                drop(file_lock);
                std::thread::sleep(Duration::from_millis(50));
                return Ok(());
            }
        };
        drop(file_lock);
        let spawned = spawn_jailed(
            &config,
            launcher,
            &reserved.generation,
            &reserved.run,
            &reserved.identity,
            shape,
            reserved.network_slot,
        );
        let mut child = match spawned {
            Ok(child) => child,
            Err(_) => {
                let _ = destroy_generation(Some(&config), &reserved.generation);
                return Ok(());
            }
        };
        let pid = child.id();
        let process = match process::identify(pid) {
            Ok(process) => process,
            Err(error) => {
                let _ = child.kill();
                let _ = child.wait();
                let _ = destroy_generation(Some(&config), &reserved.generation);
                return Err(error);
            }
        };
        if !write_process(&reserved.generation, &process)? {
            let _ = child.kill();
            let _ = child.wait();
            let _ = destroy_generation(Some(&config), &reserved.generation);
            return Ok(());
        }
        let ready = wait_ready(launcher, handle, &reserved.jail_root, &mut child);
        if self.stop.load(Ordering::SeqCst) || ready.is_err() || !reserved.generation.exists() {
            let _ = child.kill();
            let _ = child.wait();
            if reserved.generation.exists() {
                let _ = destroy_generation(Some(&config), &reserved.generation);
            }
            return Ok(());
        }
        let process = ready?;
        if !write_process(&reserved.generation, &process)? {
            let _ = child.kill();
            let _ = child.wait();
            let _ = destroy_generation(Some(&config), &reserved.generation);
            return Ok(());
        }
        if launcher == LauncherKind::Jailer {
            let text = fs::read_to_string(format!("/proc/{}/cgroup", process.pid))?;
            if !in_child_cgroup(&text, &config.cgroup_parent, &reserved.run) {
                let _ = child.kill();
                let _ = child.wait();
                let _ = destroy_generation(Some(&config), &reserved.generation);
                return Err(Error::Invalid(
                    "pooled jailer is not in its child cgroup".into(),
                ));
            }
        }
        let Some(file_lock) = try_lock(&self.root) else {
            let _ = child.kill();
            let _ = child.wait();
            let _ = destroy_generation(Some(&config), &reserved.generation);
            return Ok(());
        };
        // Flock then mutex, and keep the flock across the rename so a create
        // cannot allocate this uid between the listing and the publish.
        let guard = self.lock();
        if self.stop.load(Ordering::SeqCst) || !reserved.generation.exists() {
            let _ = child.kill();
            let _ = child.wait();
            if reserved.generation.exists() {
                let _ = destroy_generation(Some(&config), &reserved.generation);
            }
            return Ok(());
        }
        let ready_dir = self.root.join("jail-pool").join("ready");
        storage::private_dir(&ready_dir)?;
        let dest = ready_dir.join(&reserved.run);
        fs::rename(&reserved.generation, &dest)?;
        drop(guard);
        drop(file_lock);
        std::thread::Builder::new()
            .name("vmm-reaper".into())
            .spawn(move || {
                let _ = child.wait();
            })?;
        Ok(())
    }
}

struct Reserved {
    run: String,
    generation: PathBuf,
    jail_root: PathBuf,
    identity: JailIdentity,
    network_slot: Option<u8>,
}

fn reserve_pending(root: &Path, config: &Config, shape: Shape) -> Result<Reserved> {
    let holds = holds_of(root)?;
    let mut idents = box_idents(root)?;
    idents.extend(holds.idents);
    let identity = config.allocate(&idents)?;
    let network_slot = if shape.networked {
        let mut slots = box_slots(root)?;
        slots.extend(holds.slots);
        Some(
            (0..8)
                .find(|slot| !slots.contains(slot))
                .ok_or_else(|| Error::Invalid("network slot pool exhausted".into()))?,
        )
    } else {
        None
    };
    let run = id()?;
    let generation = root.join("jail-pool").join("pending").join(&run);
    storage::private_dir(&root.join("jail-pool"))?;
    storage::private_dir(&root.join("jail-pool").join("pending"))?;
    storage::private_dir(&generation)?;
    let jail_root = generation.join("firecracker").join(&run).join("root");
    storage::private_dir(&generation.join("firecracker"))?;
    storage::private_dir(&generation.join("firecracker").join(&run))?;
    storage::private_dir(&jail_root)?;
    if let Some(slot) = network_slot {
        let network = config.network.as_ref().ok_or_else(|| {
            Error::Invalid("jail pool network shape requires an isolation network helper".into())
        })?;
        let topology = Topology::allocate(&network.namespace_scope, slot)?;
        network.helper("prepare", &run, &topology, identity.uid)?;
    }
    let slot = SlotFile {
        schema_version: 1,
        run: run.clone(),
        uid: identity.uid,
        gid: identity.gid,
        memory_mib: shape.memory_mib,
        vcpus: shape.vcpus,
        network_slot,
        process: None,
    };
    storage::atomic_json(&generation.join("slot.json"), &slot)?;
    Ok(Reserved {
        run,
        generation,
        jail_root,
        identity,
        network_slot,
    })
}

fn holds_of(root: &Path) -> Result<Holds> {
    let pool = Pool::new(root.to_path_buf());
    pool.held()
}

fn box_idents(root: &Path) -> Result<Vec<JailIdentity>> {
    Ok(box_rows(root)?
        .into_iter()
        .filter_map(|row| row.jail)
        .collect())
}

fn box_slots(root: &Path) -> Result<Vec<u8>> {
    Ok(box_rows(root)?
        .into_iter()
        .filter_map(|row| row.network.map(|network| network.slot))
        .collect())
}

#[derive(Deserialize)]
struct BoxHold {
    #[serde(default)]
    jail: Option<JailIdentity>,
    #[serde(default)]
    network: Option<Topology>,
}

fn box_rows(root: &Path) -> Result<Vec<BoxHold>> {
    let mut rows = Vec::new();
    let boxes = root.join("boxes");
    if !boxes.exists() {
        return Ok(rows);
    }
    for entry in fs::read_dir(&boxes)? {
        let path = entry?.path();
        let Some(name) = path.file_name().and_then(|name| name.to_str()) else {
            continue;
        };
        if !storage::valid_id(name) {
            continue;
        }
        let record = path.join("box.json");
        if record.exists() {
            rows.push(storage::read_json(&record)?);
        }
    }
    Ok(rows)
}

fn write_process(generation: &Path, process: &Identity) -> Result<bool> {
    let path = generation.join("slot.json");
    if !path.exists() {
        return Ok(false);
    }
    let mut slot = read_slot(generation)?;
    slot.process = Some(process.clone());
    storage::atomic_json(&path, &slot)?;
    Ok(true)
}

fn spawn_jailed(
    config: &Config,
    launcher: LauncherKind,
    generation: &Path,
    run: &str,
    identity: &JailIdentity,
    shape: Shape,
    network_slot: Option<u8>,
) -> Result<Child> {
    let jail_root = generation.join("firecracker").join(run).join("root");
    let log = generation.join("console.log");
    if stub_launcher(launcher) {
        return Command::new("/bin/sleep")
            .arg("120")
            .current_dir(generation)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .map_err(Into::into);
    }
    let topology = match network_slot {
        Some(slot) => {
            let network = config.network.as_ref().ok_or_else(|| {
                Error::Invalid(
                    "jail pool network shape requires an isolation network helper".into(),
                )
            })?;
            Some(Topology::allocate(&network.namespace_scope, slot)?)
        }
        None => None,
    };
    spawn_cold(
        config,
        generation,
        run,
        identity,
        shape.memory_mib,
        shape.vcpus,
        topology.as_ref(),
        &jail_root,
        &log,
    )
}

fn network_path(path: &Path) -> Result<&str> {
    path.to_str()
        .ok_or_else(|| Error::Invalid("non-UTF-8 jailer path".into()))
}

fn wait_ready(
    launcher: LauncherKind,
    handle: Option<&tokio::runtime::Handle>,
    jail_root: &Path,
    child: &mut Child,
) -> Result<Identity> {
    if stub_launcher(launcher) {
        return process::identify(child.id());
    }
    let handle = handle
        .ok_or_else(|| Error::Invalid("jail pool refill must start on the async runtime".into()))?;
    let directory = File::open(jail_root)?;
    let socket = PathBuf::from(format!("/proc/self/fd/{}/api.sock", directory.as_raw_fd()));
    let client = Client::new(&socket, Duration::from_secs(30))?;
    let pid = child.id();
    let deadline = Instant::now() + Duration::from_secs(15);
    handle.block_on(async {
        loop {
            if client.request("GET", "/", Value::Null).await.is_ok() {
                return process::identify(pid);
            }
            if Instant::now() >= deadline || process::identify(pid).is_err() {
                return Err(Error::Invalid("pooled jailer did not become ready".into()));
            }
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    })
}

#[allow(clippy::too_many_arguments)]
pub(super) fn spawn_cold(
    config: &Config,
    generation: &Path,
    run: &str,
    identity: &JailIdentity,
    memory_mib: u32,
    vcpus: u8,
    network: Option<&Topology>,
    cwd: &Path,
    log: &Path,
) -> Result<Child> {
    fs::create_dir(config.cgroup(run)?)?;
    let args = config.args(generation, run, identity, memory_mib, vcpus)?;
    let mut command = if let Some(topology) = network {
        let helper = config.network.as_ref().ok_or_else(|| {
            Error::Invalid("jail pool network shape requires an isolation network helper".into())
        })?;
        let mut command = Command::new(&helper.helper);
        command.args([
            "run",
            run,
            &topology.namespace,
            network_path(&config.jailer)?,
        ]);
        command.env("PATH", "/usr/sbin:/usr/bin:/sbin:/bin");
        command
    } else {
        Command::new(&config.jailer)
    };
    command.args(args);
    command.env_clear();
    command.env("PATH", "/usr/sbin:/usr/bin:/sbin:/bin");
    let output = fs::OpenOptions::new()
        .create(true)
        .append(true)
        .mode(0o600)
        .custom_flags(libc::O_NOFOLLOW)
        .open(log)?;
    command
        .current_dir(cwd)
        .stdin(Stdio::null())
        .stdout(output.try_clone()?)
        .stderr(output);
    let isolated = true;
    unsafe {
        command.pre_exec(move || {
            libc::umask(0o077);
            if isolated && libc::setgroups(0, std::ptr::null()) != 0 {
                return Err(std::io::Error::last_os_error());
            }
            if libc::setsid() < 0 {
                return Err(std::io::Error::last_os_error());
            }
            let limit = libc::rlimit {
                rlim_cur: 5 * 1024 * 1024 * 1024,
                rlim_max: 5 * 1024 * 1024 * 1024,
            };
            if libc::setrlimit(libc::RLIMIT_FSIZE, &limit) != 0 {
                return Err(std::io::Error::last_os_error());
            }
            Ok(())
        });
    }
    command.spawn().map_err(Into::into)
}

fn still_same(process: &Identity, generation: &Path) -> bool {
    let Ok(actual) = process::identify(process.pid) else {
        return false;
    };
    let Some(name) = generation.file_name().and_then(|name| name.to_str()) else {
        return false;
    };
    let jail_root = generation.join("firecracker").join(name).join("root");
    let in_generation = FileIdentity::read(generation)
        .ok()
        .is_some_and(|object| actual.directory_object.as_ref() == Some(&object));
    let in_jail = FileIdentity::read(&jail_root)
        .ok()
        .is_some_and(|object| actual.directory_object.as_ref() == Some(&object));
    if !in_generation && !in_jail {
        return false;
    }
    let successor = jail_root.join("firecracker");
    let successor = successor.exists().then_some(successor.as_path());
    process::same_launch(process, &actual, successor)
}

fn destroy_generation(config: Option<&Config>, generation: &Path) -> Result<()> {
    if generation.join("slot.json").exists() {
        let slot = read_slot(generation)?;
        if let Some(process) = &slot.process
            && process::identify(process.pid).is_ok()
        {
            process::terminate_with_successor(process, None)?;
        }
        if let Some(config) = config {
            if let (Some(network), Some(slot_index)) = (&config.network, slot.network_slot) {
                let topology = Topology::allocate(&network.namespace_scope, slot_index)?;
                network.helper("cleanup", &slot.run, &topology, slot.uid)?;
            }
            config.cleanup_cgroup(&slot.run)?;
        }
    }
    if generation.exists() {
        fs::remove_dir_all(generation)?;
    }
    Ok(())
}

fn record_owns(box_json: &Path, slot: &SlotFile) -> Result<bool> {
    if !box_json.exists() {
        return Ok(false);
    }
    #[derive(Deserialize)]
    struct Owner {
        run: String,
        #[serde(default)]
        jail: Option<JailIdentity>,
        #[serde(default)]
        process: Option<Identity>,
    }
    let owner: Owner = storage::read_json(box_json)?;
    let (Some(jail), Some(process), Some(slot_process)) =
        (owner.jail, owner.process, slot.process.as_ref())
    else {
        return Ok(false);
    };
    Ok(owner.run == slot.run
        && jail.uid == slot.uid
        && jail.gid == slot.gid
        && process.pid == slot_process.pid
        && process.start_ticks == slot_process.start_ticks)
}

fn read_slot(generation: &Path) -> Result<SlotFile> {
    let slot: SlotFile = storage::read_json(&generation.join("slot.json"))?;
    let name = generation
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("");
    if slot.schema_version != 1 || slot.run != name || !storage::valid_id(&slot.run) {
        return Err(Error::Invalid("invalid pooled jail slot".into()));
    }
    Ok(slot)
}

fn is_private_dir(path: &Path) -> bool {
    fs::symlink_metadata(path)
        .map(|metadata| metadata.is_dir() && !metadata.file_type().is_symlink())
        .unwrap_or(false)
}

fn count_area(dir: &Path) -> Result<usize> {
    if !dir.exists() {
        return Ok(0);
    }
    let mut count = 0;
    for entry in fs::read_dir(dir)? {
        let name = entry?.file_name();
        if name.to_str().is_some_and(storage::valid_id) {
            count += 1;
        }
    }
    Ok(count)
}

fn try_lock(root: &Path) -> Option<File> {
    storage::lock(root).ok()
}

impl Runtime {
    pub fn arm_jail_pool(&self, size: u8, shape: Option<(u32, u8, bool)>) -> Result<()> {
        let shape = shape
            .map(|(memory_mib, vcpus, networked)| Shape::new(memory_mib, vcpus, networked))
            .transpose()?;
        self.pool.arm(
            self.isolation.clone(),
            size,
            shape,
            LauncherKind::Jailer,
            tokio::runtime::Handle::try_current().ok(),
        )
    }

    pub fn shutdown_jail_pool(&self) -> Result<()> {
        self.pool.shutdown()
    }

    pub(super) fn claim_template_jail(
        &self,
        box_id: &str,
        memory_mib: u32,
        vcpus: u8,
        networked: bool,
    ) -> Result<Option<Claimed>> {
        self.pool.claim(
            &self.root.join("boxes").join(box_id),
            memory_mib,
            vcpus,
            networked,
        )
    }

    pub(super) fn persist_claim(
        &self,
        mut claimed: Claimed,
        record: &mut BoxRecord,
    ) -> Result<PooledLaunch> {
        record.run = claimed.run.clone();
        record.jail = Some(claimed.slot.identity());
        record.network = match claimed.slot.network_slot {
            Some(slot) => {
                let network = self
                    .isolation
                    .as_ref()
                    .and_then(|config| config.network.as_ref())
                    .ok_or_else(|| {
                        Error::Invalid(
                            "jail pool network shape requires an isolation network helper".into(),
                        )
                    })?;
                Some(Topology::allocate(&network.namespace_scope, slot)?)
            }
            None => None,
        };
        record.process = Some(claimed.process.clone());
        self.save(record)?;
        claimed.armed = false;
        let _ = fs::remove_file(claimed.generation.join("slot.json"));
        Ok(PooledLaunch {
            run: claimed.run.clone(),
            process: claimed.process.clone(),
            uid: claimed.slot.uid,
            gid: claimed.slot.gid,
        })
    }

    pub(super) fn assign_new_box(
        &self,
        records: &[BoxRecord],
        want_network: bool,
    ) -> Result<(Option<JailIdentity>, Option<Topology>)> {
        let mut released = false;
        loop {
            let holds = self.pool.held()?;
            let mut idents: Vec<_> = records
                .iter()
                .filter_map(|record| record.jail.clone())
                .collect();
            idents.extend(holds.idents.iter().cloned());
            let jail = self
                .isolation
                .as_ref()
                .map(|config| config.allocate(&idents))
                .transpose();
            let network = if want_network {
                self.allocate_network_held(records, &holds.slots)
            } else {
                Ok(None)
            };
            let exhausted = match (&jail, &network) {
                (Err(error), _) | (_, Err(error)) => is_exhausted(error),
                _ => false,
            };
            if exhausted && !released && self.pool.release_one()? {
                released = true;
                continue;
            }
            return Ok((jail?, network?));
        }
    }

    fn allocate_network_held(
        &self,
        records: &[BoxRecord],
        held: &[u8],
    ) -> Result<Option<Topology>> {
        let Some(config) = self
            .isolation
            .as_ref()
            .and_then(|config| config.network.as_ref())
        else {
            return Ok(None);
        };
        config.validate()?;
        let slot = (0..8)
            .find(|slot| {
                !held.contains(slot)
                    && !records.iter().any(|record| {
                        record
                            .network
                            .as_ref()
                            .is_some_and(|network| network.slot == *slot)
                    })
            })
            .ok_or_else(|| Error::Invalid("network slot pool exhausted".into()))?;
        Ok(Some(Topology::allocate(&config.namespace_scope, slot)?))
    }
}

fn is_exhausted(error: &Error) -> bool {
    let text = error.to_string();
    text.contains("isolated identity pool exhausted")
        || text.contains("network slot pool exhausted")
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt;

    fn fixture() -> (tempfile::TempDir, Runtime) {
        let root = tempfile::tempdir().unwrap();
        fs::set_permissions(root.path(), fs::Permissions::from_mode(0o700)).unwrap();
        let mut runtime = Runtime::open(root.path()).unwrap();
        runtime.isolation = Some(Config {
            firecracker: "/opt/kiln/firecracker".into(),
            jailer: "/opt/kiln/jailer".into(),
            cgroup_parent: "kiln".into(),
            uid_base: 70000,
            gid_base: 71000,
            disk_backend: isolation::DiskBackend::Copy,
            network: None,
        });
        runtime.pool.lock().isolation = runtime.isolation.clone();
        (root, runtime)
    }

    fn shape() -> Shape {
        Shape::new(128, 1, false).unwrap()
    }

    fn reserve_box(runtime: &Runtime) -> String {
        let box_id = id().unwrap();
        storage::private_dir(&runtime.root.join("boxes").join(&box_id)).unwrap();
        box_id
    }

    fn arm(runtime: &Runtime, size: u8) {
        runtime
            .pool
            .arm(
                runtime.isolation.clone(),
                size,
                Some(shape()),
                LauncherKind::Stub,
                None,
            )
            .unwrap();
    }

    fn wait_ready(runtime: &Runtime, count: usize) {
        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            let ready = count_area(&runtime.root.join("jail-pool").join("ready")).unwrap();
            if ready >= count {
                return;
            }
            assert!(Instant::now() < deadline, "pool did not refill to {count}");
            std::thread::sleep(Duration::from_millis(10));
        }
    }

    struct Guard<'a>(&'a Runtime);
    impl Drop for Guard<'_> {
        fn drop(&mut self) {
            let _ = self.0.shutdown_jail_pool();
        }
    }

    #[test]
    fn child_cgroup_line_is_the_run_and_not_the_parent() {
        assert!(in_child_cgroup("0::/kiln/abcdef\n", "kiln", "abcdef"));
        assert!(!in_child_cgroup("0::/kiln\n", "kiln", "abcdef"));
        assert!(!in_child_cgroup("0::/kiln/abcdefextra\n", "kiln", "abcdef"));
    }

    #[test]
    fn claim_binds_one_ready_jail_and_keeps_the_cwd_inode() {
        let (_root, runtime) = fixture();
        let _guard = Guard(&runtime);
        arm(&runtime, 1);
        wait_ready(&runtime, 1);
        let box_id = reserve_box(&runtime);
        let claimed = runtime
            .claim_template_jail(&box_id, 128, 1, false)
            .unwrap()
            .unwrap();
        let cwd = FileIdentity::read(&claimed.generation).unwrap();
        assert_eq!(claimed.process.directory_object.as_ref(), Some(&cwd));
        let marker = claimed.generation.join("later");
        fs::write(&marker, b"visible").unwrap();
        let via_proc = format!("/proc/{}/cwd/later", claimed.process.pid);
        assert_eq!(fs::read(&via_proc).unwrap(), b"visible");
        assert!(
            !runtime
                .root
                .join("jail-pool")
                .join("ready")
                .join(&claimed.run)
                .exists()
        );
        let pid = claimed.process.pid;
        drop(claimed);
        assert!(process::identify(pid).is_err());
    }

    #[test]
    fn empty_pool_returns_none_and_size_zero_reaps() {
        let (_root, runtime) = fixture();
        let _guard = Guard(&runtime);
        assert!(
            runtime
                .claim_template_jail(&id().unwrap(), 128, 1, false)
                .unwrap()
                .is_none()
        );
        arm(&runtime, 1);
        wait_ready(&runtime, 1);
        runtime.shutdown_jail_pool().unwrap();
        assert_eq!(
            count_area(&runtime.root.join("jail-pool").join("ready")).unwrap(),
            0
        );
        arm(&runtime, 0);
        assert!(
            runtime
                .claim_template_jail(&id().unwrap(), 128, 1, false)
                .unwrap()
                .is_none()
        );
    }

    #[test]
    fn refill_restores_n_after_a_claim() {
        let (_root, runtime) = fixture();
        let _guard = Guard(&runtime);
        arm(&runtime, 2);
        wait_ready(&runtime, 2);
        let box_id = reserve_box(&runtime);
        let claimed = runtime
            .claim_template_jail(&box_id, 128, 1, false)
            .unwrap()
            .unwrap();
        let taken = claimed.run.clone();
        drop(claimed);
        wait_ready(&runtime, 2);
        let ready = runtime.root.join("jail-pool").join("ready");
        assert!(!ready.join(&taken).exists());
    }

    #[test]
    fn shutdown_reaps_unclaimed_and_leaves_a_persisted_process() {
        let (_root, runtime) = fixture();
        let _guard = Guard(&runtime);
        arm(&runtime, 1);
        wait_ready(&runtime, 1);
        let box_id = id().unwrap();
        storage::private_dir(&runtime.root.join("boxes").join(&box_id)).unwrap();
        let mut record = serde_json::from_value(serde_json::json!({
            "schema_version": 1,
            "id": box_id,
            "name": "pooled",
            "state": "creating",
            "run": "0".repeat(32),
            "process": null,
            "image": {"schema_version":1,"architecture":"x86_64","kernel_path":"unused","kernel_sha256":"unused","rootfs_path":"unused","rootfs_sha256":"unused","agent_protocol_version":1},
            "host": "unused",
            "memory_mib": 128,
            "vcpus": 1,
            "source": null,
            "operation": "clone",
            "last_error": null,
            "disk_copy": ""
        }))
        .unwrap();
        let claimed = runtime
            .claim_template_jail(&box_id, 128, 1, false)
            .unwrap()
            .unwrap();
        let pid = claimed.process.pid;
        let launch = runtime.persist_claim(claimed, &mut record).unwrap();
        launch.matches(&record).unwrap();
        runtime.shutdown_jail_pool().unwrap();
        assert!(
            !runtime
                .root
                .join("jail-pool")
                .join("ready")
                .join(&launch.run)
                .exists()
        );
        assert!(
            !runtime
                .root
                .join("jail-pool")
                .join("pending")
                .join(&launch.run)
                .exists()
        );
        assert!(process::identify(pid).is_ok());
        let _ = process::terminate(&launch.process);
    }

    #[test]
    fn two_concurrent_claims_get_different_jails() {
        let (_root, runtime) = fixture();
        let _guard = Guard(&runtime);
        arm(&runtime, 2);
        wait_ready(&runtime, 2);
        let left_id = reserve_box(&runtime);
        let right_id = reserve_box(&runtime);
        let runtime = &runtime;
        let (left, right) = std::thread::scope(|scope| {
            let left = scope.spawn(|| runtime.claim_template_jail(&left_id, 128, 1, false));
            let right = scope.spawn(|| runtime.claim_template_jail(&right_id, 128, 1, false));
            (
                left.join().unwrap().unwrap().unwrap(),
                right.join().unwrap().unwrap().unwrap(),
            )
        });
        assert_ne!(left.run, right.run);
        assert_ne!(left.process.pid, right.process.pid);
        drop(left);
        drop(right);
    }

    #[test]
    fn held_identity_is_visible_to_allocate_and_one_release_frees_a_box() {
        let (_root, runtime) = fixture();
        let config = runtime.isolation.clone().unwrap();
        for slot in 0..8 {
            let run = format!("{slot:032x}");
            let generation = runtime.root.join("jail-pool").join("ready").join(&run);
            storage::private_dir(&runtime.root.join("jail-pool")).unwrap();
            storage::private_dir(&runtime.root.join("jail-pool").join("ready")).unwrap();
            storage::private_dir(&generation).unwrap();
            storage::atomic_json(
                &generation.join("slot.json"),
                &SlotFile {
                    schema_version: 1,
                    run,
                    uid: config.uid_base + slot,
                    gid: config.gid_base + slot,
                    memory_mib: 128,
                    vcpus: 1,
                    network_slot: None,
                    process: None,
                },
            )
            .unwrap();
        }
        let holds = runtime.pool.held().unwrap();
        assert!(config.allocate(&holds.idents).is_err());
        let (jail, network) = runtime.assign_new_box(&[], false).unwrap();
        assert!(network.is_none());
        assert!(jail.is_some());
        assert_eq!(
            count_area(&runtime.root.join("jail-pool").join("ready")).unwrap(),
            7
        );
    }

    #[test]
    fn spawn_bench_reports_p50_and_p90() {
        let report = bench_spawn(11).unwrap();
        println!(
            "jailer-spawn pool off p50={:.3}ms p90={:.3}ms pool on p50={:.3}ms p90={:.3}ms",
            report.off.p50_ms, report.off.p90_ms, report.on.p50_ms, report.on.p90_ms
        );
        assert_eq!(report.off.samples, 11);
        assert_eq!(report.on.samples, 11);
        assert!(report.off.p50_ms.is_finite() && report.on.p90_ms.is_finite());
    }

    struct Side {
        samples: usize,
        p50_ms: f64,
        p90_ms: f64,
    }
    struct Bench {
        off: Side,
        on: Side,
    }

    fn nearest(mut samples: Vec<f64>, percentile: usize) -> f64 {
        samples.sort_by(|left, right| left.partial_cmp(right).unwrap());
        let index = (samples.len() * percentile).div_ceil(100).saturating_sub(1);
        samples[index]
    }

    fn bench_spawn(samples: usize) -> Result<Bench> {
        let mut off = Vec::new();
        for _ in 0..samples {
            let root = tempfile::tempdir().unwrap();
            fs::set_permissions(root.path(), fs::Permissions::from_mode(0o700)).unwrap();
            let started = Instant::now();
            let mut child = Command::new("/bin/sleep")
                .arg("30")
                .current_dir(root.path())
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .spawn()?;
            let _ = process::identify(child.id())?;
            off.push(started.elapsed().as_secs_f64() * 1000.0);
            let _ = child.kill();
            let _ = child.wait();
        }
        let (_root, runtime) = fixture();
        let _guard = Guard(&runtime);
        arm(&runtime, 1);
        let mut on = Vec::new();
        for _ in 0..samples {
            wait_ready(&runtime, 1);
            let box_id = id()?;
            storage::private_dir(&runtime.root.join("boxes").join(&box_id))?;
            let started = Instant::now();
            let claimed = runtime
                .claim_template_jail(&box_id, 128, 1, false)?
                .ok_or_else(|| Error::Invalid("pool was empty".into()))?;
            let _ = process::identify(claimed.process.pid)?;
            on.push(started.elapsed().as_secs_f64() * 1000.0);
            drop(claimed);
        }
        Ok(Bench {
            off: Side {
                samples: off.len(),
                p50_ms: nearest(off.clone(), 50),
                p90_ms: nearest(off, 90),
            },
            on: Side {
                samples: on.len(),
                p50_ms: nearest(on.clone(), 50),
                p90_ms: nearest(on, 90),
            },
        })
    }
}
