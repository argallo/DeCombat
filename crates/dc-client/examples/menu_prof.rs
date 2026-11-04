//! Frame profile of the menu screens (offscreen, no window, no server): every screen with made-up lobby data, driven
//! by synthetic inputs (idle, pointer moves every frame, typing, clicks) and drawn each frame the way the menu
//! window draws it, with the frame profiler's stages ([`dc_client::prof`]).
//!
//! ```text
//! cargo run --release -p dc-client --example menu_prof -- --rom <CA_ROM> --out <prof.json> [--size WxH]
//!     [--frames N] [--path gpu|cpu] [--screens login,lobby,..] [--inputs idle,move,type,click]
//! ```
//! `--path cpu` is the old path (the whole screen composed on the CPU and uploaded as one image when it changed);
//! `gpu` (default) the menu's display list as overlay quads ([`dc_client::menu::paint::MenuPainter`]).
//! Prints one line per screen and input (frames, mean / median / p95 ms, fps) and writes the profiler's JSON reports
//! of every combination into `--out`.

use dc_botclient::lobby::{LobbyProfile, RoomMember, ServerEntry};
use dc_client::menu::link::{ChannelRow, LinkOptions, RoomRow, RoomView, UserRow};
use dc_client::menu::paint::MenuPainter;
use dc_client::menu::{Go, Menu, MenuInput, MenuKey, MenuOptions};
use dc_client::overlay::OverlayRenderer;
use dc_client::prof::{self, Laps, Recorder, Stage};
use dc_render::{AdapterChoice, GpuContext};
use std::collections::VecDeque;
use std::path::PathBuf;
use std::time::Instant;

fn sample_data(menu: &mut Menu) {
    let d = &mut menu.data;
    d.servers = (0..3)
        .map(|i| ServerEntry {
            id: i + 1,
            name: format!("DeCombat {}", i + 1),
            ip: "127.0.0.1".into(),
            port: 1,
            open: 1,
            capacity: 100,
            current: [10, 60, 97][i as usize],
            clan_only: 0,
            op_capacity: 0,
            op_current: 0,
        })
        .collect();
    d.server = d.servers.first().cloned();
    d.profile = LobbyProfile {
        nick: "casey".into(),
        char_no: 1,
        gp: 500000,
        exp: 225000,
        my_id: 1,
        chan_label: "Channel 1".into(),
        ..LobbyProfile::default()
    };
    d.channels = (0..3)
        .map(|i| ChannelRow {
            id: i,
            name: format!("Channel {}", i + 1),
            open: 1,
            capacity: 100,
            current: [5, 50, 90][i as usize],
            op_capacity: 0,
            op_current: 0,
        })
        .collect();
    d.rooms = (0..12)
        .map(|i| RoomRow {
            number: i,
            title: format!("room {i}"),
            locked: i % 3 == 1,
            map: (i % 3) as i32,
            mode: (i % 2) as i32,
            players: 2,
            max_players: 8,
            ..RoomRow::default()
        })
        .collect();
    d.users = (0..10)
        .map(|i| UserRow {
            account: i + 1,
            id: i + 1,
            name: format!("Player{i}"),
            exp: 0,
            ..UserRow::default()
        })
        .collect();
    d.room = RoomView {
        number: 0,
        title: "casey room".into(),
        host_id: 1,
        map: 2,
        mode: 0,
        players: 2,
        max_players: 8,
        goal: 100,
        ..RoomView::default()
    };
    for i in 1..=4u32 {
        d.members.insert(
            i,
            RoomMember {
                char_no: i as i32,
                name: format!("Player{i}"),
                team: 1 + (i % 2) as i32,
                state: (i > 1) as i32,
                exp: 0,
            },
        );
    }
    d.chat = (0..8).map(|i| format!("Player{i} : hello {i}")).collect();
}

