//! `dc-gunkit mcp`: an MCP server over stdio (JSON-RPC 2.0, one object per line, the same minimum as `dc-mapkit mcp`, MAP-58) whose
//! tools drive the studio window that is on the user's screen. It opens the window when none is running, so the person sees every
//! change the agent makes. With `--headless` the studio runs inside this process, for machines with no display.
// DC-DEFAULT: VAR-6 (proposed) the MCP server is a thin bridge: every tool is one studio command (`commands::TOOLS`)

use crate::commands::{base64_encode, Reply, TOOLS};
use serde_json::{json, Value};
use std::io::{BufRead, Write};

pub const PROTOCOL_VERSION: &str = "2025-06-18";
pub const SUPPORTED_VERSIONS: &[&str] = &["2025-06-18", "2024-11-05"];
pub const SERVER_NAME: &str = "dc-gunkit";

/// What an agent is told when it connects.
pub const INSTRUCTIONS: &str = "\
DeCombat camo studio. The tools change the gun on the user's screen as you call them.\n\
\n\
The usual loop: select_gun, then set_pattern with a path to a PNG you wrote. The studio watches that file, so after that just save over it and the gun updates by itself. Call screenshot to look at the result, and set_camera to see other sides. Check first_person and third_person both, with set_view, before calling a camo done.\n\
\n\
A pattern repeats twice across the gun's texture and the gun's own shading shows through it, so it should tile, and mid colours work best. For an animated camo call set_effect; it moves a pattern over the gun as light, which must stay subtle enough never to give a player away. A scope's lens stays clean by itself on a gun the game gives a sight; call describe_texture to see what was found and what is where on the texture, and look at its picture. set_keep changes the list. Each gun and view has its own circles.\n\
\n\
Never put the game's own textures into a pack or a commit, or anything made from them: get_texture base is for reference only. Patterns must be your own art. save_pack writes the finished pack, checks it and refuses copies of the game's art.";

/// Whatever answers the tools.
pub trait Backend {
    /// Runs a tool. The outer error means the studio could not be reached.
    fn call(&mut self, tool: &str, args: &Value) -> Result<Reply, String>;
}

fn tool_list() -> Value {
    let tools: Vec<Value> = TOOLS
        .iter()
        .map(|t| json!({"name": t.name, "description": t.about, "inputSchema": (t.schema)()}))
        .collect();
    json!({"tools": tools})
}

fn tool_result(r: Result<Reply, String>) -> Value {
    match r {
        Ok(reply) => {
            let mut content = vec![json!({"type": "text", "text": reply.json.to_string()})];
            if let Some(png) = reply.png {
                content.push(
                    json!({"type": "image", "data": base64_encode(&png), "mimeType": "image/png"}),
                );
            }
            json!({"content": content, "isError": false})
        }
        Err(e) => json!({"content": [{"type": "text", "text": e}], "isError": true}),
    }
}

/// The answer to one JSON-RPC line (`None`: a notification, which is never answered).
pub fn handle_line(line: &str, backend: &mut dyn Backend) -> Option<Value> {
    let err = |id: Value, code: i32, message: &str| {
        Some(json!({"jsonrpc": "2.0", "id": id, "error": {"code": code, "message": message}}))
    };
    let Ok(doc) = serde_json::from_str::<Value>(line) else {
        return err(Value::Null, -32700, "parse error");
    };
    let Some(obj) = doc.as_object() else {
        return err(Value::Null, -32600, "one request object per line");
    };
    let id = obj.get("id").cloned();
    let Some(method) = obj.get("method").and_then(Value::as_str) else {
        return err(id.unwrap_or(Value::Null), -32600, "no method");
    };
    // a notification has no id and is never answered
    let id = id?;
    let ok = |result: Value| Some(json!({"jsonrpc": "2.0", "id": id, "result": result}));
    let params = obj.get("params").cloned().unwrap_or(json!({}));
    match method {
        "initialize" => {
            let asked = params["protocolVersion"].as_str().unwrap_or("");
            let version = if SUPPORTED_VERSIONS.contains(&asked) {
                asked
            } else {
                PROTOCOL_VERSION
            };
            ok(json!({
                "protocolVersion": version,
                "capabilities": {"tools": {"listChanged": false}},
                "serverInfo": {"name": SERVER_NAME, "version": env!("CARGO_PKG_VERSION")},
                "instructions": INSTRUCTIONS,
            }))
        }
        "ping" => ok(json!({})),
        "tools/list" => ok(tool_list()),
        "tools/call" => {
            let Some(name) = params["name"].as_str() else {
                return err(id, -32602, "tools/call needs a name");
            };
            if !TOOLS.iter().any(|t| t.name == name) {
                return err(id, -32602, "no such tool");
            }
            let args = match params.get("arguments") {
                None | Some(Value::Null) => json!({}),
                Some(v) if v.is_object() => v.clone(),
                Some(_) => return err(id, -32602, "arguments must be an object"),
            };
            ok(tool_result(backend.call(name, &args)))
        }
        _ => err(id, -32601, "no such method"),
    }
}

