//! C ABI over `console-core`, built for `wasm32-unknown-emscripten`.
//!
//! This is a *binary* crate: `main` does nothing, the real surface is the
//! `con_*` C ABI below (see `SPEC.md`, "Single-file HTML"). Emscripten keeps
//! the symbols alive via `-sEXPORTED_FUNCTIONS` (see `web/build-engine.sh`).
//!
//! Nothing here is emscripten-specific, so the crate also builds and links
//! natively as a workspace member; the ABI is simply unused there.
//!
//! # Threading
//!
//! [`console_core::Console`] is `!Send` (it owns a Lua VM), and the wasm build
//! is single-threaded, so all global state lives in `thread_local!` +
//! `RefCell`. Every entry point below must be called from the one JS thread.
//!
//! # Ownership / lifetime contract
//!
//! * `con_alloc(len)` returns a buffer from the Rust global allocator
//!   (alignment 1). Free it with `con_free(ptr, len)` **or** hand it to
//!   `con_init`.
//! * **`con_init` takes ownership of the cart buffer**: it copies the bytes out
//!   and then frees the allocation itself. The caller must not use or free the
//!   pointer afterwards. (The JS shell in `web/template.html` relies on this:
//!   it allocs, writes, calls `con_init`, and never frees.)
//! * `con_fb()` points into a persistent buffer that is allocated once and
//!   never reallocated, so the pointer stays valid for the life of the module;
//!   its *contents* are refreshed on each `con_fb()` call.
//! * `con_audio()` works exactly like `con_fb()`, but hands out `f32` samples.
//!   The buffer is `f32`-aligned, so JS can wrap it directly in a
//!   `Float32Array(HEAPU8.buffer, ptr, 735)` view.
//! * `con_palette()` points at immutable static data; always valid.
//! * `con_dpal()` points into a thread-local array that is never reallocated,
//!   so the pointer stays valid for the life of the module; its *contents* are
//!   refreshed on each `con_dpal()` call.
//! * `con_error()` points into a `CString` owned by this module. It stays valid
//!   until the next `con_init` / `con_step`.
//! * `con_save()` points into a module-owned canonical JSON buffer. Copy
//!   `con_save_len()` bytes immediately; a later init/step may reallocate it.

use std::alloc::{Layout, alloc, dealloc};
use std::cell::RefCell;
use std::ffi::CString;
use std::ptr;

use console_core::{
    COLOR_COUNT, Console, FB_LEN, IDENTITY_PAL, SAMPLES_PER_FRAME, SCREEN_H, SCREEN_W,
};

/// A real `static` copy of the palette: `console_core::PALETTE` is a `const`,
/// so calling `.as_ptr()` on it directly would hand out a pointer to a
/// temporary. `COLOR_COUNT` entries x RGB are contiguous bytes.
static PALETTE_RGB: [[u8; 3]; COLOR_COUNT] = console_core::PALETTE;

thread_local! {
    /// The one live console, if a cart has been loaded successfully.
    static CONSOLE: RefCell<Option<Console>> = const { RefCell::new(None) };
    /// Persistent framebuffer mirror handed out by `con_fb`.
    static FB: RefCell<Vec<u8>> = const { RefCell::new(Vec::new()) };
    /// Persistent audio mirror handed out by `con_audio`.
    static AUDIO: RefCell<Vec<f32>> = const { RefCell::new(Vec::new()) };
    /// Persistent display-palette mirror handed out by `con_dpal`.
    static DPAL: RefCell<[u8; COLOR_COUNT]> = const { RefCell::new(IDENTITY_PAL) };
    /// Last error (init failure or runtime halt), NUL-terminated.
    static ERROR: RefCell<Option<CString>> = const { RefCell::new(None) };
    /// Current canonical persistence envelope and successful write revision.
    static SAVE: RefCell<Vec<u8>> = const { RefCell::new(Vec::new()) };
    static SAVE_REVISION: RefCell<u32> = const { RefCell::new(0) };
    /// Persistence-only diagnostic, distinct from a cart-halting error.
    static SAVE_DIAGNOSTIC: RefCell<Option<CString>> = const { RefCell::new(None) };
}