/// The inputs of frame `f` for scenario `what` (positions in the virtual 1024 x 768 screen).
fn inputs(what: &str, f: usize) -> Vec<MenuInput> {
    let t = f as f32 * 0.05;
    let p = [512.0 + 400.0 * t.cos(), 384.0 + 300.0 * (t * 1.3).sin()];
    match what {
        "move" => vec![MenuInput::Move(p)],
        "type" if f.is_multiple_of(2) => vec![MenuInput::Char('x')],
        "type" => vec![MenuInput::Key(MenuKey::Backspace)],
        "click" => vec![MenuInput::Press([2.0, 2.0]), MenuInput::Release([2.0, 2.0])],
        _ => vec![],
    }
}

struct Target {
    view: wgpu::TextureView,
    size: (u32, u32),
}

fn main() {
    let mut rom = None;
    let mut out = PathBuf::from("menu_prof.json");
    let mut size = (2560u32, 1440u32);
    let mut frames = 1500usize;
    let mut path = "gpu".to_string();
    let mut screens = "login,servers,channels,lobby,create,room".to_string();
    let mut what = "idle,move,type,click".to_string();
    let mut it = std::env::args().skip(1);
    while let Some(a) = it.next() {
        let v = it.next().unwrap_or_default();
        match a.as_str() {
            "--rom" => rom = Some(PathBuf::from(v)),
            "--out" => out = PathBuf::from(v),
            "--frames" => frames = v.parse().unwrap_or(frames),
            "--path" => path = v,
            "--screens" => screens = v,
            "--inputs" => what = v,
            "--size" => {
                if let Some((w, h)) = v.split_once('x') {
                    size = (w.parse().unwrap_or(2560), h.parse().unwrap_or(1440));
                }
            }
            _ => {}
        }
    }
    prof::enable(out.with_extension("last.json"));
    let rom = rom.or_else(|| dc_rom::resolve_rom().ok());
    let mut menu = Menu::new(MenuOptions {
        auth: "127.0.0.1:1".parse().unwrap(),
        user: "casey".into(),
        link: LinkOptions::new("menu-prof"),
        rom,
        font: Some((dc_client::testkit::synth_font(), 0)),
        width: size.0,
        height: size.1,
    });
    println!("{} | {}x{} | path {path}", menu.source(), size.0, size.1);
    sample_data(&mut menu);
    let ctx = GpuContext::headless(AdapterChoice::Auto).expect("adapter");
    let format = wgpu::TextureFormat::Bgra8Unorm;
    let tex = ctx.device.create_texture(&wgpu::TextureDescriptor {
        label: Some("menu prof target"),
        size: wgpu::Extent3d {
            width: size.0,
            height: size.1,
            depth_or_array_layers: 1,
        },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format,
        usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
        view_formats: &[],
    });
    let target = Target {
        view: tex.create_view(&Default::default()),
        size,
    };
    let mut painter = Painter::new(&ctx, format, &path);
    let inflight = prof::config().inflight;
    let mut reports = Vec::new();
    let mut table = Vec::new();
    for s in screens.split(',') {
        let g = match s {
            "login" => Go::Login,
            "servers" => Go::Servers,
            "channels" => Go::Channels,
            "lobby" => Go::Lobby,
            "create" => Go::Create,
            "room" => Go::Room,
            "result" => Go::Result,
            _ => continue,
        };
        for w in what.split(',') {
            menu.open(g);
            let mut rec = Recorder::new();
            rec.note = format!("menu {s} {w} {}x{} {path}", size.0, size.1);
            let mut queue = VecDeque::new();
            let mut totals = Vec::with_capacity(frames);
            for f in 0..frames {
                rec.begin(None);
                let t0 = Instant::now();
                let mut laps = Laps::start();
                for ev in inputs(w, f) {
                    menu.input(ev);
                }
                laps.lap(Stage::Input);
                menu.update();
                laps.lap(Stage::Net);
                painter.prepare(&ctx, &mut menu, &mut laps);
                let mut enc = ctx
                    .device
                    .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                        label: Some("menu prof"),
                    });
                painter.encode(&ctx, &mut enc, &target);
                laps.lap(Stage::Encode);
                let idx = ctx.queue.submit([enc.finish()]);
                laps.lap(Stage::Submit);
                queue.push_back(idx);
                while queue.len() > inflight {
                    if let Some(old) = queue.pop_front() {
                        let _ = ctx.device.poll(wgpu::PollType::WaitForSubmissionIndex(old));
                    }
                }
                let _ = ctx.device.poll(wgpu::PollType::Poll);
                laps.lap(Stage::GpuWait);
                rec.end(0);
                totals.push(t0.elapsed().as_secs_f64() * 1000.0);
            }
            let _ = ctx.device.poll(wgpu::PollType::Wait);
            reports.push(format!("\"{s}/{w}\": {}", rec.finish().trim_end()));
            let warm = prof::config().warmup.min(totals.len().saturating_sub(1));
            let mut v = totals[warm..].to_vec();
            v.sort_by(|a, b| a.total_cmp(b));
            let mean = v.iter().sum::<f64>() / v.len().max(1) as f64;
            let med = v[v.len() / 2];
            let p95 = v[(v.len() * 95 / 100).min(v.len() - 1)];
            table.push(format!(
                "| {s} | {w} | {} | {mean:.4} | {med:.4} | {p95:.4} | {:.0} | {:.0} |",
                v.len(),
                1000.0 / mean,
                1000.0 / med
            ));
        }
    }
    println!(
        "\n| screen | input | frames | mean ms | median ms | p95 ms | fps mean | fps median |"
    );
    println!("|---|---|---|---|---|---|---|---|");
    for l in &table {
        println!("{l}");
    }
    let json = format!("{{\n{}\n}}\n", reports.join(",\n"));
    std::fs::write(&out, json).expect("write report");
    let _ = std::fs::write(out.with_extension("table.md"), table.join("\n") + "\n");
    println!("{}", out.display());
}

