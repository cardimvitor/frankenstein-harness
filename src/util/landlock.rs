//! Native Linux confinement without an external binary: Landlock (filesystem + TCP) and a seccomp deny-list,
//! applied by re-executing this binary (`fh __sandbox-exec ... -- cmd`) so the wrapper composes with the shell tool.
//!
//! Policy: read/execute almost everywhere except the contents of credential directories under $HOME (`.ssh`, `.gnupg`, `.aws`, ...; their file names stay listable);
//! write only inside the workspace, /tmp, /dev and explicit extras; no TCP when network is off;
//! seccomp refuses ptrace, mount, module loading, bpf, keyctl, namespaces and similar.
use std::collections::HashSet;
use std::path::{Path, PathBuf};

const EXECUTE: u64 = 1 << 0;
const WRITE_FILE: u64 = 1 << 1;
const READ_FILE: u64 = 1 << 2;
const READ_DIR: u64 = 1 << 3;
const REMOVE_DIR: u64 = 1 << 4;
const REMOVE_FILE: u64 = 1 << 5;
const MAKE_CHAR: u64 = 1 << 6;
const MAKE_DIR: u64 = 1 << 7;
const MAKE_REG: u64 = 1 << 8;
const MAKE_SOCK: u64 = 1 << 9;
const MAKE_FIFO: u64 = 1 << 10;
const MAKE_BLOCK: u64 = 1 << 11;
const MAKE_SYM: u64 = 1 << 12;
const REFER: u64 = 1 << 13;
const TRUNCATE: u64 = 1 << 14;
const NET_BIND_TCP: u64 = 1 << 0;
const NET_CONNECT_TCP: u64 = 1 << 1;

const FILE_BITS: u64 = EXECUTE | WRITE_FILE | READ_FILE | TRUNCATE;
const READ: u64 = EXECUTE | READ_FILE | READ_DIR;
const WRITE: u64 = WRITE_FILE | REMOVE_DIR | REMOVE_FILE | MAKE_CHAR | MAKE_DIR | MAKE_REG | MAKE_SOCK | MAKE_FIFO | MAKE_BLOCK | MAKE_SYM | REFER | TRUNCATE;

const SYS_CREATE: libc::c_long = 444;
const SYS_ADD_RULE: libc::c_long = 445;
const SYS_RESTRICT: libc::c_long = 446;

#[repr(C)]
struct RulesetAttr {
    fs: u64,
    net: u64,
}

#[repr(C, packed)]
struct PathBeneath {
    allowed: u64,
    parent_fd: i32,
}

/// Landlock ABI version supported by the running kernel (0 = unavailable).
pub fn abi() -> i32 {
    let r = unsafe { libc::syscall(SYS_CREATE, std::ptr::null::<RulesetAttr>(), 0usize, 1u32) };
    if r < 0 { 0 } else { r as i32 }
}

/// Directories under $HOME that hold credentials; never readable inside the sandbox.
pub fn secret_paths(home: &Path) -> Vec<PathBuf> {
    [".ssh", ".gnupg", ".aws", ".azure", ".kube", ".docker", ".netrc", ".git-credentials", ".npmrc", ".pypirc", ".config/gcloud", ".config/gh", ".password-store", ".local/share/keyrings", ".terraform.d"].iter().map(|p| home.join(p)).filter(|p| p.exists()).collect()
}

fn add_path(ruleset: i32, path: &Path, access: u64, handled: u64) {
    let Ok(cpath) = std::ffi::CString::new(path.as_os_str().to_string_lossy().as_bytes()) else { return };
    let fd = unsafe { libc::open(cpath.as_ptr(), libc::O_PATH | libc::O_CLOEXEC) };
    if fd < 0 {
        return;
    }
    let is_dir = path.is_dir();
    let mut a = access & handled;
    if !is_dir {
        a &= FILE_BITS;
    }
    if a != 0 {
        let rule = PathBeneath { allowed: a, parent_fd: fd };
        unsafe { libc::syscall(SYS_ADD_RULE, ruleset, 1u32, &rule as *const PathBeneath, 0u32) };
    }
    unsafe { libc::close(fd) };
}

/// Grant `access` on everything under `dir` except the `denied` subtrees (Landlock is allow-list only,
/// so ancestors of a denied path are opened child by child).
fn grant_except(ruleset: i32, dir: &Path, denied: &[PathBuf], access: u64, handled: u64) {
    if !denied.iter().any(|d| d.starts_with(dir)) {
        add_path(ruleset, dir, access, handled);
        return;
    }
    let Ok(rd) = std::fs::read_dir(dir) else { return };
    for e in rd.flatten() {
        let p = e.path();
        if denied.iter().any(|d| *d == p) {
            continue;
        }
        // do not follow symlinks out of the tree: a symlink itself is granted (its target is checked on access)
        if e.file_type().map(|t| t.is_symlink()).unwrap_or(false) {
            add_path(ruleset, &p, access & FILE_BITS, handled);
        } else {
            grant_except(ruleset, &p, denied, access, handled);
        }
    }
}

