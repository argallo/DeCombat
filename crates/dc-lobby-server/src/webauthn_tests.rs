//! Tests of webauthn.rs, and the software security key the admin web tests use: a P-256 (or Ed25519) credential
//! whose `packed` attestation is signed by a leaf certificate of a TEST root made here with rcgen.
// Provenance: ORIGINAL (DeCombat test code)

use super::*;
use ring::rand::SystemRandom;
use ring::signature::{EcdsaKeyPair, Ed25519KeyPair, KeyPair as _, ECDSA_P256_SHA256_ASN1_SIGNING};

pub const RP: &str = "admin.example.com";
pub const ORIGIN: &str = "https://admin.example.com";
pub const TEST_AAGUID: [u8; 16] = [0x7e, 0x57, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14];

/// A test root CA.
pub struct TestCa {
    pub cert: rcgen::Certificate,
    pub key: rcgen::KeyPair,
}

impl TestCa {
    pub fn new(name: &str) -> TestCa {
        let key = rcgen::KeyPair::generate_for(&rcgen::PKCS_ECDSA_P256_SHA256).unwrap();
        let mut p = rcgen::CertificateParams::new(Vec::<String>::new()).unwrap();
        p.distinguished_name = rcgen::DistinguishedName::new();
        p.distinguished_name.push(rcgen::DnType::CommonName, name);
        p.is_ca = rcgen::IsCa::Ca(rcgen::BasicConstraints::Unconstrained);
        p.key_usages = vec![
            rcgen::KeyUsagePurpose::KeyCertSign,
            rcgen::KeyUsagePurpose::CrlSign,
        ];
        let cert = p.self_signed(&key).unwrap();
        TestCa { cert, key }
    }

    pub fn der(&self) -> Vec<u8> {
        self.cert.der().to_vec()
    }

    /// A trust list holding only this root.
    pub fn trust(&self) -> Trust {
        Trust {
            roots: vec![self.der()],
            intermediates: vec![],
        }
    }
}

/// An attestation leaf of `ca`: its DER and the PKCS#8 of its P-256 key.
pub fn leaf_for(ca: &TestCa, ou: &str, aaguid_ext: Option<[u8; 16]>) -> (Vec<u8>, Vec<u8>) {
    let key = rcgen::KeyPair::generate_for(&rcgen::PKCS_ECDSA_P256_SHA256).unwrap();
    let mut p = rcgen::CertificateParams::new(Vec::<String>::new()).unwrap();
    p.distinguished_name = rcgen::DistinguishedName::new();
    p.distinguished_name
        .push(rcgen::DnType::OrganizationName, "Test Keys AB");
    p.distinguished_name
        .push(rcgen::DnType::OrganizationalUnitName, ou);
    p.distinguished_name
        .push(rcgen::DnType::CommonName, "Test EE Serial 1");
    p.is_ca = rcgen::IsCa::ExplicitNoCa;
    if let Some(a) = aaguid_ext {
        let mut v = vec![0x04, 0x10];
        v.extend_from_slice(&a);
        p.custom_extensions = vec![rcgen::CustomExtension::from_oid_content(
            &[1, 3, 6, 1, 4, 1, 45724, 1, 1, 4],
            v,
        )];
    }
    let cert = p.signed_by(&key, &ca.cert, &ca.key).unwrap();
    (cert.der().to_vec(), key.serialize_der())
}

enum Cred {
    Es(EcdsaKeyPair),
    Ed(Ed25519KeyPair),
}

/// A software security key.
pub struct SoftKey {
    pub aaguid: [u8; 16],
    pub cred_id: Vec<u8>,
    pub counter: u32,
    /// Flags of the next answer (UP | UV by default).
    pub flags: u8,
    pub fmt: &'static str,
    /// Leave x5c out (self attestation).
    pub self_attest: bool,
    pub leaf_der: Vec<u8>,
    att_pkcs8: Vec<u8>,
    cred: Cred,
}

