//! Starting MacWin at login, as the menu bar / tray app: a LaunchAgent on
//! macOS, a value of the current user's `Run` key on Windows.

use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use anyhow::Context;
use macwin_platform::fs::write_atomic;

/// LaunchAgent label (and plist name) on macOS, `Run` value on Windows.
pub const LABEL: &str = "io.macwin.MacWin";
/// The `Run` value name on Windows.
pub const RUN_VALUE: &str = "MacWin";
/// The Windows tray binary (GUI subsystem, no console window).
pub const TRAY_EXE: &str = "macwin-tray.exe";

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum State {
    Off,
    /// This copy of MacWin starts at login.
    On,
    /// Another copy starts at login (`command` as registered).
    Other(String),
}

/// The login item of this user.
pub trait Autostart: Send + Sync {
    fn state(&self) -> anyhow::Result<State>;
    /// Registers this copy (replacing another one).
    fn enable(&self) -> anyhow::Result<()>;
    fn disable(&self) -> anyhow::Result<()>;
    /// What runs at login, for users.
    fn command(&self) -> String;
}

fn xml_escape(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}

/// A LaunchAgent in `~/Library/LaunchAgents` (macOS).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LaunchAgent {
    pub plist: PathBuf,
    pub program: Vec<String>,
    /// Where launchd writes the agent's stderr (early errors, panics).
    pub log: PathBuf,
}

impl LaunchAgent {
    /// The agent of the user with home `home`, starting `exe tray`.
    pub fn new(home: &Path, exe: &Path) -> LaunchAgent {
        LaunchAgent {
            plist: home
                .join("Library/LaunchAgents")
                .join(format!("{LABEL}.plist")),
            program: vec![exe.display().to_string(), "tray".into()],
            log: home.join("Library/Logs/macwin/launchd.log"),
        }
    }

    /// The property list. `KeepAlive.Crashed` restarts the app after a
    /// crash but not after Quit or an error exit; `AssociatedBundleIdentifiers`
    /// attributes the agent to MacWin.app (Login Items, Local Network).
    pub fn render(&self) -> String {
        let args: String = self
            .program
            .iter()
            .map(|a| format!("\t\t<string>{}</string>\n", xml_escape(a)))
            .collect();
        format!(
            "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n\
             <!DOCTYPE plist PUBLIC \"-//Apple//DTD PLIST 1.0//EN\" \
             \"http://www.apple.com/DTDs/PropertyList-1.0.dtd\">\n\
             <plist version=\"1.0\">\n<dict>\n\
             \t<key>Label</key>\n\t<string>{LABEL}</string>\n\
             \t<key>ProgramArguments</key>\n\t<array>\n{args}\t</array>\n\
             \t<key>RunAtLoad</key>\n\t<true/>\n\
             \t<key>KeepAlive</key>\n\t<dict>\n\t\t<key>Crashed</key>\n\t\t<true/>\n\t</dict>\n\
             \t<key>ProcessType</key>\n\t<string>Interactive</string>\n\
             \t<key>LimitLoadToSessionType</key>\n\t<string>Aqua</string>\n\
             \t<key>AssociatedBundleIdentifiers</key>\n\t<string>{LABEL}</string>\n\
             \t<key>StandardErrorPath</key>\n\t<string>{}</string>\n\
             </dict>\n</plist>\n",
            xml_escape(&self.log.display().to_string()),
        )
    }
}

impl Autostart for LaunchAgent {
    fn state(&self) -> anyhow::Result<State> {
        match fs::read_to_string(&self.plist) {
            Ok(text)
                if text.contains(&format!(
                    "<string>{}</string>",
                    xml_escape(&self.program[0])
                )) =>
            {
                Ok(State::On)
            }
            Ok(_) => Ok(State::Other(self.plist.display().to_string())),
            Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(State::Off),
            Err(e) => Err(e).with_context(|| format!("cannot read {}", self.plist.display())),
        }
    }

    fn enable(&self) -> anyhow::Result<()> {
        let exe = &self.program[0];
        // A copy on a disk image, or one macOS moved to a random read-only
        // place (App Translocation, for downloaded apps that were never
        // moved), is gone after the next login.
        if exe.starts_with("/Volumes/") || exe.contains("/AppTranslocation/") {
            anyhow::bail!(
                "MacWin runs from a disk image or a temporary location ({exe}); move MacWin.app                  to the Applications folder, open it from there and try again"
            );
        }
        for dir in [self.plist.parent(), self.log.parent()]
            .into_iter()
            .flatten()
        {
            fs::create_dir_all(dir).with_context(|| format!("cannot create {}", dir.display()))?;
        }
        write_atomic(&self.plist, self.render().as_bytes(), false)
            .with_context(|| format!("cannot write {}", self.plist.display()))
    }

    /// Takes effect at the next login: a running app keeps running.
    fn disable(&self) -> anyhow::Result<()> {
        match fs::remove_file(&self.plist) {
            Ok(()) => Ok(()),
            Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(()),
            Err(e) => Err(e).with_context(|| format!("cannot remove {}", self.plist.display())),
        }
    }

    fn command(&self) -> String {
        self.program.join(" ")
    }
}

/// The `Run` value on Windows.
#[cfg(windows)]
pub struct RunKey {
    /// The quoted path of the tray binary.
    pub command: String,
}