pub struct Policy {
    pub cwd: PathBuf,
    pub network: bool,
    pub extra_write: Vec<PathBuf>,
    pub home: Option<PathBuf>,
}

/// Restricts the current process (and all descendants). Returns a short description of what is enforced.
pub fn apply(p: &Policy) -> Result<String, String> {
    let abi = abi();
    if abi < 1 {
        return Err("Landlock is not available on this kernel".into());
    }
    let mut fs_all = EXECUTE | WRITE_FILE | READ_FILE | READ_DIR | REMOVE_DIR | REMOVE_FILE | MAKE_CHAR | MAKE_DIR | MAKE_REG | MAKE_SOCK | MAKE_FIFO | MAKE_BLOCK | MAKE_SYM;
    if abi >= 2 {
        fs_all |= REFER;
    }
    if abi >= 3 {
        fs_all |= TRUNCATE;
    }
    let net_all = if abi >= 4 && !p.network { NET_BIND_TCP | NET_CONNECT_TCP } else { 0 };
    let attr = RulesetAttr { fs: fs_all, net: net_all };
    let size = if abi >= 4 { std::mem::size_of::<RulesetAttr>() } else { 8 };
    let rs = unsafe { libc::syscall(SYS_CREATE, &attr as *const RulesetAttr, size, 0u32) };
    if rs < 0 {
        return Err(format!("landlock_create_ruleset failed: {}", std::io::Error::last_os_error()));
    }
    let rs = rs as i32;
    let denied = p.home.as_deref().map(secret_paths).unwrap_or_default();
    // directory listing works everywhere (names only); reading file contents is what the deny-list withholds
    add_path(rs, Path::new("/"), READ_DIR, fs_all);
    grant_except(rs, Path::new("/"), &denied, READ, fs_all);
    let mut writable: Vec<PathBuf> = vec![p.cwd.clone(), PathBuf::from("/tmp"), PathBuf::from("/dev")];
    writable.extend(p.extra_write.iter().cloned());
    for w in &writable {
        add_path(rs, w, WRITE | READ, fs_all);
    }
    let ok = unsafe { libc::prctl(libc::PR_SET_NO_NEW_PRIVS, 1, 0, 0, 0) };
    if ok != 0 {
        return Err("prctl(NO_NEW_PRIVS) failed".into());
    }
    let r = unsafe { libc::syscall(SYS_RESTRICT, rs, 0u32) };
    unsafe { libc::close(rs) };
    if r != 0 {
        return Err(format!("landlock_restrict_self failed: {}", std::io::Error::last_os_error()));
    }
    Ok(format!("landlock ABI {abi}: writes limited to the workspace and /tmp{}{}", if net_all != 0 { ", TCP blocked" } else { "" }, if denied.is_empty() { String::new() } else { format!(", {} credential path(s) hidden", denied.len()) }))
}

#[repr(C)]
struct SockFilter {
    code: u16,
    jt: u8,
    jf: u8,
    k: u32,
}

#[repr(C)]
struct SockFprog {
    len: u16,
    filter: *const SockFilter,
}

fn denied_syscalls() -> Vec<libc::c_long> {
    let mut v: Vec<libc::c_long> = vec![
        libc::SYS_ptrace, libc::SYS_mount, libc::SYS_umount2, libc::SYS_pivot_root, libc::SYS_chroot, libc::SYS_kexec_load, libc::SYS_init_module, libc::SYS_finit_module,
        libc::SYS_delete_module, libc::SYS_bpf, libc::SYS_keyctl, libc::SYS_add_key, libc::SYS_request_key, libc::SYS_reboot, libc::SYS_swapon, libc::SYS_swapoff,
        libc::SYS_setns, libc::SYS_unshare, libc::SYS_perf_event_open, libc::SYS_process_vm_readv, libc::SYS_process_vm_writev, libc::SYS_userfaultfd,
        libc::SYS_settimeofday, libc::SYS_clock_settime, libc::SYS_acct,
    ];
    v.retain(|n| *n != 0);
    v
}

