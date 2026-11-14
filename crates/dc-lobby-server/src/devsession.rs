//! Device sessions: sign in once per device. Our own design, no 2009 counterpart.
//!
//! A password sign-in on the identity endpoints can make a *device*: a server-made id (`dv_` + 22 base64url
//! characters, 16 random bytes) and a long-lived secret (32 random bytes) that the **client** makes and never sends
//! before it spends it. The server keeps only the SHA-256 of the secret it will accept next (`cur_hash`) and of the
//! one spent last (`prev_hash`). Each spend is a refresh: the client sends the secret and the hash of the next one it
//! has already saved, the server rotates the pair and issues an ordinary short identity token (LG-68). A spent
//! secret seen again is a copy in other hands: the device is revoked.
//!
//! [`db`] holds the table and its row operations; [`web`] the endpoints on the web port and the hourly sweep.
// DC-DEFAULT: SES-1 credential format and hashed storage; SES-2 client-proposed rotation and reuse revoke; SES-3 idle
// expiry and the per-account device cap

pub mod db;
pub mod web;
