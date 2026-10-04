use super::guards::*;
use coralspy_hook_client::{CaptureError, ErrorCode};
use coralspy_hook_protocol as p;
use std::{
    fs::{File, OpenOptions},
    io::{Read, Write},
    os::windows::{ffi::OsStrExt, fs::OpenOptionsExt},
    path::PathBuf,
    ptr::{addr_of_mut, null_mut},
    sync::atomic::Ordering,
};
use windows_sys::Win32::{
    Foundation::*,
    Security::Cryptography::*,
    Storage::FileSystem::*,
    System::{LibraryLoader::*, Memory::*},
};

pub fn nonce() -> Result<[u8; 16], CaptureError> {
    unsafe {
        let mut n = [0; 16];
        if BCryptGenRandom(
            null_mut(),
            n.as_mut_ptr(),
            16,
            BCRYPT_USE_SYSTEM_PREFERRED_RNG,
        ) < 0
            || n == [0; 16]
        {
            return Err(CaptureError::new(
                ErrorCode::OsError,
                "Cannot create an isolated capture nonce",
            ));
        }
        Ok(n)
    }
}
pub struct Mapping {
    pub ptr: *mut p::SharedMemory,
    _handle: Handle,
}
impl Mapping {
    pub fn new(req: p::RequestHeader, security: &PrivateSecurity) -> Result<Self, CaptureError> {
        unsafe {
            let name = p::mapping_name(&req.nonce)
                .into_iter()
                .map(u16::from)
                .chain(Some(0))
                .collect::<Vec<_>>();
            let sa = security.attributes();
            let h = CreateFileMappingW(
                INVALID_HANDLE_VALUE,
                &sa,
                PAGE_READWRITE,
                0,
                p::SHARED_MEMORY_BYTES,
                name.as_ptr(),
            );
            if h.is_null() {
                return Err(last(
                    ErrorCode::Ipc,
                    "Cannot create private capture mapping",
                ));
            }
            let error = GetLastError();
            let handle = Handle(h);
            if error == ERROR_ALREADY_EXISTS {
                return Err(CaptureError::new(
                    ErrorCode::Ipc,
                    "Random IPC name collision; capture refused",
                ));
            }
            let view = MapViewOfFile(
                h,
                FILE_MAP_READ | FILE_MAP_WRITE,
                0,
                0,
                p::SHARED_MEMORY_BYTES as usize,
            );
            if view.Value.is_null() {
                return Err(last(ErrorCode::Ipc, "Cannot map capture IPC"));
            }
            let ptr = view.Value as *mut p::SharedMemory;
            // Fresh paging-file mappings are zero-filled. Do not materialize 1 MiB on the stack.
            addr_of_mut!((*ptr).request).write(req);
            (*ptr).state.store(p::STATE_PENDING, Ordering::Release);
            Ok(Self {
                ptr,
                _handle: handle,
            })
        }
    }
}
impl Drop for Mapping {
    fn drop(&mut self) {
        unsafe {
            UnmapViewOfFile(MEMORY_MAPPED_VIEW_ADDRESS {
                Value: self.ptr as _,
            });
        }
    }
}
pub struct EmbeddedDll {
    pub path: PathBuf,
    dir: PathBuf,
    _lock: Option<File>,
}
impl EmbeddedDll {
    pub fn create(n: &[u8; 16], security: &PrivateSecurity) -> Result<Self, CaptureError> {
        let embedded = include_bytes!(concat!(env!("OUT_DIR"), "/payload.dll"));
        let nonce = String::from_utf8(p::nonce_hex(n).to_vec()).unwrap();
        let dir = std::env::temp_dir().join(format!("CoralSpyNext-{nonce}"));
        let dirw = dir
            .as_os_str()
            .encode_wide()
            .chain(Some(0))
            .collect::<Vec<_>>();
        let sa = security.attributes();
        if unsafe { CreateDirectoryW(dirw.as_ptr(), &sa) } == 0 {
            return Err(last(
                ErrorCode::AccessDenied,
                "Cannot create private one-shot payload directory",
            ));
        }
        let basename = String::from_utf8(p::hook_filename(n).to_vec()).unwrap();
        let path = dir.join(basename);
        let mut output = Self {
            path,
            dir,
            _lock: None,
        };
        // Create-new in an owner-only directory; never follow an existing module path.
        let mut file = OpenOptions::new()
            .create_new(true)
            .read(true)
            .write(true)
            .share_mode(FILE_SHARE_READ)
            .open(&output.path)
            .map_err(|e| {
                CaptureError::new(
                    ErrorCode::OsError,
                    format!("Cannot extract built-in payload: {e}"),
                )
            })?;
        file.write_all(embedded)
            .and_then(|_| file.flush())
            .map_err(|e| CaptureError::new(ErrorCode::OsError, e.to_string()))?;
        drop(file);
        let mut file = OpenOptions::new()
            .read(true)
            .share_mode(FILE_SHARE_READ)
            .open(&output.path)
            .map_err(|e| CaptureError::new(ErrorCode::OsError, e.to_string()))?;
        if file
            .metadata()
            .map_err(|e| CaptureError::new(ErrorCode::OsError, e.to_string()))?
            .len()
            != embedded.len() as u64
        {
            return Err(CaptureError::new(
                ErrorCode::AccessDenied,
                "Embedded payload size changed",
            ));
        }
        let mut actual = Vec::new();
        std::io::Read::by_ref(&mut file)
            .take(embedded.len() as u64 + 1)
            .read_to_end(&mut actual)
            .map_err(|e| CaptureError::new(ErrorCode::OsError, e.to_string()))?;
        // Byte equality is stronger than a digest: only the exact compiled-in payload is loaded.
        if actual.as_slice() != embedded {
            return Err(CaptureError::new(
                ErrorCode::AccessDenied,
                "Embedded payload verification failed",
            ));
        }
        // Keep the no-write/no-delete sharing lock throughout LoadLibrary, hook, and unhook.
        output._lock = Some(file);
        Ok(output)
    }
}
impl Drop for EmbeddedDll {
    fn drop(&mut self) {
        self._lock.take();
        let _ = std::fs::remove_file(&self.path);
        let _ = std::fs::remove_dir(&self.dir);
    }
}
pub struct Module(pub HMODULE);
impl Module {
    pub fn load(dll: &EmbeddedDll) -> Result<Self, CaptureError> {
        unsafe {
            let path = dll
                .path
                .as_os_str()
                .encode_wide()
                .chain(Some(0))
                .collect::<Vec<_>>();
            let h = LoadLibraryExW(
                path.as_ptr(),
                null_mut(),
                LOAD_LIBRARY_SEARCH_DLL_LOAD_DIR | LOAD_LIBRARY_SEARCH_SYSTEM32,
            );
            if h.is_null() {
                return Err(last(
                    ErrorCode::OsError,
                    "Cannot load the verified fixed payload",
                ));
            }
            Ok(Self(h))
        }
    }
}
impl Drop for Module {
    fn drop(&mut self) {
        unsafe {
            FreeLibrary(self.0);
        }
    }
}
