use crate::Result;
use serde::{Deserialize, Serialize};
use std::{
    fs::{self, File, OpenOptions},
    io::Write,
    net::SocketAddr,
    path::{Path, PathBuf},
};
use ting_core::{
    hash::hex,
    protocol::{Encode, Genesis},
};

/// A fixed per-user default; the working directory never selects a node.
pub fn default_directory() -> Result<PathBuf> {
    let variable = if cfg!(windows) { "USERPROFILE" } else { "HOME" };
    let home = std::env::var_os(variable)
        .filter(|value| !value.is_empty())
        .map(PathBuf::from)
        .filter(|path| path.is_absolute())
        .ok_or_else(|| format!("cannot locate your home directory: set {variable} to an absolute path or use --data-dir DIRECTORY"))?;
    Ok(home.join(".ting"))
}

fn read_config(dir: &Path) -> Result<Vec<u8>> {
    let path = dir.join("config.json");
    fs::read(&path).map_err(|e| {
        if e.kind() == std::io::ErrorKind::NotFound {
            format!(
                "node is not initialized at {}. Run: ting --data-dir \"{}\" init",
                dir.display(),
                dir.display()
            )
        } else {
            format!("cannot read {}: {e}", path.display())
        }
    })
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Config {
    pub version: u32,
    pub name: String,
    pub control: SocketAddr,
    pub p2p: SocketAddr,
    pub api: SocketAddr,
    pub api_enabled: bool,
    /// Answer browser applications from any origin, such as a wallet in a web page.
    #[serde(default)]
    pub api_cors: bool,
    pub peers: Vec<SocketAddr>,
    /// Find more peers through the configured ones: learn the addresses
    /// they share and keep connections to up to `max_peers` peers in all.
    #[serde(default = "yes")]
    pub discovery: bool,
    #[serde(default = "eight")]
    pub max_peers: u32,
    pub votes: [u8; 4],
    /// The built-in CPU miner: whether it runs from start, with how many
    /// threads, and the least number of seconds between its own blocks.
    #[serde(default)]
    pub miner_enabled: bool,
    #[serde(default = "one")]
    pub miner_threads: u32,
    #[serde(default)]
    pub miner_spacing: u64,
    /// Producer data for the blocks this node builds, hex; none when unset.
    /// Programs read it: a coin pays its fees and rewards to the account it
    /// names.
    #[serde(default)]
    pub producer_hex: Option<String>,
    /// Canonical blocks whose undo data the node keeps; 0 keeps everything
    /// (wiki: node/storage).
    #[serde(default = "thirty_days")]
    pub undo_window: u64,
    /// The database's page cache, in MiB (wiki: node/storage).
    #[serde(default = "cache_mib")]
    pub db_cache_mib: u64,
    /// Most transactions the pool holds (wiki: node/mining).
    #[serde(default = "pool_transactions")]
    pub mempool_transactions: usize,
    /// Most MiB of encoded transactions the pool holds; at least the data of
    /// four blocks at the next block's capacity, whatever this says.
    #[serde(default = "pool_mib")]
    pub mempool_mib: u64,
}
fn pool_transactions() -> usize {
    1_024
}
fn pool_mib() -> u64 {
    16
}
fn thirty_days() -> u64 {
    43_200
}
fn cache_mib() -> u64 {
    crate::store::CACHE_BYTES >> 20
}
fn yes() -> bool {
    true
}
fn eight() -> u32 {
    8
}
fn one() -> u32 {
    1
}
/// Producer data may be anything a block can hold; this keeps it sensible.
pub const PRODUCER_MAX: usize = 4096;
impl Default for Config {
    fn default() -> Self {
        Self {
            version: 1,
            name: "Ting node".into(),
            control: "127.0.0.1:18440".parse().unwrap(),
            p2p: "127.0.0.1:18442".parse().unwrap(),
            api: "127.0.0.1:18441".parse().unwrap(),
            api_enabled: false,
            api_cors: false,
            peers: vec![],
            discovery: true,
            max_peers: 8,
            votes: [0; 4],
            miner_enabled: false,
            miner_threads: 1,
            miner_spacing: 0,
            producer_hex: None,
            undo_window: thirty_days(),
            db_cache_mib: cache_mib(),
            mempool_transactions: pool_transactions(),
            mempool_mib: pool_mib(),
        }
    }
}
impl Config {
    pub fn validate(&self) -> Result<()> {
        if self.version != 1
            || !self.control.ip().is_loopback()
            || self.name.len() > 128
            || self.peers.len() > 32
            || !(1..=64).contains(&self.max_peers)
            || self.votes.iter().any(|v| *v > 2)
            || !(1..=64).contains(&self.miner_threads)
            || self.db_cache_mib == 0
            || self.mempool_transactions == 0
            || self.mempool_mib == 0
        {
            return Err(
                "invalid configuration (control must be loopback, maximum 32 peers, max_peers 1..64, votes 0..2, miner threads 1..64, db_cache_mib, mempool_transactions and mempool_mib at least 1)"
                    .into(),
            );
        }
        if let Some(producer) = &self.producer_hex {
            crate::unhex(producer, PRODUCER_MAX)
                .map_err(|_| "invalid configuration (producer_hex must be lowercase hex, at most 4096 bytes)")?;
        }
        Ok(())
    }
    /// The database's page cache in bytes.
    pub fn cache_bytes(&self) -> u64 {
        self.db_cache_mib.saturating_mul(1 << 20)
    }
    /// The producer data of the blocks this node builds.
    pub fn producer(&self) -> Vec<u8> {
        self.producer_hex
            .as_deref()
            .filter(|p| !p.is_empty())
            .and_then(|p| crate::unhex(p, PRODUCER_MAX).ok())
            .unwrap_or_default()
    }
    pub fn load(dir: &Path) -> Result<Self> {
        let c: Self = serde_json::from_slice(&read_config(dir)?)
            .map_err(|e| format!("invalid configuration at {}: {e}", dir.display()))?;
        c.validate()?;
        Ok(c)
    }
    pub fn save(&self, dir: &Path) -> Result<()> {
        self.validate()?;
        atomic_write(
            &dir.join("config.json"),
            &serde_json::to_vec_pretty(self).map_err(|e| e.to_string())?,
        )
    }
}
pub fn atomic_write(path: &Path, bytes: &[u8]) -> Result<()> {
    let temp = path.with_extension("tmp");
    let mut f = File::create(&temp).map_err(|e| e.to_string())?;
    f.write_all(bytes)
        .and_then(|()| f.sync_all())
        .map_err(|e| e.to_string())?;
    drop(f);
    fs::rename(temp, path).map_err(|e| e.to_string())
}
pub fn lock(dir: &Path) -> Result<File> {
    fs::create_dir_all(dir).map_err(|e| e.to_string())?;
    let f = OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(dir.join("node.lock"))
        .map_err(|e| e.to_string())?;
    f.try_lock()
        .map_err(|e| format!("data directory is locked: {e}"))?;
    Ok(f)
}
/// The production network's launch time, its genesis timestamp: with the
/// production profile it fixes the genesis every node builds for itself
/// (rules §3), 2026-10-21 00:00:00 UTC.
pub const LAUNCH: Option<u64> = Some(1_792_540_800);
/// The production network's seed nodes, as `host:port` names: a new node with
/// discovery on looks them up as it starts and learns their addresses, so that
/// it finds the network with no peer configured (wiki: node/sync).
pub const SEEDS: &[&str] = &["node1.ting-node.com:18442", "node2.ting-node.com:18442"];
/// The production network's genesis (rules §3).
pub fn production_genesis() -> Option<Genesis> {
    LAUNCH.map(Genesis::production)
}
/// A development chain (wiki: start/node).
pub fn development_genesis() -> Genesis {
    Genesis::development()
}
/// Initializes `dir` for a development chain, or for the network whose
/// genesis file is `supplied`.
pub fn initialize(dir: &Path, c: &Config, supplied: Option<&Path>) -> Result<Genesis> {
    let g = match supplied {
        Some(p) => {
            Genesis::decode(&fs::read(p).map_err(|e| e.to_string())?).map_err(|e| e.to_string())?
        }
        None => development_genesis(),
    };
    initialize_with(dir, c, g)
}
/// Initializes `dir` for the network of genesis `g`. The production and
/// development chains start by importing their seed (wiki:
/// reference/conformance#the-seed).
pub fn initialize_with(dir: &Path, c: &Config, g: Genesis) -> Result<Genesis> {
    let g = initialize_files(dir, c, g)?;
    let height = crate::seed::import(dir, &g)?;
    if height > 0 {
        eprintln!("imported the seed: {height} blocks");
    }
    Ok(g)
}
/// Initializes `dir` for the network of genesis `g` without importing its
/// seed: the node takes the seed's blocks from its peers and checks them as it
/// checks any, as a production node does.
pub fn initialize_empty(dir: &Path, c: &Config, g: Genesis) -> Result<Genesis> {
    initialize_files(dir, c, g)
}
/// Writes the configuration, the genesis and the control token of a new data
/// directory.
fn initialize_files(dir: &Path, c: &Config, g: Genesis) -> Result<Genesis> {
    c.validate()?;
    g.validate().map_err(|e| e.to_string())?;
    let _lock = lock(dir)?;
    if dir.join("config.json").exists() || dir.join("genesis.bin").exists() {
        return Err("directory is already initialized".into());
    }
    let mut secret = [0; 32];
    getrandom::fill(&mut secret).map_err(|e| e.to_string())?;
    let mut opts = OpenOptions::new();
    opts.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        opts.mode(0o600);
    }
    let mut token = opts
        .open(dir.join("control.token"))
        .map_err(|e| e.to_string())?;
    token
        .write_all(hex(&secret).as_bytes())
        .and_then(|()| token.sync_all())
        .map_err(|e| e.to_string())?;
    drop(token);
    protect_token(dir)?;
    atomic_write(&dir.join("genesis.bin"), &g.encode())?;
    c.save(dir)?;
    Ok(g)
}
pub fn genesis(dir: &Path) -> Result<Genesis> {
    Genesis::decode(&fs::read(dir.join("genesis.bin")).map_err(|e| e.to_string())?)
        .map_err(|e| e.to_string())
}
/// Leaves `control.token` to its owner alone (wiki: node/security): on Unix
/// by its mode, on Windows by an access list that grants the current user
/// and nobody else, whatever the folder would pass down. Done at init.
pub fn protect_token(dir: &Path) -> Result<()> {
    let path = dir.join("control.token");
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&path, fs::Permissions::from_mode(0o600)).map_err(|e| e.to_string())?;
    }
    #[cfg(windows)]
    owner_only(&path)?;
    #[cfg(not(any(unix, windows)))]
    let _ = path;
    Ok(())
}
#[cfg(windows)]
#[allow(unsafe_code)]
fn owner_only(path: &Path) -> Result<()> {
    use std::os::windows::ffi::OsStrExt;
    use windows_sys::Win32::{
        Foundation::{CloseHandle, HANDLE},
        Security::{
            ACCESS_ALLOWED_ACE, ACL, ACL_REVISION, AddAccessAllowedAce,
            Authorization::{SE_FILE_OBJECT, SetNamedSecurityInfoW},
            DACL_SECURITY_INFORMATION, GetLengthSid, GetTokenInformation, InitializeAcl,
            PROTECTED_DACL_SECURITY_INFORMATION, TOKEN_QUERY, TOKEN_USER, TokenUser,
        },
        Storage::FileSystem::FILE_ALL_ACCESS,
        System::Threading::{GetCurrentProcess, OpenProcessToken},
    };
    let failed = |what: &str| format!("{what}: {}", std::io::Error::last_os_error());
    let mut token: HANDLE = std::ptr::null_mut();
    // SAFETY: the process's own pseudo-handle, and a live handle to fill.
    if unsafe { OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &mut token) } == 0 {
        return Err(failed("reading the process token"));
    }
    // The token's user, a TOKEN_USER and the SID it points into, in a buffer
    // of u64 so that the structure is aligned.
    let mut length = 0u32;
    // SAFETY: a size query, with no buffer and a live length to fill.
    unsafe { GetTokenInformation(token, TokenUser, std::ptr::null_mut(), 0, &mut length) };
    let mut user = vec![0u64; (length as usize).div_ceil(8)];
    // SAFETY: the buffer holds at least `length` bytes.
    let read = unsafe {
        GetTokenInformation(
            token,
            TokenUser,
            user.as_mut_ptr().cast(),
            length,
            &mut length,
        )
    };
    // SAFETY: the handle came from OpenProcessToken and is closed once.
    unsafe { CloseHandle(token) };
    if read == 0 {
        return Err(failed("reading the process user"));
    }
    // SAFETY: GetTokenInformation filled the buffer with a TOKEN_USER, whose
    // SID lies in the same buffer and lives as long as `user`.
    let sid = unsafe { (*user.as_ptr().cast::<TOKEN_USER>()).User.Sid };
    // SAFETY: a valid SID.
    let sid_length = unsafe { GetLengthSid(sid) } as usize;
    let size = std::mem::size_of::<ACL>() + std::mem::size_of::<ACCESS_ALLOWED_ACE>()
        - std::mem::size_of::<u32>()
        + sid_length;
    let mut acl = vec![0u64; size.div_ceil(8)];
    let list = acl.as_mut_ptr().cast::<ACL>();
    // SAFETY: `acl` holds `size` bytes, aligned for an ACL; the SID is valid.
    let built = unsafe {
        InitializeAcl(list, size as u32, ACL_REVISION) != 0
            && AddAccessAllowedAce(list, ACL_REVISION, FILE_ALL_ACCESS, sid) != 0
    };
    if !built {
        return Err(failed("building the token file's access list"));
    }
    let wide: Vec<u16> = path.as_os_str().encode_wide().chain([0]).collect();
    // SAFETY: a NUL-terminated path, and an access list that outlives the
    // call; owner, group and audit list stay as they are.
    let status = unsafe {
        SetNamedSecurityInfoW(
            wide.as_ptr(),
            SE_FILE_OBJECT,
            DACL_SECURITY_INFORMATION | PROTECTED_DACL_SECURITY_INFORMATION,
            std::ptr::null_mut(),
            std::ptr::null_mut(),
            list,
            std::ptr::null(),
        )
    };
    if status != 0 {
        return Err(format!(
            "setting the token file's access list: {}",
            std::io::Error::from_raw_os_error(status as i32)
        ));
    }
    Ok(())
}
pub fn token(dir: &Path) -> Result<String> {
    let t = fs::read_to_string(dir.join("control.token")).map_err(|e| e.to_string())?;
    if t.len() != 64 || crate::unhex(&t, 32).is_err() {
        return Err("invalid control token".into());
    }
    Ok(t)
}
pub fn authorized(expected: &str, supplied: &str) -> bool {
    if supplied.len() != expected.len() {
        return false;
    }
    expected
        .bytes()
        .zip(supplied.bytes())
        .fold(0, |diff, (a, b)| diff | (a ^ b))
        == 0
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Running {
    pub pid: u32,
    pub control: SocketAddr,
    pub p2p: SocketAddr,
    pub api: Option<SocketAddr>,
    pub started: u64,
}
impl Running {
    pub fn load(dir: &Path) -> Result<Self> {
        let bytes = fs::read(dir.join("runtime.json")).map_err(|e| {
            if e.kind() == std::io::ErrorKind::NotFound {
                format!(
                    "node is not running at {}. Run: ting --data-dir \"{}\" start",
                    dir.display(),
                    dir.display()
                )
            } else {
                e.to_string()
            }
        })?;
        serde_json::from_slice(&bytes).map_err(|e| e.to_string())
    }
    pub fn save(&self, dir: &Path) -> Result<()> {
        atomic_write(
            &dir.join("runtime.json"),
            &serde_json::to_vec_pretty(self).map_err(|e| e.to_string())?,
        )
    }
}