/// Store (or clear) the message returned by [`con_error`].
fn set_error(msg: Option<&str>) {
    let cstr = msg.map(|m| {
        // Interior NULs would truncate the C string; replace them.
        let sanitized = m.replace('\0', " ");
        CString::new(sanitized).unwrap_or_else(|_| CString::new("error").unwrap())
    });
    ERROR.with(|slot| *slot.borrow_mut() = cstr);
}

/// Send any `printh` output to emscripten's stdout (i.e. `console.log`).
fn drain_logs(con: &mut Console) {
    for line in con.take_logs() {
        println!("{line}");
    }
}

fn sync_save(con: &Console) {
    SAVE.with(|slot| {
        let mut slot = slot.borrow_mut();
        slot.clear();
        if let Some(document) = con.save_document() {
            slot.extend_from_slice(document.as_bytes());
        }
    });
    SAVE_REVISION.with(|slot| *slot.borrow_mut() = con.save_revision());
    let diagnostic = con.save_diagnostic().map(|message| {
        CString::new(message.replace('\0', " "))
            .unwrap_or_else(|_| CString::new("save error").unwrap())
    });
    SAVE_DIAGNOSTIC.with(|slot| *slot.borrow_mut() = diagnostic);
}

/// Allocate `len` bytes for the host to write a cart into.
///
/// Returns null if the allocation fails. `len == 0` still returns a unique
/// non-null pointer that must be freed.
#[unsafe(no_mangle)]
pub extern "C" fn con_alloc(len: usize) -> *mut u8 {
    let size = len.max(1);
    let Ok(layout) = Layout::from_size_align(size, 1) else {
        return ptr::null_mut();
    };
    // SAFETY: `size` is non-zero and the layout is valid.
    unsafe { alloc(layout) }
}

/// Free a buffer previously returned by [`con_alloc`].
///
/// # Safety
///
/// `ptr` must have come from [`con_alloc`] with the same `len`, and must not
/// have been consumed by [`con_init`] or already freed.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn con_free(ptr: *mut u8, len: usize) {
    if ptr.is_null() {
        return;
    }
    let size = len.max(1);
    let Ok(layout) = Layout::from_size_align(size, 1) else {
        return;
    };
    // SAFETY: guaranteed by the caller; layout matches `con_alloc`.
    unsafe { dealloc(ptr, layout) }
}

/// Load a cart (UTF-8 text) with seed 0 and run its `_init`.
///
/// Returns 0 on success, -1 on failure (see [`con_error`]). **Takes ownership
/// of `cart_ptr`**: the buffer is freed before returning, in both the success
/// and failure cases.
///
/// # Safety
///
/// `cart_ptr` must be a buffer of `cart_len` readable bytes returned by
/// [`con_alloc`] (with that same length), and must not be used afterwards.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn con_init(cart_ptr: *mut u8, cart_len: usize) -> i32 {
    // SAFETY: forwarded unchanged from this function's public contract.
    unsafe { con_init_save(cart_ptr, cart_len, ptr::null_mut(), 0) }
}

