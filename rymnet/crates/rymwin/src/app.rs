use crate::vfs::VirtualComputer;
use eframe::egui;
use rymnet_core::NodeIdentity;
use rymnet_storage::DataLayout;
use x25519_dalek::StaticSecret;

/// Everything an app might need beyond its own state: the account's
/// sandboxed vfs, a handle to spawn background async work on (peer
/// network calls can't block the GUI thread), and the pieces needed to
/// make peer-protocol requests. Bundled into one struct -- introduced
/// once the third app (Browser) needed genuinely different context than
/// the first two (which only ever touched the vfs), rather than growing
/// `show`'s parameter list a third time.
pub struct AppContext<'a> {
    pub vfs: &'a VirtualComputer,
    pub runtime: &'a tokio::runtime::Handle,
    pub layout: &'a DataLayout,
    pub identity: &'a NodeIdentity,
    pub username: &'a str,
    /// The account's X25519 mailbox key -- only Mail needs this (to
    /// decrypt sealed envelopes addressed to this account), but it lives
    /// here rather than being threaded through as a fifth parameter now
    /// that a second app-specific field already exists.
    pub mailbox_secret: &'a StaticSecret,
}

/// The extension point mentioned since the first desktop screen went in:
/// every app (File Manager, Rymcode, Browser, and whatever's next) is
/// one of these, drawn into its own floating window inside the single
/// 1024x768 native window rather than a separate OS window -- closer to
/// how Windows 3.x's own MDI child windows worked, and much simpler than
/// juggling multiple native viewports for what's still a small toy OS.
pub trait RymApp {
    fn title(&self) -> String;

    /// Draws this app's window. Returns `false` once the user has closed
    /// it, so the desktop knows to drop it. `launch` lets an app open
    /// other apps (Rymcode's Run button launching a `.win` script as a
    /// new window, say) without needing to reach into the desktop's own
    /// app list -- push onto it and the desktop picks it up next frame.
    fn show(&mut self, ctx: &egui::Context, app: &AppContext, launch: &mut Vec<Box<dyn RymApp>>) -> bool;
}
