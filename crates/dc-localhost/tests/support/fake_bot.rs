// A stand-in child for the supervisor tests (compiled by tests/supervise.rs with rustc).
//
// Bot role (no `-port` argument): records its arguments and its DC_SEAT_TICKET in `$DC_FAKE_BOT_DIR/<name>.txt`, then
// waits for its stdin to close (the way a bot started with --parent-pipe does) and appends "eof" before it exits.
// With DC_FAKE_BOT=stubborn it ignores the end of its stdin and sleeps for a minute instead.
//
// Server role (a `-port` argument): records its arguments in `$DC_FAKE_BOT_DIR/server.txt`, prints what
// DC_FAKE_SERVER asks for, then reads its stdin until `quit` or EOF:
//   ready       `@ready port=40000 world=x`
//   bad-before  a line that is not UTF-8, then `@ready ...`
//   bad-after   `@ready ...`, a line that is not UTF-8, then `@join 0 7 0 0 After`
//   flood       `@ready ...`, 4 MB of output, then `@join 0 8 0 0 Flood`
//   long        `@ready ...`, one 200 KB line, then `@join 0 9 0 0 Long`
//   silent      prints nothing, then waits on stdin
use std::io::{BufRead, Read, Write};

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let dir = std::env::var("DC_FAKE_BOT_DIR").expect("DC_FAKE_BOT_DIR");
    if args.iter().any(|a| a == "-port") {
        return server(&args, &dir);
    }
    let name = args
        .windows(2)
        .find(|w| w[0] == "--name")
        .map(|w| w[1].clone())
        .unwrap_or_else(|| "unnamed".into());
    let path = std::path::Path::new(&dir).join(format!("{name}.txt"));
    let ticket = std::env::var("DC_SEAT_TICKET").unwrap_or_default();
    {
        let mut f = std::fs::File::create(&path).unwrap();
        writeln!(f, "args {}", args.join(" ")).unwrap();
        writeln!(f, "ticket {ticket}").unwrap();
        writeln!(
            f,
            "joinsecret {}",
            std::env::var("DC_JOIN_SECRET").unwrap_or_default()
        )
        .unwrap();
    }
    println!("fake bot {name} running");
    if std::env::var("DC_FAKE_BOT").as_deref() == Ok("stubborn") {
        std::thread::sleep(std::time::Duration::from_secs(60));
        return;
    }
    let mut sink = Vec::new();
    let _ = std::io::stdin().read_to_end(&mut sink);
    let mut f = std::fs::OpenOptions::new().append(true).open(&path).unwrap();
    writeln!(f, "eof").unwrap();
}

fn server(args: &[String], dir: &str) {
    let path = std::path::Path::new(dir).join("server.txt");
    let mut rec = std::fs::File::create(&path).unwrap();
    writeln!(rec, "args {}", args.join(" ")).unwrap();
    writeln!(
        rec,
        "secret {}",
        std::env::var("DC_JOIN_SECRET").unwrap_or_default()
    )
    .unwrap();
    let mode = std::env::var("DC_FAKE_SERVER").unwrap_or_else(|_| "ready".into());
    let ready = b"@ready port=40000 world=x\n";
    let bad: &[u8] = b"\xff\xfe not utf-8 \xc3\x28\n";
    {
        let mut out = std::io::stdout().lock();
        match mode.as_str() {
            "bad-before" => {
                out.write_all(bad).unwrap();
                out.write_all(ready).unwrap();
            }
            "bad-after" => {
                out.write_all(ready).unwrap();
                out.write_all(bad).unwrap();
                out.write_all(b"@join 0 7 0 0 After\n").unwrap();
            }
            "flood" => {
                out.write_all(ready).unwrap();
                let line = [b'x'; 1023];
                for _ in 0..4096 {
                    out.write_all(&line).unwrap();
                    out.write_all(b"\n").unwrap();
                }
                out.write_all(b"@join 0 8 0 0 Flood\n").unwrap();
            }
            "long" => {
                out.write_all(ready).unwrap();
                out.write_all(&vec![b'y'; 200 * 1024]).unwrap();
                out.write_all(b"\n@join 0 9 0 0 Long\n").unwrap();
            }
            "silent" => {}
            _ => out.write_all(ready).unwrap(),
        }
        out.flush().unwrap();
    }
    for l in std::io::stdin().lock().lines() {
        let Ok(l) = l else { break };
        writeln!(rec, "line {l}").unwrap();
        if l == "quit" {
            break;
        }
    }
    writeln!(rec, "end").unwrap();
}
