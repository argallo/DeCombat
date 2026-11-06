//! Tests of the client update (CLI-1863..CLI-1868): addresses, the lobby's download map, SHA256SUMS, the GitHub
//! release pick, the HTTP reader and its redirects, the staging, the swap's copy rules and the friend pack's local
//! source. Every file test works in its own folder under the temp folder.

use super::*;
use std::io::Cursor;

fn temp(name: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("dc_selfupdate_{name}_{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    d
}

/// A zip of deflated entries (a folder entry when the name ends in `/`).
// Public format: PKWARE APPNOTE.TXT (local header 0x04034b50, central header 0x02014b50, end 0x06054b50)
fn zip_of(entries: &[(&str, &[u8])]) -> Vec<u8> {
    use std::io::Write as _;
    let mut out = Vec::new();
    let mut central = Vec::new();
    for (name, data) in entries {
        let mut enc =
            flate2::write::DeflateEncoder::new(Vec::new(), flate2::Compression::default());
        enc.write_all(data).unwrap();
        let packed = enc.finish().unwrap();
        let crc = crate::mods::crc32(data);
        let at = out.len() as u32;
        let head = |sig: u32, central: bool| {
            let mut h = Vec::new();
            h.extend_from_slice(&sig.to_le_bytes());
            if central {
                h.extend_from_slice(&20u16.to_le_bytes());
            }
            h.extend_from_slice(&20u16.to_le_bytes());
            h.extend_from_slice(&0u16.to_le_bytes());
            h.extend_from_slice(&8u16.to_le_bytes());
            h.extend_from_slice(&[0; 4]);
            h.extend_from_slice(&crc.to_le_bytes());
            h.extend_from_slice(&(packed.len() as u32).to_le_bytes());
            h.extend_from_slice(&(data.len() as u32).to_le_bytes());
            h.extend_from_slice(&(name.len() as u16).to_le_bytes());
            h.extend_from_slice(&0u16.to_le_bytes());
            if central {
                h.extend_from_slice(&[0; 6]);
                h.extend_from_slice(&0u32.to_le_bytes());
                h.extend_from_slice(&at.to_le_bytes());
            }
            h.extend_from_slice(name.as_bytes());
            h
        };
        out.extend(head(0x0403_4b50, false));
        out.extend_from_slice(&packed);
        central.extend(head(0x0201_4b50, true));
    }
    let cd_at = out.len() as u32;
    out.extend_from_slice(&central);
    out.extend_from_slice(&0x0605_4b50u32.to_le_bytes());
    out.extend_from_slice(&[0; 4]);
    out.extend_from_slice(&(entries.len() as u16).to_le_bytes());
    out.extend_from_slice(&(entries.len() as u16).to_le_bytes());
    out.extend_from_slice(&(central.len() as u32).to_le_bytes());
    out.extend_from_slice(&cd_at.to_le_bytes());
    out.extend_from_slice(&0u16.to_le_bytes());
    out
}

#[test]
fn only_plain_web_addresses_reach_the_browser() {
    assert!(safe_url(DEFAULT_PAGE));
    assert!(safe_url("https://example.org/a?b=1&c=%20"));
    for bad in [
        "",
        "example.org",
        "file:///C:/x",
        "javascript:alert(1)",
        "https://",
        "https:///x",
        "https://a b",
        "https://a\"b",
        "https://a^b",
        "https://a\r\nb",
    ] {
        assert!(!safe_url(bad), "{bad:?}");
    }
    assert_eq!(page_url("decombat.com"), "https://decombat.com");
    assert_eq!(
        page_url(" https://example.org/get "),
        "https://example.org/get"
    );
    assert_eq!(page_url(""), DEFAULT_PAGE);
    assert_eq!(page_url("ftp://x"), DEFAULT_PAGE);
    assert_eq!(page_url("a b"), DEFAULT_PAGE);
    assert_eq!(
        download_url("https://x.org/a.zip").as_deref(),
        Some("https://x.org/a.zip")
    );
    assert_eq!(download_url("http://x.org/a.zip"), None);
    assert!(open_url("file:///C:/Windows/notepad.exe").is_err());
}

#[test]
fn the_download_map_takes_https_links_per_platform() {
    let v: serde_json::Value = serde_json::from_str(
        r#"{"download":{"windows":"https://github.com/argallo/DeCombat/releases/download/v1.0.0/DeCombat-win-v1.0.0.zip","macos":"http://a.org/m.zip","linux":"https://x"}}"#,
    )
    .unwrap();
    let d = Downloads::from_doc(&v);
    assert_eq!(
        d.windows.as_deref(),
        Some(
            "https://github.com/argallo/DeCombat/releases/download/v1.0.0/DeCombat-win-v1.0.0.zip"
        )
    );
    assert_eq!(d.macos, None);
    // CLI-1864: a link outside the official release download is dropped (the GitHub release is looked up instead)
    let other: serde_json::Value = serde_json::from_str(
        r#"{"download":{"windows":"https://a.org/DeCombat-win-v1.0.0.zip","macos":"https://github.com/someone/DeCombat/releases/download/v1.0.0/DeCombat-mac-v1.0.0.zip"}}"#,
    )
    .unwrap();
    assert_eq!(Downloads::from_doc(&other), Downloads::default());
    assert_eq!(
        Downloads::from_doc(&serde_json::json!({})),
        Downloads::default()
    );
    assert_eq!(
        Downloads::from_doc(&serde_json::json!({"download": "x"})),
        Downloads::default()
    );
}

#[test]
fn sha256sums_are_looked_up_by_file_name() {
    let zip = b"the zip";
    let good = sha256_hex(zip);
    let sums = format!(
        "{}  DeCombat-win-v1.0.0-bots.zip\n{}  DeCombat-win-v1.0.0.zip\n{} *DeCombat-mac-v1.0.0.zip\n",
        "11".repeat(32),
        good.to_uppercase(),
        "22".repeat(32)
    );
    assert_eq!(
        sum_for(&sums, "DeCombat-win-v1.0.0.zip"),
        Some(good.clone())
    );
    assert_eq!(
        sum_for(&sums, "DeCombat-mac-v1.0.0.zip"),
        Some("22".repeat(32))
    );
    assert_eq!(sum_for(&sums, "DeCombat-win-v1.0.0"), None);
    assert_eq!(sum_for("zz  x.zip", "x.zip"), None);
    assert_eq!(
        check_sum(Some(&sums), "DeCombat-win-v1.0.0.zip", zip),
        Ok(())
    );
    assert!(check_sum(Some(&sums), "DeCombat-win-v1.0.0.zip", b"another zip").is_err());
    assert!(check_sum(Some(&sums), "DeCombat-win-v9.zip", zip).is_err());
    // CLI-1864: no sums, or sums that list nothing readable, refuse the install
    for sums in [None, Some(""), Some("not a sums file")] {
        let e = check_sum(sums, "DeCombat-win-v1.0.0.zip", zip).unwrap_err();
        assert!(e.contains("could not be verified"), "{e}");
    }
}

// CLI-1864: only `<OFFICIAL_DOWNLOAD><tag>/<file>` is a download the client takes
#[test]
fn only_the_official_release_download_is_taken() {
    let ok = "https://github.com/argallo/DeCombat/releases/download/v0.2.0-beta3/DeCombat-win-v0.2.0-beta3.zip";
    assert_eq!(
        official_release(ok),
        Some(("v0.2.0-beta3", "DeCombat-win-v0.2.0-beta3.zip"))
    );
    assert_eq!(
        sums_url_of(ok).as_deref(),
        Some("https://github.com/argallo/DeCombat/releases/download/v0.2.0-beta3/SHA256SUMS")
    );
    assert_eq!(official_link(ok), Some(ok));
    for bad in [
        "https://example.org/argallo/DeCombat/releases/download/v1/DeCombat-win-v1.zip",
        "https://github.com.evil.org/argallo/DeCombat/releases/download/v1/DeCombat-win-v1.zip",
        "https://evil.org/https://github.com/argallo/DeCombat/releases/download/v1/x.zip",
        "http://github.com/argallo/DeCombat/releases/download/v1/DeCombat-win-v1.zip",
        "https://github.com/someone/DeCombat/releases/download/v1/DeCombat-win-v1.zip",
        "https://github.com/argallo/Other/releases/download/v1/DeCombat-win-v1.zip",
        "https://github.com/argallo/decombat/releases/download/v1/DeCombat-win-v1.zip",
        "https://github.com/argallo/DeCombat/releases/download/../../../someone/x/releases/download/v1/a.zip",
        "https://github.com/argallo/DeCombat/releases/download/../x.zip",
        "https://github.com/argallo/DeCombat/releases/download/%2e%2e/x.zip",
        "https://github.com/argallo/DeCombat/releases/download/v1/../x.zip",
        "https://github.com/argallo/DeCombat/releases/download/v1/a/x.zip",
        "https://github.com/argallo/DeCombat/releases/download/v1/x.zip?redirect=https://evil.org",
        "https://github.com/argallo/DeCombat/releases/download/v1/",
        "https://github.com/argallo/DeCombat/releases/download//x.zip",
        "https://github.com/argallo/DeCombat/releases/download/v1",
    ] {
        assert_eq!(official_release(bad), None, "{bad}");
        assert_eq!(official_link(bad), None, "{bad}");
        assert_eq!(sums_url_of(bad), None, "{bad}");
    }
}

/// A canned GitHub for [`fetch_release_with`]: the release API names `tag`'s zip, the download redirects to the
/// asset storage, the storage answers `zip` and (when `sums` is set) the SHA256SUMS. Records every host and path.
fn canned_github(
    tag: &'static str,
    zip: &'static [u8],
    sums: Option<String>,
    hosts: std::sync::Arc<Mutex<Vec<String>>>,
) -> impl FnMut(&HttpsUrl) -> Result<Box<dyn Conn>, String> {
    move |u: &HttpsUrl| {
        lock(&hosts).push(format!("{}{}", u.host, u.path));
        let ok = |body: &[u8]| {
            let mut a =
                format!("HTTP/1.1 200 OK\r\nContent-Length: {}\r\n\r\n", body.len()).into_bytes();
            a.extend_from_slice(body);
            a
        };
        let dl = format!("/argallo/DeCombat/releases/download/{tag}/");
        let answer = match (u.host.as_str(), u.path.as_str()) {
            ("api.github.com", p) if p.ends_with("/latest") => {
                ok(release_json(tag, false).to_string().as_bytes())
            }
            ("github.com", p) if p.starts_with(&dl) => format!(
                "HTTP/1.1 302 Found\r\nLocation: https://release-assets.githubusercontent.com/asset/{}?sig=1\r\n\r\n",
                &p[dl.len()..]
            )
            .into_bytes(),
            ("release-assets.githubusercontent.com", p) if p.starts_with("/asset/SHA256SUMS") => {
                match &sums {
                    Some(s) => ok(s.as_bytes()),
                    None => b"HTTP/1.1 404 Not Found\r\nContent-Length: 0\r\n\r\n".to_vec(),
                }
            }
            ("release-assets.githubusercontent.com", _) => ok(zip),
            _ => b"HTTP/1.1 302 Found\r\nLocation: https://evil.example.org/x.zip\r\n\r\n".to_vec(),
        };
        Ok(Box::new(Canned {
            answer: Cursor::new(answer),
            asked: std::sync::Arc::new(Mutex::new(Vec::new())),
        }) as Box<dyn Conn>)
    }
}

// CLI-1864: a lobby link outside the official download is ignored and the GitHub release is taken; its sums decide
#[test]
fn a_foreign_lobby_link_falls_back_to_the_github_release() {
    let zip: &'static [u8] = b"the official zip";
    let name = "DeCombat-win-v0.4.0.zip";
    let sums = format!("{}  {name}\n", sha256_hex(zip));
    let hosts = std::sync::Arc::new(Mutex::new(Vec::new()));
    let mut gh = canned_github("v0.4.0", zip, Some(sums.clone()), hosts.clone());
    let (got, bytes) = fetch_release_with(
        [0, 3, 0],
        Some("https://evil.example.org/DeCombat-win-v0.4.0.zip"),
        &mut gh,
        &mut |_| {},
        &mut |_, _| {},
    )
    .unwrap();
    assert_eq!((got.as_str(), bytes.as_slice()), (name, zip));
    let asked = lock(&hosts).clone();
    assert!(asked[0].starts_with("api.github.com/"), "{asked:?}");
    assert!(!asked.iter().any(|h| h.contains("evil")), "{asked:?}");
    assert!(
        asked
            .iter()
            .any(|h| h.ends_with("/download/v0.4.0/SHA256SUMS")),
        "{asked:?}"
    );
    // the official direct link is taken without the API
    let hosts = std::sync::Arc::new(Mutex::new(Vec::new()));
    let mut gh = canned_github("v0.4.0", zip, Some(sums), hosts.clone());
    let direct = format!("{OFFICIAL_DOWNLOAD}v0.4.0/{name}");
    let (got, _) = fetch_release_with(
        [0, 3, 0],
        Some(&direct),
        &mut gh,
        &mut |_| {},
        &mut |_, _| {},
    )
    .unwrap();
    assert_eq!(got, name);
    let asked = lock(&hosts).clone();
    assert!(asked.iter().all(|h| !h.starts_with("api.")), "{asked:?}");
}

// CLI-1864: no SHA256SUMS, or one that does not list the zip, refuses the install
#[test]
fn an_update_without_its_sums_is_refused() {
    let zip: &'static [u8] = b"the official zip";
    let direct = format!("{OFFICIAL_DOWNLOAD}v0.4.0/DeCombat-win-v0.4.0.zip");
    for sums in [
        None,
        Some("garbage".to_string()),
        Some(format!("{}  other.zip\n", sha256_hex(zip))),
    ] {
        let hosts = std::sync::Arc::new(Mutex::new(Vec::new()));
        let mut gh = canned_github("v0.4.0", zip, sums, hosts);
        let e = fetch_release_with(
            [0, 3, 0],
            Some(&direct),
            &mut gh,
            &mut |_| {},
            &mut |_, _| {},
        )
        .unwrap_err();
        assert!(e.contains("could not be verified"), "{e}");
    }
}

// CLI-1864: a redirect away from GitHub's hosts ends the download before that host is reached
#[test]
fn an_update_redirected_off_github_is_refused() {
    let hosts = std::sync::Arc::new(Mutex::new(Vec::new()));
    // the canned GitHub sends a path it does not know (another tag) to evil.example.org
    let mut gh = canned_github("v0.4.0", b"zip", None, hosts.clone());
    let direct = format!("{OFFICIAL_DOWNLOAD}v0.9.9/DeCombat-win-v0.9.9.zip");
    let e = fetch_release_with(
        [0, 3, 0],
        Some(&direct),
        &mut gh,
        &mut |_| {},
        &mut |_, _| {},
    )
    .unwrap_err();
    assert!(
        e.contains("evil.example.org") && e.contains("not GitHub"),
        "{e}"
    );
    let asked = lock(&hosts).clone();
    assert!(!asked.iter().any(|h| h.contains("evil")), "{asked:?}");
}

fn release_json(tag: &str, draft: bool) -> serde_json::Value {
    let asset = |n: &str| serde_json::json!({"name": n, "browser_download_url": format!("https://github.com/argallo/DeCombat/releases/download/{tag}/{n}")});
    serde_json::json!({
        "tag_name": tag,
        "draft": draft,
        "assets": [
            asset(&format!("DeCombat-win-{tag}-bots.zip")),
            asset(&format!("DeCombat-MapKit-win-{tag}.zip")),
            asset(&format!("DeCombat-win-{tag}.zip")),
            asset(&format!("DeCombat-mac-{tag}.zip")),
            asset("SHA256SUMS"),
        ]
    })
}

#[test]
fn the_release_pick_takes_the_game_zip_and_its_sums() {
    let r = pick_release(&release_json("v0.4.0", false), Platform::Windows).unwrap();
    assert_eq!(r.tag, "v0.4.0");
    assert_eq!(r.version, Some([0, 4, 0]));
    assert_eq!(r.zip.name, "DeCombat-win-v0.4.0.zip");
    assert!(r.zip.url.ends_with("/v0.4.0/DeCombat-win-v0.4.0.zip"));
    assert_eq!(r.sums.unwrap().name, "SHA256SUMS");
    let m = pick_release(&release_json("v0.4.0", false), Platform::MacOs).unwrap();
    assert_eq!(m.zip.name, "DeCombat-mac-v0.4.0.zip");
    assert_eq!(
        pick_release(&release_json("v0.4.0", false), Platform::Other),
        None
    );
    // the list: a draft is passed over, a pre-release counts as its release
    let list = serde_json::json!([
        release_json("v0.5.0", true),
        release_json("v0.4.1-beta2", false),
        release_json("v0.4.0", false)
    ]);
    let r = pick_release(&list, Platform::Windows).unwrap();
    assert_eq!(
        (r.tag.as_str(), r.version),
        ("v0.4.1-beta2", Some([0, 4, 1]))
    );
    assert_eq!(
        pick_release(
            &serde_json::json!({"message": "Not Found"}),
            Platform::Windows
        ),
        None
    );
    for n in ["DeCombat-win-v1.zip", "DeCombat-win-x86_64.zip"] {
        assert!(is_game_zip(n, Platform::Windows), "{n}");
    }
    for n in [
        "DeCombat-win-v1-bots.zip",
        "DeCombat-MapKit-win-v1.zip",
        "DeCombat-mac-v1.zip",
        "DeCombat-win-v1.ipa",
    ] {
        assert!(!is_game_zip(n, Platform::Windows), "{n}");
    }
}

#[test]
fn addresses_split_into_host_port_and_path() {
    assert_eq!(
        parse_https("https://API.github.com/repos/x?per_page=10").unwrap(),
        HttpsUrl {
            host: "api.github.com".into(),
            port: 443,
            path: "/repos/x?per_page=10".into()
        }
    );
    assert_eq!(
        parse_https("https://h:8443").unwrap(),
        HttpsUrl {
            host: "h".into(),
            port: 8443,
            path: "/".into()
        }
    );
    assert_eq!(parse_https("https://h?x").unwrap().path, "/?x");
    for bad in [
        "http://h/x",
        "https://user@h/x",
        "https:///x",
        "https://h/a b",
        "https://h:99999/",
    ] {
        assert!(parse_https(bad).is_err(), "{bad}");
    }
}

fn read(reply: &str, cap: u64) -> Result<Reply, String> {
    let mut c = Cursor::new(reply.as_bytes().to_vec());
    let mut seen = Vec::new();
    let r = read_http(
        &mut c,
        cap,
        Instant::now() + Duration::from_secs(5),
        &mut |d, t| seen.push((d, t)),
    );
    if let Ok(rep) = &r {
        if !rep.body.is_empty() {
            assert_eq!(seen.last().map(|x| x.0), Some(rep.body.len() as u64));
        }
    }
    r
}

#[test]
fn replies_are_read_by_length_by_chunks_or_to_the_close() {
    let r = read(
        "HTTP/1.1 200 OK\r\nContent-Length: 5\r\n\r\nhelloEXTRA",
        100,
    )
    .unwrap();
    assert_eq!((r.status, r.body.as_slice()), (200, &b"hello"[..]));
    let r = read("HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\n\r\n5;x=1\r\nhello\r\n6\r\n world\r\n0\r\nX-T: 1\r\n\r\n", 100).unwrap();
    assert_eq!(r.body, b"hello world");
    let r = read("HTTP/1.0 200 OK\r\nServer: x\r\n\r\nto the end", 100).unwrap();
    assert_eq!(r.body, b"to the end");
    let r = read("HTTP/1.1 302 Found\r\nLocation: https://cdn.example.org/a.zip\r\nContent-Length: 3\r\n\r\nabc", 100).unwrap();
    assert_eq!(
        (r.status, r.location.as_deref(), r.body.len()),
        (302, Some("https://cdn.example.org/a.zip"), 0)
    );
    assert!(read("HTTP/1.1 200 OK\r\nContent-Length: 500\r\n\r\nx", 100).is_err());
    assert!(read("HTTP/1.1 200 OK\r\nContent-Length: 50\r\n\r\nshort", 100).is_err());
    assert!(read(
        "HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\n\r\nzz\r\n",
        100
    )
    .is_err());
    assert!(read(
        "HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\n\r\n80\r\n",
        100
    )
    .is_err());
    assert!(read("garbage", 100).is_err());
}

/// A canned server per host: what it was asked, what it answers.
struct Canned {
    answer: Cursor<Vec<u8>>,
    asked: std::sync::Arc<Mutex<Vec<String>>>,
}

impl Read for Canned {
    fn read(&mut self, b: &mut [u8]) -> std::io::Result<usize> {
        self.answer.read(b)
    }
}

impl Write for Canned {
    fn write(&mut self, b: &[u8]) -> std::io::Result<usize> {
        lock(&self.asked).push(String::from_utf8_lossy(b).into_owned());
        Ok(b.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

#[test]
fn downloads_follow_https_redirects_only() {
    let asked = std::sync::Arc::new(Mutex::new(Vec::new()));
    let hosts = std::sync::Arc::new(Mutex::new(Vec::new()));
    let mut connect = |u: &HttpsUrl| -> Result<Box<dyn Conn>, String> {
        lock(&hosts).push(format!("{}:{}", u.host, u.port));
        let answer = match (u.host.as_str(), u.path.as_str()) {
            ("github.com", _) => {
                "HTTP/1.1 302 Found\r\nLocation: https://cdn.example.org/blob?sig=1\r\n\r\n"
                    .to_string()
            }
            ("cdn.example.org", "/blob?sig=1") => {
                "HTTP/1.1 301 Moved\r\nLocation: /final\r\n\r\n".to_string()
            }
            ("cdn.example.org", "/final") => {
                "HTTP/1.1 200 OK\r\nContent-Length: 3\r\n\r\nzip".to_string()
            }
            ("evil.example.org", _) => {
                "HTTP/1.1 302 Found\r\nLocation: http://plain.example.org/x\r\n\r\n".to_string()
            }
            _ => "HTTP/1.1 302 Found\r\nLocation: https://loop.example.org/\r\n\r\n".to_string(),
        };
        Ok(Box::new(Canned {
            answer: Cursor::new(answer.into_bytes()),
            asked: asked.clone(),
        }))
    };
    let r = get_with(
        "https://github.com/a/b.zip",
        100,
        "*/*",
        &mut |_, _| {},
        &mut connect,
    )
    .unwrap();
    assert_eq!((r.status, r.body.as_slice()), (200, &b"zip"[..]));
    assert_eq!(
        *lock(&hosts),
        vec![
            "github.com:443",
            "cdn.example.org:443",
            "cdn.example.org:443"
        ]
    );
    let first = lock(&asked)[0].clone();
    assert!(
        first.starts_with("GET /a/b.zip HTTP/1.1\r\nHost: github.com\r\n"),
        "{first}"
    );
    assert!(first.contains("User-Agent: "), "{first}");
    let e = get_with(
        "https://evil.example.org/x",
        100,
        "*/*",
        &mut |_, _| {},
        &mut connect,
    )
    .unwrap_err();
    assert!(e.contains("not https://"), "{e}");
    let e = get_with(
        "https://loop.example.org/",
        100,
        "*/*",
        &mut |_, _| {},
        &mut connect,
    )
    .unwrap_err();
    assert!(e.contains("redirected too often"), "{e}");
    assert!(get_with(
        "http://github.com/x",
        100,
        "*/*",
        &mut |_, _| {},
        &mut connect
    )
    .is_err());
}

#[test]
fn a_zip_is_staged_without_its_top_folder() {
    let install = temp("stage");
    let exe = format!("DeCombat-win-v0.4.0/{EXE}");
    let zip = zip_of(&[
        ("DeCombat-win-v0.4.0/", b""),
        (&exe, b"new exe"),
        ("DeCombat-win-v0.4.0/DeCombat.bat", b"@echo off"),
        ("DeCombat-win-v0.4.0/docs/README.md", b"read me"),
    ]);
    let staging = stage(&zip, &install).unwrap();
    assert_eq!(staging, install.join(UPDATE_DIR).join(STAGING_DIR));
    assert_eq!(std::fs::read(staging.join(EXE)).unwrap(), b"new exe");
    assert_eq!(
        std::fs::read(staging.join("docs").join("README.md")).unwrap(),
        b"read me"
    );
    assert!(!staging.join("DeCombat-win-v0.4.0").exists());
    // a flat zip stays as it is; a zip without the exe, a damaged one or one with a bad name is refused
    let flat = zip_of(&[(EXE, b"x"), ("README.md", b"y")]);
    assert!(stage(&flat, &install).unwrap().join("README.md").is_file());
    let e = stage(&zip_of(&[("p/README.md", b"y")]), &install).unwrap_err();
    assert!(e.contains("holds no"), "{e}");
    assert!(!staging.exists());
    let mut broken = zip_of(&[(EXE, b"x")]);
    let n = broken.len();
    // the central record's CRC (the record starts where the end record says, the CRC 16 bytes in)
    let cd = u32::from_le_bytes(broken[n - 6..n - 2].try_into().unwrap()) as usize;
    broken[cd + 16] ^= 0xff;
    assert!(stage(&broken, &install).is_err());
    assert!(stage(&zip_of(&[("../evil.exe", b"x"), (EXE, b"x")]), &install).is_err());
    let _ = std::fs::remove_dir_all(&install);
}

/// A game folder as a player has it: the exe, the edited launcher, a settings file, the 2009 client folder, a file
/// of their own.
fn fake_install(name: &str) -> PathBuf {
    let d = temp(name);
    std::fs::write(d.join(EXE), b"old exe").unwrap();
    std::fs::write(d.join("DeCombat.bat"), b"rem my own server").unwrap();
    std::fs::write(d.join("settings.toml"), b"volume = 3").unwrap();
    std::fs::write(d.join("README.md"), b"old readme").unwrap();
    std::fs::write(d.join("notes.txt"), b"mine").unwrap();
    std::fs::create_dir_all(d.join("Combat Arms").join("Game")).unwrap();
    std::fs::write(d.join("Combat Arms").join("Engine.exe"), b"2009").unwrap();
    d
}

fn staged(install: &Path, files: &[(&str, &[u8])]) -> PathBuf {
    let s = install.join(UPDATE_DIR).join(STAGING_DIR);
    for (n, b) in files {
        let p = n.split('/').fold(s.clone(), |p, c| p.join(c));
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        std::fs::write(p, b).unwrap();
    }
    s
}

#[test]
fn the_swap_replaces_the_game_and_keeps_the_players_files() {
    let install = fake_install("swap");
    let s = staged(
        &install,
        &[
            (EXE, b"new exe"),
            ("DeCombat.bat", b"rem the new preset"),
            ("settings.toml", b"volume = 9"),
            ("README.md", b"new readme"),
            ("pack.json", b"{}"),
            ("Combat Arms/Engine.exe", b"not ours"),
        ],
    );
    let a = apply_staged(&s, &install).unwrap();
    assert_eq!(std::fs::read(install.join(EXE)).unwrap(), b"new exe");
    assert_eq!(
        std::fs::read(install.join("README.md")).unwrap(),
        b"new readme"
    );
    assert_eq!(std::fs::read(install.join("pack.json")).unwrap(), b"{}");
    // the player's files and the 2009 client stay; a changed launcher waits beside its old self
    assert_eq!(
        std::fs::read(install.join("DeCombat.bat")).unwrap(),
        b"rem my own server"
    );
    assert_eq!(
        std::fs::read(install.join("DeCombat.bat.new")).unwrap(),
        b"rem the new preset"
    );
    assert_eq!(
        std::fs::read(install.join("settings.toml")).unwrap(),
        b"volume = 3"
    );
    assert_eq!(std::fs::read(install.join("notes.txt")).unwrap(), b"mine");
    assert_eq!(
        std::fs::read(install.join("Combat Arms").join("Engine.exe")).unwrap(),
        b"2009"
    );
    assert_eq!(a.placed.len(), 3, "{a:?}");
    assert_eq!(a.kept.len(), 3, "{a:?}");
    // the replaced files wait in old/
    assert_eq!(
        std::fs::read(install.join(UPDATE_DIR).join(BACKUP_DIR).join(EXE)).unwrap(),
        b"old exe"
    );
    let _ = std::fs::remove_dir_all(&install);
}

#[test]
fn a_failed_swap_puts_every_replaced_file_back() {
    let install = fake_install("rollback");
    // `zz` is a file in the install: the staged `zz/b.txt` cannot go there, after the exe was replaced
    std::fs::write(install.join("zz"), b"a file").unwrap();
    let s = staged(
        &install,
        &[
            (EXE, b"new exe"),
            ("README.md", b"new readme"),
            ("zz/b.txt", b"x"),
        ],
    );
    assert!(apply_staged(&s, &install).is_err());
    assert_eq!(std::fs::read(install.join(EXE)).unwrap(), b"old exe");
    assert_eq!(
        std::fs::read(install.join("README.md")).unwrap(),
        b"old readme"
    );
    assert_eq!(std::fs::read(install.join("zz")).unwrap(), b"a file");
    let _ = std::fs::remove_dir_all(&install);
}

#[test]
fn the_helper_takes_only_this_installs_staging() {
    let install = temp("helper_args");
    let other = temp("helper_other");
    let args = |s: &Path| {
        vec![
            s.as_os_str().to_owned(),
            install.as_os_str().to_owned(),
            OsString::from("1"),
        ]
    };
    assert_eq!(finish_update(&args(&other)), 2);
    assert_eq!(
        finish_update(&args(&install.join(UPDATE_DIR).join(STAGING_DIR))),
        2
    );
    assert_eq!(finish_update(&[]), 2);
    let _ = std::fs::remove_dir_all(&install);
    let _ = std::fs::remove_dir_all(&other);
}

#[test]
fn an_ended_process_is_seen_as_ended() {
    let mut c = if cfg!(windows) {
        let mut c = std::process::Command::new("cmd");
        c.args(["/C", "exit 0"]);
        c
    } else {
        std::process::Command::new("true")
    };
    let mut child = c.spawn().unwrap();
    let pid = child.id();
    child.wait().unwrap();
    assert!(wait_for_exit(pid, Duration::from_secs(5)));
    // this test's own process runs
    assert!(!wait_for_exit(std::process::id(), Duration::ZERO));
}

#[test]
fn a_pack_file_names_its_build_and_update_folder() {
    let d = temp("pack");
    assert_eq!(read_pack(&d), None);
    std::fs::write(
        d.join(PACK_FILE),
        r#"{"commit":"ABCDEF12","update_dir":"C:\\builds\\fp_latest"}"#,
    )
    .unwrap();
    assert_eq!(
        read_pack(&d),
        Some(Pack {
            commit: "abcdef12".into(),
            update_dir: Some(PathBuf::from("C:\\builds\\fp_latest"))
        })
    );
    std::fs::write(
        d.join(PACK_FILE),
        r#"{"commit":"abcdef12","update_dir":""}"#,
    )
    .unwrap();
    assert_eq!(read_pack(&d).unwrap().update_dir, None);
    for bad in [r#"{"commit":"xyz"}"#, r#"{"update_dir":"x"}"#, "not json"] {
        std::fs::write(d.join(PACK_FILE), bad).unwrap();
        assert_eq!(read_pack(&d), None, "{bad}");
    }
    let _ = std::fs::remove_dir_all(&d);
}

// CLI-1866: a friend pack whose update folder names another build: the offer, then the same staging and swap as a
// release, the player's files kept
#[test]
fn a_friend_pack_updates_from_its_folder() {
    let install = fake_install("friend");
    let updates = temp("friend_updates");
    std::fs::write(
        install.join(PACK_FILE),
        format!(
            r#"{{"commit":"1111aaaa","update_dir":{}}}"#,
            serde_json::json!(updates.to_string_lossy())
        ),
    )
    .unwrap();
    let pack = read_pack(&install).unwrap();
    assert_eq!(local_offer(&pack), Ok(None));
    std::fs::write(
        updates.join(LATEST_FILE),
        r#"{"commit":"1111aaaa0000","zip":"DeCombat-win-1111aaaa.zip"}"#,
    )
    .unwrap();
    assert_eq!(local_offer(&pack), Ok(None), "the same build");
    std::fs::write(
        updates.join(LATEST_FILE),
        r#"{"commit":"2222bbbb","zip":"DeCombat-win-2222bbbb.zip"}"#,
    )
    .unwrap();
    assert!(local_offer(&pack).unwrap_err().contains("missing"));
    let exe = format!("DeCombat-win-2222bbbb/{EXE}");
    let new_pack = format!(
        r#"{{"commit":"2222bbbb","update_dir":{}}}"#,
        serde_json::json!(updates.to_string_lossy())
    );
    std::fs::write(
        updates.join("DeCombat-win-2222bbbb.zip"),
        zip_of(&[
            ("DeCombat-win-2222bbbb/", b""),
            (&exe, b"build 2222"),
            (
                "DeCombat-win-2222bbbb/DeCombat.bat",
                b"rem the pack's launcher",
            ),
            ("DeCombat-win-2222bbbb/README.md", b"readme 2222"),
            ("DeCombat-win-2222bbbb/pack.json", new_pack.as_bytes()),
        ]),
    )
    .unwrap();
    let o = local_offer(&pack).unwrap().expect("a newer build");
    assert!(
        o.text.contains("2222bbbb") && o.text.contains("1111aaaa"),
        "{}",
        o.text
    );
    assert!(o.page.is_empty() && !o.too_old);
    let Source::Local { zip, commit } = &o.source else {
        panic!("{o:?}")
    };
    assert_eq!(commit, "2222bbbb");
    let staging = stage(&read_capped(zip).unwrap(), &install).unwrap();
    let a = apply_staged(&staging, &install).unwrap();
    assert_eq!(std::fs::read(install.join(EXE)).unwrap(), b"build 2222");
    assert_eq!(
        std::fs::read(install.join("README.md")).unwrap(),
        b"readme 2222"
    );
    assert_eq!(read_pack(&install).unwrap().commit, "2222bbbb");
    assert_eq!(
        std::fs::read(install.join("settings.toml")).unwrap(),
        b"volume = 3"
    );
    assert_eq!(
        std::fs::read(install.join("DeCombat.bat")).unwrap(),
        b"rem my own server"
    );
    assert_eq!(std::fs::read(install.join("notes.txt")).unwrap(), b"mine");
    assert!(a.kept.iter().any(|k| k == Path::new("DeCombat.bat")));
    // after the swap the folder offers nothing more
    assert_eq!(local_offer(&read_pack(&install).unwrap()), Ok(None));
    // a zip name that leaves the folder is refused
    std::fs::write(
        updates.join(LATEST_FILE),
        r#"{"commit":"3333cccc","zip":"..\\x.zip"}"#,
    )
    .unwrap();
    assert!(local_offer(&pack).is_err());
    let _ = std::fs::remove_dir_all(&install);
    let _ = std::fs::remove_dir_all(&updates);
}

#[test]
fn players_files_are_known_by_name() {
    for n in [
        "DeCombat.bat",
        "decombat.BAT",
        "bots.bat",
        "settings.toml",
        "Settings.json",
        "x.log",
        "dc-client.out",
        "a/b/known_servers.txt",
        "server.toml",
        "net.jsonl",
    ] {
        assert!(players_file(Path::new(n)), "{n}");
    }
    for n in [
        "dc-client.exe",
        "README.md",
        "pack.json",
        "dc-botclient.exe",
        "docs/x.md",
    ] {
        assert!(!players_file(Path::new(n)), "{n}");
    }
}

/// Network look (not a gate, ignored by default): the release list of the public repository through the OS-trusted
/// TLS connection, the pick of this platform's zip, and its SHA256SUMS through GitHub's redirect.
#[test]
#[ignore]
fn live_github_release_lookup() {
    let json = "application/vnd.github+json";
    let mut r = get(
        &format!("{RELEASES_API}/latest"),
        MAX_API,
        json,
        &mut |_, _| {},
    )
    .unwrap();
    if r.status == 404 {
        r = get(
            &format!("{RELEASES_API}?per_page=10"),
            MAX_API,
            json,
            &mut |_, _| {},
        )
        .unwrap();
    }
    assert_eq!(r.status, 200);
    let doc: serde_json::Value = serde_json::from_slice(&r.body).unwrap();
    let rel = pick_release(&doc, Platform::Windows).expect("a release with the Windows zip");
    eprintln!("{} {:?} {}", rel.tag, rel.version, rel.zip.url);
    // the official form and GitHub's real redirect chain pass the update's host rule (CLI-1864)
    assert!(official_release(&rel.zip.url).is_some(), "{}", rel.zip.url);
    let sums_url = sums_url_of(&rel.zip.url).unwrap();
    assert_eq!(rel.sums.expect("SHA256SUMS").url, sums_url);
    let sums = get_with(&sums_url, MAX_SUMS, "*/*", &mut |_, _| {}, &mut |u| {
        on_github(u)?;
        dc_botclient::tls::connect_public(&u.host, u.port, NET_TIMEOUT)
            .map(|c| Box::new(c) as Box<dyn Conn>)
    })
    .unwrap();
    assert_eq!(sums.status, 200);
    let text = String::from_utf8_lossy(&sums.body);
    assert!(sum_for(&text, &rel.zip.name).is_some(), "{text}");
}
