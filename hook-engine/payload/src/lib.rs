#![cfg_attr(all(windows, not(test)), no_std)]

//! Fixed-function, one-request WH_CALLWNDPROC payload.
//!
//! This module is not a loader or a general execution interface. Its only exported
//! operation consumes a nonce-bound request for a supported non-password control
//! or a single menu activation. It never reads another process's address space.
//!
//! A hook runs inside application code. Even bounded, read-only control messages
//! can run application callbacks, trigger reentrancy, or hang in a broken window
//! procedure. The deadline is cooperative; a broker timeout cannot unwind an
//! in-flight target callback. Unhooking prevents future entries but is not proof
//! that an existing entry returned. Validate only with the owned test fixture.
//! See Microsoft: https://learn.microsoft.com/windows/win32/winmsg/callwndproc
//! and https://learn.microsoft.com/windows/win32/controls/em-streamout.

#[cfg(any(windows, test))]
mod bounded;

#[cfg(windows)]
mod windows;

// The payload has no allocator and no Rust standard-library runtime. A violated
// internal invariant must not unwind through a Win32 callback boundary.
#[cfg(all(windows, not(test)))]
#[panic_handler]
fn panic_abort(_: &core::panic::PanicInfo<'_>) -> ! {
    #[link(name = "msvcrt")]
    extern "C" {
        fn abort() -> !;
    }
    unsafe { abort() }
}

// Prebuilt GNU core can retain SEH/personality references even with panic=abort.
// Keep this link-only trampoline out of the public export list. Foreign unwinding
// is unsupported: abort instead of returning an invalid exception disposition.
#[cfg(all(windows, target_env = "gnu", target_arch = "x86_64", not(test)))]
core::arch::global_asm!(
    ".globl rust_eh_personality",
    "rust_eh_personality:",
    "jmp abort",
);
#[cfg(all(windows, target_env = "gnu", target_arch = "x86", not(test)))]
core::arch::global_asm!(
    ".globl _rust_eh_personality",
    "_rust_eh_personality:",
    "jmp _abort",
);

#[cfg(all(windows, not(any(target_arch = "x86", target_arch = "x86_64"))))]
compile_error!("The bounded hook payload supports only Windows x86 and x64.");
