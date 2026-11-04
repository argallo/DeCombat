// The admin web pages of dc-lobby-server (adminweb.rs, LG-387): sign-in, enrolment and key management with WebAuthn.
// Plain browser JavaScript, no other script. Every text from the server is drawn with textContent.
// Provenance: ORIGINAL
"use strict";
(() => {
  const $ = (id) => document.getElementById(id);
  const meta = document.querySelector('meta[name="csrf"]');
  const CSRF = meta ? meta.content : "";

  const b64u = {
    enc(buf) {
      const b = new Uint8Array(buf);
      let s = "";
      for (let i = 0; i < b.length; i++) s += String.fromCharCode(b[i]);
      return btoa(s).replace(/\+/g, "-").replace(/\//g, "_").replace(/=+$/, "");
    },
    dec(s) {
      s = s.replace(/-/g, "+").replace(/_/g, "/");
      while (s.length % 4) s += "=";
      const bin = atob(s);
      const b = new Uint8Array(bin.length);
      for (let i = 0; i < bin.length; i++) b[i] = bin.charCodeAt(i);
      return b.buffer;
    },
  };

  async function call(method, path, body) {
    const headers = {};
    const opt = { method, headers, credentials: "same-origin", cache: "no-store" };
    if (method === "POST") {
      headers["Content-Type"] = "application/json";
      if (CSRF) headers["X-Admin-CSRF"] = CSRF;
      opt.body = JSON.stringify(body || {});
    }
    const r = await fetch(path, opt);
    const j = await r.json().catch(() => ({ ok: false, message: "HTTP " + r.status }));
    if (r.status === 401) { location.href = "/login"; throw new Error("signed out"); }
    if (!r.ok || !j.ok) throw new Error(j.message || ("HTTP " + r.status));
    return j;
  }
  const post = (path, body) => call("POST", path, body);

  function say(id, text, ok) {
    const e = $(id);
    e.textContent = text;
    e.className = "msg " + (ok ? "ok" : "bad");
  }

  function why(e) {
    if (e && e.name === "NotAllowedError") return "Cancelled, timed out, or this key is not enrolled. Try again.";
    if (e && e.name === "InvalidStateError") return "This key is enrolled already.";
    if (e && e.name === "SecurityError") return "The browser refused: open this page at its https address.";
    return (e && e.message) || String(e);
  }

  function requestOptions(pk) {
    const o = Object.assign({}, pk);
    o.challenge = b64u.dec(pk.challenge);
    o.allowCredentials = (pk.allowCredentials || []).map((c) => Object.assign({}, c, { id: b64u.dec(c.id) }));
    return o;
  }

  function creationOptions(pk) {
    const o = Object.assign({}, pk);
    o.challenge = b64u.dec(pk.challenge);
    o.user = Object.assign({}, pk.user, { id: b64u.dec(pk.user.id) });
    o.excludeCredentials = (pk.excludeCredentials || []).map((c) => Object.assign({}, c, { id: b64u.dec(c.id) }));
    return o;
  }

  function assertionJSON(c) {
    const r = c.response;
    return {
      id: c.id, rawId: b64u.enc(c.rawId), type: c.type,
      response: {
        clientDataJSON: b64u.enc(r.clientDataJSON),
        authenticatorData: b64u.enc(r.authenticatorData),
        signature: b64u.enc(r.signature),
        userHandle: r.userHandle ? b64u.enc(r.userHandle) : null,
      },
    };
  }

  function attestationJSON(c) {
    const r = c.response;
    return {
      id: c.id, rawId: b64u.enc(c.rawId), type: c.type,
      response: {
        clientDataJSON: b64u.enc(r.clientDataJSON),
        attestationObject: b64u.enc(r.attestationObject),
        transports: typeof r.getTransports === "function" ? r.getTransports() : [],
      },
    };
  }

  // an assertion ceremony: /auth/login or /auth/stepup
  async function assertWith(prefix) {
    const o = await post(prefix + "/options", {});
    const c = await navigator.credentials.get({ publicKey: requestOptions(o.publicKey) });
    return post(prefix + "/finish", { ceremony: o.ceremony, credential: assertionJSON(c) });
  }

  // a registration ceremony: /auth/enroll (with the link's token) or /keys/enroll (after a confirmation)
  async function createWith(prefix, extra) {
    const o = await post(prefix + "/options", extra);
    const c = await navigator.credentials.create({ publicKey: creationOptions(o.publicKey) });
    return post(prefix + "/finish", Object.assign({ ceremony: o.ceremony, credential: attestationJSON(c) }, extra));
  }

  function busy(btn, on) { btn.disabled = on; }

  function needWebAuthn(msgId) {
    if (window.PublicKeyCredential && navigator.credentials) return true;
    say(msgId, "This browser cannot use security keys here (WebAuthn is missing).", false);
    return false;
  }

  function loginPage() {
    $("login").hidden = false;
    const go = $("login-go");
    go.addEventListener("click", async () => {
      if (!needWebAuthn("login-msg")) return;
      busy(go, true);
      say("login-msg", "Waiting for the security key...", true);
      try {
        const r = await assertWith("/auth/login");
        location.href = r.redirect || "/ops";
      } catch (e) {
        say("login-msg", why(e), false);
      } finally {
        busy(go, false);
      }
    });
  }

  function enrollPage() {
    $("enroll").hidden = false;
    const token = new URLSearchParams(location.search).get("t") || "";
    // the token stays in this page only, not in the history
    if (location.search) history.replaceState(null, "", "/enroll");
    if (!token) say("enroll-msg", "This page needs the link from `dc-lobby-server admin-web enroll`.", false);
    const go = $("enroll-go");
    go.addEventListener("click", async () => {
      if (!needWebAuthn("enroll-msg")) return;
      busy(go, true);
      say("enroll-msg", "Waiting for the security key...", true);
      try {
        const r = await createWith("/auth/enroll", { token, label: $("enroll-label").value });
        say("enroll-msg", r.message || "Enrolled.", true);
        $("enroll-next").hidden = false;
        go.hidden = true;
      } catch (e) {
        say("enroll-msg", why(e), false);
        busy(go, false);
      }
    });
  }

  const day = (t) => (t ? new Date(t * 1000).toISOString().replace("T", " ").slice(0, 16) + " UTC" : "never");

  function keysPage() {
    $("keys").hidden = false;
    const rows = $("key-rows");
    async function load() {
      let j;
      try { j = await call("GET", "/keys/list"); } catch (e) { say("keys-msg", why(e), false); return; }
      rows.textContent = "";
      const only = j.keys.length <= 1;
      for (const k of j.keys) {
        const tr = document.createElement("tr");
        const cell = (text) => { const td = document.createElement("td"); td.textContent = text; tr.append(td); return td; };
        cell(String(k.id));
        const nameTd = document.createElement("td");
        const name = document.createElement("input");
        name.value = k.label;
        name.maxLength = 40;
        const ren = document.createElement("button");
        ren.textContent = "Rename";
        ren.addEventListener("click", async () => {
          try { await post("/keys/rename", { id: k.id, label: name.value }); say("keys-msg", "Renamed.", true); load(); }
          catch (e) { say("keys-msg", why(e), false); }
        });
        nameTd.append(name, " ", ren);
        if (k.current) { const m = document.createElement("span"); m.className = "muted"; m.textContent = " (this session)"; nameTd.append(m); }
        tr.append(nameTd);
        cell(k.aaguid + " " + k.alg);
        cell(k.root);
        cell(day(k.created));
        cell(day(k.last_used));
        const act = document.createElement("td");
        const rm = document.createElement("button");
        rm.className = "danger";
        rm.textContent = "Remove";
        rm.disabled = only;
        rm.title = only ? "The last key cannot be removed" : "";
        rm.addEventListener("click", async () => {
          if (!confirm("Remove key #" + k.id + " (" + k.label + ")? Confirm with a key you keep first.")) return;
          try {
            await assertWith("/auth/stepup");
            const r = await post("/keys/remove", { id: k.id });
            if (r.signed_out) { location.href = "/login"; return; }
            say("keys-msg", "Removed.", true);
            load();
          } catch (e) { say("keys-msg", why(e), false); }
        });
        act.append(rm);
        tr.append(act);
        rows.append(tr);
      }
      $("add-go").disabled = j.keys.length >= j.max_keys;
    }
    $("add-go").addEventListener("click", async () => {
      if (!needWebAuthn("keys-msg")) return;
      const go = $("add-go");
      busy(go, true);
      try {
        say("keys-msg", "First confirm with a key you already have...", true);
        await assertWith("/auth/stepup");
        say("keys-msg", "Now plug in the new key and touch it...", true);
        const r = await createWith("/keys/enroll", { label: $("add-label").value });
        say("keys-msg", r.message || "Enrolled.", true);
        $("add-label").value = "";
      } catch (e) {
        say("keys-msg", why(e), false);
      } finally {
        busy(go, false);
        load();
      }
    });
    $("logout").addEventListener("click", async () => {
      try { await post("/logout", {}); } catch (e) { /* signed out either way */ }
      location.href = "/login";
    });
    load();
  }

  const which = document.body.dataset.page;
  if (which === "login") loginPage();
  else if (which === "enroll") enrollPage();
  else if (which === "keys") keysPage();
})();
