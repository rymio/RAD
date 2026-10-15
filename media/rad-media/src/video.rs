//! Pictures: libVLC decodes into our memory (its "vmem" output) and a writer thread sends the
//! newest frame to the app. Frames the app hasn't taken yet are replaced, never queued, so a
//! slow consumer drops frames instead of falling behind the sound.

use std::cell::UnsafeCell;
use std::ffi::{c_char, c_uint, c_void};
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::{Arc, Condvar, Mutex};

use vlc::sys::{self, libvlc_media_player_t};

use crate::out::Out;

/// Largest picture sent to the app (pixels); bigger videos are scaled down by libVLC.
const MAX_PIXELS: f64 = 1920.0 * 1080.0;

type FormatCb = unsafe extern "C" fn(*mut *mut c_void, *mut c_char, *mut c_uint, *mut c_uint, *mut c_uint, *mut c_uint) -> c_uint;
type LockCb = unsafe extern "C" fn(*mut c_void, *mut *mut c_void) -> *mut c_void;
type DisplayCb = unsafe extern "C" fn(*mut c_void, *mut c_void);

// Declared here rather than taken from `vlc::sys`: its format callback type lacks the return
// value (the number of picture buffers), which libVLC reads.
#[link(name = "vlc")]
extern "C" {
    fn libvlc_video_set_callbacks(mp: *mut libvlc_media_player_t, lock: Option<LockCb>, unlock: Option<DisplayCb>, display: Option<DisplayCb>, opaque: *mut c_void);
    fn libvlc_video_set_format_callbacks(mp: *mut libvlc_media_player_t, setup: Option<FormatCb>, cleanup: Option<unsafe extern "C" fn(*mut c_void)>);
}

pub struct Video {
    mp: *mut libvlc_media_player_t,
    /// The box the app shows video in (device pixels); 0 = no limit but MAX_PIXELS.
    max_w: AtomicU32,
    max_h: AtomicU32,
    /// Output size and the buffer libVLC draws into; touched only by libVLC's video thread.
    size: UnsafeCell<(u32, u32)>,
    pixels: UnsafeCell<Vec<u8>>,
    /// The newest finished frame, waiting for the writer.
    slot: Mutex<Option<(u32, u32, Vec<u8>)>>,
    spare: Mutex<Vec<Vec<u8>>>,
    ready: Condvar,
}

// The UnsafeCells are only used from libVLC's (single) video output thread; the player outlives
// the process' use of it.
unsafe impl Sync for Video {}

impl Video {
    /// Routes the player's pictures to `out`. The returned state lives for the whole process.
    pub fn attach(mp: *mut libvlc_media_player_t, out: Arc<Mutex<Out>>) -> &'static Video {
        let v: &'static Video = Box::leak(Box::new(Video {
            mp,
            max_w: AtomicU32::new(0),
            max_h: AtomicU32::new(0),
            size: UnsafeCell::new((0, 0)),
            pixels: UnsafeCell::new(Vec::new()),
            slot: Mutex::new(None),
            spare: Mutex::new(Vec::new()),
            ready: Condvar::new(),
        }));
        std::thread::spawn(move || v.writer(out));
        unsafe {
            libvlc_video_set_format_callbacks(mp, Some(format), None);
            libvlc_video_set_callbacks(mp, Some(lock), None, Some(display), v as *const Video as *mut c_void);
        }
        v
    }

    /// Takes effect when the next video starts.
    pub fn set_max(&self, w: u32, h: u32) {
        self.max_w.store(w, Ordering::Relaxed);
        self.max_h.store(h, Ordering::Relaxed);
    }

    fn writer(&self, out: Arc<Mutex<Out>>) {
        loop {
            let (w, h, mut px) = {
                let mut s = self.slot.lock().unwrap();
                loop {
                    if let Some(f) = s.take() {
                        break f;
                    }
                    s = self.ready.wait(s).unwrap();
                }
            };
            // libVLC leaves the alpha byte undefined; the canvas wants it opaque.
            for p in px.chunks_exact_mut(4) {
                p[3] = 255;
            }
            {
                let mut o = out.lock().unwrap();
                o.frame(w, h, 0, &px);
                if o.flush().is_err() {
                    std::process::exit(0);
                }
            }
            self.spare.lock().unwrap().push(px);
        }
    }
}