/// Load a cart with one optional canonical persisted document before `_init`.
///
/// Both non-null buffers are consumed and freed. `save_ptr == null` is valid
/// only when `save_len == 0` and means there is no initial save.
///
/// # Safety
///
/// Every non-null pointer must come from [`con_alloc`] with the corresponding
/// length and must not be used afterwards.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn con_init_save(
    cart_ptr: *mut u8,
    cart_len: usize,
    save_ptr: *mut u8,
    save_len: usize,
) -> i32 {
    CONSOLE.with(|slot| *slot.borrow_mut() = None);
    SAVE.with(|slot| slot.borrow_mut().clear());
    SAVE_REVISION.with(|slot| *slot.borrow_mut() = 0);
    SAVE_DIAGNOSTIC.with(|slot| *slot.borrow_mut() = None);

    if cart_ptr.is_null() {
        if !save_ptr.is_null() {
            unsafe { con_free(save_ptr, save_len) };
        }
        set_error(Some("con_init: null cart pointer"));
        return -1;
    }
    if save_ptr.is_null() && save_len != 0 {
        // Consume the valid cart allocation even when the second argument is
        // malformed; callers never retain ownership after entering init.
        unsafe { con_free(cart_ptr, cart_len) };
        set_error(Some("con_init_save: null save pointer with nonzero length"));
        return -1;
    }

    // SAFETY: caller guarantees `cart_len` readable bytes at `cart_ptr`.
    let bytes = unsafe { std::slice::from_raw_parts(cart_ptr, cart_len) }.to_vec();
    // SAFETY: caller guarantees the buffer came from `con_alloc(cart_len)`.
    unsafe { con_free(cart_ptr, cart_len) };

    let save_bytes = if save_ptr.is_null() {
        None
    } else {
        let bytes = unsafe { std::slice::from_raw_parts(save_ptr, save_len) }.to_vec();
        unsafe { con_free(save_ptr, save_len) };
        Some(bytes)
    };

    let text = String::from_utf8_lossy(&bytes).into_owned();
    let initial_save = save_bytes
        .as_deref()
        .map(String::from_utf8_lossy)
        .map(|value| value.into_owned());

    match Console::new_with_save(&text, 0, initial_save.as_deref()) {
        Ok(mut con) => {
            drain_logs(&mut con);
            sync_save(&con);
            CONSOLE.with(|slot| *slot.borrow_mut() = Some(con));
            set_error(None);
            0
        }
        Err(e) => {
            set_error(Some(&e.to_string()));
            -1
        }
    }
}

/// Advance one frame with the given input bitmask (bit 0 = left ... 5 = B).
///
/// Bits above 7 are ignored. A no-op if no cart is loaded. If the cart halts,
/// the error is latched into [`con_error`] and stays there (further steps keep
/// reporting it).
#[unsafe(no_mangle)]
pub extern "C" fn con_step(input_mask: u32) {
    CONSOLE.with(|slot| {
        let mut slot = slot.borrow_mut();
        let Some(con) = slot.as_mut() else {
            return;
        };
        let result = con.step((input_mask & 0xff) as u8);
        drain_logs(con);
        sync_save(con);
        match result {
            Ok(()) => set_error(None),
            Err(e) => set_error(Some(&e.to_string())),
        }
    });
}

/// Successful `save_store` / `save_clear` count for the current VM.
#[unsafe(no_mangle)]
pub extern "C" fn con_save_revision() -> u32 {
    SAVE_REVISION.with(|slot| *slot.borrow())
}

/// Byte length of the current canonical save document. Zero means cleared or absent.
#[unsafe(no_mangle)]
pub extern "C" fn con_save_len() -> usize {
    SAVE.with(|slot| slot.borrow().len())
}

/// Pointer to [`con_save_len`] canonical UTF-8 JSON bytes.
#[unsafe(no_mangle)]
pub extern "C" fn con_save() -> *const u8 {
    SAVE.with(|slot| slot.borrow().as_ptr())
}

/// NUL-terminated persistence diagnostic, or null. This never means the cart halted.
#[unsafe(no_mangle)]
pub extern "C" fn con_save_error() -> *const u8 {
    SAVE_DIAGNOSTIC.with(|slot| match slot.borrow().as_ref() {
        Some(message) => message.as_ptr().cast::<u8>(),
        None => ptr::null(),
    })
}

/// Logical framebuffer width in pixels.
#[unsafe(no_mangle)]
pub extern "C" fn con_width() -> usize {
    SCREEN_W
}

/// Logical framebuffer height in pixels.
#[unsafe(no_mangle)]
pub extern "C" fn con_height() -> usize {
    SCREEN_H
}

