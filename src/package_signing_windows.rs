use autokeyboardlayot::{
    language_package::PackageTrust,
    package_signing::{PreparedSigningInput, SigningKind},
};
use ed25519_dalek::{Signer, SigningKey};
use std::{
    fs::{File, OpenOptions},
    io::{Read, Write},
    os::windows::fs::{MetadataExt, OpenOptionsExt},
    path::{Path, PathBuf},
    time::{SystemTime, UNIX_EPOCH},
};
use windows::Win32::{
    Foundation::{HLOCAL, LocalFree},
    Security::Cryptography::{CRYPT_INTEGER_BLOB, CRYPTPROTECT_UI_FORBIDDEN, CryptUnprotectData},
    System::Com::CoTaskMemFree,
    UI::Shell::{FOLDERID_LocalAppData, KF_FLAG_DEFAULT, SHGetKnownFolderPath},
};
use zeroize::{Zeroize, Zeroizing};

type Result<T> = std::result::Result<T, &'static str>;
const REPARSE: u32 = 0x400;
const OPEN_REPARSE_POINT: u32 = 0x00200000;

fn now() -> Result<u64> {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .map_err(|_| "clock")
}

fn reject_reparse(path: &Path) -> Result<()> {
    if !path.is_absolute() {
        return Err("absolute path required");
    }
    for part in path.ancestors() {
        let metadata = std::fs::symlink_metadata(part).map_err(|_| "path metadata")?;
        if metadata.file_attributes() & REPARSE != 0 {
            return Err("reparse path refused");
        }
    }
    Ok(())
}

fn read_bounded(path: &Path, limit: usize) -> Result<Vec<u8>> {
    reject_reparse(path)?;
    let file = OpenOptions::new()
        .read(true)
        .share_mode(1)
        .custom_flags(OPEN_REPARSE_POINT)
        .open(path)
        .map_err(|_| "open input")?;
    let metadata = file.metadata().map_err(|_| "input metadata")?;
    if !metadata.is_file()
        || metadata.file_attributes() & REPARSE != 0
        || metadata.len() > limit as u64
    {
        return Err("input type or size");
    }
    let mut bytes = Vec::new();
    file.take(limit as u64 + 1)
        .read_to_end(&mut bytes)
        .map_err(|_| "read input")?;
    if bytes.len() > limit {
        return Err("input size");
    }
    Ok(bytes)
}

// Windows owns this allocation. All success/error paths wipe it before freeing.
struct Decrypted(CRYPT_INTEGER_BLOB);
impl Drop for Decrypted {
    fn drop(&mut self) {
        if !self.0.pbData.is_null() {
            unsafe {
                std::slice::from_raw_parts_mut(self.0.pbData, self.0.cbData as usize).zeroize();
                let _ = LocalFree(Some(HLOCAL(self.0.pbData.cast())));
            }
        }
    }
}

fn local_app_data() -> Result<PathBuf> {
    let ptr = unsafe { SHGetKnownFolderPath(&FOLDERID_LocalAppData, KF_FLAG_DEFAULT, None) }
        .map_err(|_| "local app data")?;
    let path = unsafe { ptr.to_string() };
    unsafe { CoTaskMemFree(Some(ptr.0.cast())) };
    path.map(PathBuf::from)
        .map_err(|_| "local app data encoding")
}

fn load_key(signer: &str, public: &[u8; 32], public_hex: &str) -> Result<SigningKey> {
    let path = local_app_data()?
        .join("AutoKeyboardLayot-Signing")
        .join(signer)
        .join("private.pkcs8.dpapi");
    let mut ciphertext = read_bounded(&path, 4096)?;
    let mut entropy =
        format!("AutoKeyboardLayot.package-signing.dpapi.v1\0{signer}\0{public_hex}").into_bytes();
    let decrypted = decrypt(&mut ciphertext, &mut entropy)?;
    if decrypted.0.cbData != 48 || decrypted.0.pbData.is_null() {
        return Err("private key encoding");
    }
    let der = unsafe { std::slice::from_raw_parts(decrypted.0.pbData, 48) };
    decode_key(der, public)
}

