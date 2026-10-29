use std::collections::BTreeMap;

pub fn parse_storage(output: &str) -> Option<(u64, u64)> {
    output.lines().find_map(|line| {
        let cells: Vec<_> = line.split_whitespace().collect();
        if cells.len() < 6 || !cells.last()?.starts_with('/') { return None; }
        let total = cells[1].parse::<u64>().ok()?.checked_mul(1024)?;
        let available = cells[3].parse::<u64>().ok()?.checked_mul(1024)?;
        (total > 0 && available <= total).then_some((total, available))
    })
}

pub fn parse_physical_value(output: &str, prefix: &str) -> Option<String> {
    output.lines().find_map(|line| {
        let value = line.trim().strip_prefix(prefix)?.trim();
        let valid = if prefix.ends_with("size:") {
            let (width, height) = value.split_once('x')?;
            width.parse::<u32>().ok()? > 0 && height.parse::<u32>().ok()? > 0
        } else { value.parse::<u32>().ok()? > 0 };
        valid.then(|| value.to_string())
    })
}

pub fn parse_enabled(dump: &str, user: Option<u32>) -> Option<bool> {
    let prefix = format!("User {}:", user?);
    let line = dump.lines().find(|line| line.trim().starts_with(&prefix))?;
    let flag = line.split_whitespace().find_map(|token| token.strip_prefix("enabled="))?;
    match flag { "1" | "true" => Some(true), "2" | "3" | "4" | "false" => Some(false), _ => None }
}

pub fn parse_permissions(dump: &str, user: Option<u32>) -> Vec<(String, Option<bool>)> {
    let mut requested = Vec::new();
    let mut grants = BTreeMap::new();
    let mut section = "";
    let mut section_indent = 0;
    let mut active_user = None;
    for line in dump.lines() {
        let trimmed = line.trim();
        let indent = line.len() - line.trim_start().len();
        if let Some(value) = trimmed.strip_prefix("User ").and_then(|text| text.split_once(':').map(|p| p.0)) {
            active_user = value.parse::<u32>().ok();
            section = "";
        }
        if !trimmed.is_empty() && indent <= section_indent { section = ""; }
        if trimmed == "requested permissions:" || trimmed == "install permissions:" || trimmed == "runtime permissions:" {
            section = trimmed;
            section_indent = indent;
            continue;
        }
        if section == "requested permissions:" {
            if !trimmed.is_empty() && !trimmed.contains(char::is_whitespace) && trimmed.contains('.') {
                requested.push(trimmed.trim_end_matches(':').to_string());
            }
        } else if section == "install permissions:" || section == "runtime permissions:" {
            // Never merge runtime grants from another Android user.
            if active_user.is_some() && (user.is_none() || active_user != user) { continue; }
            if section == "runtime permissions:" && active_user.is_none() { continue; }
            if let Some((name, flags)) = trimmed.split_once(':') {
                let granted = flags.split([',', ' ']).find_map(|flag| match flag.trim().strip_prefix("granted=") {
                    Some("true") => Some(true), Some("false") => Some(false), _ => None,
                });
                if let Some(granted) = granted { grants.insert(name.to_string(), granted); }
            }
        }
    }
    requested.sort();
    requested.dedup();
    requested.into_iter().map(|name| { let granted = grants.get(&name).copied(); (name, granted) }).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn storage_requires_valid_kib_columns() {
        assert_eq!(parse_storage("Filesystem 1K-blocks Used Available Use% Mounted on\n/dev/fuse 10000 4000 6000 40% /storage/emulated"), Some((10240000, 6144000)));
        assert_eq!(parse_storage("df: Permission denied"), None);
        assert_eq!(parse_storage("/dev/fuse 10 1 20 10% /sdcard"), None);
    }
    #[test]
    fn physical_values_exclude_overrides_and_invalid_output() {
        assert_eq!(parse_physical_value("Physical size: 1080x2400\nOverride size: 720x1600", "Physical size:"), Some("1080x2400".into()));
        assert_eq!(parse_physical_value("Physical density: 420", "Physical density:"), Some("420".into()));
        assert_eq!(parse_physical_value("Physical size: bad", "Physical size:"), None);
    }
    #[test]
    fn permissions_keep_missing_grants_unknown_and_scope_android_users() {
        let dump = "  requested permissions:\n    android.permission.CAMERA\n    android.permission.INTERNET\n    app.custom.ACCESS\n  install permissions:\n    android.permission.INTERNET: granted=true\n  User 0: installed=true enabled=2\n    runtime permissions:\n      android.permission.CAMERA: granted=false, flags=[]\n  User 10: installed=true enabled=1\n    runtime permissions:\n      android.permission.CAMERA: granted=true, flags=[]\n";
        let permissions = parse_permissions(dump, Some(0));
        assert_eq!(permissions, vec![("android.permission.CAMERA".into(), Some(false)), ("android.permission.INTERNET".into(), Some(true)), ("app.custom.ACCESS".into(), None)]);
        assert_eq!(parse_enabled(dump, Some(0)), Some(false));
        assert_eq!(parse_enabled(dump, Some(10)), Some(true));
        assert_eq!(parse_enabled(dump, None), None);
        assert_eq!(parse_enabled("User 0: enabled=0", Some(0)), None);
        assert_eq!(parse_permissions(dump, None)[0].1, None);
    }
}
