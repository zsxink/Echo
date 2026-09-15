//! Minimal, isolated `unsafe` FFI binding over the vendored libmpv (task 8.2).
//!
//! **Scope and safety:** this is the *only* module in `echo-desktop` that
//! touches libmpv C types or `unsafe`. It dynamically loads the vendored
//! `libmpv` from the app bundle (macOS `@rpath`/Frameworks) via `libloading`,
//! declares the minimal function symbols the actor needs, and types the few
//! opaque structs/constants the calls require. No FFI type crosses this module
//! boundary — the actor sees the handle only as an opaque `*mut c_void` inside
//! a private [`Handle`], never on its public API.
//!
//! **Scope decision (task 8.2):** this binding covers *handle + file-load +
//! event observation* — everything the actor needs to own libmpv on a
//! dedicated thread and prove unique-handle / generation-discard / ordered
//! destruction. Numeric property *writes* (volume, mute, pause, seek) belong to
//! task 8.8 and will extend this module there; they intentionally use string
//! property forms or a dedicated bridge that stable Rust can call (Rust cannot
//! declare C variadic functions), so no speculative variadic ABI is added here.
//!
//! **Safety preconditions** (design §11 "最小 FFI 模块隔离 `unsafe`"):
//! - libmpv is loaded once; the handle is created and destroyed by the
//!   single-threaded actor (the only caller).
//! - All function pointers resolve once at load time; the loader `Library` is
//!   kept alive as a field so every symbol and handle outlives it.
//! - `mpv_wait_event` / `mpv_observe_property` / `mpv_command` are called only
//!   from the actor thread, never concurrently.
//!
//! Constant values follow `mpv/client.h` (pinned by the 1.10 Gate).

use std::ffi::{CStr, CString};
use std::os::raw::{c_char, c_double, c_int, c_void};

/// Dynamically loaded libmpv with every resolved function pointer.
pub struct MpvSys {
    // The loader must outlive every resolved symbol and handle we create; we
    // hold it so drop ordering tears the handle down before the library.
    #[allow(dead_code)] // kept solely for drop ordering
    _lib: libloading::Library,
    /// `mpv_create` — make a new handle.
    pub create: unsafe extern "C" fn() -> *mut c_void,
    /// `mpv_initialize` — finish setup; 0 on success.
    pub initialize: unsafe extern "C" fn(*mut c_void) -> c_int,
    /// `mpv_terminate_destroy` — terminate and destroy the handle.
    pub terminate_destroy: unsafe extern "C" fn(*mut c_void),
    /// `mpv_destroy` — destroy without terminating (used when init failed).
    pub destroy: unsafe extern "C" fn(*mut c_void),
    /// `mpv_command` — run a command; `args` is a NUL-terminated char* array.
    pub command: unsafe extern "C" fn(*mut c_void, *const *const c_char) -> c_int,
    /// `mpv_set_option_string` — set an option by name/value BEFORE
    /// `mpv_initialize` (used for the audio-only hardening in task 8.3).
    pub set_option_string: unsafe extern "C" fn(*mut c_void, *const c_char, *const c_char) -> c_int,
    /// `mpv_set_property_string` — set a runtime property by name/value.
    pub set_property_string:
        unsafe extern "C" fn(*mut c_void, *const c_char, *const c_char) -> c_int,
    /// `mpv_observe_property` — subscribe to a property; returns 0 on success.
    pub observe_property: unsafe extern "C" fn(*mut c_void, u64, *const c_char, c_int) -> c_int,
    /// `mpv_wait_event` — block for the next event (timeout in seconds).
    pub wait_event: unsafe extern "C" fn(*mut c_void, c_double) -> *mut mpv_event,
    /// `mpv_wakeup` — wake the event loop (thread-safe exact wakeup).
    pub wakeup: unsafe extern "C" fn(*mut c_void),
    /// `mpv_client_api_version` — the client API version, as a sanity check.
    pub client_api_version: unsafe extern "C" fn() -> u64,
}

// SAFETY: `MpvSys` holds only resolved function pointers and the `Library` that
// loaded them. The function pointers are pure `extern "C"` thunks (Callable,
// `Send`); the `Library` is `Send + Sync` per libloading. The caller (the actor)
// guarantees all calls happen on one thread, so sharing the sys across the actor
// boundary is sound.
unsafe impl Send for MpvSys {}
unsafe impl Sync for MpvSys {}