pub fn client_data(kind: &str, challenge: &[u8], origin: &str) -> Vec<u8> {
    serde_json::json!({"type": kind, "challenge": b64::url(challenge), "origin": origin, "crossOrigin": false})
        .to_string()
        .into_bytes()
}

impl SoftKey {
    /// An ES256 credential attested by a leaf of `ca`.
    pub fn new(ca: &TestCa) -> SoftKey {
        let rng = SystemRandom::new();
        let pk = EcdsaKeyPair::generate_pkcs8(&ECDSA_P256_SHA256_ASN1_SIGNING, &rng).unwrap();
        let cred =
            EcdsaKeyPair::from_pkcs8(&ECDSA_P256_SHA256_ASN1_SIGNING, pk.as_ref(), &rng).unwrap();
        let (leaf_der, att_pkcs8) = leaf_for(ca, "Authenticator Attestation", Some(TEST_AAGUID));
        let mut id = [0u8; 32];
        ring::rand::SecureRandom::fill(&rng, &mut id).unwrap();
        SoftKey {
            aaguid: TEST_AAGUID,
            cred_id: id.to_vec(),
            counter: 0,
            flags: FLAG_UP | FLAG_UV,
            fmt: "packed",
            self_attest: false,
            leaf_der,
            att_pkcs8,
            cred: Cred::Es(cred),
        }
    }

    /// An EdDSA credential attested by a leaf of `ca`.
    pub fn new_eddsa(ca: &TestCa) -> SoftKey {
        let rng = SystemRandom::new();
        let pk = Ed25519KeyPair::generate_pkcs8(&rng).unwrap();
        let mut k = SoftKey::new(ca);
        k.cred = Cred::Ed(Ed25519KeyPair::from_pkcs8(pk.as_ref()).unwrap());
        k
    }

    pub fn cose(&self) -> Vec<u8> {
        let i = Cbor::Int;
        let m = match &self.cred {
            Cred::Es(k) => {
                let p = k.public_key().as_ref();
                vec![
                    (i(1), i(2)),
                    (i(3), i(-7)),
                    (i(-1), i(1)),
                    (i(-2), Cbor::Bytes(p[1..33].to_vec())),
                    (i(-3), Cbor::Bytes(p[33..65].to_vec())),
                ]
            }
            Cred::Ed(k) => vec![
                (i(1), i(1)),
                (i(3), i(-8)),
                (i(-1), i(6)),
                (i(-2), Cbor::Bytes(k.public_key().as_ref().to_vec())),
            ],
        };
        cbor_encode(&Cbor::Map(m))
    }

    fn auth_data(&self, rp_id: &str, attested: bool) -> Vec<u8> {
        let mut ad = sha256(rp_id.as_bytes()).to_vec();
        ad.push(self.flags | if attested { FLAG_AT } else { 0 });
        ad.extend_from_slice(&self.counter.to_be_bytes());
        if attested {
            ad.extend_from_slice(&self.aaguid);
            ad.extend_from_slice(&(self.cred_id.len() as u16).to_be_bytes());
            ad.extend_from_slice(&self.cred_id);
            ad.extend_from_slice(&self.cose());
        }
        ad
    }

    fn sign_cred(&self, msg: &[u8]) -> Vec<u8> {
        match &self.cred {
            Cred::Es(k) => k.sign(&SystemRandom::new(), msg).unwrap().as_ref().to_vec(),
            Cred::Ed(k) => k.sign(msg).as_ref().to_vec(),
        }
    }

