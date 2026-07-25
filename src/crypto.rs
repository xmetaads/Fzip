//! ZIP encryption: legacy ZipCrypto plus WinZip AES (AE-1 / AE-2).

use aes::cipher::generic_array::GenericArray;
use aes::cipher::{BlockEncrypt, KeyInit};
use hmac::{Hmac, Mac};
use sha1::Sha1;

// ---------------- ZipCrypto (legacy PKWARE) ----------------

const fn make_crc_table() -> [u32; 256] {
    let mut table = [0u32; 256];
    let mut i = 0;
    while i < 256 {
        let mut c = i as u32;
        let mut k = 0;
        while k < 8 {
            c = if c & 1 != 0 { 0xEDB8_8320 ^ (c >> 1) } else { c >> 1 };
            k += 1;
        }
        table[i] = c;
        i += 1;
    }
    table
}

static CRC_TABLE: [u32; 256] = make_crc_table();

#[inline]
fn crc32_byte(crc: u32, b: u8) -> u32 {
    (crc >> 8) ^ CRC_TABLE[((crc ^ b as u32) & 0xFF) as usize]
}

pub struct ZipCrypto {
    k0: u32,
    k1: u32,
    k2: u32,
}

impl ZipCrypto {
    pub fn new(password: &[u8]) -> Self {
        let mut z = ZipCrypto { k0: 0x1234_5678, k1: 0x2345_6789, k2: 0x3456_7890 };
        for &b in password {
            z.update(b);
        }
        z
    }

    #[inline]
    fn update(&mut self, b: u8) {
        self.k0 = crc32_byte(self.k0, b);
        self.k1 = self.k1.wrapping_add(self.k0 & 0xFF).wrapping_mul(134_775_813).wrapping_add(1);
        self.k2 = crc32_byte(self.k2, (self.k1 >> 24) as u8);
    }

    #[inline]
    fn stream_byte(&self) -> u8 {
        let t = (self.k2 | 3) as u16;
        (t.wrapping_mul(t ^ 1) >> 8) as u8
    }

    #[inline]
    pub fn decrypt_byte(&mut self, c: u8) -> u8 {
        let p = c ^ self.stream_byte();
        self.update(p);
        p
    }
}

#[derive(Debug)]
pub enum CryptoError {
    TooShort,
    WrongPassword,
    /// HMAC mismatch — the data was modified or is corrupt.
    Tampered,
}

/// Decrypt a ZipCrypto entry, returning the still-compressed payload.
/// `check` is the expected verification byte (high byte of CRC or DOS time).
pub fn zipcrypto_decrypt(raw: &[u8], password: &str, check: u8) -> Result<Vec<u8>, CryptoError> {
    if raw.len() < 12 {
        return Err(CryptoError::TooShort);
    }
    let mut z = ZipCrypto::new(password.as_bytes());
    let mut header = [0u8; 12];
    for (i, &c) in raw[..12].iter().enumerate() {
        header[i] = z.decrypt_byte(c);
    }
    if header[11] != check {
        return Err(CryptoError::WrongPassword);
    }
    let mut out = Vec::with_capacity(raw.len() - 12);
    for &c in &raw[12..] {
        out.push(z.decrypt_byte(c));
    }
    Ok(out)
}

// ---------------- WinZip AES (AE-1 / AE-2) ----------------

/// 1 = AES-128, 2 = AES-192, 3 = AES-256
pub fn aes_salt_len(strength: u8) -> usize {
    match strength {
        1 => 8,
        2 => 12,
        _ => 16,
    }
}

pub fn aes_key_len(strength: u8) -> usize {
    match strength {
        1 => 16,
        2 => 24,
        _ => 32,
    }
}

/// WinZip-flavoured AES-CTR: 128-bit little-endian counter starting at 1.
/// Symmetric, so the same routine both encrypts and decrypts.
fn aes_ctr_apply(key: &[u8], data: &mut [u8]) {
    macro_rules! run {
        ($t:ty) => {{
            let cipher = <$t>::new_from_slice(key).unwrap();
            let mut counter: u128 = 1;
            for chunk in data.chunks_mut(16) {
                let mut block = counter.to_le_bytes();
                cipher.encrypt_block(GenericArray::from_mut_slice(&mut block));
                for (b, k) in chunk.iter_mut().zip(block.iter()) {
                    *b ^= k;
                }
                counter += 1;
            }
        }};
    }
    match key.len() {
        16 => run!(aes::Aes128),
        24 => run!(aes::Aes192),
        _ => run!(aes::Aes256),
    }
}

