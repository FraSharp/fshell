// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Francesco Duca <f.duca00@gmail.com>

#![allow(
    clippy::collapsible_if,
    clippy::needless_range_loop,
    clippy::manual_is_multiple_of,
    clippy::manual_repeat_n
)]

use crate::error::BuiltinError;
use crossterm::event::{self, Event, KeyCode};
use fshell_core::RwLock;
use fshell_core::diagnostic::StringError;
use fshell_core::{FxIndexMap, Val};
use fshell_engine::{CapAction, Env, PipeSender, PipeStream, PipelinePayload};
use nu_ansi_term::{Color, Style};
use ratatui::{
    Terminal,
    backend::CrosstermBackend,
    layout::{Constraint, Direction, Layout},
    widgets::{Block, Borders, List, ListItem, ListState, Paragraph, Wrap},
};
use serde::{Deserialize, Serialize};
use std::io::{Read, Write};
#[cfg(unix)]
use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, SystemTime};
use ustr::ustr;

// IN-MEMORY SECURITY & ZEROIZATION

/// Force a volatile write to clear a String's backing memory.
pub fn zeroize_string(mut s: String) {
    let cap = s.capacity();
    if cap > 0 {
        let ptr = s.as_mut_ptr();
        unsafe {
            for i in 0..cap {
                std::ptr::write_volatile(ptr.add(i), 0u8);
            }
        }
    }
    s.clear();
}

/// A zero-dependency heap-allocated wrapper that automatically zeroizes on drop.
pub struct SecretBytes(pub Vec<u8>);

impl SecretBytes {
    pub fn new(bytes: Vec<u8>) -> Self {
        Self(bytes)
    }

    pub fn from_string(s: String) -> Self {
        let bytes = s.clone().into_bytes();
        zeroize_string(s);
        Self(bytes)
    }

    pub fn as_slice(&self) -> &[u8] {
        &self.0
    }
}

impl Drop for SecretBytes {
    fn drop(&mut self) {
        let cap = self.0.capacity();
        let ptr = self.0.as_mut_ptr();
        unsafe {
            for i in 0..cap {
                std::ptr::write_volatile(ptr.add(i), 0u8);
            }
        }
        std::sync::atomic::compiler_fence(std::sync::atomic::Ordering::SeqCst);
    }
}

/// A process-wide struct holding derived symmetric keys.
pub struct SessionKeys {
    pub enc_key: [u8; 32],
    pub mac_key: [u8; 32],
}

impl Drop for SessionKeys {
    fn drop(&mut self) {
        unsafe {
            std::ptr::write_volatile(&mut self.enc_key, [0u8; 32]);
            std::ptr::write_volatile(&mut self.mac_key, [0u8; 32]);
        }
        std::sync::atomic::compiler_fence(std::sync::atomic::Ordering::SeqCst);
    }
}

static SESSION_KEYS: RwLock<Option<SessionKeys>> = RwLock::new(None);

// CUSTOM HEX ENCODING / DECODING

fn hex_encode(bytes: &[u8]) -> String {
    let mut s = String::with_capacity(bytes.len() * 2);
    for &b in bytes {
        s.push_str(&format!("{:02x}", b));
    }
    s
}

fn hex_decode(s: &str) -> Result<Vec<u8>, StringError> {
    if s.len() % 2 != 0 {
        return Err("Odd hex string length".to_string().into());
    }
    let mut bytes = Vec::with_capacity(s.len() / 2);
    for i in (0..s.len()).step_by(2) {
        let digit_bytes = s
            .get(i..i + 2)
            .ok_or_else(|| StringError::from("Invalid hex character index"))?;
        let b = u8::from_str_radix(digit_bytes, 16)
            .map_err(|_| StringError::from(format!("Invalid hex byte: {}", digit_bytes)))?;
        bytes.push(b);
    }
    Ok(bytes)
}

// CRYPTOGRAPHY SCHEME (fshell-hash based)

pub fn get_random_bytes(buf: &mut [u8]) -> Result<(), StringError> {
    if getrandom::fill(buf).is_ok() {
        return Ok(());
    }
    if let Ok(mut file) = std::fs::File::open("/dev/urandom") {
        if file.read_exact(buf).is_ok() {
            return Ok(());
        }
    }
    Err(StringError::from(
        "secure random generation failed: no entropy source available",
    ))
}

pub fn derive_keys(password: &str, salt: &[u8; 16]) -> ([u8; 32], [u8; 32]) {
    let mut current = fshell_hash::fhash_kdf(password.as_bytes(), salt, b"vault-kdf-salt", 64);
    for _ in 0..100_000 {
        current = fshell_hash::fhash_kdf(&current, salt, b"vault-kdf-iter", 64);
    }
    let mut enc_key = [0u8; 32];
    let mut mac_key = [0u8; 32];
    enc_key.copy_from_slice(&current[0..32]);
    mac_key.copy_from_slice(&current[32..64]);
    (enc_key, mac_key)
}

pub fn pad_data(plaintext: &[u8]) -> Vec<u8> {
    let plain_len = plaintext.len();
    let block_size = 4096;
    let mut pad_len = block_size - (plain_len % block_size);
    if pad_len < 2 {
        pad_len += block_size;
    }
    let mut padded = Vec::with_capacity(plain_len + pad_len);
    padded.extend_from_slice(plaintext);

    let mut random_padding = vec![0u8; pad_len - 2];
    get_random_bytes(&mut random_padding).expect("secure random generation failed");
    padded.extend_from_slice(&random_padding);

    let pad_len_bytes = (pad_len as u16).to_be_bytes();
    padded.extend_from_slice(&pad_len_bytes);
    padded
}

pub fn unpad_data(padded: &[u8]) -> Result<Vec<u8>, StringError> {
    if padded.is_empty() || padded.len() % 4096 != 0 || padded.len() < 2 {
        return Err("Invalid padded data block size".to_string().into());
    }
    let len = padded.len();
    let mut pad_len_bytes = [0u8; 2];
    pad_len_bytes.copy_from_slice(&padded[len - 2..len]);
    let pad_len = u16::from_be_bytes(pad_len_bytes) as usize;

    if pad_len < 2 || pad_len > len {
        return Err("Corrupted padding size descriptor".to_string().into());
    }
    Ok(padded[..len - pad_len].to_vec())
}

pub fn encrypt_payload(key: &[u8; 32], plaintext: &[u8]) -> (Vec<u8>, [u8; 16]) {
    let mut iv = [0u8; 16];
    get_random_bytes(&mut iv).expect("secure random generation failed");

    let mut state_input = Vec::with_capacity(32 + 16);
    state_input.extend_from_slice(key);
    state_input.extend_from_slice(&iv);

    let keystream = fshell_hash::fhash_xof(&state_input, plaintext.len());
    let ciphertext: Vec<u8> = plaintext
        .iter()
        .zip(keystream.iter())
        .map(|(p, k)| p ^ k)
        .collect();
    (ciphertext, iv)
}

pub fn decrypt_payload(key: &[u8; 32], ciphertext: &[u8], iv: &[u8; 16]) -> Vec<u8> {
    let mut state_input = Vec::with_capacity(32 + 16);
    state_input.extend_from_slice(key);
    state_input.extend_from_slice(iv);

    let keystream = fshell_hash::fhash_xof(&state_input, ciphertext.len());
    ciphertext
        .iter()
        .zip(keystream.iter())
        .map(|(c, k)| c ^ k)
        .collect()
}

pub fn compute_mac(
    mac_key: &[u8; 32],
    ciphertext: &[u8],
    iv: &[u8; 16],
    salt: &[u8; 16],
) -> [u8; 32] {
    let mut msg = Vec::with_capacity(salt.len() + iv.len() + ciphertext.len());
    msg.extend_from_slice(salt);
    msg.extend_from_slice(iv);
    msg.extend_from_slice(ciphertext);

    let tag_vec = fshell_hash::fhash_kmac(mac_key, &msg, 32, b"vault-aead-tag");
    let mut tag = [0u8; 32];
    tag.copy_from_slice(&tag_vec);
    tag
}

pub fn constant_time_eq(a: &[u8], b: &[u8]) -> bool {
    if a.len() != b.len() {
        return false;
    }
    let mut result = 0u8;
    for (x, y) in a.iter().zip(b.iter()) {
        result |= x ^ y;
    }
    result == 0
}

// SECURE INPUT PROMPT

struct RawModeGuard;

impl RawModeGuard {
    fn new() -> Result<Self, StringError> {
        crossterm::terminal::enable_raw_mode()
            .map_err(|e| format!("Failed to enable raw mode: {}", e))?;
        Ok(Self)
    }
}

impl Drop for RawModeGuard {
    fn drop(&mut self) {
        let _ = crossterm::terminal::disable_raw_mode();
    }
}

