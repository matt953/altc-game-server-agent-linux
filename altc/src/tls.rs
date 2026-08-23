use std::{
    fs,
    path::{Path, PathBuf},
};

// Self-signed cert generated once, persisted so clients can trust-on-first-use.
pub fn ensure_tls_cert(dir: &Path) -> (PathBuf, PathBuf) {
    let cert_path = dir.join("api-cert.pem");
    let key_path = dir.join("api-key.pem");
    if !cert_path.exists() || !key_path.exists() {
        let ck = rcgen::generate_simple_self_signed(vec!["altc".into()])
            .expect("generate self-signed cert");
        fs::write(&cert_path, ck.cert.pem()).expect("write cert");
        fs::write(&key_path, ck.key_pair.serialize_pem()).expect("write key");
    }
    (cert_path, key_path)
}