/// Resumable AES-256-CTR keystream, so a file can be encrypted chunk by chunk
/// without ever holding it whole. Byte-for-byte identical to `aes_ctr_apply`.
struct CtrStream {
    cipher: aes::Aes256,
    counter: u128,
    block: [u8; 16],
    pos: usize,
}

impl CtrStream {
    fn new(key: &[u8]) -> Self {
        CtrStream {
            cipher: aes::Aes256::new_from_slice(key).expect("AES-256 key is 32 bytes"),
            counter: 1,
            block: [0u8; 16],
            pos: 16, // forces a fresh keystream block on first use
        }
    }

    fn refill(&mut self) {
        self.block = self.counter.to_le_bytes();
        self.cipher.encrypt_block(GenericArray::from_mut_slice(&mut self.block));
        self.counter += 1;
        self.pos = 0;
    }

    fn apply(&mut self, data: &mut [u8]) {
        let mut i = 0;
        while i < data.len() {
            if self.pos == 16 {
                self.refill();
            }
            let n = (16 - self.pos).min(data.len() - i);
            for k in 0..n {
                data[i + k] ^= self.block[self.pos + k];
            }
            self.pos += n;
            i += n;
        }
    }
}

/// Streaming WinZip-AES encryptor. Writes `salt | verifier` on creation,
/// encrypts and authenticates each chunk as it passes through, and appends the
/// 10-byte MAC on `finish()`.
pub struct AesWriter<W: std::io::Write> {
    inner: W,
    ctr: CtrStream,
    mac: Hmac<Sha1>,
    buf: Vec<u8>,
    written: u64,
}

impl<W: std::io::Write> AesWriter<W> {
    pub fn new(mut inner: W, password: &str) -> Result<Self, String> {
        const STRENGTH: u8 = 3; // AES-256
        let mut salt = vec![0u8; aes_salt_len(STRENGTH)];
        getrandom::fill(&mut salt).map_err(|e| format!("cannot obtain random bytes: {}", e))?;
        let keys = derive(password, &salt, aes_key_len(STRENGTH));

        inner
            .write_all(&salt)
            .and_then(|_| inner.write_all(&keys.verifier))
            .map_err(|e| format!("write error: {}", e))?;

        Ok(AesWriter {
            inner,
            ctr: CtrStream::new(&keys.enc),
            mac: <Hmac<Sha1> as Mac>::new_from_slice(&keys.auth).expect("HMAC accepts any key"),
            buf: Vec::new(),
            written: salt.len() as u64 + 2,
        })
    }

    /// Append the authentication code and hand back the inner writer plus the
    /// total number of bytes written through it.
    pub fn finish(mut self) -> Result<(W, u64), String> {
        let tag = self.mac.finalize().into_bytes();
        self.inner
            .write_all(&tag[..10])
            .map_err(|e| format!("write error: {}", e))?;
        self.written += 10;
        Ok((self.inner, self.written))
    }
}

