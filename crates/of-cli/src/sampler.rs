//! A user-space statistical profiler for `engine-rs bench` (P3, ledger t066).
//!
//! Why: some hosts set `kernel.perf_event_paranoid = 4`, so `perf`/`samply` cannot open events,
//! and sysctls may not be changeable there. This sampler needs no privileges: `setitimer(ITIMER_PROF)`
//! delivers SIGPROF every `period_us` of process CPU time; the handler records the interrupted
//! PC, LR and the frame-pointer chain (build with `-C force-frame-pointers=yes` and the
//! `profiling` profile for line tables) into a preallocated buffer. `stop` writes the raw
//! samples plus the executable's load base; `tools/perf_symbolize.py` turns them into self and
//! inclusive shares with `addr2line -f -i -C` (inlined frames included).
//!
//! Only on Linux aarch64 with the `sampler` feature (the bench box); elsewhere `start` errors.
//! The handler is async-signal-safe: no allocation, no locks, only atomics and bounded reads
//! of the main thread's stack range (taken from `/proc/self/maps` before arming).

#[cfg(all(feature = "sampler", target_os = "linux", target_arch = "aarch64"))]
#[allow(unsafe_code)]
mod imp {
    use std::io::Write;
    use std::sync::atomic::{AtomicPtr, AtomicU64, AtomicUsize, Ordering};

    const MAX_DEPTH: usize = 96;
    static BUF: AtomicPtr<u64> = AtomicPtr::new(std::ptr::null_mut());
    static CAP: AtomicUsize = AtomicUsize::new(0);
    static POS: AtomicUsize = AtomicUsize::new(0);
    static DROPPED: AtomicU64 = AtomicU64::new(0);
    static STACK_LO: AtomicU64 = AtomicU64::new(0);
    static STACK_HI: AtomicU64 = AtomicU64::new(0);

    pub struct Sampler {
        buf: Vec<u64>,
        exe_base: u64,
        exe: String,
    }

    fn maps_line(pred: impl Fn(&str) -> bool) -> Option<String> {
        std::fs::read_to_string("/proc/self/maps").ok()?.lines().find(|l| pred(l)).map(str::to_owned)
    }

    fn range(l: &str) -> (u64, u64) {
        let r = l.split_whitespace().next().unwrap_or("0-0");
        let (a, b) = r.split_once('-').unwrap_or(("0", "0"));
        (u64::from_str_radix(a, 16).unwrap_or(0), u64::from_str_radix(b, 16).unwrap_or(0))
    }

    extern "C" fn on_prof(_sig: libc::c_int, _info: *mut libc::siginfo_t, ctx: *mut libc::c_void) {
        let buf = BUF.load(Ordering::Relaxed);
        if buf.is_null() || ctx.is_null() {
            return;
        }
        // SAFETY: the kernel passes a valid ucontext_t for SA_SIGINFO handlers.
        let mc = unsafe { &(*(ctx as *const libc::ucontext_t)).uc_mcontext };
        let mut frames = [0u64; MAX_DEPTH + 2];
        let mut n = 0;
        frames[n] = mc.pc;
        n += 1;
        frames[n] = mc.regs[30]; // LR: the caller when the leaf has no frame record yet
        n += 1;
        let (lo, hi) = (STACK_LO.load(Ordering::Relaxed), STACK_HI.load(Ordering::Relaxed));
        let mut fp = mc.regs[29];
        let mut lo_bound = lo.max(mc.sp);
        while n < frames.len() && fp >= lo_bound && fp + 16 <= hi && fp % 8 == 0 {
            // SAFETY: fp lies inside the main thread's mapped stack ([sp, stack top)).
            let (next, ret) = unsafe { (*(fp as *const u64), *((fp + 8) as *const u64)) };
            if ret == 0 {
                break;
            }
            frames[n] = ret;
            n += 1;
            if next <= fp {
                break;
            }
            lo_bound = fp + 16;
            fp = next;
        }
        let cap = CAP.load(Ordering::Relaxed);
        let at = POS.fetch_add(n + 1, Ordering::Relaxed);
        if at + n + 1 > cap {
            DROPPED.fetch_add(1, Ordering::Relaxed);
            return;
        }
        // SAFETY: [at, at+n+1) is inside the buffer and reserved for this sample by fetch_add.
        unsafe {
            *buf.add(at) = n as u64;
            for (i, f) in frames[..n].iter().enumerate() {
                *buf.add(at + 1 + i) = *f;
            }
        }
    }