/// seccomp filter: listed syscalls fail with EPERM; everything else is allowed.
pub fn apply_seccomp() -> Result<(), String> {
    #[cfg(target_arch = "x86_64")]
    const ARCH: u32 = 0xC000_003E;
    #[cfg(target_arch = "aarch64")]
    const ARCH: u32 = 0xC000_00B7;
    #[cfg(not(any(target_arch = "x86_64", target_arch = "aarch64")))]
    return Ok(());
    #[cfg(any(target_arch = "x86_64", target_arch = "aarch64"))]
    {
        const BPF_LD_W_ABS: u16 = 0x20;
        const BPF_JEQ_K: u16 = 0x15;
        const BPF_RET_K: u16 = 0x06;
        const RET_ALLOW: u32 = 0x7fff_0000;
        const RET_EPERM: u32 = 0x0005_0000 | libc::EPERM as u32;
        const RET_KILL: u32 = 0x0000_0000;
        let calls: Vec<u32> = denied_syscalls().into_iter().map(|n| n as u32).collect::<HashSet<_>>().into_iter().collect();
        // layout: [arch check][x32 check][one JEQ per denied syscall][ALLOW][EPERM][KILL]
        let x32 = cfg!(target_arch = "x86_64");
        let head = if x32 { 4 } else { 3 };
        let total = head + calls.len() + 3;
        let (allow_at, eperm_at, kill_at) = (head + calls.len(), head + calls.len() + 1, head + calls.len() + 2);
        let rel = |from: usize, to: usize| (to - from - 1) as u8;
        let mut prog: Vec<SockFilter> = Vec::with_capacity(total);
        prog.push(SockFilter { code: BPF_LD_W_ABS, jt: 0, jf: 0, k: 4 });
        prog.push(SockFilter { code: BPF_JEQ_K, jt: 0, jf: rel(1, kill_at), k: ARCH });
        prog.push(SockFilter { code: BPF_LD_W_ABS, jt: 0, jf: 0, k: 0 });
        if x32 {
            // x32 syscalls carry bit 30 and would bypass the list
            prog.push(SockFilter { code: 0x35 /* JGE */, jt: rel(3, kill_at), jf: 0, k: 0x4000_0000 });
        }
        for (i, n) in calls.iter().enumerate() {
            prog.push(SockFilter { code: BPF_JEQ_K, jt: rel(head + i, eperm_at), jf: 0, k: *n });
        }
        debug_assert_eq!(prog.len(), allow_at);
        prog.push(SockFilter { code: BPF_RET_K, jt: 0, jf: 0, k: RET_ALLOW });
        prog.push(SockFilter { code: BPF_RET_K, jt: 0, jf: 0, k: RET_EPERM });
        prog.push(SockFilter { code: BPF_RET_K, jt: 0, jf: 0, k: RET_KILL });
        let fprog = SockFprog { len: prog.len() as u16, filter: prog.as_ptr() };
        if unsafe { libc::prctl(libc::PR_SET_NO_NEW_PRIVS, 1, 0, 0, 0) } != 0 {
            return Err("prctl(NO_NEW_PRIVS) failed".into());
        }
        let r = unsafe { libc::prctl(libc::PR_SET_SECCOMP, 2 /* SECCOMP_MODE_FILTER */, &fprog as *const SockFprog) };
        if r != 0 {
            return Err(format!("seccomp filter failed: {}", std::io::Error::last_os_error()));
        }
        Ok(())
    }
}

/// Entry point of `fh __sandbox-exec <cwd> <net 0|1> <extra-write, ':'-separated or -> -- <cmd> [args...]`.
/// Confines this process, then replaces it with the command. Only returns on failure.
pub fn sandbox_exec_main(argv: &[String]) -> i32 {
    use std::os::unix::process::CommandExt;
    let Some(sep) = argv.iter().position(|a| a == "--") else {
        eprintln!("fh __sandbox-exec: missing --");
        return 2;
    };
    if sep < 3 || sep + 1 >= argv.len() {
        eprintln!("fh __sandbox-exec: bad arguments");
        return 2;
    }
    let policy = Policy {
        cwd: PathBuf::from(&argv[0]),
        network: argv[1] == "1",
        extra_write: if argv[2] == "-" { vec![] } else { argv[2].split(':').map(PathBuf::from).collect() },
        home: std::env::var_os("HOME").map(PathBuf::from),
    };
    if let Err(e) = apply(&policy) {
        eprintln!("fh sandbox: {e}");
        return 126;
    }
    if let Err(e) = apply_seccomp() {
        eprintln!("fh sandbox: {e}");
        return 126;
    }
    let cmd = &argv[sep + 1..];
    let err = std::process::Command::new(&cmd[0]).args(&cmd[1..]).exec();
    eprintln!("fh sandbox: cannot exec {}: {err}", cmd[0]);
    127
}