#[cfg(windows)]
impl RunKey {
    pub fn new(tray_exe: &Path) -> RunKey {
        RunKey {
            command: format!("\"{}\"", tray_exe.display()),
        }
    }
}

#[cfg(windows)]
impl Autostart for RunKey {
    fn state(&self) -> anyhow::Result<State> {
        Ok(match macwin_platform_windows::run_value(RUN_VALUE)? {
            None => State::Off,
            Some(c) if c.eq_ignore_ascii_case(&self.command) => State::On,
            Some(c) => State::Other(c),
        })
    }

    fn enable(&self) -> anyhow::Result<()> {
        Ok(macwin_platform_windows::set_run_value(
            RUN_VALUE,
            &self.command,
        )?)
    }

    fn disable(&self) -> anyhow::Result<()> {
        Ok(macwin_platform_windows::delete_run_value(RUN_VALUE)?)
    }

    fn command(&self) -> String {
        self.command.clone()
    }
}

/// Whether `exe` lies inside a macOS app bundle.
pub fn in_app_bundle(exe: &Path) -> bool {
    exe.to_string_lossy().contains(".app/Contents/MacOS/")
}

/// The login item for this copy of MacWin, if the OS has one.
pub fn for_this_copy() -> anyhow::Result<Box<dyn Autostart>> {
    let exe = std::env::current_exe().context("cannot find this program's path")?;
    #[cfg(target_os = "macos")]
    {
        let home = std::env::var_os("HOME").context("HOME is not set")?;
        Ok(Box::new(LaunchAgent::new(Path::new(&home), &exe)))
    }
    #[cfg(windows)]
    {
        let tray = exe.with_file_name(TRAY_EXE);
        anyhow::ensure!(
            tray.exists(),
            "{} is missing next to {}",
            TRAY_EXE,
            exe.display()
        );
        Ok(Box::new(RunKey::new(&tray)))
    }
    #[cfg(not(any(target_os = "macos", windows)))]
    {
        let _ = exe;
        anyhow::bail!("starting at login is supported on macOS and Windows only")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn agent(home: &Path) -> LaunchAgent {
        LaunchAgent::new(home, Path::new("/Apps/Mac & Win.app/Contents/MacOS/macwin"))
    }

    /// LaunchAgents exist on macOS only (on Windows the paths print with
    /// `\` and differ from the expected text).
    #[cfg(unix)]
    #[test]
    fn the_launch_agent_starts_the_tray_app() {
        let a = agent(Path::new("/Users/me"));
        let text = a.render();
        assert_eq!(
            a.plist,
            Path::new("/Users/me/Library/LaunchAgents/io.macwin.MacWin.plist")
        );
        for needle in [
            "<key>Label</key>\n\t<string>io.macwin.MacWin</string>",
            "<string>/Apps/Mac &amp; Win.app/Contents/MacOS/macwin</string>\n\t\t<string>tray</string>",
            "<key>RunAtLoad</key>\n\t<true/>",
            "<key>Crashed</key>\n\t\t<true/>",
            "<key>AssociatedBundleIdentifiers</key>\n\t<string>io.macwin.MacWin</string>",
            "<string>/Users/me/Library/Logs/macwin/launchd.log</string>",
        ] {
            assert!(text.contains(needle), "{needle}\n{text}");
        }
        assert!(in_app_bundle(Path::new(&a.program[0])));
        assert!(!in_app_bundle(Path::new("/x/target/release/macwin")));
    }

    #[test]
    fn enabling_and_disabling_the_launch_agent() {
        let home = tempfile::tempdir().unwrap();
        let a = agent(home.path());
        assert_eq!(a.state().unwrap(), State::Off);
        a.enable().unwrap();
        assert_eq!(a.state().unwrap(), State::On);
        assert!(home.path().join("Library/Logs/macwin").is_dir());
        let other = LaunchAgent::new(
            home.path(),
            Path::new("/Other/MacWin.app/Contents/MacOS/macwin"),
        );
        assert!(matches!(other.state().unwrap(), State::Other(_)));
        other.enable().unwrap();
        assert_eq!(other.state().unwrap(), State::On);
        a.disable().unwrap();
        a.disable().unwrap();
        assert_eq!(other.state().unwrap(), State::Off);
    }

    #[test]
    fn copies_that_will_disappear_are_refused() {
        let home = tempfile::tempdir().unwrap();
        for exe in [
            "/Volumes/MacWin 0.1.0/MacWin.app/Contents/MacOS/macwin",
            "/private/var/folders/x/T/AppTranslocation/1234/d/MacWin.app/Contents/MacOS/macwin",
        ] {
            let a = LaunchAgent::new(home.path(), Path::new(exe));
            let err = a.enable().unwrap_err().to_string();
            assert!(err.contains("Applications"), "{err}");
            assert_eq!(a.state().unwrap(), State::Off);
        }
    }

    /// The plist is valid for macOS itself.
    #[cfg(target_os = "macos")]
    #[test]
    fn plutil_accepts_the_launch_agent() {
        let home = tempfile::tempdir().unwrap();
        let a = agent(home.path());
        a.enable().unwrap();
        let out = std::process::Command::new("plutil")
            .arg("-lint")
            .arg(&a.plist)
            .output()
            .unwrap();
        assert!(out.status.success(), "{out:?}");
    }
}