    /// The answer to `navigator.credentials.create`: (clientDataJSON, attestationObject).
    pub fn create(&mut self, rp_id: &str, origin: &str, challenge: &[u8]) -> (Vec<u8>, Vec<u8>) {
        let cdj = client_data("webauthn.create", challenge, origin);
        let ad = self.auth_data(rp_id, true);
        let mut signed = ad.clone();
        signed.extend_from_slice(&sha256(&cdj));
        let rng = SystemRandom::new();
        let att = EcdsaKeyPair::from_pkcs8(&ECDSA_P256_SHA256_ASN1_SIGNING, &self.att_pkcs8, &rng)
            .unwrap();
        let t = |s: &str| Cbor::Text(s.into());
        let stmt = if self.fmt == "none" {
            Cbor::Map(vec![])
        } else if self.self_attest {
            Cbor::Map(vec![
                (t("alg"), Cbor::Int(-7)),
                (t("sig"), Cbor::Bytes(self.sign_cred(&signed))),
            ])
        } else {
            Cbor::Map(vec![
                (t("alg"), Cbor::Int(-7)),
                (
                    t("sig"),
                    Cbor::Bytes(att.sign(&rng, &signed).unwrap().as_ref().to_vec()),
                ),
                (
                    t("x5c"),
                    Cbor::Array(vec![Cbor::Bytes(self.leaf_der.clone())]),
                ),
            ])
        };
        let ao = Cbor::Map(vec![
            (t("fmt"), t(self.fmt)),
            (t("attStmt"), stmt),
            (t("authData"), Cbor::Bytes(ad)),
        ]);
        (cdj, cbor_encode(&ao))
    }

    /// The answer to `navigator.credentials.get`: (clientDataJSON, authenticatorData, signature). The counter grows.
    pub fn get(
        &mut self,
        rp_id: &str,
        origin: &str,
        challenge: &[u8],
    ) -> (Vec<u8>, Vec<u8>, Vec<u8>) {
        self.counter = self.counter.wrapping_add(1);
        let cdj = client_data("webauthn.get", challenge, origin);
        let ad = self.auth_data(rp_id, false);
        let mut signed = ad.clone();
        signed.extend_from_slice(&sha256(&cdj));
        let sig = self.sign_cred(&signed);
        (cdj, ad, sig)
    }
}

fn now() -> i64 {
    crate::clock::unix_now()
}

const CH: &[u8] = b"0123456789abcdef0123456789abcdef";

fn exp() -> Expect<'static> {
    Expect {
        rp_id: RP,
        origin: ORIGIN,
        challenge: CH,
    }
}

#[test]
fn base64_round_trips_and_refuses_junk() {
    for n in 0..40u8 {
        let d: Vec<u8> = (0..n).map(|i| i.wrapping_mul(37)).collect();
        assert_eq!(b64::decode(&b64::url(&d)).unwrap(), d);
        assert_eq!(b64::decode(&b64::std(&d)).unwrap(), d);
    }
    assert_eq!(b64::std(b"hi"), "aGk=");
    assert_eq!(b64::url(&[0xfb, 0xff]), "-_8");
    assert!(b64::decode("a").is_none());
    assert!(b64::decode("ab!c").is_none());
    assert!(b64::decode("aGk=x").is_none());
}

#[test]
fn cbor_reads_what_it_writes_and_refuses_the_rest() {
    let v = Cbor::Map(vec![
        (Cbor::Int(1), Cbor::Int(-7)),
        (Cbor::Text("x".into()), Cbor::Bytes(vec![1; 300])),
        (
            Cbor::Int(70000),
            Cbor::Array(vec![Cbor::Bool(true), Cbor::Null, Cbor::Int(-70000)]),
        ),
    ]);
    let b = cbor_encode(&v);
    assert_eq!(cbor_decode_all(&b).unwrap(), v);
    assert!(cbor_decode_all(&b[..b.len() - 1]).is_err(), "truncated");
    let mut extra = b.clone();
    extra.push(0);
    assert!(cbor_decode_all(&extra).is_err(), "trailing byte");
    assert!(cbor_decode_all(&[0x5f]).is_err(), "indefinite bytes");
    assert!(
        cbor_decode_all(&[0xfb, 0, 0, 0, 0, 0, 0, 0, 0]).is_err(),
        "float"
    );
    assert!(cbor_decode_all(&[0xc0, 0x00]).is_err(), "tag");
    assert!(
        cbor_decode_all(&[0xa2, 0x01, 0x01, 0x01, 0x02]).is_err(),
        "duplicate key"
    );
    assert!(
        cbor_decode_all(&[0x9b, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff]).is_err(),
        "huge array"
    );
    assert!(
        cbor_decode_all(&[0x5b, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff]).is_err(),
        "huge bytes"
    );
    assert!(cbor_decode_all(&[0x81; 40]).is_err(), "too deep");
    let many = |n: usize| cbor_encode(&Cbor::Array(vec![Cbor::Int(0); n]));
    assert!(cbor_decode_all(&many(64)).is_ok());
    assert!(cbor_decode_all(&many(65)).is_err(), "too many items");
}

