//! The flow against an in-memory fake authority that keeps the server side's rules: only hashes, rotation by
//! compare-and-swap, the last spent secret = reuse = revoked, anything else refused without a change.

use crate::secret::{b64url_decode, hash_text};
use crate::*;
use std::cell::RefCell;
use std::sync::{Arc, Mutex};

const AUTH: &str = "auth.example:28012";
const LOGIN: &str = "player1";

/// How the fake's network behaves for the next request.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Net {
    Ok,
    /// The request never reaches the authority, and the client knows it (no connection).
    LostBefore,
    /// The request never reaches the authority, but the client cannot tell (the connection broke mid-way).
    Dropped,
    /// The authority handles the request, the answer is lost.
    LostAfter,
    /// The authority answers this status without handling the request.
    Status(u16),
}

type SendHook = Box<dyn FnMut(&str, &serde_json::Value) + Send>;

#[derive(Default)]
struct Device {
    id: String,
    cur: Vec<u8>,
    prev: Option<Vec<u8>>,
    revoked: bool,
    generation: u32,
}

struct Fake {
    devices: Vec<Device>,
    plan: Vec<Net>,
    requests: Vec<(String, serde_json::Value)>,
    /// A hook run when a request leaves the client (to look at the store at that moment).
    on_send: Option<SendHook>,
}

impl Fake {
    fn new() -> Fake {
        Fake {
            devices: Vec::new(),
            plan: Vec::new(),
            requests: Vec::new(),
            on_send: None,
        }
    }

    fn device(&self, id: &str) -> &Device {
        self.devices.iter().find(|d| d.id == id).unwrap()
    }

    fn handle(&mut self, path: &str, v: &serde_json::Value) -> Reply {
        let s = |k: &str| v.get(k).and_then(|x| x.as_str()).unwrap_or("").to_string();
        let ok = |body: serde_json::Value| Reply {
            status: 200,
            body: body.to_string(),
        };
        let refused = Reply {
            status: 401,
            body: "{\"error\":\"signed_out\"}".into(),
        };
        match path {
            DEVICE_PATH => {
                if s("password") != "correct horse" {
                    return Reply {
                        status: 401,
                        body: "{}".into(),
                    };
                }
                let n = self.devices.len();
                let id = format!("dv_{}", crate::secret::b64url(&[n as u8; 16]));
                self.devices.push(Device {
                    id: id.clone(),
                    cur: b64url_decode(&s("next_hash")).unwrap(),
                    ..Device::default()
                });
                ok(
                    serde_json::json!({"device": id, "token": "tok-0", "expires": 900, "account": 7,
                    "nick": "Seven", "idle_days": 180}),
                )
            }
            REFRESH_PATH => {
                let Some(d) = self.devices.iter_mut().find(|d| d.id == s("device")) else {
                    return refused;
                };
                let h = b64url_decode(&hash_text(s("secret").as_bytes())).unwrap();
                if d.revoked {
                    return refused;
                }
                if h == d.cur {
                    let next = b64url_decode(&s("next_hash")).unwrap();
                    d.prev = Some(std::mem::replace(&mut d.cur, next));
                    d.generation += 1;
                    return ok(serde_json::json!({"token": format!("tok-{}", d.generation),
                        "expires": 900 + d.generation, "account": 7, "nick": "Seven", "device_expires": 5000}));
                }
                if d.prev.as_deref() == Some(&h[..]) {
                    d.revoked = true;
                }
                refused
            }
            SIGNOUT_PATH => {
                if let Some(d) = self.devices.iter_mut().find(|d| d.id == s("device")) {
                    let h = b64url_decode(&hash_text(s("secret").as_bytes())).unwrap();
                    if h == d.cur {
                        d.revoked = true;
                    }
                }
                ok(serde_json::json!({}))
            }
            _ => Reply {
                status: 404,
                body: String::new(),
            },
        }
    }
}

/// The client's end of the fake (shared so a test can look at the authority between calls).
#[derive(Clone)]
struct Wire(Arc<Mutex<Fake>>);

impl Transport for Wire {
    fn post(&mut self, path: &str, body: &str) -> Result<Reply, NetError> {
        let v: serde_json::Value = serde_json::from_str(body).unwrap();
        let mut f = self.0.lock().unwrap();
        f.requests.push((path.to_string(), v.clone()));
        if let Some(h) = f.on_send.as_mut() {
            h(path, &v);
        }
        let net = if f.plan.is_empty() {
            Net::Ok
        } else {
            f.plan.remove(0)
        };
        match net {
            Net::Ok => Ok(f.handle(path, &v)),
            Net::LostBefore => Err(NetError::not_sent("connection refused")),
            Net::Dropped => Err(NetError::unknown("connection reset")),
            Net::LostAfter => {
                f.handle(path, &v);
                Err(NetError::unknown("connection reset"))
            }
            Net::Status(s) => Ok(Reply {
                status: s,
                body: String::new(),
            }),
        }
    }
}

fn setup() -> (Arc<MemoryStore>, Wire, String) {
    let store = Arc::new(MemoryStore::new());
    let mut wire = Wire(Arc::new(Mutex::new(Fake::new())));
    let s = sign_in(
        &*store,
        &mut wire,
        AUTH,
        LOGIN,
        "correct horse",
        &DeviceInfo {
            name: "Desk".into(),
            platform: "windows".into(),
        },
    )
    .unwrap();
    assert!(s.store_error.is_none());
    (store, wire, s.device)
}

fn kept(store: &MemoryStore) -> Credential {
    store.load(AUTH, LOGIN).unwrap().unwrap()
}

fn hash_of(s: &Secret) -> Vec<u8> {
    b64url_decode(&s.proposed_hash()).unwrap()
}

#[test]
fn sign_in_sends_only_the_hash_and_keeps_the_device() {
    let (store, wire, device) = setup();
    let c = kept(&store);
    assert_eq!((c.device.as_str(), c.secrets.len()), (device.as_str(), 1));
    let f = wire.0.lock().unwrap();
    let (path, body) = &f.requests[0];
    assert_eq!(path, DEVICE_PATH);
    assert!(body.get("secret").is_none());
    assert!(!body.to_string().contains(c.secrets[0].expose()));
    assert_eq!(f.device(&device).cur, hash_of(&c.secrets[0]));
    // the login is kept for the authority and found without case
    assert!(store.load(AUTH, "PLAYER1").unwrap().is_some());
    assert!(store.load("other:1", LOGIN).unwrap().is_none());
}

