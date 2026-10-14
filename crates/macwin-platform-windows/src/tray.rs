//! The notification area icon (Shell_NotifyIcon) with its popup menu, and
//! small dialog windows, all on the calling thread. A hidden window owns
//! the icon and polls the agent's commands with a 100 ms timer.
//!
//! Window procedures only borrow the thread's state briefly and never while
//! calling Win32 functions that send messages back (they would re-enter).

use std::cell::RefCell;
use std::ffi::c_void;
use std::mem::{size_of, zeroed};
use std::ptr;
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::mpsc::{Receiver, TryRecvError};

use macwin_platform::PlatformError;
use macwin_platform::tray::{
    Dialog, MenuItem, POLL, TrayEvent, TrayState, TrayView, UiCommand, icon,
};
use tracing::warn;
use windows_sys::Win32::Foundation::{HWND, LPARAM, LRESULT, RECT, WPARAM};
use windows_sys::Win32::Graphics::Gdi::{
    COLOR_BTNFACE, CreateFontIndirectW, DeleteObject, GetSysColorBrush, HFONT,
};
use windows_sys::Win32::System::LibraryLoader::GetModuleHandleW;
use windows_sys::Win32::UI::HiDpi::{GetDpiForSystem, SystemParametersInfoForDpi};
use windows_sys::Win32::UI::Input::KeyboardAndMouse::SetFocus;
use windows_sys::Win32::UI::Shell::{
    NIF_ICON, NIF_MESSAGE, NIF_SHOWTIP, NIF_TIP, NIM_ADD, NIM_DELETE, NIM_MODIFY, NIM_SETVERSION,
    NIN_SELECT, NINF_KEY, NOTIFYICON_VERSION_4, NOTIFYICONDATAW, Shell_NotifyIconW,
};
use windows_sys::Win32::UI::WindowsAndMessaging::{
    AdjustWindowRectEx, AppendMenuW, BS_DEFPUSHBUTTON, BS_PUSHBUTTON, CreateIcon, CreatePopupMenu,
    CreateWindowExW, DefWindowProcW, DestroyIcon, DestroyMenu, DestroyWindow, DispatchMessageW,
    ES_AUTOHSCROLL, GetCursorPos, GetMessageW, GetSystemMetrics, GetWindowTextLengthW,
    GetWindowTextW, HICON, IDC_ARROW, IDCANCEL, IDOK, IsDialogMessageW, KillTimer, LoadCursorW,
    MB_ICONINFORMATION, MB_OK, MB_SETFOREGROUND, MB_TOPMOST, MF_CHECKED, MF_GRAYED, MF_SEPARATOR,
    MF_STRING, MSG, MessageBoxW, NONCLIENTMETRICSW, PostMessageW, PostQuitMessage, RegisterClassW,
    RegisterWindowMessageW, SM_CXSMICON, SPI_GETNONCLIENTMETRICS, SPI_GETWORKAREA, SW_SHOW,
    SendMessageW, SetForegroundWindow, SetTimer, ShowWindow, SystemParametersInfoW, TPM_NONOTIFY,
    TPM_RETURNCMD, TPM_RIGHTBUTTON, TrackPopupMenuEx, TranslateMessage, WM_APP, WM_CLOSE,
    WM_COMMAND, WM_CONTEXTMENU, WM_DESTROY, WM_ENDSESSION, WM_NULL, WM_SETFONT, WM_TIMER,
    WNDCLASSW, WS_CAPTION, WS_CHILD, WS_EX_CLIENTEDGE, WS_EX_DLGMODALFRAME, WS_EX_TOPMOST,
    WS_OVERLAPPED, WS_POPUP, WS_SYSMENU, WS_TABSTOP, WS_VISIBLE,
};

const WM_TRAY: u32 = WM_APP + 1;
const TIMER_ID: usize = 1;
const ICON_ID: u32 = 1;
// From WinUser.h / CommCtrl.h (their windows-sys modules need extra features).
const SS_LEFT: u32 = 0x0000;
const SS_NOPREFIX: u32 = 0x0080;
const EM_SETLIMITTEXT: u32 = 0x00C5;
const NIN_KEYSELECT: u32 = NIN_SELECT | NINF_KEY;
/// Control ids of dialog buttons: `BUTTON_BASE + index`.
const BUTTON_BASE: i32 = 100;
const EDIT_ID: i32 = 50;
const MAIN_CLASS: &str = "MacWinTray";
const DIALOG_CLASS: &str = "MacWinDialog";

