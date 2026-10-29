use super::*;

pub(super) fn read_ssh_binary_string<'a>(
    bytes: &'a [u8],
    cursor: &mut usize,
    field: &str,
) -> Result<&'a [u8], String> {
    let end = cursor
        .checked_add(4)
        .ok_or_else(|| format!("OpenSSH 私钥的 {field} 长度无效"))?;
    if end > bytes.len() {
        return Err(format!("OpenSSH 私钥缺少 {field} 长度"));
    }
    let length = u32::from_be_bytes(bytes[*cursor..end].try_into().unwrap()) as usize;
    *cursor = end;
    let end = cursor
        .checked_add(length)
        .ok_or_else(|| format!("OpenSSH 私钥的 {field} 长度无效"))?;
    if end > bytes.len() {
        return Err(format!("OpenSSH 私钥的 {field} 内容不完整"));
    }
    let value = &bytes[*cursor..end];
    *cursor = end;
    Ok(value)
}

pub(super) fn read_ssh_u32(bytes: &[u8], cursor: &mut usize, field: &str) -> Result<u32, String> {
    let end = cursor
        .checked_add(4)
        .ok_or_else(|| format!("OpenSSH 私钥的 {field} 无效"))?;
    if end > bytes.len() {
        return Err(format!("OpenSSH 私钥缺少 {field}"));
    }
    let value = u32::from_be_bytes(bytes[*cursor..end].try_into().unwrap());
    *cursor = end;
    Ok(value)
}

pub(super) fn der_length(length: usize) -> Vec<u8> {
    if length < 128 {
        return vec![length as u8];
    }
    let mut bytes = Vec::new();
    let mut value = length;
    while value > 0 {
        bytes.push((value & 0xff) as u8);
        value >>= 8;
    }
    bytes.reverse();
    let mut result = vec![0x80 | bytes.len() as u8];
    result.extend(bytes);
    result
}

pub(super) fn der_tlv(tag: u8, value: &[u8]) -> Vec<u8> {
    let mut result = vec![tag];
    result.extend(der_length(value.len()));
    result.extend(value);
    result
}

pub(super) fn der_sequence(parts: impl IntoIterator<Item = Vec<u8>>) -> Vec<u8> {
    let body = parts.into_iter().flatten().collect::<Vec<_>>();
    der_tlv(0x30, &body)
}

pub(super) fn der_integer_bytes(bytes: &[u8]) -> Vec<u8> {
    let first_nonzero = bytes
        .iter()
        .position(|byte| *byte != 0)
        .unwrap_or(bytes.len());
    let mut value = bytes[first_nonzero..].to_vec();
    if value.is_empty() {
        value.push(0);
    } else if value[0] & 0x80 != 0 {
        value.insert(0, 0);
    }
    der_tlv(0x02, &value)
}

pub(super) fn der_integer_biguint(value: &BigUint) -> Vec<u8> {
    der_integer_bytes(&value.to_bytes_be())
}

pub(super) fn pem_encode(label: &str, der: &[u8]) -> String {
    let encoded = base64::engine::general_purpose::STANDARD.encode(der);
    let mut pem = format!("-----BEGIN {label}-----\n");
    for chunk in encoded.as_bytes().chunks(64) {
        pem.push_str(std::str::from_utf8(chunk).unwrap_or_default());
        pem.push('\n');
    }
    pem.push_str(&format!("-----END {label}-----\n"));
    pem
}

