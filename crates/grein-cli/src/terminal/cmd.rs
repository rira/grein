use std::io::IsTerminal;
use std::sync::Arc;

use grein_tui::terminal::providers::default_registry;
use grein_tui::terminal::{
    HintLevel, TerminalCapabilities, TerminalEnv, TerminalHint, TerminalKind,
};
use serde::{Deserialize, Serialize};

use crate::args::{TerminalArgs, TerminalCommand, TerminalSetupArgs, TerminalStatusArgs};
use crate::terminal::fs::RealConfigFs;
use crate::terminal::host::TerminalSetupHost;
use crate::terminal::port::{
    InstallStatus, RevertReceipt, SetupPlan, SetupReceipt, TerminalStatusEntry,
};
use crate::terminal::providers::default_providers;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TerminalStatusReport {
    pub active_terminal: TerminalKind,
    pub active_adapter: String,
    pub capabilities: TerminalCapabilities,
    pub hints: Vec<TerminalHint>,
    pub entries: Vec<TerminalStatusEntry>,
}

pub fn default_setup_host() -> TerminalSetupHost {
    TerminalSetupHost::new(default_providers(), Arc::new(RealConfigFs::new()))
        .with_registry(default_registry())
}

pub fn collect_terminal_status_with(
    host: &TerminalSetupHost,
    env: &TerminalEnv,
) -> Result<TerminalStatusReport, Box<dyn std::error::Error + Send + Sync>> {
    let registry = default_registry();
    let adapter = registry
        .resolve_or_err(env)
        .map_err(|err| Box::new(err) as Box<dyn std::error::Error + Send + Sync>)?;
    let active_terminal = host.active_terminal(env);
    let capabilities = adapter.capabilities(env);
    let hints = adapter.diagnostic_hints(env);
    let entries = host.status(env)?;

    Ok(TerminalStatusReport {
        active_terminal,
        active_adapter: adapter.display_name().to_string(),
        capabilities,
        hints,
        entries,
    })
}

pub fn handle_terminal_command(
    args: &TerminalArgs,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    match &args.command {
        TerminalCommand::Status(status_args) => handle_terminal_status(status_args),
        TerminalCommand::Setup(setup_args) => handle_terminal_setup(setup_args),
    }
}

pub fn handle_terminal_status(
    args: &TerminalStatusArgs,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let host = default_setup_host();
    let env = TerminalEnv::from_env();
    handle_terminal_status_with(args, &host, &env)
}

pub fn handle_terminal_status_with(
    args: &TerminalStatusArgs,
    host: &TerminalSetupHost,
    env: &TerminalEnv,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let report = collect_terminal_status_with(host, env)?;
    if args.json {
        println!("{}", serde_json::to_string_pretty(&report)?);
        return Ok(());
    }

    let ansi = std::io::stdout().is_terminal();
    println!(
        "{} {} ({})",
        style_bold("Active terminal:", ansi),
        report.active_terminal.display_name(),
        report.active_terminal
    );
    println!(
        "{} color={:?}, keyboard={:?}, osc52={}, focus={}, paste={}",
        style_bold("Capabilities:   ", ansi),
        report.capabilities.color,
        report.capabilities.keyboard_protocol,
        report.capabilities.osc52_clipboard,
        report.capabilities.focus_events,
        report.capabilities.bracketed_paste,
    );
    if !report.hints.is_empty() {
        for hint in &report.hints {
            let tag = match hint.level {
                HintLevel::Info => style_cyan("INFO", ansi),
                HintLevel::Warn => style_yellow("WARN", ansi),
            };
            println!("  [{tag}] {}", hint.message);
        }
    }
    println!();
    println!(
        "{:<16} {:<8} {:<16} {}",
        style_bold("TERMINAL", ansi),
        style_bold("ACTIVE", ansi),
        style_bold("STATUS", ansi),
        style_bold("CONFIG PATHS", ansi)
    );
    for entry in &report.entries {
        let active_str = if entry.active_detected {
            style_green("yes", ansi)
        } else {
            style_dim("no", ansi)
        };
        let status_str = color_status(entry.install_status, ansi);
        let paths = if entry.config_paths.is_empty() {
            "-".to_string()
        } else {
            entry
                .config_paths
                .iter()
                .map(|p| p.display().to_string())
                .collect::<Vec<_>>()
                .join(", ")
        };
        println!(
            "{:<16} {:<8} {:<16} {}",
            entry.kind.as_str(),
            active_str,
            status_str,
            paths
        );
        for note in &entry.notes {
            println!("  {} {}", style_dim("note:", ansi), note);
        }
    }
    Ok(())
}

pub fn handle_terminal_setup(
    args: &TerminalSetupArgs,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let host = default_setup_host();
    let env = TerminalEnv::from_env();
    handle_terminal_setup_with(args, &host, &env)
}

