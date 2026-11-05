//! The link between `dc-gunkit mcp` and the running studio: one JSON object per line on a loopback socket. The studio writes its
//! port and a random token to a file only this user reads, so that an agent's MCP server can find the window that is already open
//! and nothing else on the machine can drive it.
// DC-DEFAULT: VAR-6 (proposed) loopback only, a token per run, one studio at a time

use crate::commands::{base64_decode, base64_encode, Reply};
use serde_json::{json, Value};
use std::io::{BufRead, BufReader, Write};
use std::net::{TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::sync::mpsc::{channel, Receiver, Sender};
use std::time::Duration;

/// The studio's folder for its own small files.
pub fn home_dir() -> Option<PathBuf> {
    let var = |k: &str| {
        std::env::var_os(k)
            .filter(|d| !d.is_empty())
            .map(PathBuf::from)
    };
    if let Some(d) = var("DC_GUNKIT_HOME") {
        return Some(d);
    }
    if cfg!(windows) {
        return var("APPDATA").map(|d| d.join("DeCombat").join("gunkit"));
    }
    if cfg!(target_os = "macos") {
        return var("HOME").map(|d| d.join("Library/Application Support/DeCombat/gunkit"));
    }
    var("XDG_CONFIG_HOME")
        .or_else(|| var("HOME").map(|d| d.join(".config")))
        .map(|d| d.join("decombat").join("gunkit"))
}

fn live_file(home: &Path) -> PathBuf {
    home.join("live.json")
}

/// A command from an agent, waiting for its answer.
pub struct Request {
    pub tool: String,
    pub args: Value,
    pub reply: Sender<Result<Reply, String>>,
}

/// Where the studio listens: dropping it removes the file that says so.
pub struct Listener {
    file: PathBuf,
    token: String,
    pub requests: Receiver<Request>,
}

impl Drop for Listener {
    fn drop(&mut self) {
        // only our own: another studio may have taken over
        let ours = std::fs::read_to_string(&self.file)
            .ok()
            .and_then(|t| serde_json::from_str::<Value>(&t).ok())
            .is_some_and(|v| v["token"] == self.token.as_str());
        if ours {
            let _ = std::fs::remove_file(&self.file);
        }
    }
}

fn random_token() -> Result<String, String> {
    use std::collections::hash_map::RandomState;
    use std::hash::{BuildHasher, Hasher};
    // the OS's random seeds, which std draws for every hasher; enough to keep other local programs out of a loopback port
    let mut out = String::new();
    for _ in 0..4 {
        let mut h = RandomState::new().build_hasher();
        h.write_u32(std::process::id());
        out.push_str(&format!("{:016x}", h.finish()));
    }
    Ok(out)
}

fn write_private(path: &Path, text: &str) -> Result<(), String> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        let mut f = std::fs::OpenOptions::new()
            .write(true)
            .create(true)
            .truncate(true)
            .mode(0o600)
            .open(path)
            .map_err(|e| e.to_string())?;
        f.write_all(text.as_bytes()).map_err(|e| e.to_string())
    }
    #[cfg(not(unix))]
    {
        std::fs::write(path, text).map_err(|e| e.to_string())
    }
}

fn answer(stream: &mut TcpStream, result: Result<Reply, String>) -> std::io::Result<()> {
    let doc = match result {
        Ok(r) => json!({
            "ok": true,
            "result": r.json,
            "png": r.png.map(|p| base64_encode(&p)),
        }),
        Err(e) => json!({"ok": false, "error": e}),
    };
    writeln!(stream, "{doc}")?;
    stream.flush()
}

fn serve(stream: TcpStream, token: String, tx: Sender<Request>, wake: impl Fn()) {
    let _ = stream.set_nodelay(true);
    let Ok(mut out) = stream.try_clone() else {
        return;
    };
    let mut lines = BufReader::new(stream).lines();
    while let Some(Ok(line)) = lines.next() {
        let Ok(doc) = serde_json::from_str::<Value>(&line) else {
            let _ = answer(&mut out, Err("not JSON".into()));
            continue;
        };
        if doc["token"] != token.as_str() {
            let _ = answer(&mut out, Err("wrong token".into()));
            return;
        }
        let (reply, rx) = channel();
        let request = Request {
            tool: doc["tool"].as_str().unwrap_or_default().to_string(),
            args: doc.get("args").cloned().unwrap_or(json!({})),
            reply,
        };
        if tx.send(request).is_err() {
            return;
        }
        wake();
        let result = rx
            .recv()
            .unwrap_or_else(|_| Err("the studio closed".to_string()));
        if answer(&mut out, result).is_err() {
            return;
        }
    }
}

/// Starts listening. `wake` is called when a command arrives, so a window that is idle draws again.
pub fn listen(home: &Path, wake: impl Fn() + Send + Clone + 'static) -> Result<Listener, String> {
    std::fs::create_dir_all(home).map_err(|e| format!("{}: {e}", home.display()))?;
    let socket = TcpListener::bind(("127.0.0.1", 0)).map_err(|e| e.to_string())?;
    let port = socket.local_addr().map_err(|e| e.to_string())?.port();
    let token = random_token()?;
    let file = live_file(home);
    let doc = json!({"port": port, "token": token, "pid": std::process::id()});
    write_private(&file, &doc.to_string())?;
    let (tx, rx) = channel();
    let accept_token = token.clone();
    std::thread::spawn(move || {
        for stream in socket.incoming().flatten() {
            let (token, tx, wake) = (accept_token.clone(), tx.clone(), wake.clone());
            std::thread::spawn(move || serve(stream, token, tx, wake));
        }
    });
    Ok(Listener {
        file,
        token,
        requests: rx,
    })
}

