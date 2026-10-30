//! The persisted TLS identity (`<data_dir>/ls/cert.pem` + `key.pem`).

use anyhow::{Context, Result};
use localsend::crypto::cert::{fingerprint_from_cert_der, generate_self_signed};
use std::path::Path;

#[derive(Clone)]
pub(crate) struct Identity {
    pub cert_pem: String,
    pub key_pem: String,
    /// SHA-256 of the certificate DER, uppercase hex (LocalSend fingerprint).
    pub fingerprint: String,
}

pub(crate) fn fingerprint_of(cert_pem: &str) -> Option<String> {
    let p = pem::parse(cert_pem.as_bytes()).ok()?;
    (p.tag() == "CERTIFICATE").then(|| fingerprint_from_cert_der(p.contents()))
}

/// Loads the identity, or generates and saves a new one when missing/broken.
pub(crate) fn load_or_create(dir: &Path) -> Result<Identity> {
    let cert_path = dir.join("cert.pem");
    let key_path = dir.join("key.pem");
    if let (Ok(cert_pem), Ok(key_pem)) = (
        std::fs::read_to_string(&cert_path),
        std::fs::read_to_string(&key_path),
    ) {
        if let Some(fingerprint) = fingerprint_of(&cert_pem) {
            if pem::parse(key_pem.as_bytes()).is_ok() {
                return Ok(Identity {
                    cert_pem,
                    key_pem,
                    fingerprint,
                });
            }
        }
        tracing::warn!(
            "LocalSend identity in {} unreadable, generating a new one",
            dir.display()
        );
    }
    let c = generate_self_signed().context("tạo chứng chỉ LocalSend")?;
    std::fs::create_dir_all(dir).with_context(|| format!("tạo thư mục {}", dir.display()))?;
    write_private(&key_path, &c.private_key_pem)?;
    std::fs::write(&cert_path, &c.certificate_pem)
        .with_context(|| format!("lưu {}", cert_path.display()))?;
    Ok(Identity {
        cert_pem: c.certificate_pem,
        key_pem: c.private_key_pem,
        fingerprint: c.fingerprint,
    })
}

fn write_private(path: &Path, data: &str) -> Result<()> {
    #[cfg(unix)]
    {
        use std::io::Write;
        use std::os::unix::fs::OpenOptionsExt;
        let mut f = std::fs::OpenOptions::new()
            .create(true)
            .write(true)
            .truncate(true)
            .mode(0o600)
            .open(path)
            .with_context(|| format!("lưu {}", path.display()))?;
        f.write_all(data.as_bytes())?;
        Ok(())
    }
    #[cfg(not(unix))]
    {
        std::fs::write(path, data).with_context(|| format!("lưu {}", path.display()))
    }
}
