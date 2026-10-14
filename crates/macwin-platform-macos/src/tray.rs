//! The menu bar icon: an NSStatusItem with an NSMenu, on the main thread
//! inside `NSApplication::run`. A 100 ms timer polls the agent's commands.
//! Dialogs are `osascript` child processes, so they never block the run
//! loop; their result is collected by the same timer.

use std::cell::RefCell;
use std::io::Read;
use std::process::{Child, Command, Stdio};
use std::sync::mpsc::{Receiver, TryRecvError};

use macwin_platform::PlatformError;
use macwin_platform::image::encode_png;
use macwin_platform::tray::{
    Dialog, MenuItem, POLL, TrayEvent, TrayState, TrayView, UiCommand, icon, osascript_args,
    parse_osascript,
};
use objc2::rc::Retained;
use objc2::runtime::{AnyObject, NSObject, Sel};
use objc2::{
    AnyThread, DefinedClass, MainThreadMarker, MainThreadOnly, define_class, msg_send, sel,
};
use objc2_app_kit::{
    NSApplication, NSApplicationActivationPolicy, NSControlStateValueOff, NSControlStateValueOn,
    NSImage, NSMenu, NSMenuItem, NSStatusBar, NSStatusItem, NSVariableStatusItemLength,
};
use objc2_foundation::{NSData, NSObjectProtocol, NSSize, NSString, NSTimer, ns_string};
use tracing::warn;

const OSASCRIPT: &str = "/usr/bin/osascript";
/// The icon's size in points; it is drawn at twice that for Retina screens.
const ICON_POINTS: f64 = 18.0;

struct OpenDialog {
    id: u32,
    dialog: Dialog,
    child: Child,
}

struct Ivars {
    commands: Receiver<UiCommand>,
    events: Box<dyn Fn(TrayEvent) + Send>,
    item: Retained<NSStatusItem>,
    menu: Retained<NSMenu>,
    images: Vec<(TrayState, Retained<NSImage>)>,
    dialog: RefCell<Option<OpenDialog>>,
}

define_class!(
    // SAFETY: NSObject has no subclassing requirements, and `Target` does
    // not implement Drop.
    #[unsafe(super(NSObject))]
    #[thread_kind = MainThreadOnly]
    #[name = "MacWinTrayTarget"]
    #[ivars = Ivars]
    struct Target;

    impl Target {
        #[unsafe(method(menuClicked:))]
        fn menu_clicked(&self, sender: &NSMenuItem) {
            (self.ivars().events)(TrayEvent::Clicked(sender.tag() as u32));
        }

        #[unsafe(method(tick:))]
        fn tick(&self, _timer: &NSTimer) {
            self.poll();
        }
    }

    unsafe impl NSObjectProtocol for Target {}
);

impl Target {
    fn new(mtm: MainThreadMarker, ivars: Ivars) -> Retained<Target> {
        let this = Target::alloc(mtm).set_ivars(ivars);
        // SAFETY: NSObject's designated initializer.
        unsafe { msg_send![super(this), init] }
    }

    fn poll(&self) {
        let mtm = self.mtm();
        loop {
            match self.ivars().commands.try_recv() {
                Ok(UiCommand::Update(view)) => self.render(&view, mtm),
                Ok(UiCommand::Open { id, dialog }) => self.open(id, dialog),
                Ok(UiCommand::Close { id }) => self.close(Some(id)),
                Ok(UiCommand::Quit) | Err(TryRecvError::Disconnected) => {
                    self.close(None);
                    NSStatusBar::systemStatusBar().removeStatusItem(&self.ivars().item);
                    // The agent has stopped everything; this ends the process.
                    NSApplication::sharedApplication(mtm).terminate(None);
                    return;
                }
                Err(TryRecvError::Empty) => break,
            }
        }
        self.check_dialog();
    }

    fn render(&self, view: &TrayView, mtm: MainThreadMarker) {
        let menu = &self.ivars().menu;
        menu.removeAllItems();
        for item in &view.items {
            let it = match item {
                MenuItem::Separator => NSMenuItem::separatorItem(mtm),
                MenuItem::Label(text) => {
                    let it = menu_item(mtm, text, None);
                    it.setEnabled(false);
                    it
                }
                MenuItem::Action {
                    id,
                    label,
                    enabled,
                    checked,
                } => {
                    let it = menu_item(mtm, label, Some(sel!(menuClicked:)));
                    let target: &AnyObject = self;
                    // SAFETY: `self` implements `menuClicked:` and lives as
                    // long as the run loop (menu items keep weak targets).
                    unsafe { it.setTarget(Some(target)) };
                    it.setTag(*id as isize);
                    it.setEnabled(*enabled);
                    it.setState(if *checked {
                        NSControlStateValueOn
                    } else {
                        NSControlStateValueOff
                    });
                    it
                }
            };
            menu.addItem(&it);
        }
        if let Some(button) = self.ivars().item.button(mtm) {
            if let Some((_, image)) = self.ivars().images.iter().find(|(s, _)| *s == view.state) {
                button.setImage(Some(image));
            }
            button.setToolTip(Some(&NSString::from_str(&view.tooltip)));
        }
    }

