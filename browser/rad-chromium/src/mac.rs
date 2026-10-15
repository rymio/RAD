//! macOS: Chromium requires the application object to implement its event-handling protocols
//! (`CrAppProtocol` and friends), so the engine installs this `NSApplication` subclass before
//! anything else touches `NSApp`. The engine never shows a window, so it also stays out of the
//! Dock and the app switcher.

use std::cell::Cell;

use cef::application_mac::{CefAppProtocol, CrAppControlProtocol, CrAppProtocol};
use objc2::rc::Retained;
use objc2::runtime::{Bool, NSObjectProtocol};
use objc2::{define_class, msg_send, ClassType, DefinedClass, MainThreadMarker};
use objc2_app_kit::{NSApp, NSApplication, NSApplicationActivationPolicy, NSEvent};

#[derive(Default)]
pub struct Ivars {
    handling_send_event: Cell<Bool>,
}

define_class!(
    #[unsafe(super(NSApplication))]
    #[ivars = Ivars]
    pub struct RadApplication;

    impl RadApplication {
        #[unsafe(method(sendEvent:))]
        unsafe fn send_event(&self, event: &NSEvent) {
            let was = self.ivars().handling_send_event.get();
            self.ivars().handling_send_event.set(Bool::YES);
            let _: () = msg_send![super(self), sendEvent: event];
            self.ivars().handling_send_event.set(was);
        }
    }

    unsafe impl NSObjectProtocol for RadApplication {}

    unsafe impl CrAppControlProtocol for RadApplication {
        #[unsafe(method(setHandlingSendEvent:))]
        unsafe fn set_handling_send_event(&self, v: Bool) {
            self.ivars().handling_send_event.set(v);
        }
    }

    unsafe impl CrAppProtocol for RadApplication {
        #[unsafe(method(isHandlingSendEvent))]
        unsafe fn is_handling_send_event(&self) -> Bool {
            self.ivars().handling_send_event.get()
        }
    }

    unsafe impl CefAppProtocol for RadApplication {}
);

pub fn setup_application() {
    let _: Retained<RadApplication> = unsafe { msg_send![RadApplication::class(), sharedApplication] };
    let mtm = MainThreadMarker::new().expect("on the main thread");
    let app = NSApp(mtm);
    assert!(app.isKindOfClass(RadApplication::class()), "NSApp was created before the engine's application class");
    app.setActivationPolicy(NSApplicationActivationPolicy::Prohibited);
}