/// "TaskbarCreated": Explorer restarted, the icon has to be added again.
static TASKBAR_CREATED: AtomicU32 = AtomicU32::new(0);

struct DialogWin {
    id: u32,
    hwnd: HWND,
    /// Null without input.
    edit: HWND,
    buttons: Vec<String>,
}

struct State {
    hwnd: HWND,
    commands: Receiver<UiCommand>,
    events: Box<dyn Fn(TrayEvent) + Send>,
    view: Option<TrayView>,
    icons: Vec<(TrayState, HICON)>,
    dialog: Option<DialogWin>,
    font: HFONT,
}

thread_local! {
    static STATE: RefCell<Option<State>> = const { RefCell::new(None) };
}

/// Runs `f` on the state, unless it is missing or already borrowed.
fn with_state<R>(f: impl FnOnce(&mut State) -> R) -> Option<R> {
    STATE.with(|cell| {
        let mut guard = cell.try_borrow_mut().ok()?;
        guard.as_mut().map(f)
    })
}

fn emit(e: TrayEvent) {
    with_state(|s| (s.events)(e));
}

fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}

/// Copies `s` into a fixed NUL-terminated buffer, truncating it.
fn copy_wide(dst: &mut [u16], s: &str) {
    let n = dst.len() - 1;
    let mut len = 0;
    for (d, c) in dst.iter_mut().zip(s.encode_utf16().take(n)) {
        *d = c;
        len += 1;
    }
    dst[len] = 0;
}

fn control_id(id: i32) -> *mut c_void {
    ptr::without_provenance_mut(id as usize)
}

fn icon_for(state: TrayState, size: i32) -> HICON {
    let img = icon(state, size as u32, false);
    let mut bgra = Vec::with_capacity(img.pixels.len());
    for p in img.pixels.chunks(4) {
        bgra.extend([p[2], p[1], p[0], p[3]]);
    }
    // An all-zero AND mask: the alpha channel decides.
    let mask = vec![0u8; ((size as usize).div_ceil(16) * 2) * size as usize];
    // SAFETY: both bitmaps have `size` rows of the right width for 32 and
    // 1 bits per pixel.
    unsafe {
        CreateIcon(
            GetModuleHandleW(ptr::null()),
            size,
            size,
            1,
            32,
            mask.as_ptr(),
            bgra.as_ptr(),
        )
    }
}

fn notify_data(hwnd: HWND, icon: HICON, tip: &str) -> NOTIFYICONDATAW {
    // SAFETY: plain data, for which all zeros is a valid empty value.
    let mut nid: NOTIFYICONDATAW = unsafe { zeroed() };
    nid.cbSize = size_of::<NOTIFYICONDATAW>() as u32;
    nid.hWnd = hwnd;
    nid.uID = ICON_ID;
    nid.uFlags = NIF_MESSAGE | NIF_ICON | NIF_TIP | NIF_SHOWTIP;
    nid.uCallbackMessage = WM_TRAY;
    nid.hIcon = icon;
    copy_wide(&mut nid.szTip, tip);
    nid
}

/// The icon and tooltip of the current view.
fn current_icon() -> Option<NOTIFYICONDATAW> {
    with_state(|s| {
        let state = s.view.as_ref().map_or(TrayState::Idle, |v| v.state);
        let tip = s.view.as_ref().map_or("MacWin", |v| v.tooltip.as_str());
        let icon = s
            .icons
            .iter()
            .find(|(st, _)| *st == state)
            .map_or(ptr::null_mut(), |(_, i)| *i);
        notify_data(s.hwnd, icon, tip)
    })
}