#[test]
fn the_embedded_yubico_certificates_parse_and_chain_to_their_roots() {
    let t = Trust::yubico();
    assert_eq!((t.roots.len(), t.intermediates.len()), (3, 5));
    let fp: Vec<String> = t.roots.iter().map(|d| hex(&sha256(d))).collect();
    assert!(fp
        .contains(&"0fa1386f80eb8713263ae5c1d84deb455bdf08aea50ab05503cefee82b092d42".to_string()));
    assert!(fp
        .contains(&"35f1a54b353bfb711e6d42adbeb76c0e9dead095018e6a94783ba2192fd6faad".to_string()));
    assert!(fp
        .contains(&"62760c6a6ef91679f454c8902b80fd009825b3f25da90f1fbace2ec6586cd5a8".to_string()));
    // the roots' own signatures (RSA with SHA-256 and SHA-512) check out with the code the chains use
    for d in &t.roots {
        let c = parse_cert(d).unwrap();
        assert!(signed_by(&c, &c), "{}", cn(&c));
    }
    // every FIDO attestation intermediate reaches Yubico Attestation Root 1 through the published ones
    let mut reached = 0;
    for d in &t.intermediates {
        let c = parse_cert(d).unwrap();
        if cn(&c).starts_with("Yubico FIDO Attestation") {
            assert_eq!(
                chain_to_root(&c, &[], &t, now()).unwrap(),
                "Yubico Attestation Root 1"
            );
            reached += 1;
        }
    }
    assert_eq!(reached, 3);
    // and nothing made by a test root reaches them
    let ca = TestCa::new("Test FIDO Root");
    let (leaf, _) = leaf_for(&ca, "Authenticator Attestation", None);
    assert!(chain_to_root(&parse_cert(&leaf).unwrap(), &[], &t, now()).is_err());
}

#[test]
fn a_packed_attestation_from_a_trusted_root_registers() {
    let ca = TestCa::new("Test FIDO Root");
    for mut k in [SoftKey::new(&ca), SoftKey::new_eddsa(&ca)] {
        let (cdj, ao) = k.create(RP, ORIGIN, CH);
        let c = verify_registration(&cdj, &ao, &exp(), &ca.trust(), now()).unwrap();
        assert_eq!(c.cred_id, k.cred_id);
        assert_eq!(c.public_key, k.cose());
        assert_eq!(c.aaguid, TEST_AAGUID);
        assert_eq!(c.root, "Test FIDO Root");
        assert!(c.alg == ALG_ES256 || c.alg == ALG_EDDSA);
        // and its assertions verify with the stored key
        let (cdj, ad, sig) = k.get(RP, ORIGIN, CH);
        assert_eq!(
            verify_assertion(&cdj, &ad, &sig, &exp(), &c.public_key, 0).unwrap(),
            1
        );
    }
}