/// The current video track's size and pixel shape (sample aspect ratio). For some formats
/// (MPEG-PS) the size includes the decoder's padding.
pub unsafe fn track_shape(mp: *mut libvlc_media_player_t) -> Option<(f64, f64, f64)> {
    let md = sys::libvlc_media_player_get_media(mp);
    if md.is_null() {
        return None;
    }
    let mut tracks = std::ptr::null_mut();
    let n = sys::libvlc_media_tracks_get(md, &mut tracks);
    let mut found = None;
    for i in 0..n as usize {
        let t = *tracks.add(i);
        if t.is_null() || (*t).i_type != sys::libvlc_track_type_t::Video {
            continue;
        }
        let vt = (*t).video();
        if vt.is_null() || (*vt).i_width == 0 || (*vt).i_height == 0 {
            continue;
        }
        let sar = if (*vt).i_sar_num > 0 && (*vt).i_sar_den > 0 { (*vt).i_sar_num as f64 / (*vt).i_sar_den as f64 } else { 1.0 };
        found = Some(((*vt).i_width as f64, (*vt).i_height as f64, sar));
        break;
    }
    if n > 0 {
        sys::libvlc_media_tracks_release(tracks, n);
    }
    sys::libvlc_media_release(md);
    found
}

/// Picks the output size (the video fitted into the app's box) and asks for RGBA. libVLC offers
/// the decoder's size, which may include padding; whatever we answer, it scales the visible
/// picture to fill it, so the answer follows the track's shape (anamorphic video stretched out).
/// Where that is still slightly off, the app draws frames at the `aspect` the status reports.
unsafe extern "C" fn format(opaque: *mut *mut c_void, chroma: *mut c_char, width: *mut c_uint, height: *mut c_uint, pitches: *mut c_uint, lines: *mut c_uint) -> c_uint {
    let v = &*(*opaque as *const Video);
    let (sw, sh) = track_shape(v.mp).map(|(w, h, sar)| (w * sar, h)).unwrap_or((*width as f64, *height as f64));
    if sw < 1.0 || sh < 1.0 {
        return 0;
    }
    let (bw, bh) = (v.max_w.load(Ordering::Relaxed) as f64, v.max_h.load(Ordering::Relaxed) as f64);
    let mut scale = (MAX_PIXELS / (sw * sh)).sqrt().min(1.0);
    if bw >= 1.0 && bh >= 1.0 {
        scale = scale.min(bw / sw).min(bh / sh);
    }
    let w = (((sw * scale) as u32) & !1).max(2);
    let h = (((sh * scale) as u32) & !1).max(2);
    std::ptr::copy_nonoverlapping(b"RGBA".as_ptr() as *const c_char, chroma, 4);
    *width = w;
    *height = h;
    *pitches = w * 4;
    *lines = h;
    *v.size.get() = (w, h);
    let px = &mut *v.pixels.get();
    px.clear();
    px.resize(w as usize * h as usize * 4, 0);
    1
}

unsafe extern "C" fn lock(opaque: *mut c_void, planes: *mut *mut c_void) -> *mut c_void {
    let v = &*(opaque as *const Video);
    *planes = (*v.pixels.get()).as_mut_ptr() as *mut c_void;
    std::ptr::null_mut()
}

unsafe extern "C" fn display(opaque: *mut c_void, _picture: *mut c_void) {
    let v = &*(opaque as *const Video);
    let (w, h) = *v.size.get();
    let src = &*v.pixels.get();
    let mut buf = v.spare.lock().unwrap().pop().unwrap_or_default();
    buf.clear();
    buf.extend_from_slice(src);
    let old = v.slot.lock().unwrap().replace((w, h, buf));
    if let Some((_, _, old)) = old {
        v.spare.lock().unwrap().push(old);
    }
    v.ready.notify_one();
}
