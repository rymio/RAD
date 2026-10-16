use pdfium_render::prelude::*;
use std::path::PathBuf;
use std::sync::mpsc;

pub enum Command {
    Open { path: PathBuf, password: Option<String> },
    RenderPage { page_index: u16, target_width: i32 },
}

pub enum Event {
    Opened { page_count: u16 },
    PasswordRequired,
    PageRendered { page_index: u16, width: u32, height: u32, rgba: Vec<u8> },
    Error(String),
}

pub struct PdfSession {
    pub command_tx: mpsc::Sender<Command>,
    pub events_rx: mpsc::Receiver<Event>,
}

/// Loading a PDF and rendering its pages happens entirely on a dedicated
/// OS thread, for the same underlying reason `terminal.rs`'s
/// `spawn_run_session` isolates RustPython's interpreter on its own
/// thread: `PdfDocument<'_>`'s lifetime is tied to the `&Pdfium` binding
/// that created it, which doesn't work cleanly as a value moved across
/// `spawn_blocking` calls one at a time. Keeping the binding and the
/// currently-open document alive together on one thread for as long as
/// the file stays open sidesteps that entirely -- only plain messages
/// (page indices, byte buffers, strings) ever cross the thread boundary.
///
/// Binding to Pdfium is **not** guaranteed to succeed: this is the only
/// native (non-Rust) runtime dependency anywhere in this workspace, and
/// most machines won't have the Pdfium shared library sitting next to
/// the `rymwin` binary until someone deliberately downloads one (see
/// INSTALL_AND_CLI_GUIDE.md). Deliberately never calls `Pdfium::default()`,
/// which panics if no library is found -- the fallible
/// `bind_to_library`/`bind_to_system_library` chain is used directly so a
/// missing library becomes an `Event::Error` the UI can show, not a
/// crash that takes down the rest of the desktop.
pub fn spawn_pdf_session() -> PdfSession {
    let (command_tx, command_rx) = mpsc::channel::<Command>();
    let (events_tx, events_rx) = mpsc::channel::<Event>();

    std::thread::spawn(move || {
        let bindings = Pdfium::bind_to_library(Pdfium::pdfium_platform_library_name_at_path("./"))
            .or_else(|_| Pdfium::bind_to_system_library());
        let bindings = match bindings {
            Ok(bindings) => bindings,
            Err(err) => {
                let _ = events_tx.send(Event::Error(format!(
                    "Pdfium library not found ({err}) -- see INSTALL_AND_CLI_GUIDE.md's PDF Reader section"
                )));
                // Nothing more this thread can usefully do -- drain
                // commands forever so the UI's sends never fail with a
                // disconnected-channel error, which would otherwise
                // read as a second, more confusing failure.
                while command_rx.recv().is_ok() {}
                return;
            }
        };
        let pdfium = Pdfium::new(bindings);
        let mut document: Option<PdfDocument> = None;

        while let Ok(command) = command_rx.recv() {
            match command {
                Command::Open { path, password } => {
                    match pdfium.load_pdf_from_file(&path, password.as_deref()) {
                        Ok(doc) => {
                            let page_count = doc.pages().len() as u16;
                            document = Some(doc);
                            let _ = events_tx.send(Event::Opened { page_count });
                        }
                        Err(PdfiumError::PdfiumLibraryInternalError(PdfiumInternalError::PasswordError)) => {
                            let _ = events_tx.send(Event::PasswordRequired);
                        }
                        Err(err) => {
                            let _ = events_tx.send(Event::Error(format!("{err}")));
                        }
                    }
                }
                Command::RenderPage { page_index, target_width } => {
                    let Some(doc) = &document else {
                        let _ = events_tx.send(Event::Error("no PDF is open".to_string()));
                        continue;
                    };
                    // Written as nested matches rather than a `.and_then`
                    // chain deliberately: `PdfBitmap<'_>` borrows from
                    // `page`, so `page` has to still be in scope when the
                    // bitmap's bytes are extracted, not dropped at the
                    // end of a closure the bitmap tried to escape from.
                    match doc.pages().get(page_index as PdfPageIndex) {
                        Ok(page) => {
                            let config = PdfRenderConfig::new().set_target_width(target_width);
                            match page.render_with_config(&config) {
                                Ok(bitmap) => {
                                    let width = bitmap.width() as u32;
                                    let height = bitmap.height() as u32;
                                    let rgba = bitmap.as_rgba_bytes();
                                    let _ = events_tx.send(Event::PageRendered { page_index, width, height, rgba });
                                }
                                Err(err) => {
                                    let _ = events_tx.send(Event::Error(format!("{err}")));
                                }
                            }
                        }
                        Err(err) => {
                            let _ = events_tx.send(Event::Error(format!("{err}")));
                        }
                    }
                }
            }
        }
    });

    PdfSession { command_tx, events_rx }
}
