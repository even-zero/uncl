mod app;
mod constants;
mod geometry;

use anyhow::Result;
use std::fmt;
use std::sync::OnceLock;
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

/// Start the UI, ask for a toggle, or refuse — uncl does not nest.
///
/// [`Nesting`] below makes the call from the `UNCL_ROLE` marker uncl leaves in
/// every child's environment and, on Linux, the process tree above this one.
///
/// * **toggle** — a child of the owner pane with a clear path to it: the terminal
///   this process would take over is uncl's own, so "launch uncl here" means what
///   `Home` means. One private `OSC` goes back over the pty and this process exits
///   without drawing a cell; see [`app::control`].
/// * **refuse** — inside the float, or inside uncl through a multiplexer (which
///   would swallow the request and nest anyway). One sentence on stderr, exit
///   non-zero so a script can tell.
/// * **launch** — anything else. When the UI hands the terminal back to the shell
///   that started us, that is the last moment uncl exists, and it says so.
///
/// Flags (`--doctor`, `--version`, `--help`) are not launches: they are handled
/// above this and still work from inside, because printing a report inside the
/// float breaks nothing.
fn launch(fresh: bool) -> Result<()> {
    match Nesting::get().launch() {
        // The UI has handed the terminal back to the shell that started us, so
        // this is the last thing uncl says: a word on the way out.
        Launch::Ui => {
            run(fresh)?;
            println!("uncl says, \"when the money's gone, it's time to move on.\"");
            Ok(())
        }
        // The same request `Home` makes, over the pane's own pty; see `control`.
        Launch::Toggle => Ok(control::send_toggle_request()?),
        Launch::Refuse(hint) => {
            eprintln!("You're already within uncl, nesting prohibited.");
            if let Some(hint) = hint {
                eprintln!("{hint}");
            }
            std::process::exit(1)
        }
    }
}

// ─── which pane am I in? ──────────────────────────────────────────────────────
//
// uncl does not nest: a `uncl` started inside uncl either toggles the float (the
// owner pane — this is `Home`) or refuses (the float itself). That rests on
// `UNCL_ROLE`, which uncl writes into every child it spawns, and that is exact
// until a terminal multiplexer gets in the way: a multiplexer's panes inherit the
// *server's* environment, frozen when that server started. zellij has no
// equivalent of tmux's `update-environment`, so a zellij server started outside
// uncl hands its panes no marker at all, and a launch inside the float would
// nest. So the marker is the fast path and the process tree (`/proc`, Linux only)
// is the fallback. The readme has the long version; `--doctor` prints what this
// process actually sees.

/// The line printed when a refusal is not self-explanatory: the launch *looks*
/// like an owner-pane one, but the toggle could not reach the owner.
const BEHIND_MULTIPLEXER: &str =
    "uncl: a multiplexer is in between, so the float cannot be toggled from here";

/// The line printed when uncl is above us but the environment no longer says so.
const MARKER_LOST: &str = "uncl: this shell is inside uncl, but the marker did not survive the trip (see `uncl --doctor`)";

/// The process name that means "this is uncl's own pty".
const UNCL: &str = "uncl";

/// Multiplexers whose panes take the environment of their server, not of the
/// terminal drawing them.
const MULTIPLEXERS: &[&str] = &["tmux", "zellij", "screen"];

/// How far up the process tree to walk: a shell hierarchy is a handful of levels,
/// and the cap is what keeps a broken process table from looping.
const DEPTH: usize = 128;

/// What a launch here has to do.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Launch {
    /// Not inside uncl: start the UI.
    Ui,
    /// A child of the owner pane: the same as pressing `Home`.
    Toggle,
    /// Already inside uncl. `Some(line)` explains a refusal that is not obvious.
    Refuse(Option<&'static str>),
}

/// The one answer to "is this process inside uncl?" — probed once, then reused.
///
/// Nothing it reads can change while uncl is starting, so it is a singleton
/// rather than something passed around: the environment is fixed at exec and the
/// ancestors are the ones we were launched under.
struct Nesting {
    /// `UNCL_ROLE`: which pane spawned us, if the marker survived the trip.
    role: Option<ChildRole>,
    /// Where uncl sits in the process tree, if anywhere.
    above: Option<Above>,
}

/// Where uncl sits above us, and what stands between.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Above {
    /// An `uncl` process: our output reaches it.
    Uncl,
    /// An `uncl` behind a multiplexer: our output stops at the multiplexer.
    UnclBehindMultiplexer,
}

