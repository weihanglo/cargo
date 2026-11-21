//! A rustc driver that prints the stack on `STATUS_STACK_OVERFLOW` and on the
//! first access violation.
//!
//! The windows-gnu overflow happens at process exit and goes away under a debugger,
//! and WER skips crashes during process shutdown.
//! So we walk the stack in-process, from a vectored exception handler,
//! and print each frame as `module+offset` for offline symbolization.
//!
//! Set `OVERFLOW_DRIVER_SELFTEST=1` to overflow on purpose and check the handler.
//!
//! After rustc returns, it prints how many other threads are still alive.
//! Set `OVERFLOW_DRIVER_WAIT=1` to also wait for them to exit before exiting.

#![feature(rustc_private)]

extern crate rustc_driver;

use std::process::ExitCode;

rustc_driver::override_c_allocator_in_binary!();

fn main() -> ExitCode {
    #[cfg(all(windows, target_arch = "x86_64"))]
    unsafe {
        veh::install();
    }
    if std::env::var_os("OVERFLOW_DRIVER_SELFTEST").is_some() {
        std::hint::black_box(recurse(0));
    }
    let code = rustc_driver::main();
    #[cfg(windows)]
    unsafe {
        threads::report_alive_at_exit();
    }
    code
}

#[allow(unconditional_recursion)]
#[inline(never)]
fn recurse(n: u64) -> u64 {
    let buf = std::hint::black_box([n; 32]);
    recurse(std::hint::black_box(n + 1)) + buf[3]
}

#[cfg(all(windows, target_arch = "x86_64"))]
mod veh {
    use std::arch::asm;
    use std::ffi::c_void;
    use std::ptr::null_mut;
    use std::sync::atomic::{AtomicBool, Ordering};

    const EXCEPTION_STACK_OVERFLOW: u32 = 0xC000_00FD;
    const EXCEPTION_ACCESS_VIOLATION: u32 = 0xC000_0005;
    const EXCEPTION_CONTINUE_SEARCH: i32 = 0;
    const STD_ERROR_HANDLE: u32 = -12i32 as u32;
    const GET_MODULE_HANDLE_EX_FLAG_UNCHANGED_REFCOUNT: u32 = 0x2;
    const GET_MODULE_HANDLE_EX_FLAG_FROM_ADDRESS: u32 = 0x4;

    // x64 `CONTEXT` layout.
    const CONTEXT_SIZE: usize = 0x4D0;
    const CTX_RSP: usize = 0x98;
    const CTX_RIP: usize = 0xF8;

    const HEAD: usize = 128;
    const TAIL: usize = 64;
    const MAX_FRAMES: usize = 10_000_000;

    #[repr(C)]
    pub struct ExceptionPointers {
        record: *const ExceptionRecord,
        context: *const u8,
    }

    #[repr(C)]
    struct ExceptionRecord {
        code: u32,
        flags: u32,
        record: *const ExceptionRecord,
        address: u64,
        num_params: u32,
        info: [u64; 15],
    }

    #[repr(C, align(16))]
    struct Context([u8; CONTEXT_SIZE]);

    type LookupFn = unsafe extern "system" fn(u64, *mut u64, *mut c_void) -> *mut c_void;
    type UnwindFn = unsafe extern "system" fn(
        u32,
        u64,
        u64,
        *mut c_void,
        *mut Context,
        *mut *mut c_void,
        *mut u64,
        *mut c_void,
    ) -> *mut c_void;

    #[link(name = "kernel32")]
    unsafe extern "system" {
        fn AddVectoredExceptionHandler(
            first: u32,
            handler: unsafe extern "system" fn(*mut ExceptionPointers) -> i32,
        ) -> *mut c_void;
        fn GetModuleHandleA(name: *const u8) -> *mut c_void;
        fn GetProcAddress(module: *mut c_void, name: *const u8) -> *mut c_void;
        fn GetModuleHandleExW(flags: u32, addr: *const u16, out: *mut *mut c_void) -> i32;
        fn GetModuleFileNameW(module: *mut c_void, buf: *mut u16, len: u32) -> u32;
        fn GetStdHandle(id: u32) -> *mut c_void;
        fn WriteFile(
            h: *mut c_void,
            buf: *const u8,
            len: u32,
            written: *mut u32,
            overlapped: *mut c_void,
        ) -> i32;
        fn GetCurrentThreadId() -> u32;
    }