#[test]
fn a_refused_or_unreachable_sign_in_keeps_nothing() {
    let store = MemoryStore::new();
    let mut wire = Wire(Arc::new(Mutex::new(Fake::new())));
    let info = DeviceInfo::default();
    let r = sign_in(&store, &mut wire, AUTH, LOGIN, "wrong", &info);
    assert_eq!(r.unwrap_err(), SignInError::Refused);
    wire.0.lock().unwrap().plan = vec![Net::Status(404), Net::Status(429), Net::LostBefore];
    for want in [SignInError::NotSupported, SignInError::Limited] {
        let r = sign_in(&store, &mut wire, AUTH, LOGIN, "correct horse", &info);
        assert_eq!(r.unwrap_err(), want);
    }
    let r = sign_in(&store, &mut wire, AUTH, LOGIN, "correct horse", &info);
    assert!(matches!(r, Err(SignInError::Unreachable(_))));
    assert!(store.load(AUTH, LOGIN).unwrap().is_none());
}

#[test]
fn each_refresh_rotates_to_a_new_secret() {
    let (store, mut wire, device) = setup();
    let s0 = kept(&store).secrets[0].clone();
    let t1 = refresh(&*store, &mut wire, AUTH, LOGIN).unwrap();
    assert_eq!(
        (t1.token.as_str(), t1.account, t1.device_expires),
        ("tok-1", 7, Some(5000))
    );
    let c = kept(&store);
    assert_eq!(c.secrets.len(), 1);
    assert!(!c.secrets[0].same_as(&s0));
    assert_eq!(
        wire.0.lock().unwrap().device(&device).cur,
        hash_of(&c.secrets[0])
    );
    let t2 = refresh(&*store, &mut wire, AUTH, LOGIN).unwrap();
    assert_eq!(t2.token.as_str(), "tok-2");
    assert!(!wire.0.lock().unwrap().device(&device).revoked);
}

#[test]
fn the_next_secret_is_saved_before_the_request_leaves() {
    let (store, mut wire, _) = setup();
    let seen = Arc::new(Mutex::new(0));
    let (st, n) = (store.clone(), seen.clone());
    wire.0.lock().unwrap().on_send = Some(Box::new(move |path, body| {
        if path != REFRESH_PATH {
            return;
        }
        // at the moment of sending, the store already holds the secret whose hash is proposed, in front
        let c = st.load(AUTH, LOGIN).unwrap().unwrap();
        let proposed = body["next_hash"].as_str().unwrap();
        assert_eq!(c.secrets[0].proposed_hash(), proposed);
        assert_eq!(c.secrets[1].expose(), body["secret"].as_str().unwrap());
        *n.lock().unwrap() += 1;
    }));
    refresh(&*store, &mut wire, AUTH, LOGIN).unwrap();
    refresh(&*store, &mut wire, AUTH, LOGIN).unwrap();
    assert_eq!(*seen.lock().unwrap(), 2);
}

#[test]
fn an_answer_lost_after_the_authority_rotated_does_not_lock_out() {
    let (store, mut wire, device) = setup();
    wire.0.lock().unwrap().plan = vec![Net::LostAfter];
    let r = refresh(&*store, &mut wire, AUTH, LOGIN);
    assert!(matches!(r, Err(RefreshError::Unreachable(_))));
    // both the proposed and the spent secret are kept, newest first
    let c = kept(&store);
    assert_eq!(c.secrets.len(), 2);
    assert_eq!(
        wire.0.lock().unwrap().device(&device).cur,
        hash_of(&c.secrets[0])
    );
    // the next launch spends the new secret first: no reuse, no revoke
    refresh(&*store, &mut wire, AUTH, LOGIN).unwrap();
    assert!(!wire.0.lock().unwrap().device(&device).revoked);
    assert_eq!(kept(&store).secrets.len(), 1);
}

#[test]
fn a_request_that_never_arrived_falls_back_to_the_older_secret() {
    let (store, mut wire, device) = setup();
    // known not sent: the proposal is dropped again at once
    wire.0.lock().unwrap().plan = vec![Net::LostBefore];
    assert!(refresh(&*store, &mut wire, AUTH, LOGIN).is_err());
    assert_eq!(kept(&store).secrets.len(), 1);
    // broken mid-way (it never arrived, but the client cannot know): kept undecided
    wire.0.lock().unwrap().plan = vec![Net::Dropped];
    assert!(refresh(&*store, &mut wire, AUTH, LOGIN).is_err());
    assert_eq!(kept(&store).secrets.len(), 2);
    let before = wire.0.lock().unwrap().requests.len();
    let t = refresh(&*store, &mut wire, AUTH, LOGIN).unwrap();
    assert_eq!(t.token.as_str(), "tok-1");
    // the never-accepted secret was refused (no change), the older one then worked
    assert_eq!(wire.0.lock().unwrap().requests.len() - before, 2);
    assert!(!wire.0.lock().unwrap().device(&device).revoked);
    let c = kept(&store);
    assert_eq!(c.secrets.len(), 1);
    assert_eq!(
        wire.0.lock().unwrap().device(&device).cur,
        hash_of(&c.secrets[0])
    );
}

/// A store whose saves fail from the `fail_from`-th call on (1-based).
struct Flaky {
    inner: MemoryStore,
    saves: RefCell<u32>,
    fail_from: u32,
}

impl CredentialStore for Flaky {
    fn load(&self, a: &str, l: &str) -> Result<Option<Credential>, StoreError> {
        self.inner.load(a, l)
    }
    fn save(&self, c: &Credential) -> Result<(), StoreError> {
        *self.saves.borrow_mut() += 1;
        if *self.saves.borrow() >= self.fail_from {
            return Err(StoreError("disk full".into()));
        }
        self.inner.save(c)
    }
    fn delete(&self, a: &str, l: &str) -> Result<(), StoreError> {
        self.inner.delete(a, l)
    }
}

#[test]
fn a_crash_between_the_answer_and_the_last_save_does_not_lock_out() {
    let (store, mut wire, device) = setup();
    let flaky = Flaky {
        inner: MemoryStore::new(),
        saves: RefCell::new(0),
        fail_from: 2,
    };
    flaky.inner.save(&kept(&store)).unwrap();
    // the pre-send save works, the save after the 200 fails (as if the process died there)
    let t = refresh(&flaky, &mut wire, AUTH, LOGIN).unwrap();
    assert_eq!(t.token.as_str(), "tok-1");
    let c = flaky.inner.load(AUTH, LOGIN).unwrap().unwrap();
    assert_eq!(c.secrets.len(), 2);
    // the next launch (a working store) goes on from the newest secret
    refresh(&flaky.inner, &mut wire, AUTH, LOGIN).unwrap();
    assert!(!wire.0.lock().unwrap().device(&device).revoked);
}