/// mpv event IDs (`MPV_EVENT_*`) the actor observes. Values from client.h.
pub mod event_id {
    pub const NONE: i32 = 0;
    pub const SHUTDOWN: i32 = 1;
    /// `MPV_EVENT_START_FILE = 6` (note: `LOG_MESSAGE` is 2, unused here —
    /// log events fall through the catch-all arm).
    pub const START_FILE: i32 = 6;
    pub const END_FILE: i32 = 7;
    pub const FILE_LOADED: i32 = 8;
    pub const PROPERTY_CHANGE: i32 = 22;
    pub const ERROR: i32 = 26;
}

/// Property data format (`mpv_format`, subset used).
pub mod format_ {
    pub const NONE: i32 = 0;
    pub const STRING: i32 = 1;
    pub const OSD_STRING: i32 = 2;
    pub const FLAG: i32 = 3;
    pub const INT64: i32 = 4;
    pub const DOUBLE: i32 = 5;
}

/// `MPV_ERROR_*` codes (subset used by load/observe).
///
/// Note: `MPV_ERROR_OPTION_NOT_FOUND` and `MPV_ERROR_PROPERTY_FORMAT` share the
/// numeric value `-5` in libmpv's client API. `set_option_string` returns
/// `-5` when the option is *not supported by the compiled library* — distinct
/// from a rejected value, which is a different code. The tolerant handle
/// creation ([`Handle::create`]) uses `OPTION_NOT_FOUND` to skip hardening
/// options that a capability-reduced build compiled out.
pub mod err_ {
    pub const SUCCESS: i32 = 0;
    pub const NOMEM: i32 = -3;
    pub const PROPERTY_NOT_FOUND: i32 = -4;
    /// `-5` is both `MPV_ERROR_OPTION_NOT_FOUND` (for `set_option_string`) and
    /// `MPV_ERROR_PROPERTY_FORMAT` (for property reads); kept under the role it
    /// plays in this module.
    pub const PROPERTY_FORMAT: i32 = -5;
    /// `MPV_ERROR_OPTION_NOT_FOUND = -5`: the option does not exist in this
    /// build (a capability compiled out), as opposed to a rejected value.
    pub const OPTION_NOT_FOUND: i32 = -5;
    pub const PROPERTY_UNAVAILABLE: i32 = -6;
    pub const COMMAND: i32 = -9;
    pub const LOADING_FAILED: i32 = -17;
}

/// `MPV_END_FILE_REASON` values.
pub mod eof_reason {
    pub const EOF: i32 = 0;
    pub const STOP: i32 = 1;
    pub const QUIT: i32 = 2;
    pub const ERROR: i32 = 3;
    pub const REDIRECT: i32 = 4;
}

/// Payload of an `END_FILE` event — the head of `mpv/client.h`'s
/// `mpv_event_end_file`. We only ever read `reason` (first field), so the
/// trailing fields present in newer libmpv versions are intentionally not
/// mirrored: declaring fewer fields can never overread.
#[repr(C)]
pub struct mpv_event_end_file {
    pub reason: c_int,
    pub error: c_int,
}

/// One observed property (subset: the named fields we read; everything else is
/// opaque padding so we never dereference beyond what we own).
///
/// Layout **must** mirror `mpv/client.h`'s `mpv_event` exactly — including the
/// `reply_userdata` field the naive eye skips: `data` sits at offset 16, not 8.
/// Omitting `reply_userdata` once made every event's `data` read as the
/// userdata itself (null), silently dropping every `PROPERTY_CHANGE`.
#[repr(C)]
pub struct mpv_event {
    pub event_id: c_int,
    pub error: c_int,
    /// The `reply_userdata` we passed to `mpv_observe_property` (always 0).
    pub reply_userdata: u64,
    /// `event_id == PROPERTY_CHANGE` → `*mut mpv_event_property`.
    pub data: *mut c_void,
}

/// Payload of a `PROPERTY_CHANGE` event.
#[repr(C)]
pub struct mpv_event_property {
    pub name: *const c_char,
    pub format: c_int,
    /// Pointer to the value (READ ONLY); interpreted per `format`.
    pub data: *const c_void,
}

