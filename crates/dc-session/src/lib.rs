//! Device sessions, client side: sign in once per device. Our own design, no 2009 counterpart.
//!
//! A password sign-in at the identity authority makes a *device*: the authority names it (`dv_...`) and the client
//! makes its first secret, sending only the secret's hash. From then on each launch *refreshes*: the client spends
//! its current secret and proposes the hash of the next one, and gets an ordinary short identity token back. The
//! token lives in memory only; the device id and the secrets live in a [`CredentialStore`].
//!
//! Rotation is client-proposed: the next secret is saved **before** the request leaves, so a crash or a lost answer
//! between "the authority rotated" and "the client saved" cannot lock the player out. The client keeps every secret
//! that may still be current, newest first, and tries them in that order ([`flow::refresh`]).
//!
//! The crate does no networking itself: the caller passes a [`Transport`] that posts a JSON body over its own TLS
//! connection to the authority. Storage is behind [`CredentialStore`]; [`FileStore`] is the portable one (an
//! owner-only file where the platform has permissions), [`MemoryStore`] keeps nothing on disk.
// DC-DEFAULT: SES-1 credential format, SES-2 client-proposed rotation, SES-8 the store, SES-9 what a refusal and an
// unreachable authority do

pub mod flow;
pub mod lock;
pub mod secret;
pub mod store;
#[cfg(windows)]
pub mod win;

pub use flow::{
    refresh, sign_in, sign_out, DeviceInfo, NetError, RefreshError, Reply, Session, SignIn,
    SignInError, Transport, DEVICE_PATH, REFRESH_PATH, SIGNOUT_PATH,
};
pub use secret::{Secret, SecretError};
pub use store::{
    default_store, Credential, CredentialStore, FallbackStore, FileStore, MemoryStore, StoreError,
    StoreLock, STORE_FILE,
};

#[cfg(test)]
mod tests;