#[test]
fn nothing_is_sent_when_the_next_secret_cannot_be_saved() {
    let (store, mut wire, _) = setup();
    let flaky = Flaky {
        inner: MemoryStore::new(),
        saves: RefCell::new(0),
        fail_from: 1,
    };
    flaky.inner.save(&kept(&store)).unwrap();
    let before = wire.0.lock().unwrap().requests.len();
    let r = refresh(&flaky, &mut wire, AUTH, LOGIN);
    assert!(matches!(r, Err(RefreshError::Store(_))));
    assert_eq!(wire.0.lock().unwrap().requests.len(), before);
}

#[test]
fn a_revoked_device_is_signed_out_and_forgotten() {
    let (store, mut wire, device) = setup();
    wire.0.lock().unwrap().devices[0].revoked = true;
    assert_eq!(
        refresh(&*store, &mut wire, AUTH, LOGIN).unwrap_err(),
        RefreshError::SignedOut
    );
    assert!(store.load(AUTH, LOGIN).unwrap().is_none());
    assert_eq!(
        refresh(&*store, &mut wire, AUTH, LOGIN).unwrap_err(),
        RefreshError::NoCredential
    );
    assert!(wire.0.lock().unwrap().device(&device).revoked);
}

#[test]
fn unreachable_limited_unsupported_and_server_errors_keep_the_credential() {
    let (store, mut wire, device) = setup();
    wire.0.lock().unwrap().plan = vec![
        Net::LostBefore,
        Net::Status(429),
        Net::Status(404),
        Net::Status(500),
        Net::Status(503),
    ];
    let s0 = kept(&store).secrets[0].clone();
    for _ in 0..5 {
        let e = refresh(&*store, &mut wire, AUTH, LOGIN).unwrap_err();
        assert!(matches!(
            e,
            RefreshError::Unreachable(_) | RefreshError::Limited | RefreshError::NotSupported
        ));
        // the authority changed nothing, so neither does the client: the current secret alone
        let c = kept(&store);
        assert_eq!(c.secrets.len(), 1);
        assert!(c.secrets[0].same_as(&s0));
    }
    // any number of such launches later, one request refreshes
    let before = wire.0.lock().unwrap().requests.len();
    refresh(&*store, &mut wire, AUTH, LOGIN).unwrap();
    assert_eq!(wire.0.lock().unwrap().requests.len() - before, 1);
    assert!(!wire.0.lock().unwrap().device(&device).revoked);
}

/// A 401 that is not the authority's own refusal (a captive portal's page) never drops a secret.
#[test]
fn a_401_from_something_else_signs_nobody_out() {
    let (store, mut wire, device) = setup();
    wire.0.lock().unwrap().plan = vec![Net::Status(401); 12];
    for _ in 0..12 {
        assert!(matches!(
            refresh(&*store, &mut wire, AUTH, LOGIN),
            Err(RefreshError::Unreachable(_))
        ));
        assert_eq!(kept(&store).secrets.len(), 1);
    }
    refresh(&*store, &mut wire, AUTH, LOGIN).unwrap();
    assert!(!wire.0.lock().unwrap().device(&device).revoked);
}

#[test]
fn undecided_secrets_resolve_newest_first_without_a_revoke() {
    let (store, mut wire, device) = setup();
    // two lost answers after the authority rotated each time: the newest kept secret is current
    wire.0.lock().unwrap().plan = vec![Net::LostAfter, Net::LostAfter];
    assert!(refresh(&*store, &mut wire, AUTH, LOGIN).is_err());
    assert!(refresh(&*store, &mut wire, AUTH, LOGIN).is_err());
    assert_eq!(kept(&store).secrets.len(), 3);
    refresh(&*store, &mut wire, AUTH, LOGIN).unwrap();
    assert!(!wire.0.lock().unwrap().device(&device).revoked);
    // one lost after, one that never arrived: the second-newest is current, the newest is refused first
    wire.0.lock().unwrap().plan = vec![Net::LostAfter, Net::Dropped];
    assert!(refresh(&*store, &mut wire, AUTH, LOGIN).is_err());
    assert!(refresh(&*store, &mut wire, AUTH, LOGIN).is_err());
    refresh(&*store, &mut wire, AUTH, LOGIN).unwrap();
    assert!(!wire.0.lock().unwrap().device(&device).revoked);
    assert_eq!(kept(&store).secrets.len(), 1);
}

#[test]
fn a_copied_credential_is_caught_as_reuse() {
    let (store, mut wire, device) = setup();
    // someone copies the store, then refreshes with the copy first
    let thief = MemoryStore::new();
    thief.save(&kept(&store)).unwrap();
    refresh(&thief, &mut wire, AUTH, LOGIN).unwrap();
    // the owner's next refresh presents the spent secret: the device is revoked for both
    assert_eq!(
        refresh(&*store, &mut wire, AUTH, LOGIN).unwrap_err(),
        RefreshError::SignedOut
    );
    assert!(wire.0.lock().unwrap().device(&device).revoked);
    assert_eq!(
        refresh(&thief, &mut wire, AUTH, LOGIN).unwrap_err(),
        RefreshError::SignedOut
    );
}

#[test]
fn sign_out_tells_the_authority_and_forgets_even_when_unreachable() {
    let (store, mut wire, device) = setup();
    sign_out(&*store, &mut wire, AUTH, LOGIN).unwrap();
    assert!(store.load(AUTH, LOGIN).unwrap().is_none());
    assert!(wire.0.lock().unwrap().device(&device).revoked);
    let (store, mut wire, _) = setup();
    wire.0.lock().unwrap().plan = vec![Net::LostBefore];
    sign_out(&*store, &mut wire, AUTH, LOGIN).unwrap();
    assert!(store.load(AUTH, LOGIN).unwrap().is_none());
    // nothing kept: nothing to do
    sign_out(&*store, &mut wire, AUTH, LOGIN).unwrap();
}