    // The handler runs on the overflowed stack, with only the 20 KiB that
    // std reserves via `SetThreadStackGuarantee`. Keep all buffers static.
    static BUSY: AtomicBool = AtomicBool::new(false);
    static mut LOOKUP: Option<LookupFn> = None;
    static mut UNWIND: Option<UnwindFn> = None;
    static mut CTX: Context = Context([0; CONTEXT_SIZE]);
    static mut HEAD_PCS: [u64; HEAD] = [0; HEAD];
    static mut TAIL_PCS: [u64; TAIL] = [0; TAIL];
    static mut NAME: [u16; 520] = [0; 520];
    static mut OUT: [u8; 8192] = [0; 8192];
    static mut OUT_LEN: usize = 0;

    pub unsafe fn install() {
        unsafe {
            let ntdll = GetModuleHandleA(c"ntdll.dll".as_ptr().cast());
            let lookup = GetProcAddress(ntdll, c"RtlLookupFunctionEntry".as_ptr().cast());
            let unwind = GetProcAddress(ntdll, c"RtlVirtualUnwind".as_ptr().cast());
            LOOKUP = std::mem::transmute::<*mut c_void, Option<LookupFn>>(lookup);
            UNWIND = std::mem::transmute::<*mut c_void, Option<UnwindFn>>(unwind);
            // First in the chain, so it runs before std's handler.
            AddVectoredExceptionHandler(1, handler);
        }
    }

    unsafe extern "system" fn handler(info: *mut ExceptionPointers) -> i32 {
        unsafe {
            let rec = &*(*info).record;
            let interesting =
                rec.code == EXCEPTION_STACK_OVERFLOW || rec.code == EXCEPTION_ACCESS_VIOLATION;
            if interesting && !BUSY.swap(true, Ordering::SeqCst) {
                report(rec, (*info).context);
            }
        }
        EXCEPTION_CONTINUE_SEARCH
    }

    unsafe fn reg(c: *mut Context, off: usize) -> u64 {
        unsafe { c.cast::<u8>().add(off).cast::<u64>().read_unaligned() }
    }

    unsafe fn set_reg(c: *mut Context, off: usize, v: u64) {
        unsafe { c.cast::<u8>().add(off).cast::<u64>().write_unaligned(v) }
    }

    unsafe fn report(rec: &ExceptionRecord, ctx: *const u8) {
        unsafe {
            let (Some(lookup), Some(unwind)) = (LOOKUP, UNWIND) else {
                w(b"[overflow-driver] unwinder not available\n");
                flush();
                return;
            };
            let c = &raw mut CTX;
            std::ptr::copy_nonoverlapping(ctx, c.cast::<u8>(), CONTEXT_SIZE);

            let (stack_base, stack_limit, dealloc): (u64, u64, u64);
            asm!("mov {}, qword ptr gs:[0x08]", out(reg) stack_base);
            asm!("mov {}, qword ptr gs:[0x10]", out(reg) stack_limit);
            asm!("mov {}, qword ptr gs:[0x1478]", out(reg) dealloc);

            w(b"\n[overflow-driver] exception ");
            hex(rec.code as u64);
            w(b" flags=");
            hex(rec.flags as u64);
            w(b" at ");
            frame_name(rec.address);
            if rec.code == EXCEPTION_ACCESS_VIOLATION && rec.num_params >= 2 {
                w(b" (");
                w(match rec.info[0] {
                    0 => b"read",
                    1 => b"write",
                    8 => b"execute",
                    _ => b"access",
                });
                w(b" ");
                hex(rec.info[1]);
                w(b")");
            }
            w(b" thread ");
            dec(GetCurrentThreadId() as u64);
            w(b" rsp=");
            hex(reg(c, CTX_RSP));
            w(b" stack_base=");
            hex(stack_base);
            w(b" stack_limit=");
            hex(stack_limit);
            w(b" reserve=");
            hex(stack_base.wrapping_sub(dealloc));
            w(b"\n");

            let head = &raw mut HEAD_PCS;
            let tail = &raw mut TAIL_PCS;
            let mut n = 0usize;
            loop {
                let pc = reg(c, CTX_RIP);
                if pc == 0 || n >= MAX_FRAMES {
                    break;
                }
                if n < HEAD {
                    (*head)[n] = pc;
                }
                (*tail)[n % TAIL] = pc;
                n += 1;

                let rsp = reg(c, CTX_RSP);
                let mut image_base = 0u64;
                let f = lookup(pc, &mut image_base, null_mut());
                if f.is_null() {
                    // Leaf function: the return address is at rsp.
                    if rsp == 0 || rsp >= stack_base {
                        break;
                    }
                    set_reg(c, CTX_RIP, (rsp as *const u64).read());
                    set_reg(c, CTX_RSP, rsp + 8);
                } else {
                    let mut handler_data = null_mut();
                    let mut establisher = 0u64;
                    unwind(
                        0,
                        image_base,
                        pc,
                        f,
                        c,
                        &mut handler_data,
                        &mut establisher,
                        null_mut(),
                    );
                }
                let new_rsp = reg(c, CTX_RSP);
                if new_rsp <= rsp || new_rsp >= stack_base {
                    break;
                }
            }

            w(b"[overflow-driver] frames: ");
            dec(n as u64);
            w(b"\n");
            for i in 0..n.min(HEAD) {
                frame(i, (*head)[i]);
            }
            if n > HEAD {
                let start = HEAD.max(n.saturating_sub(TAIL));
                if start > HEAD {
                    w(b"  ...\n");
                }
                for i in start..n {
                    frame(i, (*tail)[i % TAIL]);
                }
            }
            w(b"[overflow-driver] end\n");
            flush();
        }
    }