fn add_icon() {
    let Some(mut nid) = current_icon() else {
        return;
    };
    // SAFETY: a fully initialised NOTIFYICONDATAW.
    unsafe {
        if Shell_NotifyIconW(NIM_ADD, &nid) == 0 {
            warn!("cannot add the notification area icon");
        }
        nid.Anonymous.uVersion = NOTIFYICON_VERSION_4;
        Shell_NotifyIconW(NIM_SETVERSION, &nid);
    }
}

fn update(view: TrayView) {
    with_state(|s| s.view = Some(view));
    if let Some(nid) = current_icon() {
        // SAFETY: a fully initialised NOTIFYICONDATAW.
        unsafe { Shell_NotifyIconW(NIM_MODIFY, &nid) };
    }
}

/// `&` would underline the next letter in a menu.
fn menu_text(s: &str) -> Vec<u16> {
    wide(&s.replace('&', "&&"))
}

fn show_menu(hwnd: HWND, x: i32, y: i32) {
    let Some(Some(view)) = with_state(|s| s.view.clone()) else {
        return;
    };
    // SAFETY: plain Win32 calls with valid handles; the menu is destroyed
    // before returning.
    let chosen = unsafe {
        let menu = CreatePopupMenu();
        if menu.is_null() {
            return;
        }
        for item in &view.items {
            match item {
                MenuItem::Separator => {
                    AppendMenuW(menu, MF_SEPARATOR, 0, ptr::null());
                }
                MenuItem::Label(text) => {
                    AppendMenuW(menu, MF_STRING | MF_GRAYED, 0, menu_text(text).as_ptr());
                }
                MenuItem::Action {
                    id,
                    label,
                    enabled,
                    checked,
                } => {
                    let mut flags = MF_STRING;
                    if !enabled {
                        flags |= MF_GRAYED;
                    }
                    if *checked {
                        flags |= MF_CHECKED;
                    }
                    AppendMenuW(menu, flags, *id as usize + 1, menu_text(label).as_ptr());
                }
            }
        }
        // Without it the menu would not close when clicking elsewhere.
        SetForegroundWindow(hwnd);
        let cmd = TrackPopupMenuEx(
            menu,
            TPM_RETURNCMD | TPM_NONOTIFY | TPM_RIGHTBUTTON,
            x,
            y,
            hwnd,
            ptr::null(),
        );
        PostMessageW(hwnd, WM_NULL, 0, 0);
        DestroyMenu(menu);
        cmd
    };
    if chosen > 0 {
        emit(TrayEvent::Clicked(chosen as u32 - 1));
    }
}

fn window_text(hwnd: HWND) -> String {
    // SAFETY: `hwnd` is a live edit control; the buffer holds len + 1 units.
    unsafe {
        let len = GetWindowTextLengthW(hwnd).max(0) as usize;
        let mut buf = vec![0u16; len + 1];
        let n = GetWindowTextW(hwnd, buf.as_mut_ptr(), buf.len() as i32).max(0) as usize;
        String::from_utf16_lossy(&buf[..n.min(len)])
    }
}

/// Closes dialog `id` (any for `None`) without reporting it.
fn close_dialog(id: Option<u32>) {
    let open = with_state(|s| s.dialog.take_if(|d| id.is_none_or(|id| d.id == id))).flatten();
    if let Some(d) = open {
        // SAFETY: our own dialog window.
        unsafe { DestroyWindow(d.hwnd) };
    }
}

/// The user ended the dialog `hwnd`.
fn finish_dialog(hwnd: HWND, button: Option<usize>) {
    let open = with_state(|s| s.dialog.take_if(|d| d.hwnd == hwnd)).flatten();
    let Some(d) = open else {
        return;
    };
    let text = if d.edit.is_null() {
        String::new()
    } else {
        window_text(d.edit)
    };
    let button = button.filter(|i| d.buttons.get(*i).is_some_and(|b| b != "Cancel"));
    // SAFETY: our own dialog window.
    unsafe { DestroyWindow(d.hwnd) };
    emit(TrayEvent::DialogDone {
        id: d.id,
        button,
        text,
    });
}

