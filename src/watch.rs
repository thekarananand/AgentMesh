//! Wakes the session poll when the agent CLI writes a pid file, so a status flip (busy, waiting,
//! idle) shows within ~100 ms instead of at the next 2 s poll. FSEvents, called directly: no
//! crate in the lock wraps it. Any failure leaves the poll alone, which still works.

use std::{
    ffi::{CString, c_char, c_void},
    os::unix::ffi::OsStrExt,
    path::Path,
};

use futures::channel::mpsc::UnboundedSender;

type CFIndex = isize;
type CFRef = *const c_void;
type FSEventStreamRef = *mut c_void;
type DispatchQueue = *mut c_void;

#[repr(C)]
struct FSEventStreamContext {
    version: CFIndex,
    info: *mut c_void,
    retain: Option<extern "C" fn(*const c_void) -> *const c_void>,
    release: Option<extern "C" fn(*const c_void)>,
    copy_description: Option<extern "C" fn(*const c_void) -> CFRef>,
}

type FSEventStreamCallback = extern "C" fn(
    stream: FSEventStreamRef,
    info: *mut c_void,
    count: usize,
    paths: *mut c_void,
    flags: *const u32,
    ids: *const u64,
);

const SINCE_NOW: u64 = u64::MAX;
const NO_DEFER: u32 = 0x2;
const FILE_EVENTS: u32 = 0x10;
const UTF8: u32 = 0x0800_0100;
/// Seconds FSEvents batches events for; a status flip rewrites its file more than once.
const LATENCY: f64 = 0.1;

#[link(name = "CoreFoundation", kind = "framework")]
unsafe extern "C" {
    static kCFTypeArrayCallBacks: c_void;
    fn CFStringCreateWithCString(allocator: CFRef, text: *const c_char, encoding: u32) -> CFRef;
    fn CFArrayCreate(
        allocator: CFRef,
        values: *const CFRef,
        count: CFIndex,
        callbacks: *const c_void,
    ) -> CFRef;
    fn CFRelease(object: CFRef);
}

#[link(name = "CoreServices", kind = "framework")]
unsafe extern "C" {
    fn FSEventStreamCreate(
        allocator: CFRef,
        callback: FSEventStreamCallback,
        context: *const FSEventStreamContext,
        paths: CFRef,
        since: u64,
        latency: f64,
        flags: u32,
    ) -> FSEventStreamRef;
    fn FSEventStreamSetDispatchQueue(stream: FSEventStreamRef, queue: DispatchQueue);
    fn FSEventStreamStart(stream: FSEventStreamRef) -> u8;
    fn FSEventStreamStop(stream: FSEventStreamRef);
    fn FSEventStreamInvalidate(stream: FSEventStreamRef);
    fn FSEventStreamRelease(stream: FSEventStreamRef);
}

unsafe extern "C" {
    fn dispatch_queue_create(label: *const c_char, attributes: *const c_void) -> DispatchQueue;
    fn dispatch_release(object: DispatchQueue);
}

extern "C" fn on_events(
    _: FSEventStreamRef,
    info: *mut c_void,
    _: usize,
    _: *mut c_void,
    _: *const u32,
    _: *const u64,
) {
    // SAFETY: `info` is the `Box<UnboundedSender<()>>` given at creation; FSEvents keeps it
    // alive until `release_info` runs, after the last callback.
    let wake = unsafe { &*(info as *const UnboundedSender<()>) };
    wake.unbounded_send(()).ok();
}

extern "C" fn release_info(info: *const c_void) {
    // SAFETY: called once by FSEvents when the stream is released; `info` came from
    // `Box::into_raw` in `Watcher::new`.
    drop(unsafe { Box::from_raw(info as *mut UnboundedSender<()>) });
}

/// Sends on `wake` whenever anything under `dir` changes. Stops when dropped.
pub struct Watcher {
    stream: FSEventStreamRef,
    queue: DispatchQueue,
}

impl Watcher {
    pub fn new(dir: &Path, wake: UnboundedSender<()>) -> Option<Self> {
        let path = CString::new(dir.as_os_str().as_bytes()).ok()?;
        let info = Box::into_raw(Box::new(wake));
        let context = FSEventStreamContext {
            version: 0,
            info: info.cast(),
            retain: None,
            release: Some(release_info),
            copy_description: None,
        };
        // SAFETY: plain CoreFoundation/FSEvents calls; every created object is released below
        // or owned by the returned `Watcher`.
        unsafe {
            let text = CFStringCreateWithCString(std::ptr::null(), path.as_ptr(), UTF8);
            if text.is_null() {
                drop(Box::from_raw(info));
                return None;
            }
            let paths = CFArrayCreate(std::ptr::null(), &text, 1, &kCFTypeArrayCallBacks);
            CFRelease(text);
            if paths.is_null() {
                drop(Box::from_raw(info));
                return None;
            }
            let stream = FSEventStreamCreate(
                std::ptr::null(),
                on_events,
                &context,
                paths,
                SINCE_NOW,
                LATENCY,
                NO_DEFER | FILE_EVENTS,
            );
            CFRelease(paths);
            if stream.is_null() {
                drop(Box::from_raw(info));
                return None;
            }
            // From here the stream owns `info` and frees it through `release_info`.
            let queue = dispatch_queue_create(c"agentmesh.watch".as_ptr(), std::ptr::null());
            FSEventStreamSetDispatchQueue(stream, queue);
            let watcher = Self { stream, queue };
            (FSEventStreamStart(stream) != 0).then_some(watcher)
        }
    }
}

impl Drop for Watcher {
    fn drop(&mut self) {
        // SAFETY: `stream` and `queue` were created in `new` and are released only here.
        unsafe {
            FSEventStreamStop(self.stream);
            FSEventStreamInvalidate(self.stream);
            FSEventStreamRelease(self.stream);
            dispatch_release(self.queue);
        }
    }
}

#[cfg(test)]
mod tests {
    use std::{fs, time::Duration};

    use futures::channel::mpsc::unbounded;

    use super::*;

    #[test]
    fn writes_wake_the_receiver() {
        let dir = std::env::temp_dir().join(format!("ccp-watch-{}", std::process::id()));
        fs::create_dir_all(&dir).unwrap();
        // FSEvents reports real paths (`/private/var/...`).
        let dir = fs::canonicalize(&dir).unwrap();
        let (wake, mut woken) = unbounded();
        let watcher = Watcher::new(&dir, wake).unwrap();
        std::thread::sleep(Duration::from_millis(200));
        fs::write(dir.join("123.json"), "{}").unwrap();
        let mut got = false;
        for _ in 0..40 {
            if woken.try_recv().is_ok() {
                got = true;
                break;
            }
            std::thread::sleep(Duration::from_millis(50));
        }
        drop(watcher);
        fs::remove_dir_all(&dir).ok();
        assert!(got, "no FSEvents wake-up within 2 s");
    }
}