pub(super) fn openssh_private_key_to_pem(contents: &str) -> Result<String, String> {
    let encoded = contents
        .lines()
        .filter(|line| !line.starts_with("-----"))
        .collect::<String>();
    let bytes = base64::engine::general_purpose::STANDARD
        .decode(encoded)
        .map_err(|error| format!("OpenSSH 私钥 Base64 无效: {error}"))?;
    let mut cursor = 0usize;
    if !bytes.starts_with(b"openssh-key-v1\0") {
        return Err("不是有效的 OpenSSH 私钥".to_string());
    }
    cursor += b"openssh-key-v1\0".len();
    let cipher = read_ssh_binary_string(&bytes, &mut cursor, "加密算法")?;
    let kdf = read_ssh_binary_string(&bytes, &mut cursor, "KDF")?;
    let _kdf_options = read_ssh_binary_string(&bytes, &mut cursor, "KDF 参数")?;
    if cipher != b"none" || kdf != b"none" {
        return Err("暂不支持加密的新版 OpenSSH 私钥，请使用 SSH Agent 或未加密私钥".to_string());
    }
    if read_ssh_u32(&bytes, &mut cursor, "公钥数量")? != 1 {
        return Err("暂只支持包含一个密钥的 OpenSSH 私钥".to_string());
    }
    let _public_blob = read_ssh_binary_string(&bytes, &mut cursor, "公钥数据")?;
    let private_blob = read_ssh_binary_string(&bytes, &mut cursor, "私钥数据")?;
    let mut private_cursor = 0usize;
    let check_one = read_ssh_u32(private_blob, &mut private_cursor, "校验值")?;
    let check_two = read_ssh_u32(private_blob, &mut private_cursor, "校验值")?;
    if check_one != check_two {
        return Err("OpenSSH 私钥校验值不匹配，文件可能已损坏".to_string());
    }
    let key_type = read_ssh_binary_string(private_blob, &mut private_cursor, "密钥类型")?;
    match key_type {
        b"ssh-ed25519" => {
            let public_key =
                read_ssh_binary_string(private_blob, &mut private_cursor, "Ed25519 公钥")?;
            let private_key =
                read_ssh_binary_string(private_blob, &mut private_cursor, "Ed25519 私钥")?;
            if public_key.len() != 32
                || private_key.len() != 64
                || private_key[32..] != public_key[..]
            {
                return Err("OpenSSH Ed25519 私钥数据长度无效".to_string());
            }
            let algorithm = der_sequence([der_tlv(0x06, &[0x2b, 0x65, 0x70])]);
            let private_octet = der_tlv(0x04, &der_tlv(0x04, &private_key[..32]));
            Ok(pem_encode(
                "PRIVATE KEY",
                &der_sequence([der_integer_bytes(&[0]), algorithm, private_octet]),
            ))
        }
        b"ssh-rsa" => {
            let n = BigUint::from_bytes_be(read_ssh_binary_string(
                private_blob,
                &mut private_cursor,
                "RSA n",
            )?);
            let e = BigUint::from_bytes_be(read_ssh_binary_string(
                private_blob,
                &mut private_cursor,
                "RSA e",
            )?);
            let d = BigUint::from_bytes_be(read_ssh_binary_string(
                private_blob,
                &mut private_cursor,
                "RSA d",
            )?);
            let iqmp = BigUint::from_bytes_be(read_ssh_binary_string(
                private_blob,
                &mut private_cursor,
                "RSA iqmp",
            )?);
            let p = BigUint::from_bytes_be(read_ssh_binary_string(
                private_blob,
                &mut private_cursor,
                "RSA p",
            )?);
            let q = BigUint::from_bytes_be(read_ssh_binary_string(
                private_blob,
                &mut private_cursor,
                "RSA q",
            )?);
            let dmp1 = &d % (&p - BigUint::from(1u8));
            let dmq1 = &d % (&q - BigUint::from(1u8));
            Ok(pem_encode(
                "RSA PRIVATE KEY",
                &der_sequence([
                    der_integer_bytes(&[0]),
                    der_integer_biguint(&n),
                    der_integer_biguint(&e),
                    der_integer_biguint(&d),
                    der_integer_biguint(&p),
                    der_integer_biguint(&q),
                    der_integer_biguint(&dmp1),
                    der_integer_biguint(&dmq1),
                    der_integer_biguint(&iqmp),
                ]),
            ))
        }
        _ => Err(format!(
            "暂不支持新版 OpenSSH 密钥类型：{}",
            String::from_utf8_lossy(key_type)
        )),
    }
}

pub(super) fn write_temporary_private_key(pem: &str) -> Result<PathBuf, String> {
    let stamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_err(|error| format!("无法生成 SSH 临时密钥路径: {error}"))?
        .as_nanos();
    let path = std::env::temp_dir().join(format!(
        "android-tool-ssh-key-{}-{stamp}.pem",
        std::process::id()
    ));
    let mut file = fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&path)
        .map_err(|error| format!("无法创建 SSH 临时密钥文件: {error}"))?;
    if let Err(error) = file.write_all(pem.as_bytes()) {
        let _ = fs::remove_file(&path);
        return Err(format!("无法写入 SSH 临时密钥文件: {error}"));
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = fs::set_permissions(&path, fs::Permissions::from_mode(0o600));
    }
    Ok(path)
}