pub fn read_password_prompt(prompt: &str) -> Result<String, StringError> {
    print!("{}", prompt);
    let _ = std::io::stdout().flush();

    let _raw_guard = RawModeGuard::new()?;
    let mut password = String::new();
    loop {
        if event::poll(Duration::from_millis(100)).unwrap_or(false) {
            if let Ok(Event::Key(key_event)) = event::read() {
                if key_event.kind == crossterm::event::KeyEventKind::Release {
                    continue;
                }
                match key_event.code {
                    KeyCode::Enter => {
                        println!();
                        break;
                    }
                    KeyCode::Esc => {
                        println!();
                        return Err("Password input cancelled".to_string().into());
                    }
                    KeyCode::Backspace => {
                        password.pop();
                    }
                    KeyCode::Char(c) => {
                        password.push(c);
                    }
                    _ => {}
                }
            }
        }
    }
    Ok(password)
}

// TOTP PROTOCOL IMPLEMENTATION

pub fn base32_decode(s: &str) -> Option<Vec<u8>> {
    let cleaned = s.to_uppercase().replace(" ", "").replace("-", "");
    let mut bytes = Vec::new();
    let mut buffer = 0u32;
    let mut bits = 0;

    for c in cleaned.chars() {
        if c == '=' {
            break;
        }
        let val = match c {
            'A'..='Z' => c as u32 - 'A' as u32,
            '2'..='7' => c as u32 - '2' as u32 + 26,
            _ => return None,
        };
        buffer = (buffer << 5) | val;
        bits += 5;
        if bits >= 8 {
            bits -= 8;
            bytes.push((buffer >> bits) as u8);
        }
    }
    Some(bytes)
}

pub fn sha1(data: &[u8]) -> [u8; 20] {
    let mut h0 = 0x67452301u32;
    let mut h1 = 0xEFCDAB89u32;
    let mut h2 = 0x98BADCFEu32;
    let mut h3 = 0x10325476u32;
    let mut h4 = 0xC3D2E1F0u32;

    let mut padded = data.to_vec();
    padded.push(0x80);
    let orig_bits = (data.len() as u64) * 8;
    while (padded.len() + 8) % 64 != 0 {
        padded.push(0x00);
    }
    padded.extend_from_slice(&orig_bits.to_be_bytes());

    for chunk in padded.as_chunks::<64>().0 {
        let mut w = [0u32; 80];
        for i in 0..16 {
            let mut b = [0u8; 4];
            b.copy_from_slice(&chunk[i * 4..(i + 1) * 4]);
            w[i] = u32::from_be_bytes(b);
        }
        for i in 16..80 {
            w[i] = (w[i - 3] ^ w[i - 8] ^ w[i - 14] ^ w[i - 16]).rotate_left(1);
        }

        let mut a = h0;
        let mut b = h1;
        let mut c = h2;
        let mut d = h3;
        let mut e = h4;
        for i in 0..80 {
            let (f, k) = if i < 20 {
                ((b & c) | ((!b) & d), 0x5A827999)
            } else if i < 40 {
                (b ^ c ^ d, 0x6ED9EBA1)
            } else if i < 60 {
                ((b & c) | (b & d) | (c & d), 0x8F1BBCDC)
            } else {
                (b ^ c ^ d, 0xCA62C1D6)
            };
            let temp = a
                .rotate_left(5)
                .wrapping_add(f)
                .wrapping_add(e)
                .wrapping_add(k)
                .wrapping_add(w[i]);
            e = d;
            d = c;
            c = b.rotate_left(30);
            b = a;
            a = temp;
        }
        h0 = h0.wrapping_add(a);
        h1 = h1.wrapping_add(b);
        h2 = h2.wrapping_add(c);
        h3 = h3.wrapping_add(d);
        h4 = h4.wrapping_add(e);
    }

    let mut out = [0u8; 20];
    out[0..4].copy_from_slice(&h0.to_be_bytes());
    out[4..8].copy_from_slice(&h1.to_be_bytes());
    out[8..12].copy_from_slice(&h2.to_be_bytes());
    out[12..16].copy_from_slice(&h3.to_be_bytes());
    out[16..20].copy_from_slice(&h4.to_be_bytes());
    out
}

pub fn hmac_sha1(key: &[u8], message: &[u8]) -> [u8; 20] {
    let mut k_pad = [0u8; 64];
    if key.len() > 64 {
        k_pad[0..20].copy_from_slice(&sha1(key));
    } else {
        k_pad[0..key.len()].copy_from_slice(key);
    }
    let mut ipad = [0u8; 64];
    let mut opad = [0u8; 64];
    for i in 0..64 {
        ipad[i] = k_pad[i] ^ 0x36;
        opad[i] = k_pad[i] ^ 0x5C;
    }
    let mut inner = ipad.to_vec();
    inner.extend_from_slice(message);
    let mut outer = opad.to_vec();
    outer.extend_from_slice(&sha1(&inner));
    sha1(&outer)
}

pub fn generate_totp(secret_b32: &str, time_sec: u64) -> Result<String, StringError> {
    let key =
        base32_decode(secret_b32).ok_or_else(|| StringError::from("Invalid Base32 Secret Key"))?;
    let counter = time_sec / 30;
    let hmac_res = hmac_sha1(&key, &counter.to_be_bytes());

    let offset = (hmac_res[19] & 0xF) as usize;
    let code = u32::from_be_bytes([
        hmac_res[offset] & 0x7F,
        hmac_res[offset + 1],
        hmac_res[offset + 2],
        hmac_res[offset + 3],
    ]);
    let pin = code % 1_000_000;
    Ok(format!("{:06}", pin))
}

// WORDLIST AND GENERATORS

const WORDLIST_RAW: &str = include_str!("wordlist.txt");

#[derive(Clone, Copy, PartialEq, Debug)]
pub enum Separator {
    Hyphen,
    Space,
    Camel,
    Numeric,
}

pub fn generate_passphrase(words_count: usize, separator: Separator) -> String {
    let wordlist: Vec<&str> = WORDLIST_RAW
        .lines()
        .map(|l| l.trim())
        .filter(|l| !l.is_empty())
        .collect();
    if wordlist.is_empty() {
        return "fallback-entropy".to_string();
    }

    let mut chosen_words = Vec::new();
    let mut entropy = vec![0u8; words_count * 4];
    get_random_bytes(&mut entropy).expect("secure random generation failed");

    let mut entropy_idx = 0;
    while chosen_words.len() < words_count {
        if entropy_idx + 1 >= entropy.len() {
            entropy = vec![0u8; words_count * 4];
            get_random_bytes(&mut entropy).expect("secure random generation failed");
            entropy_idx = 0;
        }
        let b1 = entropy[entropy_idx] as u32;
        let b2 = entropy[entropy_idx + 1] as u32;
        entropy_idx += 2;

        let val = (b1 << 8) | b2;
        let limit = 65536 - (65536 % wordlist.len() as u32);
        if val < limit {
            let idx = (val % wordlist.len() as u32) as usize;
            chosen_words.push(wordlist[idx]);
        }
    }

    match separator {
        Separator::Hyphen => chosen_words.join("-"),
        Separator::Space => chosen_words.join(" "),
        Separator::Camel => {
            let mut s = String::new();
            for (i, word) in chosen_words.iter().enumerate() {
                if i == 0 {
                    s.push_str(word);
                } else {
                    let mut chars = word.chars();
                    if let Some(first) = chars.next() {
                        s.push(first.to_ascii_uppercase());
                        s.extend(chars);
                    }
                }
            }
            s
        }
        Separator::Numeric => {
            let mut s = String::new();
            let mut digits = vec![0u8; words_count - 1];
            get_random_bytes(&mut digits).expect("secure random generation failed");
            for (i, word) in chosen_words.iter().enumerate() {
                s.push_str(word);
                if i < chosen_words.len() - 1 {
                    s.push_str(&(digits[i] % 10).to_string());
                }
            }
            s
        }
    }
}

pub fn generate_random_password(
    length: usize,
    use_upper: bool,
    use_lower: bool,
    use_digits: bool,
    use_symbols: bool,
) -> String {
    let mut chars: Vec<u8> = Vec::new();
    if use_lower {
        chars.extend(b"abcdefghijklmnopqrstuvwxyz");
    }
    if use_upper {
        chars.extend(b"ABCDEFGHIJKLMNOPQRSTUVWXYZ");
    }
    if use_digits {
        chars.extend(b"0123456789");
    }
    if use_symbols {
        chars.extend(b"!@#$%^&*()-_=+[]{}|;:,.<>?");
    }
    if chars.is_empty() {
        chars.extend(b"abcdefghijklmnopqrstuvwxyzabcdefghijklmnopqrstuvwxyz");
    }

    let mut pwd = String::new();
    let mut entropy = vec![0u8; length * 2];
    get_random_bytes(&mut entropy).expect("secure random generation failed");

    let mut entropy_idx = 0;
    while pwd.len() < length {
        if entropy_idx >= entropy.len() {
            entropy = vec![0u8; length * 2];
            get_random_bytes(&mut entropy).expect("secure random generation failed");
            entropy_idx = 0;
        }
        let b = entropy[entropy_idx] as usize;
        entropy_idx += 1;

        let limit = 256 - (256 % chars.len());
        if b < limit {
            let idx = b % chars.len();
            pwd.push(chars[idx] as char);
        }
    }
    pwd
}