    impl Sampler {
        pub fn start(period_us: u32, cap_words: usize) -> Result<Sampler, String> {
            let exe = std::env::current_exe().map_err(|e| e.to_string())?.to_string_lossy().into_owned();
            let base_line = maps_line(|l| l.ends_with(exe.as_str())).ok_or("exe not in /proc/self/maps")?;
            let exe_base = range(&base_line).0;
            let st = maps_line(|l| l.ends_with("[stack]")).ok_or("no [stack] in /proc/self/maps")?;
            let (lo, hi) = range(&st);
            STACK_LO.store(lo, Ordering::Relaxed);
            STACK_HI.store(hi, Ordering::Relaxed);
            let mut buf = vec![0u64; cap_words];
            CAP.store(cap_words, Ordering::Relaxed);
            POS.store(0, Ordering::Relaxed);
            BUF.store(buf.as_mut_ptr(), Ordering::Release);
            // SAFETY: plain libc calls with fully initialised arguments.
            unsafe {
                let mut sa: libc::sigaction = std::mem::zeroed();
                sa.sa_sigaction = on_prof as *const () as usize;
                sa.sa_flags = libc::SA_SIGINFO | libc::SA_RESTART;
                libc::sigemptyset(&mut sa.sa_mask);
                if libc::sigaction(libc::SIGPROF, &sa, std::ptr::null_mut()) != 0 {
                    return Err("sigaction failed".into());
                }
                let tv = libc::timeval { tv_sec: 0, tv_usec: period_us as libc::suseconds_t };
                let it = libc::itimerval { it_interval: tv, it_value: tv };
                if libc::setitimer(libc::ITIMER_PROF, &it, std::ptr::null_mut()) != 0 {
                    return Err("setitimer failed".into());
                }
            }
            Ok(Sampler { buf, exe_base, exe })
        }

        /// Disarm and write `<path>`: a header line, then one line per sample (hex addresses,
        /// leaf first).
        pub fn stop(self, path: &std::path::Path) -> std::io::Result<usize> {
            // SAFETY: disarming the timer with a zero itimerval.
            unsafe {
                let z = libc::timeval { tv_sec: 0, tv_usec: 0 };
                let it = libc::itimerval { it_interval: z, it_value: z };
                libc::setitimer(libc::ITIMER_PROF, &it, std::ptr::null_mut());
            }
            BUF.store(std::ptr::null_mut(), Ordering::Release);
            let end = POS.load(Ordering::Relaxed).min(self.buf.len());
            let mut f = std::io::BufWriter::new(std::fs::File::create(path)?);
            writeln!(f, "# exe={} base={:x} dropped={}", self.exe, self.exe_base, DROPPED.load(Ordering::Relaxed))?;
            let (mut i, mut samples) = (0, 0);
            while i < end {
                let n = self.buf[i] as usize;
                if n == 0 || i + 1 + n > end {
                    break;
                }
                let line: Vec<String> = self.buf[i + 1..i + 1 + n].iter().map(|a| format!("{a:x}")).collect();
                writeln!(f, "{}", line.join(" "))?;
                samples += 1;
                i += n + 1;
            }
            f.flush()?;
            Ok(samples)
        }
    }
}

#[cfg(not(all(feature = "sampler", target_os = "linux", target_arch = "aarch64")))]
mod imp {
    pub struct Sampler;
    impl Sampler {
        pub fn start(_period_us: u32, _cap_words: usize) -> Result<Sampler, String> {
            Err("the sampler needs the `sampler` feature on linux aarch64".into())
        }
        pub fn stop(self, _path: &std::path::Path) -> std::io::Result<usize> {
            Ok(0)
        }
    }
}

pub use imp::Sampler;