pub fn handle_terminal_setup_with(
    args: &TerminalSetupArgs,
    host: &TerminalSetupHost,
    env: &TerminalEnv,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    if args.apply {
        let receipt = host.apply(&args.app, env)?;
        render_setup_receipt(&receipt, args.json)
    } else if args.revert {
        let receipt = host.revert(&args.app, env)?;
        render_revert_receipt(&receipt, args.json)
    } else {
        let plan = host.plan(&args.app, env)?;
        render_setup_plan(&plan, args.json)
    }
}

fn render_setup_plan(
    plan: &SetupPlan,
    json: bool,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    if json {
        println!("{}", serde_json::to_string_pretty(plan)?);
        return Ok(());
    }

    let ansi = std::io::stdout().is_terminal();
    println!(
        "{} {} (dry-run, status={})",
        style_bold("Setup plan for", ansi),
        plan.terminal.display_name(),
        color_status(plan.status_before, ansi)
    );
    if plan.changes.is_empty() {
        println!("  No file changes required.");
    } else {
        for change in &plan.changes {
            println!(
                "  [{}] {} — {}",
                style_cyan(&change.action.to_string(), ansi),
                change.path.display(),
                change.summary
            );
            if !change.snippet.is_empty() {
                for line in change.snippet.lines() {
                    println!("      {}", style_dim(line, ansi));
                }
            }
        }
    }
    print_list("Warnings", &plan.warnings, ansi);
    print_list("Manual steps", &plan.manual_steps, ansi);
    Ok(())
}

fn render_setup_receipt(
    receipt: &SetupReceipt,
    json: bool,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    if json {
        println!("{}", serde_json::to_string_pretty(receipt)?);
        return Ok(());
    }

    let ansi = std::io::stdout().is_terminal();
    println!(
        "{} {} (applied={}, status={})",
        style_bold("Setup result for", ansi),
        receipt.terminal.display_name(),
        receipt.applied,
        color_status(receipt.status_after, ansi)
    );
    for path in &receipt.changed_paths {
        println!("  [{}] {}", style_green("changed", ansi), path.display());
    }
    for path in &receipt.backup_paths {
        println!("  [{}] {}", style_cyan("backup", ansi), path.display());
    }
    print_list("Warnings", &receipt.warnings, ansi);
    print_list("Manual steps", &receipt.manual_steps, ansi);
    Ok(())
}

fn render_revert_receipt(
    receipt: &RevertReceipt,
    json: bool,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    if json {
        println!("{}", serde_json::to_string_pretty(receipt)?);
        return Ok(());
    }

    let ansi = std::io::stdout().is_terminal();
    println!(
        "{} {} (reverted={})",
        style_bold("Revert result for", ansi),
        receipt.terminal.display_name(),
        receipt.reverted
    );
    for path in &receipt.restored_paths {
        println!("  [{}] {}", style_green("restored", ansi), path.display());
    }
    for path in &receipt.removed_paths {
        println!("  [{}] {}", style_yellow("removed", ansi), path.display());
    }
    print_list("Warnings", &receipt.warnings, ansi);
    Ok(())
}

fn print_list(label: &str, items: &[String], ansi: bool) {
    if items.is_empty() {
        return;
    }
    println!("{}:", style_bold(label, ansi));
    for item in items {
        println!("  - {item}");
    }
}

fn color_status(status: InstallStatus, ansi: bool) -> String {
    let text = status.label();
    match status {
        InstallStatus::Configured => style_green(text, ansi),
        InstallStatus::Partial => style_yellow(text, ansi),
        InstallStatus::NotConfigured => style_cyan(text, ansi),
        InstallStatus::NotApplicable | InstallStatus::Unsupported => style_dim(text, ansi),
    }
}

fn style_bold(text: &str, ansi: bool) -> String {
    if ansi {
        format!("\x1b[1m{text}\x1b[0m")
    } else {
        text.to_string()
    }
}

fn style_green(text: &str, ansi: bool) -> String {
    if ansi {
        format!("\x1b[32m{text}\x1b[0m")
    } else {
        text.to_string()
    }
}

fn style_yellow(text: &str, ansi: bool) -> String {
    if ansi {
        format!("\x1b[33m{text}\x1b[0m")
    } else {
        text.to_string()
    }
}

fn style_cyan(text: &str, ansi: bool) -> String {
    if ansi {
        format!("\x1b[36m{text}\x1b[0m")
    } else {
        text.to_string()
    }
}

fn style_dim(text: &str, ansi: bool) -> String {
    if ansi {
        format!("\x1b[2m{text}\x1b[0m")
    } else {
        text.to_string()
    }
}