/// Pointer to `con_width() * con_height()` palette indices, row-major. Never null.
///
/// The pointer is stable across calls; the contents are refreshed per call.
/// Before a successful [`con_init`] the buffer reads as all zeros.
#[unsafe(no_mangle)]
pub extern "C" fn con_fb() -> *const u8 {
    FB.with(|fb| {
        let mut fb = fb.borrow_mut();
        if fb.len() != FB_LEN {
            // Allocated exactly once, so the pointer below never moves.
            fb.clear();
            fb.resize(FB_LEN, 0);
        }
        CONSOLE.with(|slot| {
            if let Some(con) = slot.borrow().as_ref() {
                fb.copy_from_slice(con.framebuffer());
            }
        });
        fb.as_ptr()
    })
}

/// Pointer to the latest frame's audio: [`SAMPLES_PER_FRAME`] mono f32 samples
/// at 44100 Hz, in [-1, 1]. Never null.
///
/// These are the samples rendered by the most recent [`con_step`]. The pointer
/// is stable across calls and 4-byte aligned (it is the data pointer of a
/// `Vec<f32>`), so JS can view it as `Float32Array(HEAPU8.buffer, p, 735)`.
/// Contents are refreshed per call: before the first [`con_step`] — and for a
/// halted console — the buffer reads as silence (all zeros).
#[unsafe(no_mangle)]
pub extern "C" fn con_audio() -> *const f32 {
    AUDIO.with(|audio| {
        let mut audio = audio.borrow_mut();
        if audio.len() != SAMPLES_PER_FRAME {
            // Allocated exactly once, so the pointer below never moves.
            audio.clear();
            audio.resize(SAMPLES_PER_FRAME, 0.0);
        }
        CONSOLE.with(|slot| {
            if let Some(con) = slot.borrow().as_ref() {
                audio.copy_from_slice(con.audio_frame());
            }
        });
        audio.as_ptr()
    })
}

/// Number of entries exposed by [`con_palette`] and [`con_dpal`].
#[unsafe(no_mangle)]
pub extern "C" fn con_color_count() -> usize {
    COLOR_COUNT
}

/// Pointer to the fixed `con_color_count() * 3` RGB palette. Never null.
#[unsafe(no_mangle)]
pub extern "C" fn con_palette() -> *const u8 {
    PALETTE_RGB.as_ptr().cast::<u8>()
}

/// Pointer to the `con_color_count()`-byte display palette: an index -> index map the host
/// applies at scanout (`pal(c0, c1, 1)` in Lua). Never null.
///
/// The framebuffer from [`con_fb`] always holds raw draw-space indices; the
/// shell composes `palette[dpal[idx]]`. Identity unless the cart has
/// remapped it, and before any successful [`con_init`]. The pointer is stable
/// across calls; the contents are refreshed per call.
#[unsafe(no_mangle)]
pub extern "C" fn con_dpal() -> *const u8 {
    DPAL.with(|slot| {
        CONSOLE.with(|console| {
            if let Some(con) = console.borrow().as_ref() {
                *slot.borrow_mut() = *con.display_palette();
            }
        });
        // Points into the thread-local's own storage, which never moves.
        slot.as_ptr().cast::<u8>()
    })
}

/// NUL-terminated UTF-8 error message, or null when there is no error.
///
/// Covers both `con_init` failures and runtime halts. Valid until the next
/// `con_init` / `con_step`.
#[unsafe(no_mangle)]
pub extern "C" fn con_error() -> *const u8 {
    ERROR.with(|slot| match slot.borrow().as_ref() {
        // The CString stays owned by the thread-local, so the pointer outlives
        // this borrow; see the module-level lifetime contract.
        Some(msg) => msg.as_ptr().cast::<u8>(),
        None => ptr::null(),
    })
}

fn main() {
    // Intentionally empty: this binary exists to be built for
    // wasm32-unknown-emscripten; the real surface is the con_* C ABI.
}