fn open_dialog(id: u32, d: &Dialog) {
    close_dialog(None);
    let Some(font) = with_state(|s| s.font) else {
        return;
    };
    // SAFETY: plain Win32 calls; every window created here is a child of
    // the dialog, which is destroyed by close_dialog / finish_dialog.
    unsafe {
        let hinstance = GetModuleHandleW(ptr::null());
        let dpi = GetDpiForSystem().max(96) as i32;
        let px = |v: i32| v * dpi / 96;
        let (width, margin, gap) = (px(400), px(14), px(8));
        let (button_w, button_h, edit_h) = (px(96), px(28), px(26));
        let lines: i32 = d
            .message
            .lines()
            .map(|l| l.chars().count() as i32 / 60 + 1)
            .sum();
        let text_h = px(18) * lines.max(1);
        let input_h = if d.input { edit_h + margin } else { 0 };
        let client_h = margin + text_h + margin + input_h + button_h + margin;
        let style = WS_POPUP | WS_CAPTION | WS_SYSMENU;
        let ex = WS_EX_DLGMODALFRAME | WS_EX_TOPMOST;
        let mut r = RECT {
            left: 0,
            top: 0,
            right: width,
            bottom: client_h,
        };
        AdjustWindowRectEx(&mut r, style, 0, ex);
        let (w, h) = (r.right - r.left, r.bottom - r.top);
        let mut work = RECT::default();
        SystemParametersInfoW(SPI_GETWORKAREA, 0, (&raw mut work).cast(), 0);
        let x = work.left + (work.right - work.left - w).max(0) / 2;
        let y = work.top + (work.bottom - work.top - h).max(0) / 2;
        let class = wide(DIALOG_CLASS);
        let hwnd = CreateWindowExW(
            ex,
            class.as_ptr(),
            wide("MacWin").as_ptr(),
            style,
            x,
            y,
            w,
            h,
            ptr::null_mut(),
            ptr::null_mut(),
            hinstance,
            ptr::null(),
        );
        if hwnd.is_null() {
            warn!("cannot create a dialog window");
            emit(TrayEvent::DialogDone {
                id,
                button: None,
                text: String::new(),
            });
            return;
        }
        let child =
            |class: &str, text: &str, style: u32, ex: u32, rect: (i32, i32, i32, i32), cid: i32| {
                let c = CreateWindowExW(
                    ex,
                    wide(class).as_ptr(),
                    wide(text).as_ptr(),
                    WS_CHILD | WS_VISIBLE | style,
                    rect.0,
                    rect.1,
                    rect.2,
                    rect.3,
                    hwnd,
                    control_id(cid),
                    hinstance,
                    ptr::null(),
                );
                SendMessageW(c, WM_SETFONT, font as WPARAM, 1);
                c
            };
        child(
            "STATIC",
            &d.message.replace('\n', "\r\n"),
            SS_LEFT | SS_NOPREFIX,
            0,
            (margin, margin, width - 2 * margin, text_h),
            0,
        );
        let mut top = margin + text_h + margin;
        let edit = if d.input {
            let e = child(
                "EDIT",
                "",
                WS_TABSTOP | ES_AUTOHSCROLL as u32,
                WS_EX_CLIENTEDGE,
                (margin, top, width - 2 * margin, edit_h),
                EDIT_ID,
            );
            SendMessageW(e, EM_SETLIMITTEXT, 32, 0);
            top += input_h;
            e
        } else {
            ptr::null_mut()
        };
        let n = d.buttons.len() as i32;
        let mut last = ptr::null_mut();
        for (i, label) in d.buttons.iter().enumerate() {
            let i = i as i32;
            let kind = if i == n - 1 {
                BS_DEFPUSHBUTTON
            } else {
                BS_PUSHBUTTON
            };
            let left = width - margin - (n - i) * button_w - (n - i - 1) * gap;
            last = child(
                "BUTTON",
                label,
                WS_TABSTOP | kind as u32,
                0,
                (left, top, button_w, button_h),
                BUTTON_BASE + i,
            );
        }
        with_state(|s| {
            s.dialog = Some(DialogWin {
                id,
                hwnd,
                edit,
                buttons: d.buttons.clone(),
            })
        });
        ShowWindow(hwnd, SW_SHOW);
        SetForegroundWindow(hwnd);
        SetFocus(if edit.is_null() { last } else { edit });
    }
}