#[test]
fn answers_that_are_not_the_endpoints_are_unreachable() {
    let (store, _, _) = setup();
    struct Junk(u16, &'static str);
    impl Transport for Junk {
        fn post(&mut self, _: &str, _: &str) -> Result<Reply, NetError> {
            Ok(Reply {
                status: self.0,
                body: self.1.into(),
            })
        }
    }
    for body in [
        "",
        "[]",
        "{\"token\":\"\"}",
        "{\"token\":\"t\",\"expires\":1}",
    ] {
        let r = refresh(&*store, &mut Junk(200, body), AUTH, LOGIN);
        assert!(matches!(r, Err(RefreshError::Unreachable(_))), "{body}");
    }
    // the credential survives, with the undecided secrets in front (bounded)
    assert!(kept(&store).secrets.len() <= store::MAX_SECRETS);
    let s = MemoryStore::new();
    let long = format!(
        "{{\"device\":\"dv_{}\",\"token\":\"t\",\"expires\":1,\"account\":1}}",
        "A".repeat(23)
    );
    let r = sign_in(
        &s,
        &mut Junk(200, Box::leak(long.into_boxed_str())),
        AUTH,
        LOGIN,
        "p",
        &DeviceInfo::default(),
    );
    assert!(
        matches!(r, Err(SignInError::Unreachable(_))),
        "a malformed device id"
    );
}

#[test]
fn debug_output_hides_secrets_and_tokens() {
    let (store, mut wire, _) = setup();
    let c = kept(&store);
    let d = format!("{c:?}");
    assert!(!d.contains(c.secrets[0].expose()), "{d}");
    let t = refresh(&*store, &mut wire, AUTH, LOGIN).unwrap();
    assert!(!format!("{t:?}").contains("tok-1"));
}

// ------------------------------------------------------------------------------------------------ the file store

fn temp_dir(name: &str) -> std::path::PathBuf {
    let d = std::env::temp_dir().join(format!("dc-session-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    d
}

fn cred(authority: &str, login: &str) -> Credential {
    Credential {
        authority: authority.into(),
        login: login.into(),
        device: format!("dv_{}", "A".repeat(22)),
        secrets: vec![Secret::generate().unwrap(), Secret::generate().unwrap()],
        rev: 0,
    }
}

#[test]
fn the_file_store_round_trips_several_entries_and_deletes() {
    let dir = temp_dir("rt");
    let fs = FileStore::in_profile(&dir);
    assert!(fs.load(AUTH, LOGIN).unwrap().is_none(), "no file yet");
    let a = cred(AUTH, LOGIN);
    let b = cred("other:1", LOGIN);
    fs.save(&a).unwrap();
    fs.save(&b).unwrap();
    let back = fs.load(AUTH, "Player1").unwrap().unwrap();
    assert_eq!(back.device, a.device);
    assert_eq!(back.secrets.len(), 2);
    assert!(back.secrets[0].same_as(&a.secrets[0]) && back.secrets[1].same_as(&a.secrets[1]));
    // a save replaces the entry of the same authority and login
    let mut a2 = a.clone();
    a2.secrets.truncate(1);
    fs.save(&a2).unwrap();
    assert_eq!(fs.load(AUTH, LOGIN).unwrap().unwrap().secrets.len(), 1);
    assert!(fs.load("other:1", LOGIN).unwrap().is_some());
    fs.delete(AUTH, LOGIN).unwrap();
    assert!(fs.load(AUTH, LOGIN).unwrap().is_none());
    assert!(fs.load("other:1", LOGIN).unwrap().is_some());
    fs.delete("other:1", LOGIN).unwrap();
    assert!(!fs.path().exists(), "the last entry removes the file");
    fs.delete("other:1", LOGIN).unwrap();
    assert!(!fs.path().with_extension("json.tmp").exists());
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn a_damaged_or_oversized_file_is_an_error_and_is_never_overwritten() {
    let dir = temp_dir("bad");
    std::fs::create_dir_all(&dir).unwrap();
    let fs = FileStore::in_profile(&dir);
    let good = Secret::generate().unwrap();
    let bad_secret = format!(
        "{{\"version\":1,\"devices\":[{{\"authority\":\"{AUTH}\",\"login\":\"{LOGIN}\",\"device\":\"dv_x\",\"secrets\":[\"bad\",\"{}\"]}}]}}",
        good.expose()
    );
    let oversized = format!(
        "{{\"version\":1,\"devices\":[],\"pad\":\"{}\"}}",
        "x".repeat(1 << 20)
    );
    for junk in [
        "".to_string(),
        "not json".into(),
        "{\"version\":2,\"devices\":[]}".into(),
        "{\"version\":1,\"devices\":[{\"login\":\"x\"}]}".into(),
        bad_secret,
        oversized,
    ] {
        std::fs::write(fs.path(), &junk).unwrap();
        let e = fs.load(AUTH, LOGIN).unwrap_err();
        assert!(!e.0.contains(good.expose()), "the error names no secret");
        // a save (another authority's credential) refuses instead of wiping what it cannot read
        assert!(fs.save(&cred("other:1", LOGIN)).is_err());
        assert!(fs.delete(AUTH, LOGIN).is_err());
        assert_eq!(
            std::fs::read_to_string(fs.path()).unwrap(),
            junk,
            "left as it was"
        );
    }
    // the refresh flow reports it and sends nothing
    let mut wire = Wire(Arc::new(Mutex::new(Fake::new())));
    assert!(matches!(
        refresh(&fs, &mut wire, AUTH, LOGIN),
        Err(RefreshError::Store(_))
    ));
    assert!(wire.0.lock().unwrap().requests.is_empty());
    // removed by the player, the store works again
    std::fs::remove_file(fs.path()).unwrap();
    fs.save(&cred(AUTH, LOGIN)).unwrap();
    assert_eq!(fs.load(AUTH, LOGIN).unwrap().unwrap().secrets.len(), 2);
    let _ = std::fs::remove_dir_all(&dir);
}

/// Two game instances on one profile (two stores over one folder, on two threads) refresh at the same time,
/// many times: they take turns, each sees the other's rotation, and the device is never taken for a stolen copy.
#[test]
fn two_stores_on_one_profile_take_turns_and_never_trip_reuse() {
    let dir = temp_dir("two");
    let wire = Wire(Arc::new(Mutex::new(Fake::new())));
    sign_in(
        &FileStore::in_profile(&dir),
        &mut wire.clone(),
        AUTH,
        LOGIN,
        "correct horse",
        &DeviceInfo::default(),
    )
    .unwrap();
    let barrier = Arc::new(std::sync::Barrier::new(2));
    let run = |_: u32| {
        let (dir, mut wire, barrier) = (dir.clone(), wire.clone(), barrier.clone());
        std::thread::spawn(move || {
            let fs = FileStore::in_profile(&dir);
            barrier.wait();
            (0..15)
                .map(|_| refresh(&fs, &mut wire, AUTH, LOGIN).is_ok())
                .filter(|ok| *ok)
                .count()
        })
    };
    let (a, b) = (run(0), run(1));
    let ok = a.join().unwrap() + b.join().unwrap();
    assert_eq!(ok, 30, "every refresh worked");
    let f = wire.0.lock().unwrap();
    assert!(!f.devices[0].revoked);
    assert_eq!(f.devices[0].generation, 30);
    drop(f);
    let left: Vec<_> = std::fs::read_dir(&dir)
        .unwrap()
        .map(|e| e.unwrap().file_name().to_string_lossy().to_string())
        .collect();
    let mut left = left;
    left.sort();
    assert_eq!(
        left,
        vec![STORE_FILE.to_string(), format!("{STORE_FILE}.lock")],
        "no temporary file is left (the lock file stays, unlocked)"
    );
    let _ = std::fs::remove_dir_all(&dir);
}

/// A lock held by another instance for longer than the wait: the refresh gives up before sending anything.
#[test]
fn a_busy_store_sends_nothing() {
    let dir = temp_dir("busy");
    let wire = Wire(Arc::new(Mutex::new(Fake::new())));
    let fs = FileStore::in_profile(&dir).with_lock_wait(std::time::Duration::from_millis(50));
    sign_in(
        &fs,
        &mut wire.clone(),
        AUTH,
        LOGIN,
        "correct horse",
        &DeviceInfo::default(),
    )
    .unwrap();
    let (d2, w2) = (dir.clone(), wire.clone());
    let (held_tx, held_rx) = std::sync::mpsc::channel();
    let (done_tx, done_rx) = std::sync::mpsc::channel::<()>();
    let holder = std::thread::spawn(move || {
        let other = FileStore::in_profile(&d2);
        let _lock = other.lock().unwrap();
        held_tx.send(()).unwrap();
        done_rx.recv().unwrap();
        drop(w2);
    });
    held_rx.recv().unwrap();
    let before = wire.0.lock().unwrap().requests.len();
    let r = refresh(&fs, &mut wire.clone(), AUTH, LOGIN);
    assert!(matches!(r, Err(RefreshError::Store(_))), "{r:?}");
    assert_eq!(wire.0.lock().unwrap().requests.len(), before);
    done_tx.send(()).unwrap();
    holder.join().unwrap();
    assert!(refresh(&fs, &mut wire.clone(), AUTH, LOGIN).is_ok());
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn the_refresh_flow_works_on_the_file_store() {
    let dir = temp_dir("flow");
    let fs = FileStore::in_profile(&dir);
    let mut wire = Wire(Arc::new(Mutex::new(Fake::new())));
    sign_in(
        &fs,
        &mut wire,
        AUTH,
        LOGIN,
        "correct horse",
        &DeviceInfo::default(),
    )
    .unwrap();
    wire.0.lock().unwrap().plan = vec![Net::LostAfter];
    assert!(refresh(&fs, &mut wire, AUTH, LOGIN).is_err());
    // a new process: a fresh FileStore over the same file
    let fs2 = FileStore::in_profile(&dir);
    assert_eq!(
        refresh(&fs2, &mut wire, AUTH, LOGIN)
            .unwrap()
            .token
            .as_str(),
        "tok-2"
    );
    assert!(!wire.0.lock().unwrap().devices[0].revoked);
    let _ = std::fs::remove_dir_all(&dir);
}

#[cfg(unix)]
#[test]
fn the_file_is_owner_only_on_unix() {
    use std::os::unix::fs::PermissionsExt;
    let dir = temp_dir("mode");
    let fs = FileStore::in_profile(&dir);
    fs.save(&cred(AUTH, LOGIN)).unwrap();
    let mode = std::fs::metadata(fs.path()).unwrap().permissions().mode();
    assert_eq!(mode & 0o777, 0o600);
    let _ = std::fs::remove_dir_all(&dir);
}

// ------------------------------------------------------------------------------------------------ the fallback store

/// A store double that can be made to fail per operation: an OS store that cannot be read, refuses a save, or cannot
/// remove a copy.
struct Broken {
    inner: MemoryStore,
    reads: std::sync::atomic::AtomicBool,
    saves: std::sync::atomic::AtomicBool,
    deletes: std::sync::atomic::AtomicBool,
}

impl Broken {
    fn new() -> Broken {
        Broken {
            inner: MemoryStore::new(),
            reads: std::sync::atomic::AtomicBool::new(true),
            saves: std::sync::atomic::AtomicBool::new(true),
            deletes: std::sync::atomic::AtomicBool::new(true),
        }
    }
    fn set(&self, reads: bool, saves: bool, deletes: bool) {
        use std::sync::atomic::Ordering::SeqCst;
        self.reads.store(reads, SeqCst);
        self.saves.store(saves, SeqCst);
        self.deletes.store(deletes, SeqCst);
    }
    /// Every operation fails.
    fn set_down(&self, d: bool) {
        self.set(!d, !d, !d);
    }
    fn ok(flag: &std::sync::atomic::AtomicBool, what: &str) -> Result<(), StoreError> {
        if flag.load(std::sync::atomic::Ordering::SeqCst) {
            Ok(())
        } else {
            Err(StoreError(format!("the store cannot {what}")))
        }
    }
}

impl CredentialStore for &Broken {
    fn load(&self, a: &str, l: &str) -> Result<Option<Credential>, StoreError> {
        Broken::ok(&self.reads, "read")?;
        self.inner.load(a, l)
    }
    fn save(&self, c: &Credential) -> Result<(), StoreError> {
        Broken::ok(&self.saves, "save")?;
        self.inner.save(c)
    }
    fn delete(&self, a: &str, l: &str) -> Result<(), StoreError> {
        Broken::ok(&self.deletes, "remove")?;
        self.inner.delete(a, l)
    }
}

fn with_secret(c: &Credential) -> Credential {
    let mut c = c.clone();
    c.secrets = vec![Secret::generate().unwrap()];
    c
}

/// The OS store refuses a save it can still read: the file takes over, and the newest save is always the copy loaded
/// (saves follow each other within the same millisecond here, as they did when this test failed at random).
#[test]
fn the_file_store_takes_over_when_the_os_store_fails_and_never_shadows_a_newer_copy() {
    let dir = temp_dir("fallback");
    let os = Broken::new();
    let both = FallbackStore::new(&os, FileStore::in_profile(&dir));
    let a = cred(AUTH, LOGIN);
    both.save(&a).unwrap();
    assert!(os.inner.load(AUTH, LOGIN).unwrap().is_some());
    assert!(
        !dir.join(STORE_FILE).exists(),
        "nothing on disk while the OS store works"
    );
    // the OS store refuses the save: the newer credential goes to the file, the OS copy is removed, the file's loads
    let b = with_secret(&a);
    os.set(true, false, true);
    both.save(&b).unwrap();
    os.set(true, true, true);
    assert!(os.inner.load(AUTH, LOGIN).unwrap().is_none());
    let got = both.load(AUTH, LOGIN).unwrap().unwrap();
    assert!(
        got.secrets[0].same_as(&b.secrets[0]),
        "the file's newer copy wins"
    );
    // the OS store works again: the next save lands there and the file copy is removed
    let c = with_secret(&b);
    both.save(&c).unwrap();
    assert!(!dir.join(STORE_FILE).exists());
    assert!(both.load(AUTH, LOGIN).unwrap().unwrap().secrets[0].same_as(&c.secrets[0]));
    // the OS store cannot be read: nothing is loaded or sent (fail closed)
    let mut wire = Wire(Arc::new(Mutex::new(Fake::new())));
    sign_in(
        &both,
        &mut wire,
        AUTH,
        LOGIN,
        "correct horse",
        &DeviceInfo::default(),
    )
    .unwrap();
    os.set_down(true);
    let before = wire.0.lock().unwrap().requests.len();
    assert!(matches!(
        refresh(&both, &mut wire, AUTH, LOGIN),
        Err(RefreshError::Store(_))
    ));
    assert_eq!(wire.0.lock().unwrap().requests.len(), before);
    os.set_down(false);
    // the OS store refuses saves: refreshes go through the file
    os.set(true, false, true);
    refresh(&both, &mut wire, AUTH, LOGIN).unwrap();
    os.set(true, true, true);
    refresh(&both, &mut wire, AUTH, LOGIN).unwrap();
    assert!(!wire.0.lock().unwrap().devices[0].revoked);
    both.delete(AUTH, LOGIN).unwrap();
    assert!(both.load(AUTH, LOGIN).unwrap().is_none());
    let _ = std::fs::remove_dir_all(&dir);
}

/// Saves in quick succession (many within one millisecond) alternating between the stores: every load returns the
/// last save and the revisions only grow. No clock is read, so a clock that steps back cannot reorder copies: a
/// copy stamped far above the present (as a clock running ahead, then set back, would have left it) is simply
/// followed by a higher revision.
#[test]
fn revisions_order_copies_without_a_clock() {
    let (os, file) = (Broken::new(), Broken::new());
    let both = FallbackStore::new(&os, &file);
    let mut c = cred(AUTH, LOGIN);
    let mut last_rev = 0;
    for i in 0..200 {
        c = with_secret(&c);
        os.set(true, i % 3 != 0, true);
        both.save(&c).unwrap();
        let got = both.load(AUTH, LOGIN).unwrap().unwrap();
        assert!(got.secrets[0].same_as(&c.secrets[0]), "save {i}");
        assert!(got.rev > last_rev, "save {i}");
        last_rev = got.rev;
        // exactly one store holds a copy after a successful save
        let held = [
            os.inner.load(AUTH, LOGIN).unwrap(),
            file.inner.load(AUTH, LOGIN).unwrap(),
        ];
        assert_eq!(held.iter().filter(|x| x.is_some()).count(), 1, "save {i}");
    }
    os.set(true, true, true);
    // a copy with a revision far above the present (an old clock-stamped one) is still followed
    let mut ahead = with_secret(&c);
    ahead.rev = 9_000_000_000_000;
    os.inner.save(&ahead).unwrap();
    let newer = with_secret(&ahead);
    both.save(&newer).unwrap();
    let got = both.load(AUTH, LOGIN).unwrap().unwrap();
    assert!(got.secrets[0].same_as(&newer.secrets[0]));
    assert_eq!(got.rev, 9_000_000_000_001);
    // the revision cannot wrap round
    let mut top = with_secret(&c);
    top.rev = u64::MAX;
    os.inner.save(&top).unwrap();
    assert!(both.save(&with_secret(&c)).is_err());
}

/// Two different copies with one revision are never resolved by a guess.
#[test]
fn equal_revisions_with_different_secrets_are_an_error() {
    let (os, file) = (Broken::new(), Broken::new());
    let both = FallbackStore::new(&os, &file);
    let mut a = cred(AUTH, LOGIN);
    a.rev = 7;
    let mut b = with_secret(&a);
    b.rev = 7;
    os.inner.save(&a).unwrap();
    file.inner.save(&b).unwrap();
    assert!(both.load(AUTH, LOGIN).is_err());
    // the same copy in both stores is fine
    file.inner.save(&a).unwrap();
    assert!(both.load(AUTH, LOGIN).unwrap().unwrap().secrets[0].same_as(&a.secrets[0]));
}

/// A copy that a save could not remove is never presented: the save fails (the refresh sends nothing), and if the
/// store holding the newer copy later cannot be read, load fails instead of returning the older one.
#[test]
fn a_copy_left_behind_is_never_presented() {
    let (os, file) = (Broken::new(), Broken::new());
    let both = FallbackStore::new(&os, &file);
    let mut wire = Wire(Arc::new(Mutex::new(Fake::new())));
    // the device's credential goes to the file (the OS store refuses), then one refresh through the file
    os.set(true, false, true);
    sign_in(
        &both,
        &mut wire,
        AUTH,
        LOGIN,
        "correct horse",
        &DeviceInfo::default(),
    )
    .unwrap();
    refresh(&both, &mut wire, AUTH, LOGIN).unwrap();
    os.set(true, true, true);
    // the next refresh: the new copy lands in the OS store, the file copy cannot be removed: the save fails and
    // nothing is sent
    file.set(true, true, false);
    let before = wire.0.lock().unwrap().requests.len();
    assert!(matches!(
        refresh(&both, &mut wire, AUTH, LOGIN),
        Err(RefreshError::Store(_))
    ));
    assert_eq!(
        wire.0.lock().unwrap().requests.len(),
        before,
        "nothing sent"
    );
    // both stores hold a copy now; the newer (the OS store's) is the one loaded
    assert!(os.inner.load(AUTH, LOGIN).unwrap().is_some());
    assert!(file.inner.load(AUTH, LOGIN).unwrap().is_some());
    let newest = both.load(AUTH, LOGIN).unwrap().unwrap();
    assert!(newest.rev > file.inner.load(AUTH, LOGIN).unwrap().unwrap().rev);
    // the OS store becomes unreadable: the older file copy is NOT returned
    os.set(false, true, true);
    assert!(both.load(AUTH, LOGIN).is_err());
    assert!(matches!(
        refresh(&both, &mut wire, AUTH, LOGIN),
        Err(RefreshError::Store(_))
    ));
    assert_eq!(
        wire.0.lock().unwrap().requests.len(),
        before,
        "still nothing sent"
    );
    // the file unreadable instead: the same
    os.set(true, true, true);
    file.set(false, true, true);
    assert!(both.load(AUTH, LOGIN).is_err());
    // both readable and removals working: the flow goes on from the newest copy, no secret was spent twice
    file.set(true, true, true);
    refresh(&both, &mut wire, AUTH, LOGIN).unwrap();
    refresh(&both, &mut wire, AUTH, LOGIN).unwrap();
    assert!(!wire.0.lock().unwrap().devices[0].revoked);
    assert!(
        file.inner.load(AUTH, LOGIN).unwrap().is_none(),
        "the left copy is gone"
    );
}

/// A damaged fallback file is "cannot be read": load fails closed even with a good OS copy (the damaged file may
/// have held a newer one).
#[test]
fn a_damaged_fallback_file_fails_closed() {
    let dir = temp_dir("fail-closed");
    std::fs::create_dir_all(&dir).unwrap();
    let os = Broken::new();
    let fs = FileStore::in_profile(&dir);
    std::fs::write(fs.path(), "not json").unwrap();
    let both = FallbackStore::new(&os, fs);
    let c = cred(AUTH, LOGIN);
    os.inner.save(&c).unwrap();
    assert!(both.load(AUTH, LOGIN).is_err());
    assert!(
        both.save(&with_secret(&c)).is_err(),
        "nothing is written either"
    );
    assert!(os.inner.load(AUTH, LOGIN).unwrap().unwrap().secrets[0].same_as(&c.secrets[0]));
    // removed by the player, the store works again
    std::fs::remove_file(dir.join(STORE_FILE)).unwrap();
    assert!(both.load(AUTH, LOGIN).unwrap().is_some());
    let _ = std::fs::remove_dir_all(&dir);
}

/// Signing out forgets the device even when its credential cannot be read, and reports a removal that failed.
#[test]
fn sign_out_removes_what_it_can_and_reports_what_it_cannot() {
    let (os, file) = (Broken::new(), Broken::new());
    let both = FallbackStore::new(&os, &file);
    let mut wire = Wire(Arc::new(Mutex::new(Fake::new())));
    sign_in(
        &both,
        &mut wire,
        AUTH,
        LOGIN,
        "correct horse",
        &DeviceInfo::default(),
    )
    .unwrap();
    // unreadable: nothing is sent, the local copy is still removed, the read error is reported
    os.set(false, true, true);
    assert!(sign_out(&both, &mut wire, AUTH, LOGIN).is_err());
    assert!(
        os.inner.load(AUTH, LOGIN).unwrap().is_none(),
        "removed anyway"
    );
    // readable but the removal fails: reported
    os.set(true, true, true);
    sign_in(
        &both,
        &mut wire,
        AUTH,
        LOGIN,
        "correct horse",
        &DeviceInfo::default(),
    )
    .unwrap();
    os.set(true, true, false);
    assert!(sign_out(&both, &mut wire, AUTH, LOGIN).is_err());
    os.set(true, true, true);
    sign_out(&both, &mut wire, AUTH, LOGIN).unwrap();
    assert!(both.load(AUTH, LOGIN).unwrap().is_none());
}

#[test]
fn the_default_store_of_a_profile_round_trips() {
    // on Windows this writes the Credential Manager: serialised with the other such tests, a workaround for a flake
    // whose root cause is unproven (see `win::TEST_SERIAL`)
    #[cfg(windows)]
    let _serial = crate::win::TEST_SERIAL
        .lock()
        .unwrap_or_else(|e| e.into_inner());
    let dir = temp_dir("default");
    let s = default_store(&dir);
    // a login of this test run only, so a real credential is never touched
    let login = format!("dc-session-test-{}", std::process::id());
    let c = cred("test.invalid:1", &login);
    s.save(&c).unwrap();
    assert_eq!(
        s.load("test.invalid:1", &login)
            .unwrap()
            .unwrap()
            .secrets
            .len(),
        2
    );
    s.delete("test.invalid:1", &login).unwrap();
    assert!(s.load("test.invalid:1", &login).unwrap().is_none());
    let _ = std::fs::remove_dir_all(&dir);
}

// ------------------------------------------------------------------------------------------------ recovery

/// The damaged-file backups next to the store file, with their contents.
fn backups(dir: &std::path::Path) -> Vec<Vec<u8>> {
    std::fs::read_dir(dir)
        .unwrap()
        .map(|e| e.unwrap().path())
        .filter(|p| {
            p.file_name()
                .unwrap()
                .to_string_lossy()
                .starts_with(&format!("{STORE_FILE}.damaged-"))
        })
        .map(|p| std::fs::read(p).unwrap())
        .collect()
}

/// A real damaged or oversized store file: a refresh still sends nothing, a sign-in moves the file aside (kept, as
/// it was) and keeps its new device, and the next launch refreshes silently.
#[test]
fn a_sign_in_recovers_from_a_damaged_store_file() {
    for (name, junk) in [
        (
            "garbage",
            b"{\"version\":1,\"devices\":[{\"login\":".to_vec(),
        ),
        ("oversized", vec![b' '; (1 << 20) + 10]),
    ] {
        let dir = temp_dir(&format!("recover-{name}"));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join(STORE_FILE), &junk).unwrap();
        let fs = FileStore::in_profile(&dir);
        let mut wire = Wire(Arc::new(Mutex::new(Fake::new())));
        assert!(matches!(
            refresh(&fs, &mut wire, AUTH, LOGIN),
            Err(RefreshError::Store(_))
        ));
        assert!(
            wire.0.lock().unwrap().requests.is_empty(),
            "{name}: nothing sent"
        );
        assert_eq!(
            std::fs::read(dir.join(STORE_FILE)).unwrap(),
            junk,
            "{name}: a refresh never clears"
        );
        let s = sign_in(
            &fs,
            &mut wire,
            AUTH,
            LOGIN,
            "correct horse",
            &DeviceInfo::default(),
        )
        .unwrap();
        assert!(s.store_error.is_none(), "{name}: {:?}", s.store_error);
        assert_eq!(
            backups(&dir),
            vec![junk.clone()],
            "{name}: the damaged file is kept aside"
        );
        // the next launch
        let next = FileStore::in_profile(&dir);
        refresh(&next, &mut wire, AUTH, LOGIN).unwrap();
        assert!(!wire.0.lock().unwrap().devices[0].revoked);
        let _ = std::fs::remove_dir_all(&dir);
    }
}

/// Signing out with a damaged store file removes it from the store (moved aside), sends nothing (the device id
/// cannot be read) and reports the read error.
#[test]
fn a_sign_out_removes_a_damaged_copy() {
    let dir = temp_dir("signout-damaged");
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join(STORE_FILE), "not json").unwrap();
    let fs = FileStore::in_profile(&dir);
    let mut wire = Wire(Arc::new(Mutex::new(Fake::new())));
    assert!(sign_out(&fs, &mut wire, AUTH, LOGIN).is_err());
    assert!(wire.0.lock().unwrap().requests.is_empty());
    assert!(
        fs.load(AUTH, LOGIN).unwrap().is_none(),
        "the store is usable again"
    );
    assert_eq!(backups(&dir), vec![b"not json".to_vec()]);
    let _ = std::fs::remove_dir_all(&dir);
}

/// A copy stamped with the highest revision blocks every save; a sign-in clears it and starts again.
#[test]
fn a_copy_with_the_highest_revision_is_recovered_by_a_sign_in() {
    let dir = temp_dir("rev-max");
    let os = Broken::new();
    let both = FallbackStore::new(&os, FileStore::in_profile(&dir));
    let mut top = cred(AUTH, LOGIN);
    top.rev = u64::MAX;
    FileStore::in_profile(&dir).save(&top).unwrap();
    assert!(
        both.save(&cred(AUTH, LOGIN)).is_err(),
        "no revision above it"
    );
    let mut wire = Wire(Arc::new(Mutex::new(Fake::new())));
    let s = sign_in(
        &both,
        &mut wire,
        AUTH,
        LOGIN,
        "correct horse",
        &DeviceInfo::default(),
    )
    .unwrap();
    assert!(s.store_error.is_none(), "{:?}", s.store_error);
    refresh(&both, &mut wire, AUTH, LOGIN).unwrap();
    assert!(both.load(AUTH, LOGIN).unwrap().unwrap().rev < 10);
    let _ = std::fs::remove_dir_all(&dir);
}

/// A sign-in and refreshes of two game instances on one profile take turns under the store's lock: no revision is
/// stamped twice (that would fail closed) and no spent secret is presented.
#[test]
fn a_sign_in_and_refreshes_of_two_instances_take_turns() {
    let dir = temp_dir("signin-race");
    let os = Broken::new();
    let wire = Wire(Arc::new(Mutex::new(Fake::new())));
    sign_in(
        &FallbackStore::new(&os, FileStore::in_profile(&dir)),
        &mut wire.clone(),
        AUTH,
        LOGIN,
        "correct horse",
        &DeviceInfo::default(),
    )
    .unwrap();
    std::thread::scope(|s| {
        let refresher = s.spawn(|| {
            let st = FallbackStore::new(&os, FileStore::in_profile(&dir));
            let mut w = wire.clone();
            (0..15)
                .filter(|_| refresh(&st, &mut w, AUTH, LOGIN).is_ok())
                .count()
        });
        let signer = s.spawn(|| {
            let st = FallbackStore::new(&os, FileStore::in_profile(&dir));
            let mut w = wire.clone();
            (0..5)
                .filter(|_| {
                    sign_in(
                        &st,
                        &mut w,
                        AUTH,
                        LOGIN,
                        "correct horse",
                        &DeviceInfo::default(),
                    )
                    .is_ok_and(|r| r.store_error.is_none())
                })
                .count()
        });
        assert_eq!(refresher.join().unwrap(), 15, "every refresh worked");
        assert_eq!(signer.join().unwrap(), 5, "every sign-in was kept");
    });
    let st = FallbackStore::new(&os, FileStore::in_profile(&dir));
    assert!(st.load(AUTH, LOGIN).unwrap().is_some());
    assert!(
        wire.0.lock().unwrap().devices.iter().all(|d| !d.revoked),
        "no secret was presented twice"
    );
    let _ = std::fs::remove_dir_all(&dir);
}

/// On Windows, a real damaged entry in the Credential Manager (under a prefix of this run only): a refresh sends
/// nothing, a sign-in clears it and is kept, the next launch refreshes, a sign-out removes a damaged entry.
#[cfg(windows)]
#[test]
fn a_damaged_credential_manager_entry_is_recovered() {
    let _serial = crate::win::TEST_SERIAL
        .lock()
        .unwrap_or_else(|e| e.into_inner());
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let win = crate::win::WinStore::with_prefix(&format!(
        "DeCombatTest-recover-{}-{nanos}",
        std::process::id()
    ));
    let dir = temp_dir("win-recover");
    let both = FallbackStore::new(win.clone(), FileStore::in_profile(&dir));
    for junk in [vec![0xffu8; 2560], b"{\"device\":".to_vec()] {
        win.write_raw(AUTH, LOGIN, &junk).unwrap();
        assert!(win.load(AUTH, LOGIN).is_err(), "the entry is damaged");
        let mut wire = Wire(Arc::new(Mutex::new(Fake::new())));
        assert!(matches!(
            refresh(&both, &mut wire, AUTH, LOGIN),
            Err(RefreshError::Store(_))
        ));
        assert!(wire.0.lock().unwrap().requests.is_empty());
        let s = sign_in(
            &both,
            &mut wire,
            AUTH,
            LOGIN,
            "correct horse",
            &DeviceInfo::default(),
        )
        .unwrap();
        assert!(s.store_error.is_none(), "{:?}", s.store_error);
        refresh(&both, &mut wire, AUTH, LOGIN).unwrap();
        // damaged again, then signed out: removed
        win.write_raw(AUTH, LOGIN, &junk).unwrap();
        assert!(sign_out(&both, &mut wire, AUTH, LOGIN).is_err());
        assert!(win.load(AUTH, LOGIN).unwrap().is_none());
    }
    let _ = win.delete(AUTH, LOGIN);
    let _ = std::fs::remove_dir_all(&dir);
}
