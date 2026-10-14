//! Command-line interface.

use std::net::SocketAddr;

use clap::{ArgGroup, Args, Parser, Subcommand, ValueEnum};
use macwin_core::types::Side;
use macwin_net::Fingerprint;

#[derive(Debug, Parser)]
#[command(
    name = "macwin",
    version,
    about = "Share one keyboard, mouse and clipboard between a Mac and a Windows PC on one LAN",
    after_help = "Getting started: run `macwin pair` on one machine; it shows a code and the command\n\
                  to type on the other machine. Then run `macwin run` on both."
)]
pub struct Cli {
    #[command(subcommand)]
    pub command: Command,
}

#[derive(Debug, Subcommand)]
pub enum Command {
    /// Print this machine's identity fingerprint and where its key is kept
    /// (created on first use).
    Identity,
    /// Pair with the other machine using a one-time code.
    Pair(PairArgs),
    /// Forget the paired machine.
    Unpair,
    /// Show or change the configuration.
    #[command(subcommand)]
    Config(ConfigCmd),
    /// Share the keyboard, mouse and clipboard with the paired machine and
    /// reconnect whenever the connection drops (Ctrl+C to stop).
    Run,
    /// Like `run`, with a menu bar (macOS) or notification area (Windows)
    /// icon to pair, pause, choose the side and quit.
    Tray,
    /// Start the MacWin icon (`tray`) at login: on, off, or show whether it
    /// does.
    Autostart {
        #[arg(value_enum)]
        action: Option<Switch>,
    },
    /// Read-only checks of this machine.
    #[command(subcommand)]
    Probe(Probe),
    /// Experimental tools.
    #[command(subcommand)]
    Spike(Spike),
}

#[derive(Debug, Args)]
pub struct PairArgs {
    /// The other machine's address (HOST or HOST:PORT), as printed by its
    /// `macwin pair`. Without it, macwin looks on the local network for a
    /// machine waiting for pairing; when there is none, this machine waits
    /// and shows a code.
    #[arg(value_name = "HOST[:PORT]")]
    pub address: Option<String>,
    /// The code shown on the other machine (asked for when omitted).
    #[arg(long, conflicts_with = "listen")]
    pub code: Option<String>,
    /// Which side of this machine the other machine's screens are on.
    #[arg(long, value_enum)]
    pub side: Option<SideArg>,
    /// Only wait (no search), on ADDR:PORT instead of the configured listen
    /// address.
    #[arg(long, value_name = "ADDR:PORT", conflicts_with = "address")]
    pub listen: Option<SocketAddr>,
}

#[derive(Debug, Subcommand)]
pub enum ConfigCmd {
    /// Print the configuration.
    Show,
    /// Change one setting. Keys: side (left|right|none), address
    /// (HOST[:PORT]|none), listen (ADDR:PORT), remap (on|off), discovery
    /// (on|off), clipboard (on|off), clipboard-max-kb (1-16384),
    /// clipboard-images (on|off), clipboard-files (on|off),
    /// clipboard-files-max-mb (1-65536).
    Set { key: String, value: String },
}

#[derive(Debug, Subcommand)]
pub enum Probe {
    /// List the screens in native coordinates.
    Screens,
    /// Show the privacy permissions macwin needs (macOS).
    Permissions {
        /// Ask macOS to show the permission prompts.
        #[arg(long)]
        prompt: bool,
    },
    /// Show clipboard format names and sizes, never the content.
    Clipboard {
        /// Keep watching and print every change (Ctrl+C to stop).
        #[arg(long)]
        watch: bool,
    },
}

#[derive(Debug, Subcommand)]
pub enum Spike {
    /// Measure round-trip latency to the peer over the stream and datagram paths.
    Net(SpikeNetArgs),
    /// Print local input events; with --grab, also swallow them for a while.
    Capture(SpikeCaptureArgs),
    /// Move the cursor in a small square and type "hello" (physical keys).
    Inject(SpikeInjectArgs),
    /// Share this machine's mouse and keyboard with the peer, both ways.
    Kvm(SpikeKvmArgs),
}

#[derive(Debug, Args)]
#[command(group(ArgGroup::new("mode").required(true).args(["listen", "connect"])))]
pub struct SpikeNetArgs {
    /// Listen on ADDR:PORT (for example 0.0.0.0:47800) and answer pings.
    #[arg(long, value_name = "ADDR:PORT")]
    pub listen: Option<SocketAddr>,
    /// Connect to HOST:PORT and measure.
    #[arg(long, value_name = "HOST:PORT")]
    pub connect: Option<String>,
    /// SHA-256 fingerprint of the peer (`macwin identity` on the peer prints it).
    #[arg(long, value_name = "HEX")]
    pub peer_fp: Fingerprint,
    /// Pings per path (dialer only).
    #[arg(long, default_value_t = 1000)]
    pub count: u32,
    /// Milliseconds between pings (dialer only).
    #[arg(long, default_value_t = 2)]
    pub interval_ms: u64,
}