fn quit() {
    close_dialog(None);
    let Some((hwnd, icons)) = with_state(|s| (s.hwnd, std::mem::take(&mut s.icons))) else {
        return;
    };
    // SAFETY: our own window, timer and icons.
    unsafe {
        let nid = notify_data(hwnd, ptr::null_mut(), "");
        Shell_NotifyIconW(NIM_DELETE, &nid);
        for (_, i) in icons {
            DestroyIcon(i);
        }
        KillTimer(hwnd, TIMER_ID);
        DestroyWindow(hwnd);
    }
}

fn poll() {
    loop {
        let next = with_state(|s| s.commands.try_recv());
        match next {
            Some(Ok(UiCommand::Update(view))) => update(view),
            Some(Ok(UiCommand::Open { id, dialog })) => open_dialog(id, &dialog),
            Some(Ok(UiCommand::Close { id })) => close_dialog(Some(id)),
            Some(Ok(UiCommand::Quit)) | Some(Err(TryRecvError::Disconnected)) => {
                quit();
                return;
            }
            Some(Err(TryRecvError::Empty)) | None => return,
        }
    }
}

unsafe extern "system" fn main_proc(
    hwnd: HWND,
    msg: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    match msg {
        WM_TIMER if wparam == TIMER_ID => {
            poll();
            0
        }
        WM_TRAY => {
            let event = (lparam as u32) & 0xFFFF;
            if matches!(event, WM_CONTEXTMENU | NIN_SELECT | NIN_KEYSELECT) {
                // Version 4 passes the anchor point in wParam.
                let mut x = (wparam & 0xFFFF) as u16 as i16 as i32;
                let mut y = ((wparam >> 16) & 0xFFFF) as u16 as i16 as i32;
                let mut p = Default::default();
                // SAFETY: an out pointer to a POINT.
                if x == 0 && y == 0 && unsafe { GetCursorPos(&mut p) } != 0 {
                    (x, y) = (p.x, p.y);
                }
                show_menu(hwnd, x, y);
            }
            0
        }
        WM_ENDSESSION if wparam != 0 => {
            emit(TrayEvent::Exit);
            0
        }
        WM_DESTROY => {
            // SAFETY: ends this thread's message loop.
            unsafe { PostQuitMessage(0) };
            0
        }
        m if m != 0 && m == TASKBAR_CREATED.load(Ordering::Relaxed) => {
            add_icon();
            0
        }
        // SAFETY: the default handling of a message for our window.
        _ => unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) },
    }
}

unsafe extern "system" fn dialog_proc(
    hwnd: HWND,
    msg: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    match msg {
        WM_COMMAND => {
            let id = (wparam & 0xFFFF) as i32;
            let buttons = with_state(|s| {
                s.dialog
                    .as_ref()
                    .filter(|d| d.hwnd == hwnd)
                    .map(|d| d.buttons.len())
            })
            .flatten()
            .unwrap_or(0);
            let button = match id {
                // Return: the default (last) button.
                IDOK => buttons.checked_sub(1),
                // Escape.
                IDCANCEL => None,
                id if id >= BUTTON_BASE => Some((id - BUTTON_BASE) as usize),
                // Notifications of the edit control.
                _ => return 0,
            };
            finish_dialog(hwnd, button);
            0
        }
        WM_CLOSE => {
            finish_dialog(hwnd, None);
            0
        }
        // SAFETY: the default handling of a message for our window.
        _ => unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) },
    }
}

/// The font Windows uses for message boxes, at the system DPI.
fn message_font() -> HFONT {
    let mut ncm = NONCLIENTMETRICSW {
        cbSize: size_of::<NONCLIENTMETRICSW>() as u32,
        ..Default::default()
    };
    // SAFETY: `ncm` is a NONCLIENTMETRICSW of the declared size.
    unsafe {
        let ok = SystemParametersInfoForDpi(
            SPI_GETNONCLIENTMETRICS,
            ncm.cbSize,
            (&raw mut ncm).cast(),
            0,
            GetDpiForSystem(),
        );
        if ok == 0 {
            return ptr::null_mut();
        }
        CreateFontIndirectW(&ncm.lfMessageFont)
    }
}