    unsafe fn frame(i: usize, pc: u64) {
        unsafe {
            w(b"  #");
            dec(i as u64);
            w(b" ");
            frame_name(pc);
            w(b"\n");
        }
    }

    unsafe fn frame_name(pc: u64) {
        unsafe {
            hex(pc);
            w(b" ");
            let mut module = null_mut();
            let flags =
                GET_MODULE_HANDLE_EX_FLAG_FROM_ADDRESS | GET_MODULE_HANDLE_EX_FLAG_UNCHANGED_REFCOUNT;
            if GetModuleHandleExW(flags, pc as *const u16, &mut module) == 0 {
                w(b"?");
                return;
            }
            let name = &raw mut NAME;
            let len = GetModuleFileNameW(module, (*name).as_mut_ptr(), 520) as usize;
            let mut start = 0;
            for j in 0..len {
                if (*name)[j] == b'\\' as u16 || (*name)[j] == b'/' as u16 {
                    start = j + 1;
                }
            }
            for j in start..len {
                let ch = (*name)[j];
                w(&[if ch < 0x80 { ch as u8 } else { b'?' }]);
            }
            w(b"+");
            hex(pc - module as u64);
        }
    }

    unsafe fn w(bytes: &[u8]) {
        unsafe {
            let out = &raw mut OUT;
            for &b in bytes {
                if OUT_LEN == (*out).len() {
                    flush();
                }
                (*out)[OUT_LEN] = b;
                OUT_LEN += 1;
            }
        }
    }

    unsafe fn dec(mut v: u64) {
        let mut buf = [0u8; 20];
        let mut i = buf.len();
        loop {
            i -= 1;
            buf[i] = b'0' + (v % 10) as u8;
            v /= 10;
            if v == 0 {
                break;
            }
        }
        unsafe { w(&buf[i..]) }
    }

    unsafe fn hex(mut v: u64) {
        let mut buf = [0u8; 18];
        let mut i = buf.len();
        loop {
            i -= 1;
            buf[i] = b"0123456789abcdef"[(v & 0xf) as usize];
            v >>= 4;
            if v == 0 {
                break;
            }
        }
        i -= 2;
        buf[i] = b'0';
        buf[i + 1] = b'x';
        unsafe { w(&buf[i..]) }
    }

    unsafe fn flush() {
        unsafe {
            let out = &raw const OUT;
            let mut written = 0u32;
            WriteFile(
                GetStdHandle(STD_ERROR_HANDLE),
                (*out).as_ptr(),
                OUT_LEN as u32,
                &mut written,
                null_mut(),
            );
            OUT_LEN = 0;
        }
    }
}

#[cfg(windows)]
mod threads {
    use std::ffi::c_void;
    use std::time::Instant;

    const TH32CS_SNAPTHREAD: u32 = 0x4;
    const SYNCHRONIZE: u32 = 0x0010_0000;
    const THREAD_QUERY_INFORMATION: u32 = 0x0040;
    const THREAD_QUERY_LIMITED_INFORMATION: u32 = 0x0800;
    const THREAD_QUERY_SET_WIN32_START_ADDRESS: u32 = 9;
    const GET_MODULE_HANDLE_EX_FLAGS: u32 = 0x4 | 0x2;
    const WAIT_OBJECT_0: u32 = 0;

    #[repr(C)]
    struct ThreadEntry32 {
        size: u32,
        usage: u32,
        thread_id: u32,
        owner_process_id: u32,
        base_pri: i32,
        delta_pri: i32,
        flags: u32,
    }

    #[link(name = "kernel32")]
    unsafe extern "system" {
        fn CreateToolhelp32Snapshot(flags: u32, pid: u32) -> *mut c_void;
        fn Thread32First(snap: *mut c_void, entry: *mut ThreadEntry32) -> i32;
        fn Thread32Next(snap: *mut c_void, entry: *mut ThreadEntry32) -> i32;
        fn OpenThread(access: u32, inherit: i32, tid: u32) -> *mut c_void;
        fn WaitForSingleObject(h: *mut c_void, ms: u32) -> u32;
        fn CloseHandle(h: *mut c_void) -> i32;
        fn GetCurrentProcessId() -> u32;
        fn GetCurrentThreadId() -> u32;
        fn GetModuleHandleExW(flags: u32, addr: *const u16, out: *mut *mut c_void) -> i32;
        fn GetModuleFileNameW(module: *mut c_void, buf: *mut u16, len: u32) -> u32;
        fn GetThreadDescription(h: *mut c_void, desc: *mut *mut u16) -> i32;
        fn LocalFree(mem: *mut c_void) -> *mut c_void;
    }