#[test]
fn a_chain_through_an_intermediate_registers() {
    let root = TestCa::new("Test FIDO Root");
    // an intermediate signed by the root, then the leaf signed by the intermediate
    let ikey = rcgen::KeyPair::generate_for(&rcgen::PKCS_ECDSA_P256_SHA256).unwrap();
    let mut p = rcgen::CertificateParams::new(Vec::<String>::new()).unwrap();
    p.distinguished_name = rcgen::DistinguishedName::new();
    p.distinguished_name
        .push(rcgen::DnType::CommonName, "Test FIDO Intermediate");
    p.is_ca = rcgen::IsCa::Ca(rcgen::BasicConstraints::Constrained(0));
    p.key_usages = vec![rcgen::KeyUsagePurpose::KeyCertSign];
    let icert = p.signed_by(&ikey, &root.cert, &root.key).unwrap();
    let inter = TestCa {
        cert: icert,
        key: ikey,
    };
    let mut k = SoftKey::new(&inter);
    let (cdj, ao) = k.create(RP, ORIGIN, CH);
    // the intermediate is in the trust pool (as Yubico's are), not in x5c
    let t = Trust {
        roots: vec![root.der()],
        intermediates: vec![inter.der()],
    };
    assert_eq!(
        verify_registration(&cdj, &ao, &exp(), &t, now())
            .unwrap()
            .root,
        "Test FIDO Root"
    );
    // without it there is no path
    assert!(verify_registration(&cdj, &ao, &exp(), &root.trust(), now()).is_err());
}

#[test]
fn registrations_that_are_not_attested_hardware_keys_are_refused() {
    let ca = TestCa::new("Test FIDO Root");
    let reg = |k: &mut SoftKey, t: &Trust| {
        let (cdj, ao) = k.create(RP, ORIGIN, CH);
        verify_registration(&cdj, &ao, &exp(), t, now())
    };
    let mut k = SoftKey::new(&ca);
    k.fmt = "none";
    assert!(reg(&mut k, &ca.trust())
        .unwrap_err()
        .contains("no attestation"));
    let mut k = SoftKey::new(&ca);
    k.self_attest = true;
    assert!(reg(&mut k, &ca.trust())
        .unwrap_err()
        .contains("self attestation"));
    let mut k = SoftKey::new(&ca);
    k.fmt = "apple";
    assert!(reg(&mut k, &ca.trust())
        .unwrap_err()
        .contains("built-in passkey"));
    let mut k = SoftKey::new(&ca);
    k.fmt = "fido-u2f";
    assert!(reg(&mut k, &ca.trust()).unwrap_err().contains("U2F"));
    // an untrusted root (another test root, and Yubico's real list)
    let other = TestCa::new("Other Root");
    let mut k = SoftKey::new(&ca);
    assert!(reg(&mut k, &other.trust())
        .unwrap_err()
        .contains("Yubico root"));
    assert!(reg(&mut k, &Trust::yubico())
        .unwrap_err()
        .contains("Yubico root"));
    // a synced passkey, a key without PIN verification
    let mut k = SoftKey::new(&ca);
    k.flags |= FLAG_BE;
    assert!(reg(&mut k, &ca.trust())
        .unwrap_err()
        .contains("synced passkey"));
    let mut k = SoftKey::new(&ca);
    k.flags = FLAG_UP;
    assert!(reg(&mut k, &ca.trust()).unwrap_err().contains("PIN"));
    // a leaf without the attestation OU, a leaf naming another model
    let mut k = SoftKey::new(&ca);
    (k.leaf_der, k.att_pkcs8) = leaf_for(&ca, "Something Else", Some(TEST_AAGUID));
    assert!(reg(&mut k, &ca.trust())
        .unwrap_err()
        .contains("not an authenticator attestation"));
    let mut k = SoftKey::new(&ca);
    (k.leaf_der, k.att_pkcs8) = leaf_for(&ca, "Authenticator Attestation", Some([9; 16]));
    assert!(reg(&mut k, &ca.trust()).unwrap_err().contains("AAGUID"));
    // a signature by another key than the leaf's
    let mut k = SoftKey::new(&ca);
    let (_, other_pkcs8) = leaf_for(&ca, "Authenticator Attestation", None);
    k.att_pkcs8 = other_pkcs8;
    assert!(reg(&mut k, &ca.trust())
        .unwrap_err()
        .contains("attestation signature"));
    // wrong origin, rp id, challenge, type
    let mut k = SoftKey::new(&ca);
    let (cdj, ao) = k.create(RP, "https://evil.example.com", CH);
    assert!(verify_registration(&cdj, &ao, &exp(), &ca.trust(), now())
        .unwrap_err()
        .contains("origin"));
    let (cdj, ao) = k.create("evil.example.com", ORIGIN, CH);
    assert!(verify_registration(&cdj, &ao, &exp(), &ca.trust(), now())
        .unwrap_err()
        .contains("relying-party"));
    let (cdj, ao) = k.create(RP, ORIGIN, b"another challenge");
    assert!(verify_registration(&cdj, &ao, &exp(), &ca.trust(), now())
        .unwrap_err()
        .contains("challenge"));
    let (_, ao) = k.create(RP, ORIGIN, CH);
    let cdj = client_data("webauthn.get", CH, ORIGIN);
    assert!(verify_registration(&cdj, &ao, &exp(), &ca.trust(), now()).is_err());
}