impl<W: std::io::Write> std::io::Write for AesWriter<W> {
    fn write(&mut self, data: &[u8]) -> std::io::Result<usize> {
        self.buf.clear();
        self.buf.extend_from_slice(data);
        self.ctr.apply(&mut self.buf);
        self.mac.update(&self.buf);
        self.inner.write_all(&self.buf)?;
        self.written += self.buf.len() as u64;
        Ok(data.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        self.inner.flush()
    }
}

struct DerivedKeys {
    enc: Vec<u8>,
    auth: Vec<u8>,
    verifier: [u8; 2],
}

fn derive(password: &str, salt: &[u8], key_len: usize) -> DerivedKeys {
    let mut out = vec![0u8; key_len * 2 + 2];
    pbkdf2::pbkdf2_hmac::<Sha1>(password.as_bytes(), salt, 1000, &mut out);
    DerivedKeys {
        enc: out[..key_len].to_vec(),
        auth: out[key_len..key_len * 2].to_vec(),
        verifier: [out[key_len * 2], out[key_len * 2 + 1]],
    }
}

/// Decrypt an AES entry laid out as salt | verifier(2) | ciphertext | mac(10).
pub fn aes_decrypt(raw: &[u8], strength: u8, password: &str) -> Result<Vec<u8>, CryptoError> {
    let salt_len = aes_salt_len(strength);
    let key_len = aes_key_len(strength);
    if raw.len() < salt_len + 2 + 10 {
        return Err(CryptoError::TooShort);
    }
    let salt = &raw[..salt_len];
    let verifier = &raw[salt_len..salt_len + 2];
    let ciphertext = &raw[salt_len + 2..raw.len() - 10];
    let auth_code = &raw[raw.len() - 10..];

    let keys = derive(password, salt, key_len);
    if keys.verifier != verifier {
        return Err(CryptoError::WrongPassword);
    }

    // Verify BEFORE decrypting (encrypt-then-MAC) so tampered data never
    // reaches the decryption path.
    let mut mac = <Hmac<Sha1> as Mac>::new_from_slice(&keys.auth).unwrap();
    mac.update(ciphertext);
    if mac.finalize().into_bytes()[..10] != *auth_code {
        return Err(CryptoError::Tampered);
    }

    let mut plain = ciphertext.to_vec();
    aes_ctr_apply(&keys.enc, &mut plain);
    Ok(plain)
}

/// Encrypt one entry with AES-256 (AE-2), producing
/// salt | verifier | ciphertext | mac.
pub fn aes_encrypt(plain: &[u8], password: &str) -> Result<Vec<u8>, String> {
    const STRENGTH: u8 = 3; // AES-256
    let salt_len = aes_salt_len(STRENGTH);
    let key_len = aes_key_len(STRENGTH);

    let mut salt = vec![0u8; salt_len];
    getrandom::fill(&mut salt).map_err(|e| format!("cannot obtain random bytes: {}", e))?;

    let keys = derive(password, &salt, key_len);

    let mut body = plain.to_vec();
    aes_ctr_apply(&keys.enc, &mut body);

    let mut mac = <Hmac<Sha1> as Mac>::new_from_slice(&keys.auth).unwrap();
    mac.update(&body);
    let tag = mac.finalize().into_bytes();

    let mut out = Vec::with_capacity(salt_len + 2 + body.len() + 10);
    out.extend_from_slice(&salt);
    out.extend_from_slice(&keys.verifier);
    out.extend_from_slice(&body);
    out.extend_from_slice(&tag[..10]);
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    #[test]
    fn aes_round_trip() {
        let data = b"some data that really must stay confidential";
        let enc = aes_encrypt(data, "Passw0rd!2026").unwrap();
        let dec = aes_decrypt(&enc, 3, "Passw0rd!2026").unwrap();
        assert_eq!(&dec[..], &data[..]);
    }

    #[test]
    fn aes_rejects_wrong_password() {
        let enc = aes_encrypt(b"abc", "right").unwrap();
        assert!(matches!(aes_decrypt(&enc, 3, "wrong"), Err(CryptoError::WrongPassword)));
    }

    #[test]
    fn aes_detects_tampering() {
        let mut enc = aes_encrypt(b"important payload", "pw").unwrap();
        let n = enc.len();
        enc[n - 15] ^= 0xFF; // flip one byte of ciphertext
        assert!(matches!(aes_decrypt(&enc, 3, "pw"), Err(CryptoError::Tampered)));
    }

    #[test]
    fn streaming_matches_one_shot() {
        // The streaming writer must produce a byte stream the normal decryptor
        // accepts, including across awkward chunk boundaries.
        let data: Vec<u8> = (0..5000u32).map(|i| (i % 251) as u8).collect();
        let mut sink: Vec<u8> = Vec::new();
        {
            let mut w = AesWriter::new(&mut sink, "pw").unwrap();
            // Deliberately uneven chunks that straddle AES block boundaries
            let mut at = 0;
            for len in [1usize, 15, 16, 17, 4951] {
                w.write_all(&data[at..at + len]).unwrap();
                at += len;
            }
            assert_eq!(at, data.len());
            w.finish().unwrap();
        }
        let back = aes_decrypt(&sink, 3, "pw").unwrap();
        assert_eq!(back, data);
    }

    #[test]
    fn streaming_rejects_wrong_password() {
        let mut sink: Vec<u8> = Vec::new();
        {
            let mut w = AesWriter::new(&mut sink, "right").unwrap();
            w.write_all(b"secret").unwrap();
            w.finish().unwrap();
        }
        assert!(matches!(aes_decrypt(&sink, 3, "wrong"), Err(CryptoError::WrongPassword)));
    }

    #[test]
    fn aes_salt_is_random_each_time() {
        let a = aes_encrypt(b"x", "pw").unwrap();
        let b = aes_encrypt(b"x", "pw").unwrap();
        assert_ne!(a, b, "salt must be random on every encryption");
    }
}