    /// Thread name as set by `SetThreadDescription` (Rust std does this).
    unsafe fn thread_name(h: *mut c_void) -> String {
        unsafe {
            let mut p = std::ptr::null_mut();
            if GetThreadDescription(h, &mut p) < 0 || p.is_null() {
                return "?".to_string();
            }
            let mut len = 0;
            while *p.add(len) != 0 {
                len += 1;
            }
            let name = String::from_utf16_lossy(std::slice::from_raw_parts(p, len));
            LocalFree(p.cast());
            name
        }
    }

    #[link(name = "ntdll")]
    unsafe extern "system" {
        fn NtQueryInformationThread(
            h: *mut c_void,
            class: u32,
            info: *mut c_void,
            len: u32,
            ret_len: *mut u32,
        ) -> i32;
    }

    /// Module file name of the thread's start address, e.g. `ntdll.dll`.
    unsafe fn start_module(h: *mut c_void) -> String {
        unsafe {
            let mut addr = 0usize;
            let status = NtQueryInformationThread(
                h,
                THREAD_QUERY_SET_WIN32_START_ADDRESS,
                (&raw mut addr).cast(),
                size_of::<usize>() as u32,
                std::ptr::null_mut(),
            );
            let mut module = std::ptr::null_mut();
            if status < 0
                || GetModuleHandleExW(GET_MODULE_HANDLE_EX_FLAGS, addr as *const u16, &mut module)
                    == 0
            {
                return "?".to_string();
            }
            let mut buf = [0u16; 520];
            let len = GetModuleFileNameW(module, buf.as_mut_ptr(), buf.len() as u32) as usize;
            let path = String::from_utf16_lossy(&buf[..len]);
            path.rsplit(['\\', '/']).next().unwrap_or("?").to_string()
        }
    }

    unsafe fn other_threads() -> Vec<u32> {
        let mut out = Vec::new();
        unsafe {
            let snap = CreateToolhelp32Snapshot(TH32CS_SNAPTHREAD, 0);
            if snap as isize == -1 {
                return out;
            }
            let (pid, me) = (GetCurrentProcessId(), GetCurrentThreadId());
            let mut e: ThreadEntry32 = std::mem::zeroed();
            e.size = size_of::<ThreadEntry32>() as u32;
            let mut ok = Thread32First(snap, &mut e);
            while ok != 0 {
                if e.owner_process_id == pid && e.thread_id != me {
                    out.push(e.thread_id);
                }
                ok = Thread32Next(snap, &mut e);
            }
            CloseHandle(snap);
        }
        out
    }

    pub unsafe fn report_alive_at_exit() {
        let wait = std::env::var_os("OVERFLOW_DRIVER_WAIT").is_some_and(|v| !v.is_empty());
        let start = Instant::now();
        // Threads not started by the OS loader's thread pool (start address in ntdll).
        let mut ours = Vec::new();
        let mut pool = 0;
        for tid in unsafe { other_threads() } {
            unsafe {
                let access =
                    SYNCHRONIZE | THREAD_QUERY_INFORMATION | THREAD_QUERY_LIMITED_INFORMATION;
                let h = OpenThread(access, 0, tid);
                if h.is_null() {
                    continue;
                }
                let module = start_module(h);
                if module.eq_ignore_ascii_case("ntdll.dll") {
                    pool += 1;
                    CloseHandle(h);
                } else {
                    ours.push((h, format!("{}@{module}", thread_name(h))));
                }
            }
        }
        if ours.is_empty() {
            for (h, _) in ours {
                unsafe { CloseHandle(h) };
            }
            return;
        }
        let modules: Vec<&str> = ours.iter().map(|(_, m)| m.as_str()).collect();
        let mut msg = format!(
            "[overflow-driver-exit] alive={} ntdll_pool={pool} threads={modules:?}",
            ours.len()
        );
        let mut timed_out = 0;
        for (h, _) in &ours {
            unsafe {
                if wait && WaitForSingleObject(*h, 2000) != WAIT_OBJECT_0 {
                    timed_out += 1;
                }
                CloseHandle(*h);
            }
        }
        if wait {
            msg += &format!(" waited_us={} timed_out={timed_out}", start.elapsed().as_micros());
        }
        eprintln!("{msg}");
    }
}