fn decrypt(ciphertext: &mut [u8], entropy: &mut [u8]) -> Result<Decrypted> {
    let input = CRYPT_INTEGER_BLOB {
        cbData: ciphertext.len() as u32,
        pbData: ciphertext.as_mut_ptr(),
    };
    let entropy = CRYPT_INTEGER_BLOB {
        cbData: entropy.len() as u32,
        pbData: entropy.as_mut_ptr(),
    };
    let mut decrypted = Decrypted(CRYPT_INTEGER_BLOB::default());
    unsafe {
        CryptUnprotectData(
            &input,
            None,
            Some(&entropy),
            None,
            None,
            CRYPTPROTECT_UI_FORBIDDEN,
            &mut decrypted.0,
        )
    }
    .map_err(|_| "DPAPI decryption")?;
    Ok(decrypted)
}

fn decode_key(der: &[u8], public: &[u8; 32]) -> Result<SigningKey> {
    // Accept precisely the single fixed DER encoding created by our key tool.
    // Exact length + full prefix excludes trailing data and alternate ASN.1 forms.
    if der.len() != 48
        || der[..16]
            != [
                0x30, 0x2e, 0x02, 0x01, 0x00, 0x30, 0x05, 0x06, 0x03, 0x2b, 0x65, 0x70, 0x04, 0x22,
                0x04, 0x20,
            ]
    {
        return Err("private key encoding");
    }
    let mut seed = Zeroizing::new([0u8; 32]);
    seed.copy_from_slice(&der[16..]);
    let key = SigningKey::from_bytes(&seed);
    if key.verifying_key().to_bytes() != *public {
        return Err("private key identity");
    }
    Ok(key)
}

#[cfg(test)]
mod tests {
    use super::*;
    use windows::{Win32::Security::Cryptography::CryptProtectData, core::PCWSTR};

    // Deliberately public fixture seed. Tests never call load_key or read the
    // production key directory, and never create any persistent private file.
    fn fixture() -> (Vec<u8>, [u8; 32]) {
        let mut der = vec![
            0x30, 0x2e, 0x02, 0x01, 0x00, 0x30, 0x05, 0x06, 0x03, 0x2b, 0x65, 0x70, 0x04, 0x22,
            0x04, 0x20,
        ];
        der.extend_from_slice(&[42; 32]);
        (
            der,
            SigningKey::from_bytes(&[42; 32]).verifying_key().to_bytes(),
        )
    }

    #[test]
    fn strict_der_and_public_identity() {
        let (der, public) = fixture();
        assert!(decode_key(&der, &public).is_ok());
        for length in 0..48 {
            assert!(decode_key(&der[..length], &public).is_err());
        }
        let mut trailing = der.clone();
        trailing.push(0);
        assert!(decode_key(&trailing, &public).is_err());
        for index in 0..16 {
            let mut altered = der.clone();
            altered[index] ^= 1;
            assert!(decode_key(&altered, &public).is_err());
        }
        assert!(decode_key(&der, &[0; 32]).is_err());
    }