#[derive(Debug, Args)]
pub struct SpikeCaptureArgs {
    /// Swallow local input and hide the cursor while running. Only the
    /// emergency chord (Ctrl+Alt+Shift+Esc) or the timeout ends it early.
    #[arg(long)]
    pub grab: bool,
    /// How long to run, 1-60 seconds.
    #[arg(long, default_value_t = 10, value_parser = clap::value_parser!(u64).range(1..=60))]
    pub seconds: u64,
}

#[derive(Debug, Args)]
pub struct SpikeInjectArgs {
    /// Required, so the cursor never moves by accident.
    #[arg(long, required = true)]
    pub demo: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub enum Switch {
    On,
    Off,
}

/// Where the peer's screens are.
#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub enum SideArg {
    Left,
    Right,
}

impl From<SideArg> for Side {
    fn from(s: SideArg) -> Side {
        match s {
            SideArg::Left => Side::Left,
            SideArg::Right => Side::Right,
        }
    }
}

#[derive(Debug, Args)]
#[command(group(ArgGroup::new("mode").required(true).args(["listen", "connect"])))]
pub struct SpikeKvmArgs {
    /// Listen on ADDR:PORT (for example 0.0.0.0:47800).
    #[arg(long, value_name = "ADDR:PORT")]
    pub listen: Option<SocketAddr>,
    /// Connect to HOST:PORT.
    #[arg(long, value_name = "HOST:PORT")]
    pub connect: Option<String>,
    /// SHA-256 fingerprint of the peer (`macwin identity` on the peer prints it).
    #[arg(long, value_name = "HEX")]
    pub peer_fp: Fingerprint,
    /// Which side of this machine the peer's screens are on.
    #[arg(long, value_enum)]
    pub peer_side: SideArg,
    /// Forward Ctrl and Cmd/Win unchanged (no Cmd->Ctrl remap).
    #[arg(long)]
    pub no_remap: bool,
}

#[cfg(test)]
mod tests {
    use clap::CommandFactory;

    use super::*;

    const FP: &str = "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855";

    #[test]
    fn clap_definition_is_consistent() {
        Cli::command().debug_assert();
    }

    #[test]
    fn spike_net_needs_exactly_one_mode() {
        let ok = Cli::try_parse_from([
            "macwin",
            "spike",
            "net",
            "--listen",
            "127.0.0.1:0",
            "--peer-fp",
            FP,
        ])
        .unwrap();
        match ok.command {
            Command::Spike(Spike::Net(a)) => {
                assert_eq!(a.listen, Some("127.0.0.1:0".parse().unwrap()));
                assert_eq!((a.count, a.interval_ms), (1000, 2));
                assert_eq!(a.peer_fp.to_string(), FP);
            }
            other => panic!("unexpected {other:?}"),
        }
        let dial = Cli::try_parse_from([
            "macwin",
            "spike",
            "net",
            "--connect",
            "pc.local:47800",
            "--peer-fp",
            FP,
            "--count",
            "20",
        ])
        .unwrap();
        assert!(matches!(
            dial.command,
            Command::Spike(Spike::Net(SpikeNetArgs { count: 20, .. }))
        ));
        assert!(
            Cli::try_parse_from(["macwin", "spike", "net", "--peer-fp", FP]).is_err(),
            "no mode"
        );
        assert!(
            Cli::try_parse_from([
                "macwin",
                "spike",
                "net",
                "--listen",
                "127.0.0.1:0",
                "--connect",
                "x:1",
                "--peer-fp",
                FP
            ])
            .is_err(),
            "both modes"
        );
        assert!(
            Cli::try_parse_from([
                "macwin",
                "spike",
                "net",
                "--listen",
                "127.0.0.1:0",
                "--peer-fp",
                "nothex"
            ])
            .is_err()
        );
        assert!(
            Cli::try_parse_from(["macwin", "spike", "net", "--listen", "127.0.0.1:0"]).is_err(),
            "pin is mandatory"
        );
        assert!(matches!(
            Cli::try_parse_from(["macwin", "identity"]).unwrap().command,
            Command::Identity
        ));
    }

    #[test]
    fn probes_parse() {
        assert!(matches!(
            Cli::try_parse_from(["macwin", "probe", "screens"])
                .unwrap()
                .command,
            Command::Probe(Probe::Screens)
        ));
        assert!(matches!(
            Cli::try_parse_from(["macwin", "probe", "permissions", "--prompt"])
                .unwrap()
                .command,
            Command::Probe(Probe::Permissions { prompt: true })
        ));
        assert!(matches!(
            Cli::try_parse_from(["macwin", "probe", "clipboard", "--watch"])
                .unwrap()
                .command,
            Command::Probe(Probe::Clipboard { watch: true })
        ));
    }