/// Shows the notification area icon and runs this thread's message loop
/// until `UiCommand::Quit` (or the end of the command channel).
pub fn run(
    commands: Receiver<UiCommand>,
    events: Box<dyn Fn(TrayEvent) + Send>,
) -> Result<(), PlatformError> {
    let main_class = wide(MAIN_CLASS);
    let dialog_class = wide(DIALOG_CLASS);
    // SAFETY: plain Win32 calls on this thread; the state lives in a thread
    // local until the loop ends.
    unsafe {
        let hinstance = GetModuleHandleW(ptr::null());
        let cursor = LoadCursorW(ptr::null_mut(), IDC_ARROW);
        let classes = [
            (main_class.as_ptr(), main_proc as _, ptr::null_mut()),
            (
                dialog_class.as_ptr(),
                dialog_proc as _,
                GetSysColorBrush(COLOR_BTNFACE),
            ),
        ];
        for (name, proc_, brush) in classes {
            let wc = WNDCLASSW {
                lpfnWndProc: Some(proc_),
                hInstance: hinstance,
                hCursor: cursor,
                hbrBackground: brush,
                lpszClassName: name,
                ..Default::default()
            };
            // A second run on this thread finds the classes registered.
            RegisterClassW(&wc);
        }
        let hwnd = CreateWindowExW(
            0,
            main_class.as_ptr(),
            wide("MacWin").as_ptr(),
            WS_OVERLAPPED,
            0,
            0,
            0,
            0,
            ptr::null_mut(),
            ptr::null_mut(),
            hinstance,
            ptr::null(),
        );
        if hwnd.is_null() {
            return Err(PlatformError::Backend(
                "cannot create the tray window".into(),
            ));
        }
        TASKBAR_CREATED.store(
            RegisterWindowMessageW(wide("TaskbarCreated").as_ptr()),
            Ordering::Relaxed,
        );
        let size = GetSystemMetrics(SM_CXSMICON).max(16);
        let icons = TrayState::ALL
            .iter()
            .map(|s| (*s, icon_for(*s, size)))
            .collect();
        STATE.with(|cell| {
            *cell.borrow_mut() = Some(State {
                hwnd,
                commands,
                events,
                view: None,
                icons,
                dialog: None,
                font: message_font(),
            })
        });
        add_icon();
        SetTimer(hwnd, TIMER_ID, POLL.as_millis() as u32, None);
        let mut msg = MSG::default();
        loop {
            let r = GetMessageW(&mut msg, ptr::null_mut(), 0, 0);
            if r == 0 || r == -1 {
                break;
            }
            let dialog = with_state(|s| s.dialog.as_ref().map(|d| d.hwnd)).flatten();
            if let Some(d) = dialog
                && IsDialogMessageW(d, &msg) != 0
            {
                continue;
            }
            TranslateMessage(&msg);
            DispatchMessageW(&msg);
        }
        if let Some(state) = STATE.with(|cell| cell.borrow_mut().take())
            && !state.font.is_null()
        {
            DeleteObject(state.font);
        }
    }
    Ok(())
}

/// Shows `message` in a message box and waits until it is closed.
pub fn notice(message: &str) {
    let (text, title) = (wide(message), wide("MacWin"));
    // SAFETY: NUL-terminated strings and no owner window.
    unsafe {
        MessageBoxW(
            ptr::null_mut(),
            text.as_ptr(),
            title.as_ptr(),
            MB_OK | MB_ICONINFORMATION | MB_TOPMOST | MB_SETFOREGROUND,
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wide_strings_are_truncated_and_terminated() {
        let mut buf = [1u16; 4];
        copy_wide(&mut buf, "abcdef");
        assert_eq!(buf, [97, 98, 99, 0]);
        copy_wide(&mut buf, "a");
        assert_eq!(&buf[..2], [97, 0]);
        assert_eq!(menu_text("A & B"), wide("A && B"));
    }

    #[test]
    fn icons_can_be_created() {
        let i = icon_for(TrayState::Connected, 16);
        assert!(!i.is_null());
        // SAFETY: the icon created above.
        unsafe { DestroyIcon(i) };
    }
}