/// `cpu`: the old menu window path (the whole screen composed on the CPU and uploaded when it changed); `gpu`: the
/// menu window's path now ([`MenuPainter`]).
enum Painter {
    Cpu(OverlayRenderer),
    Gpu(MenuPainter),
}

impl Painter {
    fn new(ctx: &GpuContext, format: wgpu::TextureFormat, path: &str) -> Painter {
        match path {
            "cpu" => Painter::Cpu(OverlayRenderer::new(ctx, format)),
            _ => Painter::Gpu(MenuPainter::new(ctx, format)),
        }
    }

    fn prepare(&mut self, ctx: &GpuContext, menu: &mut Menu, laps: &mut Laps) {
        match self {
            Painter::Cpu(layer) => {
                if menu.needs_redraw() {
                    let img = menu.image();
                    laps.lap(Stage::HudCompose);
                    layer.update(ctx, &img);
                    laps.lap(Stage::HudUpload);
                }
            }
            Painter::Gpu(p) => {
                p.prepare(ctx, menu, laps);
            }
        }
    }

    fn encode(&self, ctx: &GpuContext, enc: &mut wgpu::CommandEncoder, t: &Target) {
        match self {
            Painter::Gpu(p) => p.encode(ctx, enc, &t.view, t.size),
            Painter::Cpu(layer) => {
                {
                    let _clear = enc.begin_render_pass(&wgpu::RenderPassDescriptor {
                        label: Some("menu clear"),
                        color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                            view: &t.view,
                            resolve_target: None,
                            ops: wgpu::Operations {
                                load: wgpu::LoadOp::Clear(wgpu::Color::BLACK),
                                store: wgpu::StoreOp::Store,
                            },
                        })],
                        depth_stencil_attachment: None,
                        timestamp_writes: None,
                        occlusion_query_set: None,
                    });
                }
                layer.encode(ctx, enc, &t.view, t.size, (0, 0));
            }
        }
    }
}
