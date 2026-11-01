//! Tells when a window's full-screen transition has finished (entered or exited). GPUI only
//! reports the bounds change at its start; AppKit posts these notifications at its end. Any
//! failure returns `None`, and the caller falls back to a timer.

// objc 0.2's macros test `feature = "cargo-clippy"`, which newer rustc flags.
#![allow(unexpected_cfgs)]

use std::{
    ffi::{CStr, c_void},
    sync::Once,
};

use futures::channel::mpsc::{UnboundedReceiver, UnboundedSender, unbounded};
use gpui::Window;
use objc::{
    class,
    declare::ClassDecl,
    msg_send,
    runtime::{Class, Object, Sel},
    sel, sel_impl,
};
use raw_window_handle::{HasWindowHandle, RawWindowHandle};

const CLASS: &CStr = c"AgentMeshFullscreenObserver";
const SENDER: &str = "sender";
const NOTIFICATIONS: [&CStr; 2] = [
    c"NSWindowDidEnterFullScreenNotification",
    c"NSWindowDidExitFullScreenNotification",
];

/// Registered with the notification center while alive.
pub struct TransitionObserver {
    observer: *mut Object,
}

/// Observes `window`'s NSWindow; the receiver gets one `()` per finished transition.
pub fn observe(window: &Window) -> Option<(TransitionObserver, UnboundedReceiver<()>)> {
    let RawWindowHandle::AppKit(handle) = HasWindowHandle::window_handle(window).ok()?.as_raw()
    else {
        return None;
    };
    let class = observer_class()?;
    let (sender, receiver) = unbounded();
    unsafe {
        let ns_view = handle.ns_view.as_ptr().cast::<Object>();
        let ns_window: *mut Object = msg_send![ns_view, window];
        if ns_window.is_null() {
            return None;
        }
        let observer: *mut Object = msg_send![class, new];
        let sender = Box::into_raw(Box::new(sender)).cast::<c_void>();
        (*observer).set_ivar(SENDER, sender);
        let center: *mut Object = msg_send![class!(NSNotificationCenter), defaultCenter];
        for name in NOTIFICATIONS {
            let name: *mut Object =
                msg_send![class!(NSString), stringWithUTF8String: name.as_ptr()];
            let _: () = msg_send![center, addObserver: observer
                                         selector: sel!(transitionDidEnd:)
                                             name: name
                                           object: ns_window];
        }
        Some((TransitionObserver { observer }, receiver))
    }
}

impl Drop for TransitionObserver {
    fn drop(&mut self) {
        unsafe {
            let center: *mut Object = msg_send![class!(NSNotificationCenter), defaultCenter];
            let _: () = msg_send![center, removeObserver: self.observer];
            let sender: *mut c_void = *(*self.observer).get_ivar(SENDER);
            drop(Box::from_raw(sender.cast::<UnboundedSender<()>>()));
            let _: () = msg_send![self.observer, release];
        }
    }
}

fn observer_class() -> Option<&'static Class> {
    static REGISTER: Once = Once::new();
    REGISTER.call_once(|| {
        let Some(mut decl) = ClassDecl::new(CLASS.to_str().unwrap_or_default(), class!(NSObject))
        else {
            return;
        };
        decl.add_ivar::<*mut c_void>(SENDER);
        unsafe {
            decl.add_method(
                sel!(transitionDidEnd:),
                transition_did_end as extern "C" fn(&Object, Sel, *mut Object),
            );
        }
        decl.register();
    });
    Class::get(CLASS.to_str().ok()?)
}

extern "C" fn transition_did_end(this: &Object, _: Sel, _: *mut Object) {
    unsafe {
        let sender: *mut c_void = *this.get_ivar(SENDER);
        if let Some(sender) = sender.cast::<UnboundedSender<()>>().as_ref() {
            sender.unbounded_send(()).ok();
        }
    }
}
