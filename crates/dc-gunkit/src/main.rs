//! `dc-gunkit`: the console tool. With no verb it opens the window. `mcp` lets an agent drive the window; the other verbs need no
//! window at all.

use dc_gunkit::commands::{self, Reply};
use dc_gunkit::live::{self, Client};
use dc_gunkit::mcp::{self, Backend};
use dc_gunkit::{pack, rom, studio::Studio};
use serde_json::{json, Value};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

const USAGE: &str = "\
dc-gunkit: design camos on the game's own guns, by hand or with an agent (docs/GUNKIT_AGENTS.md)

  dc-gunkit [gui] [--rom DIR] [--gun NAME]   open the window
  dc-gunkit mcp [--headless] [--rom DIR]     an MCP server over stdio that drives the window, and opens it if needed;
                                             --headless runs without a window, for machines with no display
  dc-gunkit guns [--rom DIR] [--filter F]    list the guns, as JSON
  dc-gunkit render --gun G -o OUT.png [--rom DIR] [--view first_person|third_person] [--pattern PNG|PRESET]
                   [--open PACK_DIR] [--yaw D] [--pitch D] [--size WxH]
                                             one picture, with no window
  dc-gunkit check PACK_DIR [--rom DIR]       check a pack as dc-modkit does, and read its variants

The game folder is found by --rom, CA_ROM, the last one used, or a rom folder near the program.
";

struct Args(Vec<String>);

impl Args {
    fn value(&self, flag: &str) -> Option<String> {
        self.0
            .iter()
            .position(|a| a == flag)
            .and_then(|i| self.0.get(i + 1))
            .cloned()
    }

    fn has(&self, flag: &str) -> bool {
        self.0.iter().any(|a| a == flag)
    }

    fn rom(&self) -> Option<PathBuf> {
        self.value("--rom").map(PathBuf::from)
    }
}

fn find_rom(args: &Args) -> Result<PathBuf, String> {
    let named = args.rom();
    rom::discover(named.as_deref(), live::home_dir().as_deref()).ok_or_else(|| match &named {
        Some(n) => format!(
            "{} is not a game folder (it needs Engine.exe and a Game folder)",
            n.display()
        ),
        None => "no game folder found: pass --rom or set CA_ROM".to_string(),
    })
}

/// Starts the window, apart from this process: its output must not reach an MCP client's stdout.
fn start_window(named_rom: Option<&Path>) -> Result<(), String> {
    let exe = std::env::current_exe().map_err(|e| e.to_string())?;
    let gui = exe.with_file_name(if cfg!(windows) {
        "dc-gunkit-gui.exe"
    } else {
        "dc-gunkit-gui"
    });
    let mut c = std::process::Command::new(&gui);
    if let Some(r) = named_rom {
        c.arg("--rom").arg(r);
    }
    c.stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
        .map(|_| ())
        .map_err(|e| format!("could not open the window ({}): {e}", gui.display()))
}

/// Drives the window on the user's screen, opening it when none is running.
struct WindowBackend {
    home: PathBuf,
    rom: Option<PathBuf>,
    client: Option<Client>,
}

impl WindowBackend {
    fn connect(&mut self) -> Result<&mut Client, String> {
        if self.client.is_none() {
            self.client = Client::connect(&self.home);
        }
        if self.client.is_none() {
            start_window(self.rom.as_deref())?;
            let start = Instant::now();
            while self.client.is_none() && start.elapsed() < Duration::from_secs(30) {
                std::thread::sleep(Duration::from_millis(150));
                self.client = Client::connect(&self.home);
            }
        }
        self.client
            .as_mut()
            .ok_or_else(|| "the studio window did not start".to_string())
    }
}

impl Backend for WindowBackend {
    fn call(&mut self, tool: &str, args: &Value) -> Result<Reply, String> {
        for _ in 0..2 {
            match self.connect()?.call(tool, args) {
                Ok(answer) => return answer,
                // the window was closed since the last call: open a new one
                Err(_) => self.client = None,
            }
        }
        Err("the studio window is not answering".into())
    }
}

/// Runs the studio inside this process.
struct LocalBackend {
    rom: PathBuf,
    studio: Option<Studio>,
}

impl Backend for LocalBackend {
    fn call(&mut self, tool: &str, args: &Value) -> Result<Reply, String> {
        if tool == "window_screenshot" {
            return Err("there is no window: this server runs with --headless".into());
        }
        if self.studio.is_none() {
            self.studio = Some(Studio::open(&self.rom)?);
        }
        let s = self.studio.as_mut().ok_or("no studio")?;
        commands::run(s, tool, args)
    }
}