// DB LOADER / SAVER

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct VaultEntry {
    pub id: String,
    pub name: String,
    pub username: Option<String>,
    pub password: Option<String>,
    pub url: Option<String>,
    pub totp_secret: Option<String>,
    pub notes: Option<String>,
    pub tags: Vec<String>,
    pub created_at: String,
    pub updated_at: String,
}

#[derive(Serialize, Deserialize, Debug)]
struct EncryptedVault {
    salt: String,
    iv: String,
    ciphertext: String,
    mac: String,
}

fn get_vault_path() -> Result<PathBuf, StringError> {
    if let Some(cfg) = fshell_engine::config_dir() {
        Ok(cfg.join("vault.enc"))
    } else {
        Err(StringError::from("Could not locate config directory"))
    }
}

fn load_vault_file(path: &Path, password: &str) -> Result<Vec<VaultEntry>, StringError> {
    let mut f = std::fs::File::open(path).map_err(|e| format!("Failed to open vault: {}", e))?;
    let enc_vault: EncryptedVault =
        serde_json::from_reader(&mut f).map_err(|e| format!("Corrupt vault structure: {}", e))?;

    let salt = hex_decode(&enc_vault.salt).map_err(|_| "Invalid salt hex")?;
    let iv = hex_decode(&enc_vault.iv).map_err(|_| "Invalid iv hex")?;
    let ciphertext = hex_decode(&enc_vault.ciphertext).map_err(|_| "Invalid ciphertext hex")?;
    let stored_mac = hex_decode(&enc_vault.mac).map_err(|_| "Invalid mac hex")?;

    let mut salt_arr = [0u8; 16];
    salt_arr.copy_from_slice(&salt);
    let mut iv_arr = [0u8; 16];
    iv_arr.copy_from_slice(&iv);

    let (enc_key, mac_key) = derive_keys(password, &salt_arr);

    let computed_mac = compute_mac(&mac_key, &ciphertext, &iv_arr, &salt_arr);
    if !constant_time_eq(&computed_mac, &stored_mac) {
        return Err(StringError::from(
            "Password incorrect or vault tampered with",
        ));
    }

    let padded_plaintext = decrypt_payload(&enc_key, &ciphertext, &iv_arr);
    let plaintext = unpad_data(&padded_plaintext)?;

    let entries: Vec<VaultEntry> = serde_json::from_slice(&plaintext)
        .map_err(|e| format!("Failed to parse decrypted database: {}", e))?;

    Ok(entries)
}

fn save_vault_file(path: &Path, entries: &[VaultEntry], password: &str) -> Result<(), StringError> {
    let mut salt = [0u8; 16];
    get_random_bytes(&mut salt)?;

    let (enc_key, mac_key) = derive_keys(password, &salt);

    let plaintext =
        serde_json::to_vec(entries).map_err(|e| format!("Failed to serialize entries: {}", e))?;
    let padded = pad_data(&plaintext);

    let (ciphertext, iv) = encrypt_payload(&enc_key, &padded);
    let mac = compute_mac(&mac_key, &ciphertext, &iv, &salt);

    let enc_vault = EncryptedVault {
        salt: hex_encode(&salt),
        iv: hex_encode(&iv),
        ciphertext: hex_encode(&ciphertext),
        mac: hex_encode(&mac),
    };

    let parent = path.parent().unwrap_or_else(|| Path::new("."));
    std::fs::create_dir_all(parent).map_err(|e| format!("Failed to create vault dir: {}", e))?;
    #[cfg(unix)]
    {
        let _ = std::fs::set_permissions(parent, std::fs::Permissions::from_mode(0o700));
    }
    let tmp_path = {
        let mut p = path.as_os_str().to_os_string();
        p.push(".tmp");
        PathBuf::from(p)
    };
    {
        #[cfg(unix)]
        let mut f = std::fs::OpenOptions::new()
            .create(true)
            .write(true)
            .truncate(true)
            .mode(0o600)
            .open(&tmp_path)
            .map_err(|e| format!("Failed to create vault temp file: {}", e))?;
        #[cfg(not(unix))]
        let mut f = std::fs::OpenOptions::new()
            .create(true)
            .write(true)
            .truncate(true)
            .open(&tmp_path)
            .map_err(|e| format!("Failed to create vault temp file: {}", e))?;
        serde_json::to_writer(&mut f, &enc_vault)
            .map_err(|e| format!("Failed to write vault JSON: {}", e))?;
        f.flush()
            .map_err(|e| format!("Failed to flush vault file: {}", e))?;
        f.sync_all()
            .map_err(|e| format!("Failed to fsync vault file: {}", e))?;
        #[cfg(unix)]
        {
            let _ = std::fs::set_permissions(&tmp_path, std::fs::Permissions::from_mode(0o600));
        }
    }
    std::fs::rename(&tmp_path, path)
        .map_err(|e| format!("Failed to atomically move vault file: {}", e))?;
    if let Ok(dir) = std::fs::File::open(parent) {
        let _ = dir.sync_all();
    }
    #[cfg(unix)]
    {
        let _ = std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600));
    }
    Ok(())
}

fn generate_id() -> String {
    let mut bytes = [0u8; 4];
    get_random_bytes(&mut bytes).expect("secure random generation failed");
    hex_encode(&bytes)
}

// SESSION LOCK MANAGER

fn get_active_session_keys() -> Option<([u8; 32], [u8; 32])> {
    let lock = SESSION_KEYS.read();
    lock.as_ref().map(|s| (s.enc_key, s.mac_key))
}

fn set_session_keys(enc_key: [u8; 32], mac_key: [u8; 32]) {
    *SESSION_KEYS.write() = Some(SessionKeys { enc_key, mac_key });
}

fn clear_session_keys() {
    *SESSION_KEYS.write() = None;
}

fn load_vault_with_session(path: &Path, _env: &Env) -> Result<Vec<VaultEntry>, StringError> {
    if let Some((enc_key, mac_key)) = get_active_session_keys() {
        let mut f =
            std::fs::File::open(path).map_err(|e| format!("Failed to open vault: {}", e))?;
        let enc_vault: EncryptedVault = serde_json::from_reader(&mut f)
            .map_err(|e| format!("Corrupt vault structure: {}", e))?;

        let salt = hex_decode(&enc_vault.salt).map_err(|_| "Invalid salt hex")?;
        let iv = hex_decode(&enc_vault.iv).map_err(|_| "Invalid iv hex")?;
        let ciphertext = hex_decode(&enc_vault.ciphertext).map_err(|_| "Invalid ciphertext hex")?;
        let stored_mac = hex_decode(&enc_vault.mac).map_err(|_| "Invalid mac hex")?;

        let mut salt_arr = [0u8; 16];
        salt_arr.copy_from_slice(&salt);
        let mut iv_arr = [0u8; 16];
        iv_arr.copy_from_slice(&iv);

        let computed_mac = compute_mac(&mac_key, &ciphertext, &iv_arr, &salt_arr);
        if !constant_time_eq(&computed_mac, &stored_mac) {
            return Err(StringError::from("Session key invalid or vault modified"));
        }

        let padded_plaintext = decrypt_payload(&enc_key, &ciphertext, &iv_arr);
        let plaintext = unpad_data(&padded_plaintext)?;
        let entries: Vec<VaultEntry> =
            serde_json::from_slice(&plaintext).map_err(|e| StringError::from(format!("{}", e)))?;
        Ok(entries)
    } else {
        let pwd = read_password_prompt("Enter master password: ")?;
        let entries = load_vault_file(path, &pwd)?;
        Ok(entries)
    }
}

fn save_vault_with_session(
    path: &Path,
    entries: &[VaultEntry],
    _env: &Env,
) -> Result<(), StringError> {
    if let Some((enc_key, mac_key)) = get_active_session_keys() {
        let mut salt = [0u8; 16];
        get_random_bytes(&mut salt)?;

        let plaintext =
            serde_json::to_vec(entries).map_err(|e| format!("Failed to serialize: {}", e))?;
        let padded = pad_data(&plaintext);
        let (ciphertext, iv) = encrypt_payload(&enc_key, &padded);
        let mac = compute_mac(&mac_key, &ciphertext, &iv, &salt);

        let enc_vault = EncryptedVault {
            salt: hex_encode(&salt),
            iv: hex_encode(&iv),
            ciphertext: hex_encode(&ciphertext),
            mac: hex_encode(&mac),
        };

        let parent = path.parent().unwrap_or_else(|| Path::new("."));
        std::fs::create_dir_all(parent)
            .map_err(|e| format!("Failed to create vault dir: {}", e))?;
        #[cfg(unix)]
        {
            let _ = std::fs::set_permissions(parent, std::fs::Permissions::from_mode(0o700));
        }
        let tmp_path = {
            let mut p = path.as_os_str().to_os_string();
            p.push(".tmp");
            PathBuf::from(p)
        };
        {
            #[cfg(unix)]
            let mut f = std::fs::OpenOptions::new()
                .create(true)
                .write(true)
                .truncate(true)
                .mode(0o600)
                .open(&tmp_path)
                .map_err(|e| format!("Failed to create vault temp file: {}", e))?;
            #[cfg(not(unix))]
            let mut f = std::fs::OpenOptions::new()
                .create(true)
                .write(true)
                .truncate(true)
                .open(&tmp_path)
                .map_err(|e| format!("Failed to create vault temp file: {}", e))?;
            serde_json::to_writer(&mut f, &enc_vault)
                .map_err(|e| format!("Failed to write vault JSON: {}", e))?;
            f.flush()
                .map_err(|e| format!("Failed to flush vault file: {}", e))?;
            f.sync_all()
                .map_err(|e| format!("Failed to fsync vault file: {}", e))?;
            #[cfg(unix)]
            {
                let _ = std::fs::set_permissions(&tmp_path, std::fs::Permissions::from_mode(0o600));
            }
        }
        std::fs::rename(&tmp_path, path)
            .map_err(|e| format!("Failed to atomically move vault file: {}", e))?;
        if let Ok(dir) = std::fs::File::open(parent) {
            let _ = dir.sync_all();
        }
        #[cfg(unix)]
        {
            let _ = std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600));
        }
        Ok(())
    } else {
        let pwd = read_password_prompt("Enter master password to authorize changes: ")?;
        save_vault_file(path, entries, &pwd)
    }
}