    #[test]
    fn native_dpapi_roundtrip_and_wrong_entropy_refusal() {
        let (mut der, public) = fixture();
        let mut entropy = b"AutoKeyboardLayot signing utility public test fixture".to_vec();
        let input = CRYPT_INTEGER_BLOB {
            cbData: der.len() as u32,
            pbData: der.as_mut_ptr(),
        };
        let entropy_blob = CRYPT_INTEGER_BLOB {
            cbData: entropy.len() as u32,
            pbData: entropy.as_mut_ptr(),
        };
        let mut protected = Decrypted(CRYPT_INTEGER_BLOB::default());
        unsafe {
            CryptProtectData(
                &input,
                PCWSTR::null(),
                Some(&entropy_blob),
                None,
                None,
                CRYPTPROTECT_UI_FORBIDDEN,
                &mut protected.0,
            )
        }
        .expect("protect public fixture");
        let cipher = unsafe {
            std::slice::from_raw_parts_mut(protected.0.pbData, protected.0.cbData as usize)
        };
        let plaintext = decrypt(cipher, &mut entropy).expect("decrypt public fixture");
        assert_eq!(plaintext.0.cbData, 48);
        let bytes =
            unsafe { std::slice::from_raw_parts(plaintext.0.pbData, plaintext.0.cbData as usize) };
        assert!(decode_key(bytes, &public).is_ok());
        entropy[0] ^= 1;
        assert!(decrypt(cipher, &mut entropy).is_err());
        entropy[0] ^= 1;
        cipher[0] ^= 1;
        assert!(decrypt(cipher, &mut entropy).is_err());
    }

    #[test]
    fn bounded_file_read_refuses_directory_and_oversize() {
        let temp = tempfile::tempdir().expect("test directory");
        assert!(read_bounded(temp.path(), 4).is_err());
        let path = temp.path().join("public-input.json");
        std::fs::write(&path, b"12345").expect("public fixture write");
        assert!(read_bounded(&path, 4).is_err());
        assert_eq!(read_bounded(&path, 5).expect("bounded read"), b"12345");
        assert!(reject_reparse(Path::new("relative")).is_err());
    }
}

pub fn run() -> Result<()> {
    let args: Vec<_> = std::env::args_os().skip(1).collect();
    if args.len() != 3 {
        return Err("usage: sign-language-package package|catalog ABS_INPUT ABS_OUTPUT");
    }
    let (kind, limit) = match args[0].to_str() {
        Some("package") => (SigningKind::Package, 64 * 1024 * 1024),
        Some("catalog") => (SigningKind::Catalog, 1024 * 1024),
        _ => return Err("input kind"),
    };
    // The runtime trust parser validates the embedded metadata/fingerprint first.
    let trust = PackageTrust::release().map_err(|_| "release trust")?;
    let metadata: serde_json::Value =
        serde_json::from_slice(include_bytes!("../data/package-signing/public-key.json"))
            .map_err(|_| "public metadata")?;
    let signer = metadata["signer"].as_str().ok_or("public signer")?;
    let hex = metadata["public_key_hex"].as_str().ok_or("public key")?;
    if hex.len() != 64 || !hex.is_ascii() {
        return Err("public key");
    }
    let mut public = [0u8; 32];
    for (index, byte) in public.iter_mut().enumerate() {
        *byte = u8::from_str_radix(&hex[index * 2..index * 2 + 2], 16).map_err(|_| "public key")?;
    }
    let output = Path::new(&args[2]);
    reject_reparse(output.parent().ok_or("output parent")?)?;
    if !output.is_absolute() || output.try_exists().map_err(|_| "output metadata")? {
        return Err("output must be new and absolute");
    }
    let bytes = read_bounded(Path::new(&args[1]), limit)?;
    let prepared = PreparedSigningInput::prepare(kind, signer, &bytes, now()?)
        .map_err(|_| "input validation")?;
    let key = load_key(signer, &public, hex)?;
    let signature = key.sign(&prepared.signing_message()).to_bytes();
    drop(key);
    let signed = prepared
        .finalize(signer, signature, &trust, now()?)
        .map_err(|_| "final verification")?;
    reject_reparse(output.parent().ok_or("output parent")?)?;
    let mut file: File = OpenOptions::new()
        .write(true)
        .create_new(true)
        .share_mode(0)
        .custom_flags(OPEN_REPARSE_POINT)
        .open(output)
        .map_err(|_| "exclusive output creation")?;
    file.write_all(&signed)
        .and_then(|()| file.sync_all())
        .map_err(|_| "output write; inspect partial file")?;
    println!("SIGNED_OUTPUT_VERIFIED");
    Ok(())
}