impl Nesting {
    /// The process-wide instance.
    fn get() -> &'static Self {
        static INSTANCE: OnceLock<Nesting> = OnceLock::new();
        INSTANCE.get_or_init(|| Self {
            role: ChildRole::from_env(),
            above: Above::read(),
        })
    }

    /// What to do here.
    ///
    /// The marker wins when it is there, because it names the pane; the tree only
    /// says "somewhere inside uncl". Where both are available and disagree, the
    /// tree wins: a marker we cannot reach through is worse than no marker.
    fn launch(&self) -> Launch {
        use ChildRole::{Owner, Tenant};

        match (self.role, self.above) {
            // The float, whatever the tree says: a third terminal has nowhere to go.
            (Some(Tenant), _) => Launch::Refuse(None),
            // The owner pane behind a multiplexer: this is `Home` with the wire cut.
            (Some(Owner), Some(Above::UnclBehindMultiplexer)) => {
                Launch::Refuse(Some(BEHIND_MULTIPLEXER))
            }
            // The owner pane, and the request reaches it: this *is* `Home`.
            (Some(Owner), _) => Launch::Toggle,
            // No marker, but uncl above us: a multiplexer's server env, `sudo`, or
            // a wrapper that scrubs the environment.
            (None, Some(_)) => Launch::Refuse(Some(MARKER_LOST)),
            (None, None) => Launch::Ui,
        }
    }
}

impl fmt::Display for Nesting {
    /// One line for `--doctor`: every input to [`Nesting::launch`], and its
    /// verdict.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let role = self.role.map(ChildRole::env_value).unwrap_or("<unset>");
        let above = match self.above {
            Some(Above::Uncl) => "uncl above",
            Some(Above::UnclBehindMultiplexer) => "uncl above, a multiplexer in between",
            None if cfg!(target_os = "linux") => "no uncl above",
            None => "no uncl above (the tree is only walked on linux)",
        };
        let verdict = match self.launch() {
            Launch::Ui => "start uncl",
            Launch::Toggle => "toggle the float (this is Home)",
            Launch::Refuse(Some(_)) => "refuse to nest, and say why",
            Launch::Refuse(None) => "refuse to nest",
        };

        write!(
            f,
            "UNCL_ROLE={role}, {above}; a launch here would {verdict}"
        )
    }
}

impl Above {
    fn read() -> Option<Self> {
        Self::of(walk(stat_of))
    }

    /// What a walk of ancestor names, nearest first, means: the nearest `uncl`
    /// decides, and a multiplexer counted before it is in between.
    fn of(names: impl IntoIterator<Item = String>) -> Option<Self> {
        let mut between = false;
        for name in names {
            if name == UNCL {
                return Some(if between {
                    Self::UnclBehindMultiplexer
                } else {
                    Self::Uncl
                });
            }
            between |= MULTIPLEXERS.contains(&name.as_str());
        }
        None
    }
}

/// Ancestor names, nearest first, skipping this process.
///
/// `stat` is the platform lookup, taken as a parameter so the walk can be tested
/// without a process table to lie about.
fn walk(mut stat: impl FnMut(u32) -> Option<(String, u32)>) -> Vec<String> {
    let mut names = Vec::new();
    let mut pid = std::process::id();

    while names.len() < DEPTH {
        let Some((_, parent)) = stat(pid) else { break };
        // `0` is where the tree ends; a process that parents itself would loop.
        if parent == 0 || parent == pid {
            break;
        }
        let Some((name, _)) = stat(parent) else { break };
        names.push(name);
        pid = parent;
    }

    names
}

/// `(comm, ppid)` for one pid, from `/proc`.
#[cfg(target_os = "linux")]
fn stat_of(pid: u32) -> Option<(String, u32)> {
    parse_stat(&std::fs::read_to_string(format!("/proc/{pid}/stat")).ok()?)
}

/// No dependency-free way to walk the tree: `UNCL_ROLE` stays the only signal.
#[cfg(not(target_os = "linux"))]
fn stat_of(_: u32) -> Option<(String, u32)> {
    None
}