// CLIPBOARD INTEGRATION (Zero-dependency OS copy)

pub fn copy_to_system_clipboard(text: &str) {
    if cfg!(target_os = "macos") {
        if let Ok(mut child) = std::process::Command::new("pbcopy")
            .stdin(std::process::Stdio::piped())
            .spawn()
        {
            if let Some(mut stdin) = child.stdin.take() {
                let _ = stdin.write_all(text.as_bytes());
            }
            let _ = child.wait();
        }
    } else if cfg!(target_os = "linux") {
        if let Ok(mut child) = std::process::Command::new("wl-copy")
            .stdin(std::process::Stdio::piped())
            .spawn()
        {
            if let Some(mut stdin) = child.stdin.take() {
                let _ = stdin.write_all(text.as_bytes());
            }
            let _ = child.wait();
        } else if let Ok(mut child) = std::process::Command::new("xclip")
            .arg("-selection")
            .arg("clipboard")
            .stdin(std::process::Stdio::piped())
            .spawn()
        {
            if let Some(mut stdin) = child.stdin.take() {
                let _ = stdin.write_all(text.as_bytes());
            }
            let _ = child.wait();
        }
    }
}

// NATIVE PIPELINE DATA STREAM MAP

pub fn entry_to_val_map(entry: &VaultEntry, reveal: bool) -> Val {
    let mut m = FxIndexMap::with_hasher(fshell_hash::FxBuildHasher::default());
    m.insert(ustr("id"), Val::String(entry.id.clone()));
    m.insert(ustr("name"), Val::String(entry.name.clone()));
    m.insert(
        ustr("username"),
        entry
            .username
            .as_ref()
            .map(|s| Val::String(s.clone()))
            .unwrap_or(Val::Null),
    );

    if reveal {
        m.insert(
            ustr("password"),
            entry
                .password
                .as_ref()
                .map(|s| Val::String(s.clone()))
                .unwrap_or(Val::Null),
        );
        m.insert(
            ustr("totp_secret"),
            entry
                .totp_secret
                .as_ref()
                .map(|s| Val::String(s.clone()))
                .unwrap_or(Val::Null),
        );
    } else {
        m.insert(ustr("password"), Val::String("********".to_string()));
        m.insert(ustr("totp_secret"), Val::Null);
    }

    m.insert(
        ustr("url"),
        entry
            .url
            .as_ref()
            .map(|s| Val::String(s.clone()))
            .unwrap_or(Val::Null),
    );
    m.insert(
        ustr("notes"),
        entry
            .notes
            .as_ref()
            .map(|s| Val::String(s.clone()))
            .unwrap_or(Val::Null),
    );

    let now = SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    let totp_code = if let Some(ref sec) = entry.totp_secret {
        match generate_totp(sec, now) {
            Ok(code) => Val::String(code),
            Err(_) => Val::Null,
        }
    } else {
        Val::Null
    };
    m.insert(ustr("totp"), totp_code);

    let tags_val = Val::List(entry.tags.iter().map(|t| Val::String(t.clone())).collect());
    m.insert(ustr("tags"), tags_val);
    m.insert(ustr("created_at"), Val::String(entry.created_at.clone()));
    m.insert(ustr("updated_at"), Val::String(entry.updated_at.clone()));
    Val::Map(m)
}

// TUI: OPTION A (WATCH MODE)