impl MpvSys {
    /// Load libmpv from `path` and resolve the minimal symbol set.
    ///
    /// # Errors
    ///
    /// [`MpvLoadError::Load`] if the dylib cannot be loaded; [`MpvLoadError::Symbol`]
    /// if a required symbol is absent (ABI drift from the pinned 1.10 manifest).
    ///
    /// # Safety
    ///
    /// `path` must name a trusted, pinned libmpv artifact; the loaded library
    /// is held for the `MpvSys` lifetime.
    pub unsafe fn load(path: &std::path::Path) -> Result<Self, MpvLoadError> {
        // SAFETY: the returned Library owns the dylib; every symbol stays valid
        // while `_lib` lives (we hold it as a field).
        let lib = unsafe { libloading::Library::new(path) }.map_err(MpvLoadError::Load)?;

        macro_rules! sym {
            ($name:literal, $ty:ty) => {{
                let name_bytes: &[u8] = $name;
                // SAFETY: `get` returns a reference to the symbol of `$ty`;
                // function-pointer symbols are `Copy`, so we deref to the fn.
                *unsafe { lib.get::<$ty>(name_bytes) }.map_err(|e| MpvLoadError::Symbol {
                    name: String::from_utf8_lossy(name_bytes).into_owned(),
                    source: e,
                })?
            }};
        }

        // SAFETY: all pointer types below match mpv/client.h; verified by the
        // 1.10 Gate's pinned ABI/checksum manifest.
        // `_lib` moves `lib`, so it must be the LAST field (Rust evaluates
        // struct fields in order; the symbol fields below still borrow `lib`).
        let sys = MpvSys {
            create: sym!(b"mpv_create\0", unsafe extern "C" fn() -> *mut c_void),
            initialize: sym!(
                b"mpv_initialize\0",
                unsafe extern "C" fn(*mut c_void) -> c_int
            ),
            terminate_destroy: sym!(
                b"mpv_terminate_destroy\0",
                unsafe extern "C" fn(*mut c_void)
            ),
            destroy: sym!(b"mpv_destroy\0", unsafe extern "C" fn(*mut c_void)),
            command: sym!(
                b"mpv_command\0",
                unsafe extern "C" fn(*mut c_void, *const *const c_char) -> c_int
            ),
            set_option_string: sym!(
                b"mpv_set_option_string\0",
                unsafe extern "C" fn(*mut c_void, *const c_char, *const c_char) -> c_int
            ),
            set_property_string: sym!(
                b"mpv_set_property_string\0",
                unsafe extern "C" fn(*mut c_void, *const c_char, *const c_char) -> c_int
            ),
            observe_property: sym!(
                b"mpv_observe_property\0",
                unsafe extern "C" fn(*mut c_void, u64, *const c_char, c_int) -> c_int
            ),
            wait_event: sym!(
                b"mpv_wait_event\0",
                unsafe extern "C" fn(*mut c_void, c_double) -> *mut mpv_event
            ),
            wakeup: sym!(b"mpv_wakeup\0", unsafe extern "C" fn(*mut c_void)),
            client_api_version: sym!(b"mpv_client_api_version\0", unsafe extern "C" fn() -> u64),
            _lib: lib,
        };
        Ok(sys)
    }
}

