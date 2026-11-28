//! Where the UI thread is when it stops drawing (owner, 2026-11-27: the library scrolled like a slideshow in the real
//! window while each frame took 2 ms to draw: something held the UI thread 100+ ms between frames).
//!
//! `start()` (on the UI thread) beats a heart from a 5 ms Slint timer; a watcher thread sees the heart stop for
//! 50+ ms, suspends the UI thread for a moment, walks its stack (x64 unwind tables, as a debugger does) and
//! resumes it. `report()` prints the stalls and the stacks seen most, as `module+offset`; the Lag.exe offsets are
//! turned into function names with `addr2line` (MinGW) over the unstripped target\release\Lag.exe.
//! Only `--gpu-check` uses it: a debugging aid, never in normal runs.

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

#[derive(Default)]
struct Seen {
    /// (stack as module+offset lines) -> samples
    stacks: HashMap<Vec<String>, usize>,
    /// how long each stall lasted, ms
    stalls: Vec<u64>,
}

pub struct Probe {
    beat: Arc<AtomicU64>,
    stop: Arc<AtomicBool>,
    seen: Arc<Mutex<Seen>>,
    _timer: slint::Timer,
}

/// Starts watching the calling (UI) thread.
pub fn start() -> Option<Probe> {
    use windows::Win32::System::Threading::{GetCurrentThreadId, OpenThread, THREAD_GET_CONTEXT, THREAD_QUERY_INFORMATION, THREAD_SUSPEND_RESUME};
    let origin = Instant::now();
    let beat = Arc::new(AtomicU64::new(0));
    let timer = slint::Timer::default();
    let b = beat.clone();
    timer.start(slint::TimerMode::Repeated, Duration::from_millis(5), move || {
        b.store(origin.elapsed().as_millis() as u64, Ordering::Relaxed);
    });
    let thread = unsafe {
        OpenThread(THREAD_SUSPEND_RESUME | THREAD_GET_CONTEXT | THREAD_QUERY_INFORMATION, false, GetCurrentThreadId()).ok()?
    };
    let thread = thread.0 as usize;
    let (stop, seen) = (Arc::new(AtomicBool::new(false)), Arc::new(Mutex::new(Seen::default())));
    let (b, s, flag) = (beat.clone(), seen.clone(), stop.clone());
    std::thread::Builder::new()
        .name("lag-stall-probe".into())
        .spawn(move || {
            let mut stall_from: Option<u64> = None;
            while !flag.load(Ordering::Relaxed) {
                std::thread::sleep(Duration::from_millis(15));
                let now = origin.elapsed().as_millis() as u64;
                let last = b.load(Ordering::Relaxed);
                if last == 0 {
                    continue;
                }
                if now.saturating_sub(last) > 50 {
                    stall_from.get_or_insert(last);
                    if let Some(stack) = unsafe { sample(windows::Win32::Foundation::HANDLE(thread as *mut _)) } {
                        *s.lock().unwrap().stacks.entry(stack).or_default() += 1;
                    }
                } else if let Some(from) = stall_from.take() {
                    s.lock().unwrap().stalls.push(last.saturating_sub(from));
                }
            }
        })
        .ok()?;
    Some(Probe { beat, stop, seen, _timer: timer })
}

/// Prints what the watcher saw (and stops it).
pub fn report(p: &Probe) {
    p.stop.store(true, Ordering::Relaxed);
    let _ = &p.beat;
    let seen = p.seen.lock().unwrap();
    let mut stalls = seen.stalls.clone();
    stalls.sort_unstable();
    println!(
        "stalls of the UI thread (50+ ms): {} | longest {:?} ms | total {} ms",
        stalls.len(),
        stalls.iter().rev().take(8).collect::<Vec<_>>(),
        stalls.iter().sum::<u64>()
    );
    let mut stacks: Vec<_> = seen.stacks.iter().collect();
    stacks.sort_by(|a, b| b.1.cmp(a.1));
    for (stack, n) in stacks.into_iter().take(6) {
        println!("--- {n} sample(s):");
        for f in stack {
            println!("    {f}");
        }
    }
}

/// The stack of a thread (suspended for the walk): up to 40 frames, `module+0xoffset`.
unsafe fn sample(thread: windows::Win32::Foundation::HANDLE) -> Option<Vec<String>> {
    use windows::Win32::System::Diagnostics::Debug::{
        GetThreadContext, RtlLookupFunctionEntry, RtlVirtualUnwind, CONTEXT, CONTEXT_FULL_AMD64, UNW_FLAG_NHANDLER,
    };
    use windows::Win32::System::Threading::{ResumeThread, SuspendThread};
    if SuspendThread(thread) == u32::MAX {
        return None;
    }
    let mut ctx = CONTEXT { ContextFlags: CONTEXT_FULL_AMD64, ..Default::default() };
    let got = GetThreadContext(thread, &mut ctx);
    let mut pcs = Vec::new();
    if got.is_ok() {
        for _ in 0..40 {
            if ctx.Rip == 0 {
                break;
            }
            pcs.push(ctx.Rip);
            let mut base = 0u64;
            let entry = RtlLookupFunctionEntry(ctx.Rip, &mut base, None);
            if entry.is_null() {
                // a leaf function: the return address is on top of the stack
                ctx.Rip = *(ctx.Rsp as *const u64);
                ctx.Rsp += 8;
            } else {
                let mut data = std::ptr::null_mut();
                let mut frame = 0u64;
                let _ = RtlVirtualUnwind(UNW_FLAG_NHANDLER, base, ctx.Rip, entry, &mut ctx, &mut data, &mut frame, None);
            }
        }
    }
    ResumeThread(thread);
    Some(pcs.into_iter().map(module_offset).collect())
}

/// `Lag.exe+0x1234` for an address.
fn module_offset(pc: u64) -> String {
    use windows::Win32::Foundation::HMODULE;
    use windows::Win32::System::LibraryLoader::{
        GetModuleFileNameW, GetModuleHandleExW, GET_MODULE_HANDLE_EX_FLAG_FROM_ADDRESS, GET_MODULE_HANDLE_EX_FLAG_UNCHANGED_REFCOUNT,
    };
    unsafe {
        let mut m = HMODULE::default();
        let flags = GET_MODULE_HANDLE_EX_FLAG_FROM_ADDRESS | GET_MODULE_HANDLE_EX_FLAG_UNCHANGED_REFCOUNT;
        if GetModuleHandleExW(flags, windows::core::PCWSTR(pc as *const u16), &mut m).is_err() {
            return format!("?+{pc:#x}");
        }
        let mut name = [0u16; 260];
        let n = GetModuleFileNameW(Some(m), &mut name) as usize;
        let name = String::from_utf16_lossy(&name[..n]);
        let short = name.rsplit(['\\', '/']).next().unwrap_or("?").to_owned();
        format!("{short}+{:#x}", pc - m.0 as u64)
    }
}
