//! The menu bar (macOS) / notification area (Windows) icon: what the agent
//! shows and what the user does with it. The agent owns all logic; the OS
//! layers only render [`TrayView`]s, open [`Dialog`]s and report
//! [`TrayEvent`]s, polling their command channel every [`POLL`].

use std::time::Duration;

use crate::image::Rgba;

/// How often the OS layers look for new [`UiCommand`]s.
pub const POLL: Duration = Duration::from_millis(100);

/// Identifies a menu item in [`TrayEvent::Clicked`].
pub type ItemId = u32;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MenuItem {
    /// Greyed-out text (the status line).
    Label(String),
    Separator,
    Action {
        id: ItemId,
        label: String,
        enabled: bool,
        /// Shows a check mark.
        checked: bool,
    },
}

/// The look of the icon.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum TrayState {
    /// Waiting for the peer, or not doing anything.
    Idle,
    Connected,
    Paused,
    /// Needs the user (not paired, no side chosen, an error).
    Attention,
}

impl TrayState {
    pub const ALL: [TrayState; 4] = [
        TrayState::Idle,
        TrayState::Connected,
        TrayState::Paused,
        TrayState::Attention,
    ];
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TrayView {
    pub state: TrayState,
    /// Shown when hovering the icon (Windows keeps 127 characters).
    pub tooltip: String,
    pub items: Vec<MenuItem>,
}

/// A small dialog. At most one is open at a time.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Dialog {
    pub message: String,
    /// Ask for a line of text (a pairing code).
    pub input: bool,
    /// Button labels, left to right (at most three); the last one is the
    /// default. A button labelled "Cancel" cancels: it reports `None`.
    pub buttons: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum UiCommand {
    Update(TrayView),
    /// Opens dialog `id`, closing any other one without reporting it.
    Open {
        id: u32,
        dialog: Dialog,
    },
    /// Closes dialog `id`, if it is still open, without reporting it.
    Close {
        id: u32,
    },
    /// Removes the icon and ends the OS loop.
    Quit,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TrayEvent {
    Clicked(ItemId),
    /// The user ended dialog `id`: `button` is the index of the button,
    /// `None` when it was cancelled or closed; `text` is the input.
    DialogDone {
        id: u32,
        button: Option<usize>,
        text: String,
    },
    /// The system asks the app to quit (logout, shutdown).
    Exit,
}

// ---------- macOS dialogs ----------

/// The `osascript` arguments that show `d` (macOS has no dialog API that
/// would not block the menu bar's run loop). The message and the button
/// labels travel as arguments, never inside the script text.
pub fn osascript_args(d: &Dialog) -> Vec<String> {
    let answer = if d.input { " default answer \"\"" } else { "" };
    let mut lines = vec![
        "on run argv".to_string(),
        "set msg to item 1 of argv".into(),
        "set btns to rest of argv".into(),
        "activate".into(),
        format!(
            "set r to display dialog msg with title \"MacWin\" buttons btns \
             default button (count of btns){answer}"
        ),
        "set out to button returned of r".into(),
    ];
    if d.input {
        lines.push("set out to out & linefeed & (text returned of r)".into());
    }
    lines.extend(["return out".into(), "end run".into()]);
    let mut args = Vec::new();
    for l in lines {
        args.push("-e".into());
        args.push(l);
    }
    args.push(d.message.clone());
    args.extend(d.buttons.iter().cloned());
    args
}

/// The clicked button and the input from `osascript`'s output (a "Cancel"
/// button makes it fail instead, which is `None`).
pub fn parse_osascript(out: &str, d: &Dialog) -> (Option<usize>, String) {
    let mut lines = out.lines();
    let button = lines
        .next()
        .and_then(|b| d.buttons.iter().position(|x| x == b));
    let text = if d.input {
        lines.next().unwrap_or("").to_string()
    } else {
        String::new()
    };
    (button, text)
}

// ---------- icons ----------

#[derive(Debug, Clone, Copy)]
enum Shape {
    Rect(f32, f32, f32, f32),
    Circle(f32, f32, f32),
    Rounded(f32, f32, f32, f32, f32),
}

impl Shape {
    fn contains(self, x: f32, y: f32) -> bool {
        match self {
            Shape::Rect(x0, y0, x1, y1) => x >= x0 && x < x1 && y >= y0 && y < y1,
            Shape::Circle(cx, cy, r) => (x - cx).powi(2) + (y - cy).powi(2) < r * r,
            Shape::Rounded(x0, y0, x1, y1, r) => {
                if !Shape::Rect(x0, y0, x1, y1).contains(x, y) {
                    return false;
                }
                let cx = x.clamp(x0 + r, x1 - r);
                let cy = y.clamp(y0 + r, y1 - r);
                (x - cx).powi(2) + (y - cy).powi(2) < r * r
            }
        }
    }
}

/// Premultiplied RGBA in 0..=1, on a 16-unit grid scaled to the size.
struct Canvas {
    size: u32,
    px: Vec<[f32; 4]>,
}

const SUB: u32 = 4;

impl Canvas {
    fn new(size: u32) -> Canvas {
        Canvas {
            size,
            px: vec![[0.0; 4]; (size * size) as usize],
        }
    }

    /// The share of pixel (x, y) that `shape` covers.
    fn coverage(&self, shape: Shape, x: u32, y: u32) -> f32 {
        let unit = 16.0 / self.size as f32;
        let mut hits = 0;
        for sy in 0..SUB {
            for sx in 0..SUB {
                let ux = (x as f32 + (sx as f32 + 0.5) / SUB as f32) * unit;
                let uy = (y as f32 + (sy as f32 + 0.5) / SUB as f32) * unit;
                if shape.contains(ux, uy) {
                    hits += 1;
                }
            }
        }
        hits as f32 / (SUB * SUB) as f32
    }

    fn paint(&mut self, shape: Shape, rgb: [u8; 3], alpha: f32) {
        for y in 0..self.size {
            for x in 0..self.size {
                let a = alpha * self.coverage(shape, x, y);
                if a == 0.0 {
                    continue;
                }
                let d = &mut self.px[(y * self.size + x) as usize];
                for c in 0..3 {
                    d[c] = rgb[c] as f32 / 255.0 * a + d[c] * (1.0 - a);
                }
                d[3] = a + d[3] * (1.0 - a);
            }
        }
    }

    fn erase(&mut self, shape: Shape) {
        for y in 0..self.size {
            for x in 0..self.size {
                let keep = 1.0 - self.coverage(shape, x, y);
                for v in &mut self.px[(y * self.size + x) as usize] {
                    *v *= keep;
                }
            }
        }
    }

    fn into_rgba(self) -> Rgba {
        let mut pixels = Vec::with_capacity(self.px.len() * 4);
        for [r, g, b, a] in self.px {
            let un = |v: f32| {
                if a > 0.0 {
                    (v / a * 255.0).round().clamp(0.0, 255.0) as u8
                } else {
                    0
                }
            };
            pixels.extend([un(r), un(g), un(b), (a * 255.0).round() as u8]);
        }
        Rgba {
            width: self.size,
            height: self.size,
            pixels,
        }
    }
}

/// Two screens with stands, in 16 units, scaled by `k` around the centre.
fn glyph(k: f32) -> Vec<Shape> {
    let t = |v: f32| 8.0 + (v - 8.0) * k;
    let rect = |x0: f32, y0: f32, x1: f32, y1: f32| Shape::Rect(t(x0), t(y0), t(x1), t(y1));
    let mut shapes = Vec::new();
    for x in [0.5, 8.5] {
        shapes.extend([
            rect(x, 3.0, x + 7.0, 4.5),
            rect(x, 9.0, x + 7.0, 10.5),
            rect(x, 3.0, x + 1.5, 10.5),
            rect(x + 5.5, 3.0, x + 7.0, 10.5),
            rect(x + 2.75, 10.5, x + 4.25, 12.0),
            rect(x + 1.25, 12.0, x + 5.75, 13.5),
        ]);
    }
    shapes
}

const WHITE: [u8; 3] = [255, 255, 255];
const BLACK: [u8; 3] = [0, 0, 0];
const BLUE: [u8; 3] = [0x25, 0x63, 0xEB];

fn status_colour(state: TrayState) -> [u8; 3] {
    match state {
        TrayState::Idle => [0x8B, 0x94, 0x9E],
        TrayState::Connected => [0x2E, 0xA0, 0x43],
        TrayState::Paused => [0xD2, 0x99, 0x22],
        TrayState::Attention => [0xDA, 0x36, 0x33],
    }
}

/// The icon at `size` x `size` pixels. `template`: a black glyph on
/// transparency for the macOS menu bar, which recolours it (the state shows
/// as dimming, pause bars or a dot); otherwise a coloured icon with a status
/// dot for the Windows notification area.
pub fn icon(state: TrayState, size: u32, template: bool) -> Rgba {
    let mut c = Canvas::new(size.max(1));
    if template {
        let alpha = match state {
            TrayState::Idle | TrayState::Paused => 0.45,
            TrayState::Connected | TrayState::Attention => 1.0,
        };
        for s in glyph(1.0) {
            c.paint(s, BLACK, alpha);
        }
        match state {
            TrayState::Paused => {
                c.erase(Shape::Rect(9.75, 8.75, 16.0, 16.0));
                c.paint(Shape::Rect(10.75, 9.75, 12.5, 15.75), BLACK, 1.0);
                c.paint(Shape::Rect(13.75, 9.75, 15.5, 15.75), BLACK, 1.0);
            }
            TrayState::Attention => {
                c.erase(Shape::Circle(12.75, 12.75, 3.75));
                c.paint(Shape::Circle(12.75, 12.75, 2.75), BLACK, 1.0);
            }
            TrayState::Idle | TrayState::Connected => {}
        }
    } else {
        c.paint(Shape::Rounded(0.0, 0.0, 16.0, 16.0, 3.0), BLUE, 1.0);
        for s in glyph(0.8) {
            c.paint(s, WHITE, 1.0);
        }
        c.paint(Shape::Circle(12.5, 12.5, 3.5), WHITE, 1.0);
        c.paint(Shape::Circle(12.5, 12.5, 2.5), status_colour(state), 1.0);
    }
    c.into_rgba()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn at(img: &Rgba, x: u32, y: u32) -> [u8; 4] {
        let i = ((y * img.width + x) * 4) as usize;
        img.pixels[i..i + 4].try_into().unwrap()
    }

    #[test]
    fn osascript_dialogs() {
        let d = Dialog {
            message: "Enter the code \"x\"".into(),
            input: true,
            buttons: vec!["Cancel".into(), "Pair".into()],
        };
        let args = osascript_args(&d);
        assert_eq!(
            &args[args.len() - 3..],
            ["Enter the code \"x\"", "Cancel", "Pair"]
        );
        let script: Vec<&String> = args
            .iter()
            .skip(1)
            .step_by(2)
            .take_while(|_| true)
            .collect();
        assert!(script.iter().any(|l| l.contains("default answer")));
        assert!(args.iter().filter(|a| *a == "-e").count() >= 8);
        assert_eq!(
            parse_osascript("Pair\n123 456\n", &d),
            (Some(1), "123 456".into())
        );
        assert_eq!(parse_osascript("", &d), (None, String::new()));
        let ok = Dialog {
            message: "Paired".into(),
            input: false,
            buttons: vec!["Left".into(), "Right".into()],
        };
        assert!(
            !osascript_args(&ok)
                .iter()
                .any(|l| l.contains("default answer"))
        );
        assert_eq!(parse_osascript("Right\n", &ok), (Some(1), String::new()));
    }

    #[test]
    fn icons_have_the_size_and_differ_by_state() {
        for template in [true, false] {
            for size in [16, 20, 32, 36] {
                let all = TrayState::ALL.map(|s| icon(s, size, template));
                for img in &all {
                    assert_eq!((img.width, img.height), (size, size));
                    assert_eq!(img.pixels.len(), (size * size * 4) as usize);
                    assert_eq!(at(img, 0, 0)[3], 0, "transparent corner");
                    assert!(img.pixels.chunks(4).any(|p| p[3] >= 100), "visible pixels");
                }
                for i in 0..all.len() {
                    for j in i + 1..all.len() {
                        assert_ne!(all[i], all[j], "{template} {size} {i} {j}");
                    }
                }
            }
        }
    }

    #[test]
    fn template_icons_are_black_and_coloured_ones_are_blue_and_white() {
        let t = icon(TrayState::Connected, 32, true);
        assert!(t.pixels.chunks(4).all(|p| p[3] == 0 || p[..3] == [0, 0, 0]));
        let c = icon(TrayState::Connected, 32, false);
        // Inside the left screen: background. On its top edge: the glyph.
        assert_eq!(at(&c, 8, 13)[..3], BLUE);
        assert_eq!(at(&c, 6, 9), [255, 255, 255, 255]);
        // The status dot.
        assert_eq!(at(&c, 25, 25)[..3], status_colour(TrayState::Connected));
    }
}