/// Serves until stdin closes. Nothing but responses reaches stdout.
pub fn serve(backend: &mut dyn Backend) -> std::io::Result<()> {
    let stdin = std::io::stdin();
    let mut stdout = std::io::stdout();
    let mut raw = Vec::new();
    let mut input = stdin.lock();
    loop {
        raw.clear();
        if input.read_until(b'\n', &mut raw)? == 0 {
            return Ok(());
        }
        let line = String::from_utf8_lossy(&raw);
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        if let Some(answer) = handle_line(line, backend) {
            writeln!(stdout, "{answer}")?;
            stdout.flush()?;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Fake(Vec<String>);

    impl Backend for Fake {
        fn call(&mut self, tool: &str, args: &Value) -> Result<Reply, String> {
            self.0.push(tool.to_string());
            match tool {
                "screenshot" => Ok(Reply {
                    json: json!({"width": 2}),
                    png: Some(vec![1, 2, 3]),
                }),
                "select_gun" if args["gun"] == "nope" => Err("no gun matches".into()),
                _ => Ok(json!({"seen": args}).into()),
            }
        }
    }

    fn rpc(id: u32, method: &str, params: Value) -> String {
        json!({"jsonrpc": "2.0", "id": id, "method": method, "params": params}).to_string()
    }

    #[test]
    fn a_scripted_client_runs_the_handshake_and_the_tools() {
        let mut b = Fake(Vec::new());
        let init = handle_line(
            &rpc(1, "initialize", json!({"protocolVersion": "2024-11-05"})),
            &mut b,
        )
        .unwrap();
        assert_eq!(init["result"]["protocolVersion"], "2024-11-05");
        assert_eq!(init["result"]["serverInfo"]["name"], "dc-gunkit");
        assert!(init["result"]["instructions"]
            .as_str()
            .unwrap()
            .contains("set_pattern"));
        let other = handle_line(
            &rpc(2, "initialize", json!({"protocolVersion": "1999"})),
            &mut b,
        )
        .unwrap();
        assert_eq!(other["result"]["protocolVersion"], PROTOCOL_VERSION);
        assert!(handle_line(
            r#"{"jsonrpc":"2.0","method":"notifications/initialized"}"#,
            &mut b
        )
        .is_none());
        assert_eq!(
            handle_line(&rpc(3, "ping", json!({})), &mut b).unwrap()["result"],
            json!({})
        );

        let list = handle_line(&rpc(4, "tools/list", json!({})), &mut b).unwrap();
        let tools = list["result"]["tools"].as_array().unwrap();
        assert_eq!(tools.len(), TOOLS.len());
        for t in tools {
            assert_eq!(t["inputSchema"]["type"], "object");
            let call = rpc(
                5,
                "tools/call",
                json!({"name": t["name"], "arguments": {"gun": 25}}),
            );
            let r = handle_line(&call, &mut b).unwrap();
            assert_eq!(r["result"]["isError"], false, "{}", t["name"]);
        }
        assert_eq!(b.0.len(), TOOLS.len());
    }

    #[test]
    fn a_picture_comes_back_as_image_content() {
        let mut b = Fake(Vec::new());
        let r = handle_line(&rpc(1, "tools/call", json!({"name": "screenshot"})), &mut b).unwrap();
        let c = r["result"]["content"].as_array().unwrap();
        assert_eq!(c[0]["type"], "text");
        assert_eq!(c[1]["type"], "image");
        assert_eq!(c[1]["mimeType"], "image/png");
        assert_eq!(c[1]["data"], base64_encode(&[1, 2, 3]));
    }

    #[test]
    fn failures_are_told_the_way_mcp_wants() {
        let mut b = Fake(Vec::new());
        let r = handle_line(
            &rpc(
                1,
                "tools/call",
                json!({"name": "select_gun", "arguments": {"gun": "nope"}}),
            ),
            &mut b,
        )
        .unwrap();
        assert_eq!(r["result"]["isError"], true);
        assert_eq!(r["result"]["content"][0]["text"], "no gun matches");
        let code =
            |line: &str, b: &mut Fake| handle_line(line, b).unwrap()["error"]["code"].clone();
        assert_eq!(code("not json", &mut b), -32700);
        assert_eq!(code("[1,2]", &mut b), -32600);
        assert_eq!(code(&rpc(2, "nope", json!({})), &mut b), -32601);
        assert_eq!(
            code(&rpc(3, "tools/call", json!({"name": "nope"})), &mut b),
            -32602
        );
        assert_eq!(
            code(
                &rpc(4, "tools/call", json!({"name": "status", "arguments": 3})),
                &mut b
            ),
            -32602
        );
    }
}