    fn open(&self, id: u32, dialog: Dialog) {
        self.close(None);
        let spawned = Command::new(OSASCRIPT)
            .args(osascript_args(&dialog))
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn();
        match spawned {
            Ok(child) => {
                *self.ivars().dialog.borrow_mut() = Some(OpenDialog { id, dialog, child });
            }
            Err(e) => {
                warn!(error = %e, "cannot show a dialog");
                (self.ivars().events)(TrayEvent::DialogDone {
                    id,
                    button: None,
                    text: String::new(),
                });
            }
        }
    }

    /// Closes dialog `id` (any dialog for `None`) without reporting it.
    fn close(&self, id: Option<u32>) {
        let open = self
            .ivars()
            .dialog
            .borrow_mut()
            .take_if(|d| id.is_none_or(|id| d.id == id));
        if let Some(mut d) = open {
            let _ = d.child.kill();
            let _ = d.child.wait();
        }
    }

    fn check_dialog(&self) {
        let finished = {
            let mut slot = self.ivars().dialog.borrow_mut();
            let Some(d) = slot.as_mut() else {
                return;
            };
            match d.child.try_wait() {
                Ok(None) => return,
                Ok(Some(status)) => slot.take().map(|d| (d, status.success())),
                Err(e) => {
                    warn!(error = %e, "lost a dialog");
                    slot.take().map(|mut d| {
                        let _ = d.child.kill();
                        (d, false)
                    })
                }
            }
        };
        let Some((mut d, ok)) = finished else {
            return;
        };
        let mut out = String::new();
        if let Some(mut stdout) = d.child.stdout.take() {
            let _ = stdout.read_to_string(&mut out);
        }
        let (button, text) = if ok {
            parse_osascript(&out, &d.dialog)
        } else {
            (None, String::new())
        };
        (self.ivars().events)(TrayEvent::DialogDone {
            id: d.id,
            button,
            text,
        });
    }
}

fn menu_item(mtm: MainThreadMarker, title: &str, action: Option<Sel>) -> Retained<NSMenuItem> {
    // SAFETY: a plain title, no key equivalent, and an action that the
    // target set afterwards implements.
    unsafe {
        NSMenuItem::initWithTitle_action_keyEquivalent(
            NSMenuItem::alloc(mtm),
            &NSString::from_str(title),
            action,
            ns_string!(""),
        )
    }
}

/// A template image (macOS recolours it for light and dark menu bars).
fn status_image(state: TrayState) -> Option<Retained<NSImage>> {
    let png = encode_png(&icon(state, (ICON_POINTS * 2.0) as u32, true)).ok()?;
    let image = NSImage::initWithData(NSImage::alloc(), &NSData::with_bytes(&png))?;
    image.setSize(NSSize::new(ICON_POINTS, ICON_POINTS));
    image.setTemplate(true);
    Some(image)
}

/// Shows the menu bar icon and runs the AppKit run loop on this thread,
/// which must be the main thread. `UiCommand::Quit` ends the process.
pub fn run(
    commands: Receiver<UiCommand>,
    events: Box<dyn Fn(TrayEvent) + Send>,
) -> Result<(), PlatformError> {
    let mtm = MainThreadMarker::new().ok_or_else(|| {
        PlatformError::Backend("the menu bar icon must run on the main thread".into())
    })?;
    let app = NSApplication::sharedApplication(mtm);
    // No Dock icon, also when not started from MacWin.app (LSUIElement).
    let _ = app.setActivationPolicy(NSApplicationActivationPolicy::Accessory);
    let item = NSStatusBar::systemStatusBar().statusItemWithLength(NSVariableStatusItemLength);
    let menu = NSMenu::new(mtm);
    menu.setAutoenablesItems(false);
    item.setMenu(Some(&menu));
    let images: Vec<_> = TrayState::ALL
        .iter()
        .filter_map(|s| status_image(*s).map(|i| (*s, i)))
        .collect();
    if let Some(button) = item.button(mtm) {
        match images.first() {
            Some((_, i)) => button.setImage(Some(i)),
            None => button.setTitle(ns_string!("MacWin")),
        }
    }
    let target = Target::new(
        mtm,
        Ivars {
            commands,
            events,
            item,
            menu,
            images,
            dialog: RefCell::new(None),
        },
    );
    let target_obj: &AnyObject = &target;
    // SAFETY: `target` implements `tick:` and outlives the run loop.
    let timer = unsafe {
        NSTimer::scheduledTimerWithTimeInterval_target_selector_userInfo_repeats(
            POLL.as_secs_f64(),
            target_obj,
            sel!(tick:),
            None,
            true,
        )
    };
    timer.setTolerance(0.05);
    app.run();
    timer.invalidate();
    Ok(())
}

/// Shows `message` and waits until it is dismissed (or a minute passed).
pub fn notice(message: &str) {
    let d = Dialog {
        message: message.into(),
        input: false,
        buttons: vec!["OK".into()],
    };
    let mut args = osascript_args(&d);
    // `giving up after` belongs to the display dialog line.
    if let Some(line) = args
        .iter_mut()
        .find(|a| a.starts_with("set r to display dialog"))
    {
        line.push_str(" giving up after 60");
    }
    let _ = Command::new(OSASCRIPT)
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status();
}