/// The `(comm, ppid)` fields of a `/proc/<pid>/stat` line.
///
/// The format is `pid (comm) state ppid …`, and `comm` may itself contain spaces
/// and parentheses, so the fields are counted from the *last* `)` — everything
/// after it is plain whitespace-separated numbers.
#[cfg(any(target_os = "linux", test))]
fn parse_stat(stat: &str) -> Option<(String, u32)> {
    let (head, tail) = stat.rsplit_once(')')?;
    let comm = head.split_once('(')?.1;
    let ppid = tail.split_whitespace().nth(1)?.parse().ok()?;
    Some((comm.to_string(), ppid))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn nesting(role: Option<ChildRole>, above: Option<Above>) -> Nesting {
        Nesting { role, above }
    }

    fn names(names: &[&str]) -> Vec<String> {
        names.iter().map(|name| name.to_string()).collect()
    }

    #[test]
    fn the_launch_table() {
        use ChildRole::{Owner, Tenant};
        let uncl = Some(Above::Uncl);
        let behind = Some(Above::UnclBehindMultiplexer);

        // Outside uncl: start the UI. Also what a platform without `/proc` sees.
        assert_eq!(nesting(None, None).launch(), Launch::Ui);
        // The owner pane: this is `Home`.
        assert_eq!(nesting(Some(Owner), uncl).launch(), Launch::Toggle);
        assert_eq!(nesting(Some(Owner), None).launch(), Launch::Toggle);
        // Behind a multiplexer the request would die, marker or not.
        assert_eq!(
            nesting(Some(Owner), behind).launch(),
            Launch::Refuse(Some(BEHIND_MULTIPLEXER))
        );
        // The float: nowhere to put a third terminal.
        assert_eq!(nesting(Some(Tenant), uncl).launch(), Launch::Refuse(None));
        assert_eq!(nesting(Some(Tenant), behind).launch(), Launch::Refuse(None));
        // zellij's server env: no marker, but uncl is above us.
        assert_eq!(
            nesting(None, uncl).launch(),
            Launch::Refuse(Some(MARKER_LOST))
        );
        assert_eq!(
            nesting(None, behind).launch(),
            Launch::Refuse(Some(MARKER_LOST))
        );
    }

    #[test]
    fn the_nearest_uncl_decides_whether_a_multiplexer_is_in_between() {
        // A multiplexer under the pane's shell with uncl further up: between.
        assert_eq!(
            Above::of(names(&["sh", "zellij", "sh", "uncl"])),
            Some(Above::UnclBehindMultiplexer)
        );
        // uncl itself inside tmux and a child in its owner pane: not between.
        assert_eq!(
            Above::of(names(&["sh", "uncl", "sh", "tmux"])),
            Some(Above::Uncl)
        );
        // No uncl at all: the multiplexer says nothing.
        assert_eq!(Above::of(names(&["sh", "zellij"])), None);
        assert_eq!(Above::of(Vec::<String>::new()), None);
    }

    #[test]
    fn the_walk_reports_ancestors_only_and_always_ends() {
        // `walk` starts at this process's pid, so the fake tree is relative to
        // it: us -> shell -> systemd -> init. No literal pids anywhere.
        let me = std::process::id();
        let tree = |pid: u32| match pid {
            p if p == me => Some((String::from("uncl"), me + 1)), // never reported
            p if p == me + 1 => Some((String::from("sh"), me + 2)),
            p if p == me + 2 => Some((String::from("systemd"), 0)),
            _ => None,
        };
        assert_eq!(walk(tree), ["sh", "systemd"]);

        // Self-parented: a broken table cannot loop the walk.
        assert!(walk(|pid| Some((String::from("sh"), pid))).is_empty());

        // Unreadable pid: the table moved under us; stop rather than guess.
        assert!(walk(|_| None).is_empty());

        // Infinitely deep: the cap ends it, and only ancestors are in the list.
        let deep = walk(|pid| Some((format!("p{pid}"), pid + 1)));
        assert_eq!(deep.len(), DEPTH);
        assert_eq!(deep[0], format!("p{}", me + 1));
        assert!(!deep.iter().any(|name| name == &format!("p{me}")));
    }
    #[test]
    fn a_stat_line_is_read_from_the_last_parenthesis() {
        let uncl = Some((String::from("uncl"), 7));
        assert_eq!(parse_stat("12 (uncl) S 7 12 12 0"), uncl);
        // `comm` may hold spaces and parentheses.
        assert_eq!(
            parse_stat("12 (my shell (v2)) S 7 12 12 0"),
            Some((String::from("my shell (v2)"), 7))
        );
        // Truncated or nonsense lines end the walk instead of panicking.
        assert_eq!(parse_stat("12 (uncl) S"), None);
        assert_eq!(parse_stat("12 (uncl) S notanumber 0"), None);
        assert_eq!(parse_stat("no parens at all"), None);
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
    // Every input to the nesting decision, and its verdict: a multiplexer
    // between the panes hides the role marker, so a surprising refusal is only
    // diagnosable if the inputs are visible.
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
         border drag        resize it with the mouse\n  \
         drag               select text in the pane the drag starts in\n  \
         Ctrl+C             copy the selection (otherwise the shell's interrupt)\n  \
         Ctrl+V             paste the clipboard\n\n\
         everything else is delivered to whichever terminal has focus.",
        env!("CARGO_PKG_VERSION")
    );
}
