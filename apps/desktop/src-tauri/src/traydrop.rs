//! macOS: drag files onto the Toss icon in the menu bar (Blip-style).
//!
//! The `tray-icon` crate puts its own click view (`TaoTrayTarget`) on the status item's
//! button. We add one transparent child view on top of it that is registered for file
//! drags; clicks are handed straight back to the tray view, so the icon behaves as before.
//!
//! - drag enters the icon → [`TrayDrag::Enter`] (the shell pops the window up under the
//!   icon, so the user keeps dragging onto a contact row)
//! - drop on the icon itself → [`TrayDrag::Drop`] with the file paths ("Gửi tới ai?")

use std::sync::OnceLock;

use objc2::{
    define_class, msg_send,
    rc::Retained,
    runtime::{Bool, ProtocolObject},
    MainThreadMarker, MainThreadOnly,
};
use objc2_app_kit::{
    NSAutoresizingMaskOptions, NSDragOperation, NSDraggingInfo, NSEvent, NSPasteboardTypeFileURL,
    NSStatusItem, NSView,
};
use objc2_foundation::{NSArray, NSURL};

/// What happened on the menu bar icon.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TrayDrag {
    Enter,
    Drop(Vec<String>),
}

type Handler = Box<dyn Fn(TrayDrag) + Send + Sync + 'static>;
static HANDLER: OnceLock<Handler> = OnceLock::new();

fn emit(ev: TrayDrag) {
    if let Some(h) = HANDLER.get() {
        h(ev);
    }
}

define_class!(
    #[unsafe(super(NSView))]
    #[thread_kind = MainThreadOnly]
    #[name = "TossTrayDropView"]
    struct TrayDropView;

    impl TrayDropView {
        #[unsafe(method(draggingEntered:))]
        fn dragging_entered(&self, _sender: &ProtocolObject<dyn NSDraggingInfo>) -> NSDragOperation {
            emit(TrayDrag::Enter);
            NSDragOperation::Copy
        }

        #[unsafe(method(draggingUpdated:))]
        fn dragging_updated(&self, _sender: &ProtocolObject<dyn NSDraggingInfo>) -> NSDragOperation {
            NSDragOperation::Copy
        }

        #[unsafe(method(prepareForDragOperation:))]
        fn prepare_for_drag(&self, _sender: &ProtocolObject<dyn NSDraggingInfo>) -> Bool {
            Bool::YES
        }

        #[unsafe(method(performDragOperation:))]
        fn perform_drag(&self, sender: &ProtocolObject<dyn NSDraggingInfo>) -> Bool {
            let paths = dragged_paths(sender);
            if paths.is_empty() {
                return Bool::NO;
            }
            emit(TrayDrag::Drop(paths));
            Bool::YES
        }

        // Clicks belong to the tray icon's own view (our superview).
        #[unsafe(method(mouseDown:))]
        fn mouse_down(&self, event: &NSEvent) {
            if let Some(s) = unsafe { self.superview() } {
                s.mouseDown(event);
            }
        }

        #[unsafe(method(mouseUp:))]
        fn mouse_up(&self, event: &NSEvent) {
            if let Some(s) = unsafe { self.superview() } {
                s.mouseUp(event);
            }
        }

        #[unsafe(method(rightMouseDown:))]
        fn right_mouse_down(&self, event: &NSEvent) {
            if let Some(s) = unsafe { self.superview() } {
                s.rightMouseDown(event);
            }
        }

        #[unsafe(method(rightMouseUp:))]
        fn right_mouse_up(&self, event: &NSEvent) {
            if let Some(s) = unsafe { self.superview() } {
                s.rightMouseUp(event);
            }
        }
    }
);

/// File-system paths of the dragged items (Finder hands out file reference URLs,
/// `file:///.file/id=…`, which `filePathURL` resolves).
fn dragged_paths(sender: &ProtocolObject<dyn NSDraggingInfo>) -> Vec<String> {
    let pb = sender.draggingPasteboard();
    let mut out = Vec::new();
    let Some(items) = pb.pasteboardItems() else {
        return out;
    };
    for item in items.iter() {
        let Some(s) = (unsafe { item.stringForType(NSPasteboardTypeFileURL) }) else {
            continue;
        };
        let url = NSURL::URLWithString(&s);
        let path = url
            .as_deref()
            .and_then(|u| u.filePathURL())
            .and_then(|u| u.path());
        if let Some(p) = path {
            out.push(p.to_string());
        }
    }
    out
}

/// Makes the status item's icon accept file drags. Must run on the main thread;
/// `handler` may be called on the main thread at any time afterwards (keep it short).
pub fn attach(item: &NSStatusItem, handler: Handler) -> Result<(), &'static str> {
    let mtm = MainThreadMarker::new().ok_or("not on the main thread")?;
    let _ = HANDLER.set(handler);
    let button = item.button(mtm).ok_or("status item has no button")?;
    // The tray crate's click view, else the button itself.
    let host: Retained<NSView> = button
        .subviews()
        .iter()
        .find(|v| v.class().name().to_bytes() == b"TaoTrayTarget")
        .unwrap_or_else(|| {
            Retained::into_super(Retained::into_super(Retained::into_super(button)))
        });
    let frame = host.bounds();
    let view: Retained<TrayDropView> =
        unsafe { msg_send![TrayDropView::alloc(mtm), initWithFrame: frame] };
    view.setAutoresizingMask(
        NSAutoresizingMaskOptions::ViewWidthSizable | NSAutoresizingMaskOptions::ViewHeightSizable,
    );
    let types = NSArray::from_slice(&[unsafe { NSPasteboardTypeFileURL }]);
    view.registerForDraggedTypes(&types);
    host.addSubview(&view);
    Ok(())
}
