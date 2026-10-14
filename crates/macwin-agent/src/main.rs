//! `macwin`: the macwin command-line agent.
#![forbid(unsafe_code)]

use std::process::ExitCode;

use clap::Parser;
use macwin_agent::cli::{Cli, Command, ConfigCmd, Probe, Spike};
use macwin_agent::{autostart, commands, error_chain, logging, tray};

fn main() -> ExitCode {
    // Old macOS versions pass a process serial number to apps opened from
    // the Finder.
    let args: Vec<_> = std::env::args_os()
        .filter(|a| !a.to_string_lossy().starts_with("-psn_"))
        .collect();
    // MacWin.app opened from the Finder, the Dock or at login: the tray.
    let opened_as_app =
        args.len() == 1 && std::env::current_exe().is_ok_and(|exe| autostart::in_app_bundle(&exe));
    if opened_as_app {
        return tray::main();
    }
    let cli = Cli::parse_from(args);
    if matches!(cli.command, Command::Tray) {
        return tray::main();
    }
    logging::init();
    let runtime = match tokio::runtime::Runtime::new() {
        Ok(r) => r,
        Err(e) => {
            eprintln!("error: cannot start the async runtime: {e}");
            return ExitCode::FAILURE;
        }
    };
    match runtime.block_on(run(cli)) {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("error: {}", error_chain(&e));
            ExitCode::FAILURE
        }
    }
}

async fn run(cli: Cli) -> anyhow::Result<()> {
    match cli.command {
        Command::Identity => commands::identity(),
        Command::Pair(args) => commands::pair(args).await,
        Command::Unpair => commands::unpair(),
        Command::Config(ConfigCmd::Show) => commands::config_show(),
        Command::Config(ConfigCmd::Set { key, value }) => commands::config_set(&key, &value),
        Command::Run => commands::run().await,
        Command::Tray => unreachable!("handled before the runtime starts"),
        Command::Autostart { action } => commands::autostart(action),
        Command::Probe(Probe::Screens) => commands::probe_screens(),
        Command::Probe(Probe::Permissions { prompt }) => {
            commands::probe_permissions(prompt);
            Ok(())
        }
        Command::Probe(Probe::Clipboard { watch }) => commands::probe_clipboard(watch).await,
        Command::Spike(Spike::Net(args)) => commands::spike_net(args).await,
        Command::Spike(Spike::Capture(args)) => commands::spike_capture(args).await,
        Command::Spike(Spike::Inject(_)) => commands::spike_inject().await,
        Command::Spike(Spike::Kvm(args)) => commands::spike_kvm(args).await,
    }
}
