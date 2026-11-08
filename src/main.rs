mod app;
mod constants;
mod geometry;

use anyhow::Result;
use std::time::Duration;

use app::control;
use app::term::caps::ChildRole;

fn main() -> Result<()> {
    let mut args = std::env::args().skip(1);
    match args.next().as_deref() {
        None => launch(false),
        Some("--fresh") => launch(true),
        Some("--doctor") => {
            doctor();
            Ok(())
        }
        Some("--version" | "-V") => {
            println!("uncl {}", env!("CARGO_PKG_VERSION"));
            Ok(())
        }
        Some("--help" | "-h") => {
            help();
            Ok(())
        }
        Some(other) => {
            eprintln!("uncl: unknown argument `{other}`\n");
            help();
            std::process::exit(2);
        }
    }
}

/// Start the UI — or, when this shell is already inside uncl, explain or toggle
/// instead. uncl does not nest.
///
/// Which of the two it is comes from `UNCL_ROLE`, which uncl writes into the
/// environment of every child it spawns:
///
/// * **owner** (the shell behind the float) — the terminal this process would
///   take over is uncl's own. "launch uncl here" therefore means what `Home`
///   means: raise the float or put it away. The request goes back through the
///   pty as one private `OSC` and this process exits without drawing a cell; see
///   [`app::control`].
/// * **tenant** (the floating shell) — there is already a float *and* a shell
///   behind it, so a third terminal has nowhere to go. One sentence on stderr,
///   and exit non-zero so a script can tell.
///
/// Flags (`--doctor`, `--version`, `--help`) are not launches: they are handled
/// above this and still work from inside, because printing a report inside the
/// float breaks nothing.
fn launch(fresh: bool) -> Result<()> {
    match ChildRole::from_env() {
        Some(ChildRole::Owner) => {
            control::send_toggle_request()?;
            Ok(())
        }
        Some(ChildRole::Tenant) => {
            eprintln!("You're already within uncl, nesting refused.");
            std::process::exit(1);
        }
        None => run(fresh),
    }
}

/// Refuse to run on a console that would mangle every frame.
///
/// uncl writes raw ANSI to stdout (S2 owns the emitter). A legacy Windows console
/// needs `ENABLE_VIRTUAL_TERMINAL_PROCESSING` before it will honour any of it, and
/// crossterm's `ansi_support` module is the thing that turns it on and reports
/// whether the console can do it at all. Failing here with a sentence is strictly
/// better than painting an alternate screen full of `ESC[?25l`.
///
/// Only compiled on Windows: everywhere else the answer is a terminal that speaks
/// ANSI or is not worth supporting.
#[cfg(windows)]
fn ensure_ansi() -> Result<()> {
    if !crossterm::ansi_support::supports_ansi() {
        return Err(anyhow::anyhow!(
            "uncl needs a console with ANSI/VT support (Windows 10 1511+, or Windows Terminal).\n\
             ConPTY on this system cannot render uncl's frames."
        ));
    }
    Ok(())
}

#[cfg(not(windows))]
fn ensure_ansi() -> Result<()> {
    Ok(())
}

fn run(fresh: bool) -> Result<()> {
    ensure_ansi()?;

    // A hand-built runtime (instead of `#[tokio::main]`) so we can bound how
    // long shutdown waits for the blocking pty reader and input pump threads.
    // They end when their children die, but if one is wedged we would rather
    // exit than leave the user's terminal in raw mode.
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()?;

    let result = runtime.block_on(app::run(fresh));

    // The terminal has already been restored by the guard inside `app::run`.
    runtime.shutdown_timeout(Duration::from_millis(250));

    result
}

/// What uncl tells its children about the terminal, and why.
fn doctor() {
    // Measure the tty first, so the report shows the cell size children would
    // actually be given rather than the fallback.
    app::term::caps::probe_cell_px();
    let caps = app::term::caps::Capabilities::from_env();
    println!(
        "uncl {} — terminal capabilities\n",
        env!("CARGO_PKG_VERSION")
    );
    println!("{}", caps.summary());
    println!();
    println!(
        "children inherit: TERM={} COLORTERM={} TERM_PROGRAM=uncl",
        caps.child_term(),
        if caps.truecolor {
            "truecolor"
        } else {
            "<unset>"
        }
    );
    println!();
    println!("notes:");
    println!("  * TERM is passed through from the host, downgraded to xterm-256color for");
    println!("    screen/tmux (a dialect uncl does not speak) and xterm-kitty (its TERM");
    println!("    enables the kitty keyboard protocol, which the encoder does not implement)");
    println!("  * sixel is advertised only with UNCL_SIXEL=1 on a host known to render it");
    println!("  * kitty keyboard protocol is never advertised");
    println!("  * see docs for what the tap carries and why");
}

fn help() {
    println!(
        "uncl {}\n\
         unified nuclear cli layer\n\n\
         usage: uncl [--fresh|--doctor|--version|--help]\n\n\
         flags:\n  \
         --fresh            ignore saved position/size; spawn the float centred at 80x25\n  \
         --doctor           diagnose current pty compatibility\n  \
         --version          print current version\n  \
         --help             display this help screen\n\n\
         keys:\n  \
         Home               toggle the floating terminal\n  \
         Alt+arrows         move it\n  \
         Alt+Shift+arrows   resize it\n  \
         Alt+drag           move it with the mouse\n  \
         border drag        resize it with the mouse\n\n\
         nesting:\n  \
         uncl does not nest: running it in the background shell toggles the float\n  \
         exactly like Home, and running it inside the float is refused with a\n  \
         message instead of opening a terminal in a terminal.\n\n\
         everything else is delivered to whichever terminal has focus.",
        env!("CARGO_PKG_VERSION")
    );
}