/// Errors loading/resolving the packaged libmpv.
#[derive(Debug, thiserror::Error)]
pub enum MpvLoadError {
    /// The dylib could not be loaded (not found, wrong arch, missing deps).
    #[error("libmpv could not be loaded: {0}")]
    Load(#[source] libloading::Error),
    /// A required symbol is missing (ABI drift from the pinned 1.10 manifest).
    #[error("libmpv symbol {name:?} missing: {source}")]
    Symbol {
        name: String,
        #[source]
        source: libloading::Error,
    },
}

/// A thin wrapper over a live, initialized libmpv handle. Owned exclusively by
/// the actor thread; never exposed publicly.
pub struct Handle {
    raw: *mut c_void,
    /// Set once `terminate_destroy` runs so `Drop` stays a no-op (no double
    /// free). Since the actor owns teardown, `Drop` never calls FFI.
    terminated: bool,
}

// libmpv handles are thread-confined; the actor ensures this. We do not
// implement `Send`/`Sync` so the type system forces the actor boundary.
// (Safety note: the actor thread is the sole owner; see design §11.)

impl Handle {
    /// Create a new libmpv handle, apply the given `options` (audio-only /
    /// hardening, set BEFORE `mpv_initialize`), then initialize.
    ///
    /// `required` must all succeed — a violation of a real hardening/safety
    /// boundary (e.g. `config=no`) is a hard error. `optional` targets
    /// capabilities that may be *compiled out* of a specific libmpv build
    /// (e.g. the `audio-default` vendor build lacks Lua scripts, yt-dl and the
    /// OSC — its `set_option_string` returns `MPV_ERROR_OPTION_NOT_FOUND`); an
    /// option that the loaded library does not recognize (`-5`) is skipped with
    /// a warning instead of failing the handle, because an unavailable
    /// capability already satisfies the “disable it” hardening intent. Any
    /// other option error is still fatal.
    ///
    /// # Errors
    ///
    /// [`HandleError::Create`] if `mpv_create` returns null;
    /// [`HandleError::Option{name}`] if a *required* option (or an `optional`
    /// one with a non-`-5` error) cannot be set;
    /// [`HandleError::Initialize(code)`] if `mpv_initialize` fails. The newly
    /// created handle is released before returning on any failure.
    ///
    /// # Safety
    ///
    /// Must be called on the thread that will own the handle for its lifetime.
    pub unsafe fn create(
        sys: &MpvSys,
        required: &[(&str, &str)],
        optional: &[(&str, &str)],
    ) -> Result<Self, HandleError> {
        // SAFETY: trivial FFI calls with the returned handle; no aliasing.
        let raw = unsafe { (sys.create)() };
        if raw.is_null() {
            return Err(HandleError::Create);
        }
        // Apply pre-init options; any failure releases the handle.
        let apply = |name: &str, value: &str| -> Result<i32, HandleError> {
            let cname = CString::new(name).expect("option name has no NUL");
            let cvalue = CString::new(value).expect("option value has no NUL");
            // SAFETY: valid handle, NUL-clean strings, pre-init phase.
            Ok(unsafe { (sys.set_option_string)(raw, cname.as_ptr(), cvalue.as_ptr()) })
        };
        for (name, value) in required {
            match apply(name, value) {
                Ok(code) if code != err_::SUCCESS => {
                    // SAFETY: release the partially-configured handle.
                    unsafe { (sys.terminate_destroy)(raw) };
                    return Err(HandleError::Option {
                        name: (*name).to_owned(),
                        code,
                    });
                }
                Ok(_) => {}
                Err(e) => {
                    // SAFETY: release the partially-configured handle.
                    unsafe { (sys.terminate_destroy)(raw) };
                    return Err(e);
                }
            }
        }
        for (name, value) in optional {
            match apply(name, value) {
                Ok(code) if code != err_::SUCCESS => {
                    if code == err_::OPTION_NOT_FOUND {
                        tracing::warn!(
                            name,
                            "mpv actor: hardening option NOT supported by this libmpv build; \
                             the capability is compiled out, skipping (its absence satisfies the \
                             'disable' intent)"
                        );
                    } else {
                        // SAFETY: release the partially-configured handle.
                        unsafe { (sys.terminate_destroy)(raw) };
                        return Err(HandleError::Option {
                            name: (*name).to_owned(),
                            code,
                        });
                    }
                }
                Ok(_) => {}
                Err(e) => {
                    // SAFETY: release the partially-configured handle.
                    unsafe { (sys.terminate_destroy)(raw) };
                    return Err(e);
                }
            }
        }
        let code = unsafe { (sys.initialize)(raw) };
        if code != err_::SUCCESS {
            // SAFETY: terminate_destroy release the just-created handle.
            unsafe { (sys.terminate_destroy)(raw) };
            return Err(HandleError::Initialize(code));
        }
        Ok(Handle {
            raw,
            terminated: false,
        })
    }

    /// An ABI sanity check: the client API version, exposed for the 1.10/8.12
    /// smoke to assert the loaded library is a compatible mpv build.
    #[must_use]
    pub fn api_version(&self, sys: &MpvSys) -> u64 {
        // SAFETY: the handle is valid and owned by the caller.
        unsafe { (sys.client_api_version)() }
    }