    #[test]
    fn local_spikes_parse_with_guards() {
        let c = Cli::try_parse_from(["macwin", "spike", "capture", "--grab", "--seconds", "5"])
            .unwrap();
        assert!(matches!(
            c.command,
            Command::Spike(Spike::Capture(SpikeCaptureArgs {
                grab: true,
                seconds: 5
            }))
        ));
        assert!(
            Cli::try_parse_from(["macwin", "spike", "capture", "--seconds", "61"]).is_err(),
            "max 60 s"
        );
        assert!(Cli::try_parse_from(["macwin", "spike", "capture", "--seconds", "0"]).is_err());
        assert!(
            Cli::try_parse_from(["macwin", "spike", "inject"]).is_err(),
            "--demo is required"
        );
        assert!(Cli::try_parse_from(["macwin", "spike", "inject", "--demo"]).is_ok());
    }

    #[test]
    fn pair_config_and_run_parse() {
        let parse = |args: &[&str]| {
            let mut v = vec!["macwin"];
            v.extend_from_slice(args);
            Cli::try_parse_from(v)
        };
        match parse(&["pair"]).unwrap().command {
            Command::Pair(a) => assert!(a.address.is_none() && a.code.is_none()),
            other => panic!("unexpected {other:?}"),
        }
        match parse(&["pair", "pc.local", "--code", "123-456", "--side", "right"])
            .unwrap()
            .command
        {
            Command::Pair(a) => {
                assert_eq!(a.address.as_deref(), Some("pc.local"));
                assert_eq!(a.code.as_deref(), Some("123-456"));
                assert_eq!(a.side, Some(SideArg::Right));
            }
            other => panic!("unexpected {other:?}"),
        }
        match parse(&["pair", "--code", "123456"]).unwrap().command {
            Command::Pair(a) => assert!(a.address.is_none() && a.code.is_some(), "LAN search"),
            other => panic!("unexpected {other:?}"),
        }
        assert!(
            parse(&["pair", "--code", "123456", "--listen", "0.0.0.0:1"]).is_err(),
            "the waiting side shows a code"
        );
        assert!(
            parse(&["pair", "pc", "--listen", "0.0.0.0:1"]).is_err(),
            "dial or wait, not both"
        );
        assert!(matches!(
            parse(&["config", "set", "side", "left"]).unwrap().command,
            Command::Config(ConfigCmd::Set { .. })
        ));
        assert!(matches!(
            parse(&["config", "show"]).unwrap().command,
            Command::Config(ConfigCmd::Show)
        ));
        assert!(matches!(parse(&["run"]).unwrap().command, Command::Run));
        assert!(matches!(parse(&["tray"]).unwrap().command, Command::Tray));
        assert!(matches!(
            parse(&["autostart"]).unwrap().command,
            Command::Autostart { action: None }
        ));
        assert!(matches!(
            parse(&["autostart", "off"]).unwrap().command,
            Command::Autostart {
                action: Some(Switch::Off)
            }
        ));
        assert!(parse(&["autostart", "maybe"]).is_err());
        assert!(matches!(
            parse(&["unpair"]).unwrap().command,
            Command::Unpair
        ));
    }

    #[test]
    fn spike_kvm_requires_mode_pin_and_side() {
        let c = Cli::try_parse_from([
            "macwin",
            "spike",
            "kvm",
            "--connect",
            "pc:47800",
            "--peer-fp",
            FP,
            "--peer-side",
            "right",
            "--no-remap",
        ])
        .unwrap();
        match c.command {
            Command::Spike(Spike::Kvm(a)) => {
                assert_eq!(Side::from(a.peer_side), Side::Right);
                assert!(a.no_remap);
                assert_eq!(a.connect.as_deref(), Some("pc:47800"));
            }
            other => panic!("unexpected {other:?}"),
        }
        assert!(
            Cli::try_parse_from([
                "macwin",
                "spike",
                "kvm",
                "--listen",
                "0.0.0.0:1",
                "--peer-fp",
                FP
            ])
            .is_err(),
            "side required"
        );
        assert!(
            Cli::try_parse_from([
                "macwin",
                "spike",
                "kvm",
                "--peer-fp",
                FP,
                "--peer-side",
                "left"
            ])
            .is_err(),
            "mode required"
        );
        assert!(
            Cli::try_parse_from([
                "macwin",
                "spike",
                "kvm",
                "--listen",
                "0.0.0.0:1",
                "--peer-side",
                "up",
                "--peer-fp",
                FP
            ])
            .is_err()
        );
    }
}