/// A connection to a running studio.
pub struct Client {
    stream: TcpStream,
    lines: std::io::Lines<BufReader<TcpStream>>,
    token: String,
}

impl Client {
    /// Connects to the studio that is running, when one is.
    pub fn connect(home: &Path) -> Option<Client> {
        let text = std::fs::read_to_string(live_file(home)).ok()?;
        let doc: Value = serde_json::from_str(&text).ok()?;
        let port = u16::try_from(doc["port"].as_u64()?).ok()?;
        let token = doc["token"].as_str()?.to_string();
        let stream = TcpStream::connect_timeout(
            &std::net::SocketAddr::from(([127, 0, 0, 1], port)),
            Duration::from_secs(2),
        )
        .ok()?;
        let _ = stream.set_nodelay(true);
        let lines = BufReader::new(stream.try_clone().ok()?).lines();
        Some(Client {
            stream,
            lines,
            token,
        })
    }

    /// Runs one command in the studio.
    pub fn call(&mut self, tool: &str, args: &Value) -> Result<Result<Reply, String>, String> {
        let doc = json!({"token": self.token, "tool": tool, "args": args});
        writeln!(self.stream, "{doc}").map_err(|e| e.to_string())?;
        self.stream.flush().map_err(|e| e.to_string())?;
        let line = self
            .lines
            .next()
            .ok_or("the studio closed")?
            .map_err(|e| e.to_string())?;
        let doc: Value = serde_json::from_str(&line).map_err(|e| e.to_string())?;
        if doc["ok"] != true {
            return Ok(Err(doc["error"].as_str().unwrap_or("failed").to_string()));
        }
        let png = match doc["png"].as_str() {
            Some(b) => Some(base64_decode(b)?),
            None => None,
        };
        Ok(Ok(Reply {
            json: doc["result"].clone(),
            png,
        }))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("dc-gunkit-live-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        d
    }

    /// Answers every request with its own tool name, until the listener is dropped.
    fn echo(l: Listener) -> std::thread::JoinHandle<()> {
        std::thread::spawn(move || {
            while let Ok(r) = l.requests.recv_timeout(Duration::from_secs(5)) {
                let reply = match r.tool.as_str() {
                    "fail" => Err("it failed".to_string()),
                    "picture" => Ok(Reply {
                        json: json!({}),
                        png: Some(vec![1, 2, 3, 250]),
                    }),
                    "stop" => {
                        let _ = r.reply.send(Ok(json!({"bye": true}).into()));
                        return;
                    }
                    t => Ok(json!({"tool": t, "args": r.args}).into()),
                };
                let _ = r.reply.send(reply);
            }
        })
    }

    #[test]
    fn a_client_finds_the_studio_and_gets_answers_and_pictures() {
        let home = temp("call");
        let l = listen(&home, || {}).unwrap();
        let file = l.file.clone();
        let t = echo(l);
        let mut c = Client::connect(&home).expect("a studio is running");
        let r = c.call("status", &json!({"a": 1})).unwrap().unwrap();
        assert_eq!(r.json, json!({"tool": "status", "args": {"a": 1}}));
        assert_eq!(
            c.call("fail", &json!({})).unwrap(),
            Err("it failed".to_string())
        );
        let p = c.call("picture", &json!({})).unwrap().unwrap();
        assert_eq!(p.png, Some(vec![1, 2, 3, 250]));
        c.call("stop", &json!({})).unwrap().unwrap();
        t.join().unwrap();
        assert!(!file.exists(), "a closed studio leaves no file");
        assert!(Client::connect(&home).is_none());
        let _ = std::fs::remove_dir_all(&home);
    }

    #[test]
    fn a_wrong_token_is_turned_away() {
        let home = temp("token");
        let l = listen(&home, || {}).unwrap();
        let t = echo(l);
        let mut c = Client::connect(&home).unwrap();
        let good = c.token.clone();
        c.token = "wrong".into();
        assert_eq!(
            c.call("status", &json!({})).unwrap(),
            Err("wrong token".to_string())
        );
        let mut c = Client::connect(&home).unwrap();
        assert_eq!(c.token, good);
        c.call("stop", &json!({})).unwrap().unwrap();
        t.join().unwrap();
        let _ = std::fs::remove_dir_all(&home);
    }

    #[cfg(unix)]
    #[test]
    fn only_this_user_can_read_the_token() {
        use std::os::unix::fs::PermissionsExt;
        let home = temp("mode");
        let l = listen(&home, || {}).unwrap();
        let mode = std::fs::metadata(&l.file).unwrap().permissions().mode();
        assert_eq!(mode & 0o777, 0o600);
        drop(l);
        let _ = std::fs::remove_dir_all(&home);
    }
}