    /// Run a single command (`args` = verb + arguments, no trailing null).
    ///
    /// # Errors
    ///
    /// [`HandleError::Command(code)`] when mpv rejects the command.
    ///
    /// # Safety
    ///
    /// Must be called from the owning actor thread; `args` must be valid UTF-8
    /// NUL-clean C strings (see [`path_to_cstring`]).
    pub unsafe fn command(&self, sys: &MpvSys, args: &[CString]) -> Result<(), HandleError> {
        let mut ptrs: Vec<*const c_char> = args.iter().map(|s| s.as_ptr()).collect();
        ptrs.push(std::ptr::null());
        // SAFETY: the array is NUL-terminated and lives for the call.
        let code = unsafe { (sys.command)(self.raw, ptrs.as_ptr()) };
        if code != err_::SUCCESS {
            return Err(HandleError::Command(code));
        }
        Ok(())
    }

    /// Subscribe to a property with the given [format_]. Returns the mpv error.
    ///
    /// # Safety
    ///
    /// As [`Self::command`]. Property data arrives on the actor event loop.
    pub unsafe fn observe_property(
        &self,
        sys: &MpvSys,
        reply_userdata: u64,
        name: &CStr,
        format: c_int,
    ) -> Result<(), HandleError> {
        let code =
            unsafe { (sys.observe_property)(self.raw, reply_userdata, name.as_ptr(), format) };
        if code != err_::SUCCESS {
            return Err(HandleError::Observe {
                code,
                property: name.to_string_lossy().into_owned(),
            });
        }
        Ok(())
    }

    /// Wait for the next event; `timeout` in seconds (0.0 blocks indefinitely).
    ///
    /// # Safety
    ///
    /// Must only be called from the actor thread.
    pub unsafe fn wait_event(&self, sys: &MpvSys, timeout: f64) -> *mut mpv_event {
        // SAFETY: valid handle, actor-thread-confined.
        unsafe { (sys.wait_event)(self.raw, timeout) }
    }

    /// Wake the actor's event loop (libmpv marks `wakeup` as thread-safe).
    pub fn wakeup(&self, sys: &MpvSys) {
        // SAFETY: `mpv_wakeup` is documented thread-safe and takes a valid handle.
        unsafe { (sys.wakeup)(self.raw) };
    }