fn run(args: &Args, verb: &str) -> Result<(), String> {
    match verb {
        "mcp" => {
            let result = if args.has("--headless") {
                let rom = find_rom(args)?;
                mcp::serve(&mut LocalBackend { rom, studio: None })
            } else {
                let home = live::home_dir().ok_or("no folder for the studio's files")?;
                mcp::serve(&mut WindowBackend {
                    home,
                    rom: args.rom(),
                    client: None,
                })
            };
            result.map_err(|e| e.to_string())
        }
        "guns" => {
            let list = dc_gunkit::guns::read_guns(&find_rom(args)?)?;
            let filter = args
                .value("--filter")
                .unwrap_or_default()
                .to_ascii_lowercase();
            let out: Vec<Value> = list
                .iter()
                .filter(|g| filter.is_empty() || g.name.to_ascii_lowercase().contains(&filter))
                .map(|g| g.summary())
                .collect();
            println!("{}", json!({"guns": out}));
            Ok(())
        }
        "render" => render(args),
        "check" => {
            let dir = args
                .0
                .get(1)
                .filter(|a| !a.starts_with('-'))
                .ok_or("check needs a pack folder")?;
            let dir = PathBuf::from(dir);
            let report = dc_modkit::validate_mod(&dir, args.rom().as_deref());
            let lint = dc_modkit::lint_mod(&dir)?;
            let variants = pack::variants_of(&dir).map(|v| {
                v.iter()
                    .map(|x| json!({"row": x.row, "base": x.base, "name": x.name}))
                    .collect::<Vec<_>>()
            });
            println!(
                "{}",
                json!({
                    "ok": report.ok,
                    "errors": report.errors,
                    "warnings": report.warnings,
                    "lint": lint.iter().map(|d| d.to_string()).collect::<Vec<_>>(),
                    "variants": variants.unwrap_or_default(),
                })
            );
            if report.ok {
                Ok(())
            } else {
                std::process::exit(1)
            }
        }
        other => Err(format!("no verb named {other:?}\n\n{USAGE}")),
    }
}

fn render(args: &Args) -> Result<(), String> {
    let out = args.value("-o").ok_or("render needs -o OUT.png")?;
    let mut s = Studio::open(&find_rom(args)?)?;
    s.whole_gun = !args.has("--no-whole");
    if let Some(dir) = args.value("--open") {
        let opened = pack::open(Path::new(&dir))?;
        commands::apply_opened(&mut s, opened)?;
    }
    if let Some(g) = args.value("--gun") {
        commands::run(&mut s, "select_gun", &json!({"gun": g}))?;
    }
    if s.row().is_none() {
        return Err("render needs --gun or --open".into());
    }
    if let Some(v) = args.value("--view") {
        commands::run(&mut s, "set_view", &json!({"view": v}))?;
    }
    if let Some(p) = args.value("--pattern") {
        let a = if p.to_ascii_lowercase().ends_with(".png")
            || p.to_ascii_lowercase().ends_with(".dtx")
        {
            json!({"path": p})
        } else {
            json!({"preset": p})
        };
        commands::run(&mut s, "set_pattern", &a)?;
    }
    let number = |flag: &str| args.value(flag).and_then(|v| v.parse::<f64>().ok());
    if let Some(v) = number("--yaw") {
        commands::run(&mut s, "set_camera", &json!({"yaw": v}))?;
    }
    if let Some(v) = number("--pitch") {
        commands::run(&mut s, "set_camera", &json!({"pitch": v}))?;
    }
    let (w, h) = args
        .value("--size")
        .and_then(|t| {
            let (a, b) = t.split_once('x')?;
            Some((a.parse().ok()?, b.parse().ok()?))
        })
        .unwrap_or((1280u32, 720u32));
    let r = commands::run(
        &mut s,
        "screenshot",
        &json!({"width": w, "height": h, "path": out}),
    )?;
    println!("{}", r.json);
    Ok(())
}

fn main() {
    let args = Args(std::env::args().skip(1).collect());
    if args.has("--help") || args.has("-h") {
        print!("{USAGE}");
        return;
    }
    let verb = args.0.first().filter(|a| !a.starts_with('-')).cloned();
    let result = match verb.as_deref() {
        None | Some("gui") => open_window(&args),
        Some(v) => run(&args, v),
    };
    if let Err(e) = result {
        eprintln!("dc-gunkit: {e}");
        std::process::exit(1);
    }
}

#[cfg(feature = "gui")]
fn open_window(args: &Args) -> Result<(), String> {
    // on Windows the console binary stays small and starts the window's own binary, as dc-mapkit does
    if cfg!(windows) {
        return start_window(args.rom().as_deref());
    }
    dc_gunkit::gui::launch(args.rom(), args.value("--gun"))
}

#[cfg(not(feature = "gui"))]
fn open_window(_args: &Args) -> Result<(), String> {
    Err("this build has no window".into())
}
