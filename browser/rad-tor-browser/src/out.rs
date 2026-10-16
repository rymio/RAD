//! The framed protocol stream to the app (see the crate docs).

use std::io::{BufWriter, Write};

pub struct Out {
    w: BufWriter<Box<dyn Write>>,
}

impl Out {
    /// Takes over stdout for the protocol and points file descriptor 1 at stderr, so stray
    /// prints from Servo or its libraries end up in the log instead of corrupting the stream.
    pub fn take_stdout() -> Self {
        #[cfg(unix)]
        let sink: Box<dyn Write> = unsafe {
            use std::os::fd::FromRawFd;
            let fd = libc::dup(1);
            libc::dup2(2, 1);
            Box::new(std::fs::File::from_raw_fd(fd))
        };
        #[cfg(not(unix))]
        let sink: Box<dyn Write> = Box::new(std::io::stdout());
        Out { w: BufWriter::with_capacity(1 << 20, sink) }
    }

    fn message(&mut self, kind: u8, parts: &[&[u8]]) {
        let len: usize = parts.iter().map(|p| p.len()).sum();
        let _ = self.w.write_all(&[kind]);
        let _ = self.w.write_all(&(len as u32).to_le_bytes());
        for p in parts {
            let _ = self.w.write_all(p);
        }
    }

    pub fn event(&mut self, v: &serde_json::Value) {
        let s = serde_json::to_vec(v).unwrap_or_default();
        self.message(1, &[&s]);
        let _ = self.w.flush();
    }

    pub fn frame(&mut self, w: u32, h: u32, y0: u32, rows: &[u8]) {
        let n = (rows.len() / (w as usize * 4).max(1)) as u32;
        let mut head = [0u8; 16];
        for (i, v) in [w, h, y0, n].into_iter().enumerate() {
            head[i * 4..i * 4 + 4].copy_from_slice(&v.to_le_bytes());
        }
        self.message(2, &[&head, rows]);
    }

    pub fn flush(&mut self) -> std::io::Result<()> {
        self.w.flush()
    }
}