    /// Terminate and destroy the handle. Idempotent; after this the actor must
    /// not use the handle again.
    ///
    /// # Safety
    ///
    /// Actor-thread-confined; must be the last call on this handle.
    pub unsafe fn terminate(&mut self, sys: &MpvSys) {
        if !self.terminated {
            // SAFETY: valid handle, not yet terminated.
            unsafe { (sys.terminate_destroy)(self.raw) };
            self.terminated = true;
        }
    }
}

impl Drop for Handle {
    /// The actor calls [`Handle::terminate`] explicitly (it owns `sys`). If a
    /// handle is dropped without termination (e.g. init failed path is handled
    /// in `create`), `Drop` must not double-free: we require either `terminate`
    /// ran, or creation never completed. `Drop` is deliberately a no-op — the
    /// actor's ordered teardown (`terminate`) is the single free point. This
    /// keeps `Handle` free of a second `sys` reference while preserving the
    /// invariant of exactly one `terminate_destroy` per created handle.
    fn drop(&mut self) {}
}

// SAFETY: a libmpv handle is thread-confined; the actor is the only thread that
// creates, uses, and destroys it. It is moved across the thread boundary exactly
// once at spawn (factory closure), and confinement is enforced by the actor —
// it is never shared (`&`). Marking it `Send` lets the factory cross the spawn
// boundary, which is sound under the single-owner contract. We deliberately do
// NOT implement `Sync`: concurrent `&` access would be unsound.
unsafe impl Send for Handle {}

/// Errors creating/using a handle.
#[derive(Debug, thiserror::Error)]
pub enum HandleError {
    #[error("mpv_create returned a null handle")]
    Create,
    #[error("mpv_initialize failed with code {0}")]
    Initialize(i32),
    #[error("mpv option {name:?} set failed with code {code}")]
    Option { name: String, code: i32 },
    #[error("mpv command failed with code {0}")]
    Command(i32),
    #[error("mpv_observe_property failed with code {code} for {property:?}")]
    Observe { code: i32, property: String },
}

/// Read a `\0`-terminated C string into a Rust `String`.
///
/// # Safety
///
/// `ptr` must be null (→ `None`) or point to a valid NUL-terminated C string.
pub unsafe fn read_c_str(ptr: *const c_char) -> Option<String> {
    if ptr.is_null() {
        return None;
    }
    // SAFETY: caller guarantees a valid NUL-terminated C string.
    let cstr = unsafe { CStr::from_ptr(ptr) };
    Some(cstr.to_string_lossy().into_owned())
}

/// Convert a file path to a NUL-terminated C string for mpv (mpv takes UTF-8).
///
/// # Errors
///
/// [`FfiInputError::NonUtf8`] / [`FfiInputError::InteriorNul`] for invalid paths.
pub fn path_to_cstring(path: &std::path::Path) -> Result<CString, FfiInputError> {
    let s = path
        .to_str()
        .ok_or_else(|| FfiInputError::NonUtf8(path.display().to_string()))?;
    CString::new(s).map_err(|_| FfiInputError::InteriorNul)
}

/// Input errors converting Rust values into mpv's C-string domain.
#[derive(Debug, thiserror::Error)]
pub enum FfiInputError {
    #[error("path is not valid UTF-8: {0}")]
    NonUtf8(String),
    #[error("path contains an interior NUL")]
    InteriorNul,
}

/// Read a `f64` from an observed property's data pointer (format DOUBLE).
///
/// # Safety
///
/// `data` must be null or a valid `*const c_double` matching the declared
/// property format (the observer must not misreport the format).
pub unsafe fn read_double(data: *const c_void) -> Option<f64> {
    if data.is_null() {
        return None;
    }
    // SAFETY: caller guarantees a valid `f64` value per the property format.
    Some(unsafe { *(data as *const c_double) })
}

/// Read a `FLAG`-format property value as a `0.0`/`1.0` double.
///
/// `mute` and `pause` are flags natively. mpv *accepts* observing them as
/// `DOUBLE`, but every notification then carries `MPV_FORMAT_NONE` (verified
/// against the vendored libmpv) — subscribing to the flag format is the only
/// way the value actually arrives, and a flag's payload is a C `int`.
///
/// # Safety
///
/// `data` must be null or a valid `*const c_int` matching the declared
/// property format (the observer must not misreport the format).
pub unsafe fn read_flag(data: *const c_void) -> Option<f64> {
    if data.is_null() {
        return None;
    }
    // SAFETY: caller guarantees a valid `int` value per the property format.
    Some(f64::from(unsafe { *(data as *const std::os::raw::c_int) }))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn path_to_cstring_valid_utf8() {
        let p = std::path::Path::new("/音乐/晴天.flac");
        let c = path_to_cstring(p).expect("valid");
        assert_eq!(c.to_string_lossy(), "/音乐/晴天.flac");
    }

    #[test]
    fn path_to_cstring_rejects_interior_nul() {
        // A path cannot contain a NUL on any supported OS, but the defensive
        // rejection must hold if one somehow reaches here.
        let p = std::path::Path::new("/a\0b");
        assert!(path_to_cstring(p).is_err());
    }

    #[test]
    fn read_c_str_null_is_none() {
        // SAFETY: null pointer is explicitly allowed.
        let s = unsafe { read_c_str(std::ptr::null()) };
        assert!(s.is_none());
    }

    #[test]
    fn read_c_str_round_trips() {
        let c = CString::new("time-pos").unwrap();
        // SAFETY: `c` lives for the call and is NUL-terminated.
        let s = unsafe { read_c_str(c.as_ptr()) };
        assert_eq!(s.as_deref(), Some("time-pos"));
    }

    #[test]
    fn read_double_null_is_none_and_value_reads() {
        // SAFETY: null data is allowed → None.
        assert!(unsafe { read_double(std::ptr::null()) }.is_none());
        let v = 3.25;
        // SAFETY: `&v` is a valid f64 pointer with DOUBLE semantics.
        let got = unsafe { read_double(std::ptr::addr_of!(v).cast()) };
        assert_eq!(got, Some(3.25));
    }
}