#[test]
fn assertions_check_origin_rp_challenge_flags_signature_and_counter() {
    let ca = TestCa::new("Test FIDO Root");
    let mut k = SoftKey::new(&ca);
    let (cdj, ao) = k.create(RP, ORIGIN, CH);
    let c = verify_registration(&cdj, &ao, &exp(), &ca.trust(), now()).unwrap();
    let pk = c.public_key.clone();
    let check = |a: (Vec<u8>, Vec<u8>, Vec<u8>), stored: u32| {
        verify_assertion(&a.0, &a.1, &a.2, &exp(), &pk, stored)
    };
    assert!(check(k.get(RP, "https://evil.example.com", CH), 0)
        .unwrap_err()
        .contains("origin"));
    assert!(check(k.get("evil.example.com", ORIGIN, CH), 0)
        .unwrap_err()
        .contains("relying-party"));
    assert!(check(k.get(RP, ORIGIN, b"old challenge"), 0)
        .unwrap_err()
        .contains("challenge"));
    k.flags = FLAG_UP;
    assert!(check(k.get(RP, ORIGIN, CH), 0).unwrap_err().contains("PIN"));
    k.flags = FLAG_UV;
    assert!(check(k.get(RP, ORIGIN, CH), 0)
        .unwrap_err()
        .contains("touch"));
    k.flags = FLAG_UP | FLAG_UV;
    // a tampered signature
    let (cdj, ad, mut sig) = k.get(RP, ORIGIN, CH);
    let n = sig.len();
    sig[n - 1] ^= 1;
    assert!(verify_assertion(&cdj, &ad, &sig, &exp(), &pk, 0).is_err());
    // the counter: it must grow; a regression or a repeat is refused; 0 and 0 is a key without a counter
    let a = k.get(RP, ORIGIN, CH);
    let got = k.counter;
    assert_eq!(check(a.clone(), got - 1).unwrap(), got);
    assert!(check(a.clone(), got).unwrap_err().contains("counter"));
    assert!(check(a, got + 5).unwrap_err().contains("counter"));
    k.counter = u32::MAX; // the next get wraps to 0: a key without a counter
    let a = k.get(RP, ORIGIN, CH);
    assert_eq!(check(a, 0).unwrap(), 0);
    // another key's signature
    let mut other = SoftKey::new(&ca);
    let a = other.get(RP, ORIGIN, CH);
    assert!(check(a, 0).unwrap_err().contains("signature"));
}

#[test]
fn authenticator_data_must_be_exact() {
    let ca = TestCa::new("Test FIDO Root");
    let k = SoftKey::new(&ca);
    let mut ad = k.auth_data(RP, true);
    assert!(parse_auth_data(&ad).is_ok());
    ad.push(0);
    assert!(parse_auth_data(&ad).is_err());
    assert!(parse_auth_data(&k.auth_data(RP, false)[..36]).is_err());
    let mut flagged = k.auth_data(RP, false);
    flagged[32] |= FLAG_AT;
    assert!(parse_auth_data(&flagged).is_err());
}

#[test]
fn pem_text_with_comments_is_read() {
    let ca = TestCa::new("Test FIDO Root");
    let pem = format!("# a comment\n{}\nnot a cert\n", ca.cert.pem());
    assert_eq!(pem_certs(&pem), vec![ca.der()]);
}