pub fn watch_totps(entries: &[VaultEntry]) -> Result<(), StringError> {
    let totp_entries: Vec<&VaultEntry> =
        entries.iter().filter(|e| e.totp_secret.is_some()).collect();
    if totp_entries.is_empty() {
        return Err("No entries with TOTP secrets found in the vault."
            .to_string()
            .into());
    }

    let _raw_guard = RawModeGuard::new()?;
    let title_style = Style::new().fg(Color::Cyan).bold();
    let time_style = Style::new().fg(Color::Yellow);
    let bar_style = Style::new().fg(Color::Green);
    let name_style = Style::new().fg(Color::White).bold();
    let code_style = Style::new().fg(Color::LightGreen).bold();

    loop {
        let now = SystemTime::now()
            .duration_since(SystemTime::UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs();
        let remaining = 30 - (now % 30);

        print!("{}[2J{}[H", 27 as char, 27 as char);
        let _ = std::io::stdout().flush();

        println!(
            "{}",
            title_style.paint("=== fshell Authenticator Watch Mode ===")
        );
        println!(
            "{}",
            time_style.paint(format!("Time remaining: {}s", remaining))
        );

        let width = 30;
        let filled = (remaining as usize * width) / 30;
        let empty = width - filled;
        let bar: String = std::iter::repeat("█")
            .take(filled)
            .chain(std::iter::repeat("░").take(empty))
            .collect();
        println!("{}", bar_style.paint(format!("[{}]", bar)));
        println!();

        for entry in &totp_entries {
            let secret = entry
                .totp_secret
                .as_ref()
                .expect("totp_entries filtered to only include entries with secrets");
            let code = match generate_totp(secret, now) {
                Ok(c) => format!("{} {}", &c[0..3], &c[3..6]),
                Err(_) => "INVALID SECRET".to_string(),
            };
            println!(
                "  {:<20} : {}",
                name_style.paint(&entry.name),
                code_style.paint(code)
            );
        }
        println!("\nPress 'q' or Esc to exit...");
        let _ = std::io::stdout().flush();

        if event::poll(Duration::from_millis(500)).unwrap_or(false) {
            if let Ok(Event::Key(key_event)) = event::read() {
                if key_event.kind == crossterm::event::KeyEventKind::Press {
                    if key_event.code == KeyCode::Char('q') || key_event.code == KeyCode::Esc {
                        break;
                    }
                }
            }
        }
    }

    Ok(())
}

// TUI: OPTION B (FULL RATATUI UI EXPLICIT EXPLORER)

pub fn run_tui(path: &Path, env: &Env) -> Result<(), StringError> {
    let mut entries = load_vault_with_session(path, env)?;

    let mut stdout = std::io::stdout();
    crossterm::execute!(stdout, crossterm::terminal::EnterAlternateScreen)
        .map_err(|e| StringError::from(format!("{}", e)))?;
    let _raw_guard = RawModeGuard::new()?;

    let backend = CrosstermBackend::new(stdout);
    let mut terminal =
        Terminal::new(backend).map_err(|e| format!("Failed to init terminal: {}", e))?;

    let mut list_state = ListState::default();
    if !entries.is_empty() {
        list_state.select(Some(0));
    }

    let mut search_query = String::new();
    let mut search_mode = false;
    let mut reveal_password = false;
    let mut status_msg = "Use Arrow Keys to navigate, / to search, q to exit".to_string();
    let mut status_expiry = SystemTime::now();

    loop {
        let mut edit_target = None;
        let mut delete_target = None;
        let mut add_requested = false;
        let mut exit_requested = false;

        // Scope the immutable borrow of `entries` in `filtered`
        {
            let filtered: Vec<(usize, &VaultEntry)> = entries
                .iter()
                .enumerate()
                .filter(|(_, e)| {
                    if search_query.is_empty() {
                        true
                    } else {
                        let q = search_query.to_lowercase();
                        e.name.to_lowercase().contains(&q)
                            || e.username
                                .as_ref()
                                .map(|u| u.to_lowercase().contains(&q))
                                .unwrap_or(false)
                            || e.tags.iter().any(|t| t.to_lowercase().contains(&q))
                    }
                })
                .collect();

            let sel = list_state.selected();
            if filtered.is_empty() {
                list_state.select(None);
            } else if let Some(s) = sel {
                if s >= filtered.len() {
                    list_state.select(Some(filtered.len() - 1));
                }
            } else {
                list_state.select(Some(0));
            }

            terminal.draw(|f| {
                let chunks = Layout::default()
                    .direction(Direction::Vertical)
                    .constraints([
                        Constraint::Length(3),
                        Constraint::Min(0),
                        Constraint::Length(3),
                    ])
                    .split(f.area());

                let title = if search_mode {
                    format!(" fshell Vault Search: {}_ ", search_query)
                } else {
                    " fshell Vault Manager - Credential Explorer (Press / to search) ".to_string()
                };
                let title_block = Block::default()
                    .borders(Borders::ALL)
                    .title(title);
                f.render_widget(Paragraph::new("").block(title_block), chunks[0]);

                let mid_chunks = Layout::default()
                    .direction(Direction::Horizontal)
                    .constraints([
                        Constraint::Percentage(40),
                        Constraint::Percentage(60),
                    ])
                    .split(chunks[1]);

                let items: Vec<ListItem> = filtered.iter().map(|(_, entry)| {
                    let display = format!("{} ({})", entry.name, entry.username.as_ref().unwrap_or(&"no username".to_string()));
                    ListItem::new(display)
                }).collect();

                let list_block = List::new(items)
                    .block(Block::default().borders(Borders::ALL).title(" Credentials "))
                    .highlight_symbol("> ")
                    .highlight_style(ratatui::style::Style::default().fg(ratatui::style::Color::Cyan));
                f.render_stateful_widget(list_block, mid_chunks[0], &mut list_state);

                let active_entry = list_state.selected()
                    .and_then(|idx| filtered.get(idx))
                    .map(|(_, entry)| *entry);

                let detail_text = if let Some(entry) = active_entry {
                    let pwd = if reveal_password {
                        entry.password.as_deref().unwrap_or("")
                    } else {
                        "••••••••"
                    };
                    let now = SystemTime::now()
                        .duration_since(SystemTime::UNIX_EPOCH)
                        .unwrap_or_default()
                        .as_secs();
                    let totp = if let Some(ref sec) = entry.totp_secret {
                        match generate_totp(sec, now) {
                            Ok(code) => {
                                let rem = 30 - (now % 30);
                                format!("{} ({}s remaining)", code, rem)
                            }
                            Err(e) => format!("Error: {}", e.message),
                        }
                    } else {
                        "None".to_string()
                    };

                    format!(
                        "ID:       {}\nName:     {}\nUsername: {}\nPassword: {}  [v: reveal]\nTOTP:     {}\nURL:      {}\nTags:     {}\nCreated:  {}\nUpdated:  {}\n\nNotes:\n{}",
                        entry.id,
                        entry.name,
                        entry.username.as_deref().unwrap_or(""),
                        pwd,
                        totp,
                        entry.url.as_deref().unwrap_or(""),
                        entry.tags.join(", "),
                        entry.created_at,
                        entry.updated_at,
                        entry.notes.as_deref().unwrap_or("")
                    )
                } else {
                    "No entry selected".to_string()
                };

                let detail_block = Paragraph::new(detail_text)
                    .block(Block::default().borders(Borders::ALL).title(" Secret Details "))
                    .wrap(Wrap { trim: true });
                f.render_widget(detail_block, mid_chunks[1]);

                let display_status = if SystemTime::now() < status_expiry {
                    status_msg.clone()
                } else {
                    "Controls: [q]uit [a]dd [e]edit [d]elete [/]search [p]assword [t]otp [u]sername".to_string()
                };
                let status_block = Paragraph::new(display_status)
                    .block(Block::default().borders(Borders::ALL));
                f.render_widget(status_block, chunks[2]);
            }).map_err(|e| format!("Draw error: {}", e))?;

            if event::poll(Duration::from_millis(200)).unwrap_or(false) {
                if let Ok(Event::Key(key_event)) = event::read() {
                    if key_event.kind == crossterm::event::KeyEventKind::Press {
                        if search_mode {
                            match key_event.code {
                                KeyCode::Enter | KeyCode::Esc => {
                                    search_mode = false;
                                }
                                KeyCode::Backspace => {
                                    search_query.pop();
                                }
                                KeyCode::Char(c) => {
                                    search_query.push(c);
                                }
                                _ => {}
                            }
                        } else {
                            match key_event.code {
                                KeyCode::Char('q') | KeyCode::Esc => {
                                    exit_requested = true;
                                }
                                KeyCode::Char('/') => {
                                    search_mode = true;
                                }
                                KeyCode::Up => {
                                    let curr = list_state.selected().unwrap_or(0);
                                    if curr > 0 {
                                        list_state.select(Some(curr - 1));
                                        reveal_password = false;
                                    }
                                }
                                KeyCode::Down => {
                                    let curr = list_state.selected().unwrap_or(0);
                                    if !filtered.is_empty() && curr < filtered.len() - 1 {
                                        list_state.select(Some(curr + 1));
                                        reveal_password = false;
                                    }
                                }
                                KeyCode::Char('v') => {
                                    reveal_password = !reveal_password;
                                }
                                KeyCode::Char('p') => {
                                    if let Some((_, entry)) =
                                        list_state.selected().and_then(|idx| filtered.get(idx))
                                    {
                                        if let Some(ref pwd) = entry.password {
                                            copy_to_system_clipboard(pwd);
                                            status_msg =
                                                "Password copied to system clipboard!".to_string();
                                            status_expiry =
                                                SystemTime::now() + Duration::from_secs(3);
                                        }
                                    }
                                }
                                KeyCode::Char('u') => {
                                    if let Some((_, entry)) =
                                        list_state.selected().and_then(|idx| filtered.get(idx))
                                    {
                                        if let Some(ref u) = entry.username {
                                            copy_to_system_clipboard(u);
                                            status_msg =
                                                "Username copied to system clipboard!".to_string();
                                            status_expiry =
                                                SystemTime::now() + Duration::from_secs(3);
                                        }
                                    }
                                }
                                KeyCode::Char('t') => {
                                    if let Some((_, entry)) =
                                        list_state.selected().and_then(|idx| filtered.get(idx))
                                    {
                                        if let Some(ref sec) = entry.totp_secret {
                                            let now = SystemTime::now()
                                                .duration_since(SystemTime::UNIX_EPOCH)
                                                .unwrap_or_default()
                                                .as_secs();
                                            if let Ok(code) = generate_totp(sec, now) {
                                                copy_to_system_clipboard(&code);
                                                status_msg =
                                                    "TOTP copied to system clipboard!".to_string();
                                                status_expiry =
                                                    SystemTime::now() + Duration::from_secs(3);
                                            }
                                        }
                                    }
                                }
                                KeyCode::Char('a') => {
                                    add_requested = true;
                                }
                                KeyCode::Char('e') => {
                                    if let Some((db_idx, _)) =
                                        list_state.selected().and_then(|idx| filtered.get(idx))
                                    {
                                        edit_target = Some(*db_idx);
                                    }
                                }
                                KeyCode::Char('d') => {
                                    if let Some((db_idx, _)) =
                                        list_state.selected().and_then(|idx| filtered.get(idx))
                                    {
                                        delete_target = Some(*db_idx);
                                    }
                                }
                                _ => {}
                            }
                        }
                    }
                }
            }
        } // `filtered` dropped here, releasing `entries` borrow.

        if exit_requested {
            break;
        }

        if add_requested {
            crossterm::execute!(std::io::stdout(), crossterm::terminal::LeaveAlternateScreen).ok();
            crossterm::terminal::disable_raw_mode().ok();

            println!("\n--- Add New Entry ---");
            if let Err(e) = interactive_add(path, &mut entries, env) {
                println!("Error adding entry: {}", e.message);
                let _ = read_password_prompt("Press Enter to return...");
            } else {
                println!("Entry added successfully!");
                let _ = read_password_prompt("Press Enter to return...");
            }

            crossterm::terminal::enable_raw_mode().ok();
            crossterm::execute!(std::io::stdout(), crossterm::terminal::EnterAlternateScreen).ok();
            terminal.clear().ok();
        }

        if let Some(db_idx) = edit_target {
            crossterm::execute!(std::io::stdout(), crossterm::terminal::LeaveAlternateScreen).ok();
            crossterm::terminal::disable_raw_mode().ok();

            println!("\n--- Edit Entry ---");
            if let Err(e) = interactive_edit(path, &mut entries, db_idx, env) {
                println!("Error editing entry: {}", e.message);
                let _ = read_password_prompt("Press Enter to return...");
            } else {
                println!("Entry modified successfully!");
                let _ = read_password_prompt("Press Enter to return...");
            }

            crossterm::terminal::enable_raw_mode().ok();
            crossterm::execute!(std::io::stdout(), crossterm::terminal::EnterAlternateScreen).ok();
            terminal.clear().ok();
        }

        if let Some(db_idx) = delete_target {
            crossterm::execute!(std::io::stdout(), crossterm::terminal::LeaveAlternateScreen).ok();
            crossterm::terminal::disable_raw_mode().ok();

            let name = entries[db_idx].name.clone();
            println!("\nDelete entry '{}'? [y/N]", name);
            let mut confirm = String::new();
            if std::io::stdin().read_line(&mut confirm).is_ok()
                && confirm.trim().to_lowercase() == "y"
            {
                entries.remove(db_idx);
                if let Err(e) = save_vault_with_session(path, &entries, env) {
                    println!("Failed to save changes: {}", e.message);
                    let _ = read_password_prompt("Press Enter to return...");
                }
            }

            crossterm::terminal::enable_raw_mode().ok();
            crossterm::execute!(std::io::stdout(), crossterm::terminal::EnterAlternateScreen).ok();
            terminal.clear().ok();
        }
    }

    crossterm::execute!(std::io::stdout(), crossterm::terminal::LeaveAlternateScreen).ok();
    Ok(())
}

fn interactive_add(
    path: &Path,
    entries: &mut Vec<VaultEntry>,
    env: &Env,
) -> Result<(), StringError> {
    print!("Entry Name: ");
    let _ = std::io::stdout().flush();
    let mut name = String::new();
    std::io::stdin()
        .read_line(&mut name)
        .map_err(|e| StringError::from(format!("{}", e)))?;
    let name = name.trim().to_string();
    if name.is_empty() {
        return Err(StringError::from("Name cannot be empty"));
    }

    print!("Username / Account: ");
    let _ = std::io::stdout().flush();
    let mut username = String::new();
    std::io::stdin()
        .read_line(&mut username)
        .map_err(|e| StringError::from(format!("{}", e)))?;
    let username = username.trim().to_string();
    let username_opt = if username.is_empty() {
        None
    } else {
        Some(username)
    };

    let mut password_str = read_password_prompt("Password (leave blank to generate): ")?;
    if password_str.is_empty() {
        println!("Generate password? [Y/n]");
        let mut confirm = String::new();
        let _ = std::io::stdin().read_line(&mut confirm);
        if confirm.trim().to_lowercase() != "n" {
            println!("Choose type: 1) Password characters  2) Passphrase");
            let mut choice = String::new();
            let _ = std::io::stdin().read_line(&mut choice);
            if choice.trim() == "2" {
                password_str = generate_passphrase(4, Separator::Hyphen);
            } else {
                password_str = generate_random_password(16, true, true, true, true);
            }
            println!("Generated: {}", password_str);
        }
    }

    print!("URL: ");
    let _ = std::io::stdout().flush();
    let mut url = String::new();
    std::io::stdin()
        .read_line(&mut url)
        .map_err(|e| StringError::from(format!("{}", e)))?;
    let url = url.trim().to_string();
    let url_opt = if url.is_empty() { None } else { Some(url) };

    print!("TOTP Base32 Secret: ");
    let _ = std::io::stdout().flush();
    let mut totp = String::new();
    std::io::stdin()
        .read_line(&mut totp)
        .map_err(|e| StringError::from(format!("{}", e)))?;
    let totp = totp.trim().to_string();
    let totp_opt = if totp.is_empty() { None } else { Some(totp) };

    print!("Tags (comma separated): ");
    let _ = std::io::stdout().flush();
    let mut tags_str = String::new();
    std::io::stdin()
        .read_line(&mut tags_str)
        .map_err(|e| StringError::from(format!("{}", e)))?;
    let tags: Vec<String> = tags_str
        .split(',')
        .map(|t| t.trim().to_string())
        .filter(|t| !t.is_empty())
        .collect();

    print!("Notes: ");
    let _ = std::io::stdout().flush();
    let mut notes = String::new();
    std::io::stdin()
        .read_line(&mut notes)
        .map_err(|e| StringError::from(format!("{}", e)))?;
    let notes = notes.trim().to_string();
    let notes_opt = if notes.is_empty() { None } else { Some(notes) };

    let now_str = chrono::Utc::now().to_rfc3339();

    let new_entry = VaultEntry {
        id: generate_id(),
        name,
        username: username_opt,
        password: if password_str.is_empty() {
            None
        } else {
            Some(password_str)
        },
        url: url_opt,
        totp_secret: totp_opt,
        notes: notes_opt,
        tags,
        created_at: now_str.clone(),
        updated_at: now_str,
    };

    entries.push(new_entry);
    save_vault_with_session(path, entries, env)?;
    Ok(())
}

fn interactive_edit(
    path: &Path,
    entries: &mut [VaultEntry],
    idx: usize,
    env: &Env,
) -> Result<(), StringError> {
    let entry = &mut entries[idx];

    print!("Entry Name [{}]: ", entry.name);
    let _ = std::io::stdout().flush();
    let mut name = String::new();
    std::io::stdin()
        .read_line(&mut name)
        .map_err(|e| StringError::from(format!("{}", e)))?;
    let name = name.trim().to_string();
    if !name.is_empty() {
        entry.name = name;
    }

    print!(
        "Username / Account [{}]: ",
        entry.username.as_deref().unwrap_or("")
    );
    let _ = std::io::stdout().flush();
    let mut username = String::new();
    std::io::stdin()
        .read_line(&mut username)
        .map_err(|e| StringError::from(format!("{}", e)))?;
    let username = username.trim().to_string();
    if !username.is_empty() {
        entry.username = Some(username);
    }

    let current_pwd = entry.password.as_deref().unwrap_or("");
    let password_str =
        read_password_prompt(&format!("Password [{}] (blank to keep): ", current_pwd))?;
    if !password_str.is_empty() {
        entry.password = Some(password_str);
    }

    print!("URL [{}]: ", entry.url.as_deref().unwrap_or(""));
    let _ = std::io::stdout().flush();
    let mut url = String::new();
    std::io::stdin()
        .read_line(&mut url)
        .map_err(|e| StringError::from(format!("{}", e)))?;
    let url = url.trim().to_string();
    if !url.is_empty() {
        entry.url = Some(url);
    }

    print!(
        "TOTP Base32 Secret [{}]: ",
        entry.totp_secret.as_deref().unwrap_or("")
    );
    let _ = std::io::stdout().flush();
    let mut totp = String::new();
    std::io::stdin()
        .read_line(&mut totp)
        .map_err(|e| StringError::from(format!("{}", e)))?;
    let totp = totp.trim().to_string();
    if !totp.is_empty() {
        entry.totp_secret = Some(totp);
    }

    print!("Tags (comma separated) [{}]: ", entry.tags.join(", "));
    let _ = std::io::stdout().flush();
    let mut tags_str = String::new();
    std::io::stdin()
        .read_line(&mut tags_str)
        .map_err(|e| StringError::from(format!("{}", e)))?;
    let tags_str = tags_str.trim().to_string();
    if !tags_str.is_empty() {
        entry.tags = tags_str
            .split(',')
            .map(|t| t.trim().to_string())
            .filter(|t| !t.is_empty())
            .collect();
    }

    print!("Notes [{}]: ", entry.notes.as_deref().unwrap_or(""));
    let _ = std::io::stdout().flush();
    let mut notes = String::new();
    std::io::stdin()
        .read_line(&mut notes)
        .map_err(|e| StringError::from(format!("{}", e)))?;
    let notes = notes.trim().to_string();
    if !notes.is_empty() {
        entry.notes = Some(notes);
    }

    entry.updated_at = chrono::Utc::now().to_rfc3339();

    save_vault_with_session(path, entries, env)?;
    Ok(())
}

// BUILTIN DISPATCH & HELPERS

pub fn vault_builtin(
    _in_rx: Option<PipeStream>,
    args: Vec<Val>,
    env: &Env,
    tx: PipeSender,
) -> Result<(), StringError> {
    let mut subcommand = "ui".to_string();
    let mut sub_args = Vec::new();

    if !args.is_empty() {
        if let Val::String(ref s) = args[0] {
            if s == "-h" || s == "--help" || s == "help" {
                return show_vault_help(tx);
            }
            subcommand = s.clone();
            sub_args = args[1..].to_vec();
        } else {
            return Err("vault: expected a subcommand string".to_string().into());
        }
    }

    let path = get_vault_path()?;

    env.enforce_capability("vault", CapAction::ReadFile(path.clone()))?;
    if subcommand == "init" || subcommand == "add" || subcommand == "edit" || subcommand == "delete"
    {
        env.enforce_capability("vault", CapAction::WriteFile(path.clone()))?;
    }

    match subcommand.as_str() {
        "init" => {
            if path.exists() {
                return Err("Vault database already exists! Delete it manually if you want to reinitialize.".to_string().into());
            }
            let pwd1 = read_password_prompt("Set master password: ")?;
            let pwd2 = read_password_prompt("Confirm master password: ")?;
            if pwd1 != pwd2 {
                return Err("Passwords do not match".to_string().into());
            }
            save_vault_file(&path, &[], &pwd1)?;

            drop(tokio::spawn(async move {
                let _ = tx
                    .send(PipelinePayload::Data(Arc::new(Val::String(
                        "Vault successfully initialized.".to_string(),
                    ))))
                    .await;
            }));
        }
        "unlock" => {
            let pwd = read_password_prompt("Enter master password: ")?;
            let entries = load_vault_file(&path, &pwd)?;

            let mut salt = [0u8; 16];
            let mut f = std::fs::File::open(&path).map_err(|e| format!("Failed to open: {}", e))?;
            let enc_vault: EncryptedVault =
                serde_json::from_reader(&mut f).map_err(|e| format!("Corrupt: {}", e))?;
            let salt_vec = hex_decode(&enc_vault.salt).map_err(|_| "Invalid salt")?;
            salt.copy_from_slice(&salt_vec);

            let (enc_key, mac_key) = derive_keys(&pwd, &salt);
            set_session_keys(enc_key, mac_key);

            let count = entries.len();
            drop(tokio::spawn(async move {
                let _ = tx
                    .send(PipelinePayload::Data(Arc::new(Val::String(format!(
                        "Vault unlocked! Loaded {} entries into session.",
                        count
                    )))))
                    .await;
            }));
        }
        "lock" => {
            clear_session_keys();
            drop(tokio::spawn(async move {
                let _ = tx
                    .send(PipelinePayload::Data(Arc::new(Val::String(
                        "Session locked. Memory keys cleared.".to_string(),
                    ))))
                    .await;
            }));
        }
        "status" => {
            let exists = path.exists();
            let unlocked = get_active_session_keys().is_some();
            let mut out = format!(
                "Vault Database Path: {}\nInitialized: {}\nUnlocked Session: {}\n",
                path.display(),
                exists,
                unlocked
            );
            let count_opt = if unlocked {
                load_vault_with_session(&path, env).ok().map(|e| e.len())
            } else {
                None
            };
            if let Some(count) = count_opt {
                out.push_str(&format!("Secret Count: {}\n", count));
            }
            drop(tokio::spawn(async move {
                let _ = tx
                    .send(PipelinePayload::Data(Arc::new(Val::String(out))))
                    .await;
            }));
        }
        "list" => {
            let mut reveal = false;
            for arg in &sub_args {
                if matches!(arg, Val::String(s) if s == "--reveal") {
                    reveal = true;
                }
            }
            let entries = load_vault_with_session(&path, env)?;
            let env_clone = env.clone();

            tokio::spawn(async move {
                if !env_clone.is_captured && env_clone.is_last_stage {
                    let mut out = format!(
                        "{:<10} | {:<20} | {:<20} | {:<10}\n",
                        "ID", "Name", "Username", "Tags"
                    );
                    out.push_str(&"-".repeat(70));
                    out.push('\n');
                    for entry in &entries {
                        let id = if entry.id.len() >= 8 {
                            &entry.id[..8]
                        } else {
                            &entry.id
                        };
                        let tags = entry.tags.join(",");
                        out.push_str(&format!(
                            "{:<10} | {:<20} | {:<20} | {:<10}\n",
                            id,
                            entry.name,
                            entry.username.as_deref().unwrap_or(""),
                            tags
                        ));
                    }
                    let _ = tx
                        .send(PipelinePayload::Data(Arc::new(Val::String(out))))
                        .await;
                } else {
                    for entry in entries {
                        let map = entry_to_val_map(&entry, reveal);
                        if tx.send(PipelinePayload::Data(Arc::new(map))).await.is_err() {
                            break;
                        }
                    }
                }
            });
        }
        "get" => {
            if sub_args.is_empty() {
                return Err("vault get: expected entry name or ID".to_string().into());
            }
            let target = sub_args[0].to_text();
            let entries = load_vault_with_session(&path, env)?;
            let found = entries
                .iter()
                .find(|e| e.id == target || e.name.to_lowercase() == target.to_lowercase());

            if let Some(entry) = found {
                let entry_clone = entry.clone();
                let env_clone = env.clone();
                tokio::spawn(async move {
                    if !env_clone.is_captured && env_clone.is_last_stage {
                        let mut out = format!(
                            "ID:       {}\nName:     {}\nUsername: {}\nPassword: {}\nURL:      {}\n",
                            entry_clone.id,
                            entry_clone.name,
                            entry_clone.username.as_deref().unwrap_or(""),
                            entry_clone.password.as_deref().unwrap_or(""),
                            entry_clone.url.as_deref().unwrap_or("")
                        );
                        if let Some(ref sec) = entry_clone.totp_secret {
                            let now = SystemTime::now()
                                .duration_since(SystemTime::UNIX_EPOCH)
                                .unwrap_or_default()
                                .as_secs();
                            if let Ok(code) = generate_totp(sec, now) {
                                out.push_str(&format!("TOTP:     {}\n", code));
                            }
                        }
                        if let Some(ref notes) = entry_clone.notes {
                            out.push_str(&format!("\nNotes:\n{}\n", notes));
                        }
                        let _ = tx
                            .send(PipelinePayload::Data(Arc::new(Val::String(out))))
                            .await;
                    } else {
                        let map = entry_to_val_map(&entry_clone, true);
                        let _ = tx.send(PipelinePayload::Data(Arc::new(map))).await;
                    }
                });
            } else {
                return Err(BuiltinError::NotFound {
                    cmd: "vault".into(),
                    what: format!("entry '{target}'"),
                    span: None,
                }
                .into());
            }
        }
        "add" => {
            let mut name = None;
            let mut username = None;
            let mut password = None;
            let mut url = None;
            let mut totp = None;
            let mut tags = Vec::new();
            let mut notes = None;
            let mut gen_flag = false;
            let mut _words_count = 4;

            let mut i = 0;
            while i < sub_args.len() {
                match &sub_args[i] {
                    Val::String(s) => {
                        if s == "-u" || s == "--username" {
                            if i + 1 < sub_args.len() {
                                username = Some(sub_args[i + 1].to_text());
                                i += 2;
                            } else {
                                return Err("Missing username argument".to_string().into());
                            }
                        } else if s == "-p" || s == "--password" {
                            if i + 1 < sub_args.len() {
                                password = Some(sub_args[i + 1].to_text());
                                i += 2;
                            } else {
                                return Err("Missing password argument".to_string().into());
                            }
                        } else if s == "-l" || s == "--url" {
                            if i + 1 < sub_args.len() {
                                url = Some(sub_args[i + 1].to_text());
                                i += 2;
                            } else {
                                return Err("Missing url argument".to_string().into());
                            }
                        } else if s == "-s" || s == "--totp-secret" {
                            if i + 1 < sub_args.len() {
                                totp = Some(sub_args[i + 1].to_text());
                                i += 2;
                            } else {
                                return Err("Missing totp-secret argument".to_string().into());
                            }
                        } else if s == "-t" || s == "--tags" {
                            if i + 1 < sub_args.len() {
                                tags = sub_args[i + 1]
                                    .to_text()
                                    .split(',')
                                    .map(|t| t.trim().to_string())
                                    .filter(|t| !t.is_empty())
                                    .collect();
                                i += 2;
                            } else {
                                return Err("Missing tags argument".to_string().into());
                            }
                        } else if s == "-n" || s == "--notes" {
                            if i + 1 < sub_args.len() {
                                notes = Some(sub_args[i + 1].to_text());
                                i += 2;
                            } else {
                                return Err("Missing notes argument".to_string().into());
                            }
                        } else if s == "--gen" {
                            gen_flag = true;
                            i += 1;
                        } else if s == "--words" {
                            if i + 1 < sub_args.len() {
                                if let Ok(n) = sub_args[i + 1].to_text().parse::<usize>() {
                                    _words_count = n;
                                }
                                i += 2;
                            } else {
                                return Err("Missing words argument".to_string().into());
                            }
                        } else if s.starts_with('-') {
                            return Err(BuiltinError::InvalidArgument {
                                cmd: "vault".into(),
                                arg: format!("unknown option '{s}'"),
                                span: None,
                            }
                            .into());
                        } else {
                            name = Some(s.clone());
                            i += 1;
                        }
                    }
                    other => {
                        name = Some(other.to_text());
                        i += 1;
                    }
                }
            }

            let mut entries = load_vault_with_session(&path, env)?;

            if let Some(target_name) = name {
                let final_pwd = if let Some(p) = password {
                    p
                } else if gen_flag {
                    generate_random_password(16, true, true, true, true)
                } else {
                    String::new()
                };

                let now_str = chrono::Utc::now().to_rfc3339();
                let new_entry = VaultEntry {
                    id: generate_id(),
                    name: target_name,
                    username,
                    password: if final_pwd.is_empty() {
                        None
                    } else {
                        Some(final_pwd)
                    },
                    url,
                    totp_secret: totp,
                    notes,
                    tags,
                    created_at: now_str.clone(),
                    updated_at: now_str,
                };
                entries.push(new_entry);
                save_vault_with_session(&path, &entries, env)?;

                drop(tokio::spawn(async move {
                    let _ = tx
                        .send(PipelinePayload::Data(Arc::new(Val::String(
                            "Entry added.".to_string(),
                        ))))
                        .await;
                }));
            } else {
                interactive_add(&path, &mut entries, env)?;
                drop(tokio::spawn(async move {
                    let _ = tx
                        .send(PipelinePayload::Data(Arc::new(Val::String(
                            "Entry added via interactive wizard.".to_string(),
                        ))))
                        .await;
                }));
            }
        }
        "edit" => {
            if sub_args.is_empty() {
                return Err("vault edit: expected name or ID".to_string().into());
            }
            let target = sub_args[0].to_text();
            let mut entries = load_vault_with_session(&path, env)?;
            let idx = entries
                .iter()
                .position(|e| e.id == target || e.name.to_lowercase() == target.to_lowercase())
                .ok_or_else(|| format!("Entry '{}' not found", target))?;

            interactive_edit(&path, &mut entries, idx, env)?;

            drop(tokio::spawn(async move {
                let _ = tx
                    .send(PipelinePayload::Data(Arc::new(Val::String(
                        "Entry updated.".to_string(),
                    ))))
                    .await;
            }));
        }
        "delete" => {
            if sub_args.is_empty() {
                return Err("vault delete: expected name or ID".to_string().into());
            }
            let target = sub_args[0].to_text();
            let mut force = false;
            for arg in &sub_args[1..] {
                if matches!(arg, Val::String(s) if s == "-f" || s == "--force") {
                    force = true;
                }
            }

            let mut entries = load_vault_with_session(&path, env)?;
            let idx = entries
                .iter()
                .position(|e| e.id == target || e.name.to_lowercase() == target.to_lowercase())
                .ok_or_else(|| format!("Entry '{}' not found", target))?;

            if !force {
                print!("Delete entry '{}'? [y/N]: ", entries[idx].name);
                let _ = std::io::stdout().flush();
                let mut confirm = String::new();
                std::io::stdin()
                    .read_line(&mut confirm)
                    .map_err(|e| StringError::from(format!("{}", e)))?;
                if confirm.trim().to_lowercase() != "y" {
                    return Err("Delete aborted".to_string().into());
                }
            }

            entries.remove(idx);
            save_vault_with_session(&path, &entries, env)?;

            drop(tokio::spawn(async move {
                let _ = tx
                    .send(PipelinePayload::Data(Arc::new(Val::String(
                        "Entry deleted successfully.".to_string(),
                    ))))
                    .await;
            }));
        }
        "gen" | "generate" => {
            let mut length = 16;
            let mut words_count = 5;
            let mut passphrase_mode = false;
            let mut separator = Separator::Hyphen;
            let mut no_symbols = false;

            let mut i = 0;
            while i < sub_args.len() {
                if let Val::String(s) = &sub_args[i] {
                    if s == "-l" || s == "--length" {
                        if i + 1 < sub_args.len() {
                            if let Ok(n) = sub_args[i + 1].to_text().parse::<usize>() {
                                length = n;
                            }
                            i += 2;
                        } else {
                            return Err("Missing length argument".to_string().into());
                        }
                    } else if s == "-w" || s == "--words" {
                        passphrase_mode = true;
                        if i + 1 < sub_args.len() {
                            if let Ok(n) = sub_args[i + 1].to_text().parse::<usize>() {
                                words_count = n;
                            }
                            i += 2;
                        } else {
                            return Err("Missing words count argument".to_string().into());
                        }
                    } else if s == "--passphrase" {
                        passphrase_mode = true;
                        i += 1;
                    } else if s == "--separator" {
                        if i + 1 < sub_args.len() {
                            separator = match sub_args[i + 1].to_text().as_str() {
                                "space" => Separator::Space,
                                "camel" => Separator::Camel,
                                "numeric" => Separator::Numeric,
                                _ => Separator::Hyphen,
                            };
                            i += 2;
                        } else {
                            return Err("Missing separator value".to_string().into());
                        }
                    } else if s == "--no-symbols" {
                        no_symbols = true;
                        i += 1;
                    } else {
                        i += 1;
                    }
                } else {
                    i += 1;
                }
            }

            let result = if passphrase_mode {
                if words_count < 3 {
                    return Err("Passphrase must consist of at least 3 words"
                        .to_string()
                        .into());
                }
                generate_passphrase(words_count, separator)
            } else {
                generate_random_password(length, true, true, true, !no_symbols)
            };

            drop(tokio::spawn(async move {
                let _ = tx
                    .send(PipelinePayload::Data(Arc::new(Val::String(result))))
                    .await;
            }));
        }
        "totp" => {
            if sub_args.is_empty() {
                return Err("vault totp: expected name or ID".to_string().into());
            }
            let target = sub_args[0].to_text();
            let mut copy_flag = false;
            for arg in &sub_args[1..] {
                if matches!(arg, Val::String(s) if s == "-c" || s == "--copy") {
                    copy_flag = true;
                }
            }

            let entries = load_vault_with_session(&path, env)?;
            let entry = entries
                .iter()
                .find(|e| e.id == target || e.name.to_lowercase() == target.to_lowercase())
                .ok_or_else(|| format!("Entry '{}' not found", target))?;

            let secret = entry
                .totp_secret
                .as_ref()
                .ok_or("No TOTP secret set for this credential")?;
            let now = SystemTime::now()
                .duration_since(SystemTime::UNIX_EPOCH)
                .unwrap_or_default()
                .as_secs();
            let code = generate_totp(secret, now)?;

            if copy_flag {
                copy_to_system_clipboard(&code);
            }

            drop(tokio::spawn(async move {
                let msg = if copy_flag {
                    format!("{} (Copied to system clipboard!)", code)
                } else {
                    code
                };
                let _ = tx
                    .send(PipelinePayload::Data(Arc::new(Val::String(msg))))
                    .await;
            }));
        }
        "watch" => {
            let entries = load_vault_with_session(&path, env)?;
            watch_totps(&entries)?;
        }
        "ui" => {
            run_tui(&path, env)?;
        }
        other => {
            return Err(BuiltinError::InvalidArgument {
                cmd: "vault".into(),
                arg: format!("unknown subcommand '{other}'"),
                span: None,
            }
            .into());
        }
    }

    Ok(())
}

fn show_vault_help(tx: PipeSender) -> Result<(), StringError> {
    let help = "Usage: vault <subcommand> [args...]

fshell Secure Secrets Vault & Authenticator Manager

Subcommands:
  init                  Initialize a new vault file in ~/.config/fsh/vault.enc
  unlock                Prompts for master password and unlocks session keys in memory
  lock                  Clears stored session keys from memory
  status                Check the vault file status and lock state
  list [--reveal]       Lists all secrets in the vault (password/totps hidden by default)
  get <name|id>         Get details of a single credential entry (unmasked to terminal)
  add [flags]           Add a new credentials entry (spawns wizard if no args are set)
                        Flags: -u <user> -p <pass> -l <url> -s <totp-secret> -t <tags> -n <notes> --gen
  edit <name|id>        Edit an existing credential entry using interactive wizard
  delete <name|id> [-f] Delete credential (spawns confirmation unless --force / -f is set)
  totp <name|id> [-c]   Generate active 6-digit TOTP code (copy to clipboard using -c / --copy)
  watch                 Launches raw-terminal live updating watch mode for TOTPs
  ui                    Launches alternate-screen full Ratatui credential browser interface

Standalone Generators:
  gen / generate [flags]
                        Generate random secure string.
                        Flags:
                          -l, --length <N>     Length of character password (default: 16)
                          --no-symbols         Omit symbols in character password
                          --passphrase         Activate passphrase generation
                          -w, --words <N>      Number of words in passphrase (default: 5)
                          --separator <sep>    Separator: hyphen, space, camel, numeric
";
    tokio::spawn(async move {
        let _ = tx
            .send(PipelinePayload::Data(Arc::new(Val::String(
                help.to_string(),
            ))))
            .await;
    });
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_hex_roundtrip() {
        let original = b"hello world 123 !@#";
        let hex_str = hex_encode(original);
        let decoded = hex_decode(&hex_str).unwrap();
        assert_eq!(original.to_vec(), decoded);
    }

    #[test]
    fn test_padding_roundtrip() {
        let original = b"plaintext data block to pad";
        let padded = pad_data(original);
        assert_eq!(padded.len() % 4096, 0);
        let unpadded = unpad_data(&padded).unwrap();
        assert_eq!(original.to_vec(), unpadded);
    }

    #[test]
    fn test_encryption_roundtrip() {
        let key = [0x55u8; 32];
        let original = b"secret message to encrypt";
        let padded = pad_data(original);
        let (ciphertext, iv) = encrypt_payload(&key, &padded);
        let decrypted_padded = decrypt_payload(&key, &ciphertext, &iv);
        let decrypted = unpad_data(&decrypted_padded).unwrap();
        assert_eq!(original.to_vec(), decrypted);
    }

    #[test]
    fn test_totp_decoding_and_generation() {
        let secret = "JBSWY3DPEHPK3PXP"; // "Hello!\xde\xad\xbe\xef" in base32
        let decoded = base32_decode(secret).unwrap();
        assert_eq!(decoded, b"Hello!\xde\xad\xbe\xef");

        let now = 1600000000;
        let totp = generate_totp(secret, now);
        assert!(totp.is_ok());
        let code = totp.unwrap();
        assert_eq!(code.len(), 6);
        assert!(code.chars().all(|c| c.is_ascii_digit()));
    }

    #[test]
    fn test_password_generation() {
        let pwd = generate_random_password(24, true, true, true, true);
        assert_eq!(pwd.len(), 24);

        let passphrase = generate_passphrase(5, Separator::Hyphen);
        let words: Vec<&str> = passphrase.split('-').collect();
        assert_eq!(words.len(), 5);
    }
}
