//! Model-provider plugin adapter specs.
//!
//! The installer and the process supervisor are generic: they read a plugin's
//! package name, bridge module and pinned release from this table instead of
//! branching on the plugin id. Adding a provider means adding one row here plus
//! its bridge `.mjs`; no installer code changes.
//!
//! Everything else about a plugin (its npm dependencies, peer graph and entry
//! exports) is discovered from the published package itself, so this table
//! never needs to track upstream dependency lists.

/// One credential field the settings form asks the user for.
pub(super) struct ConfigField {
    /// Key in `PluginConfig` / the `configure_model_plugin` payload.
    pub key: &'static str,
    pub label: &'static str,
}

/// One selectable model-provider plugin.
pub(super) struct PluginSpec {
    /// Stable id used by the plugin list, provider ids and storage paths.
    pub id: &'static str,
    /// npm package that is installed.
    pub package: &'static str,
    /// Baseline version used when registry lookup is unavailable, and the
    /// minimum patch that keeps the validated adapter line.
    pub base_version: &'static str,
    /// Bridge host script embedded into the installed directory.
    pub bridge: &'static str,
    pub name: &'static str,
    pub description: &'static str,
    pub source: &'static str,
    /// Copy shown before the plugin is installed.
    pub install_hint: &'static str,
    /// Provider-id namespace this plugin owns, e.g. `plugin:workbuddy:`.
    pub provider_prefix: &'static str,
    /// Brand key for the provider icon; empty means the generic plugin icon.
    pub icon_brand: &'static str,
    /// Help text under the advanced configuration section.
    pub advanced_hint: &'static str,
    /// Credential fields the form renders, in display order.
    pub config_fields: &'static [ConfigField],
    /// Whether the bridge answers `probe` requests.
    pub supports_probe: bool,
    /// Portable runtimes the plugin needs before it can be installed.
    pub environment_tools: &'static [&'static str],
    /// Exports the installed entry point must expose, verified by importing it.
    pub required_exports: &'static [&'static str],
    /// Version line this adapter was validated against: updates stay inside it.
    pub accepted_line: &'static [u32],
    /// Lowest accepted patch within `accepted_line`.
    pub minimum_patch: u32,
}

const SPECS: &[PluginSpec] = &[
    PluginSpec {
        id: "workbuddy",
        package: "dsh-workbuddy-connect",
        base_version: "0.7.4",
        bridge: include_str!("workbuddy_bridge.mjs"),
        name: "WorkBuddy",
        description: "将 WorkBuddy / WorkBuddy AI 账号中的可用模型接入对话，使用对应账号的积分。",
        source: "https://github.com/corrinehu/dsh-workbuddy-connect",
        install_hint: "安装后复用本机 WorkBuddy 登录状态，无需填写 DeepSeek API Key",
        provider_prefix: "plugin:workbuddy:",
        icon_brand: "workbuddy",
        advanced_hint: "自动检测本机登录和程序位置；检测失败时选择登录文件或 WorkBuddy.exe。",
        config_fields: &[
            ConfigField { key: "authFileCn", label: "WorkBuddy 登录文件" },
            ConfigField { key: "electronFileCn", label: "WorkBuddy 程序" },
            ConfigField { key: "authFileAi", label: "WorkBuddy AI 登录文件" },
            ConfigField { key: "electronFileAi", label: "WorkBuddy AI 程序" },
        ],
        supports_probe: true,
        environment_tools: &["node"],
        required_exports: &[
            "WorkBuddyCredentialStore",
            "WorkBuddyUpstreamClient",
            "WORKBUDDY_VARIANTS",
            "prepareChatBody",
        ],
        accepted_line: &[0, 7],
        minimum_patch: 4,
    },
    PluginSpec {
        id: "opencode",
        package: "dsh-opencode-go",
        base_version: "0.1.22",
        bridge: include_str!("opencode_bridge.mjs"),
        name: "OpenCode",
        description: "复用本机 OpenCode Go 登录的模型和订阅额度，使用 DSH OpenCode Go 插件。",
        source: "https://github.com/Duskriver/dsh-opencode-go",
        install_hint: "安装 DSH OpenCode Go 插件后读取本机账号",
        provider_prefix: "plugin:opencode:",
        icon_brand: "",
        advanced_hint: "自动读取 OpenCode 的 auth.json / opencode.db。Go 显示 5 小时、每周、每月额度；Zen 使用账号可用模型。OAuth 账号暂不支持。",
        config_fields: &[ConfigField { key: "authFileCn", label: "OpenCode 登录文件或数据库" }],
        supports_probe: false,
        environment_tools: &["node"],
        required_exports: &["OpencodeGoCatalog", "readLiveModelIds"],
        accepted_line: &[0, 1],
        minimum_patch: 22,
    },
];

/// Every plugin id, in list order.
pub(super) fn ids() -> impl Iterator<Item = &'static str> {
    SPECS.iter().map(|spec| spec.id)
}

/// The spec owning a provider id such as `plugin:workbuddy:cn`.
///
/// The prefix is only a routing hint: it decides which plugin a provider id
/// belongs to. The real identity check happens later, against the exact
/// `settings.id` the running bridge published in its snapshot, so a mere prefix
/// match (say `plugin:workbuddy:cn:extra`) never yields a token or quota.
pub(super) fn from_provider(provider_id: &str) -> Option<&'static PluginSpec> {
    SPECS
        .iter()
        .find(|spec| provider_id.starts_with(spec.provider_prefix))
}

impl PluginSpec {
    /// Whether a published version stays on the validated adapter line.
    pub(super) fn accepts(&self, parts: &[u32]) -> bool {
        match parts {
            [major, minor, patch] => {
                *major == self.accepted_line[0]
                    && *minor == self.accepted_line[1]
                    && *patch >= self.minimum_patch
            }
            _ => false,
        }
    }
}

/// The spec for an id, or `None` when no adapter covers it.
pub(super) fn find(id: &str) -> Option<&'static PluginSpec> {
    SPECS.iter().find(|spec| spec.id == id)
}

/// The spec for an id, rejecting ids without an adapter.
pub(super) fn require(id: &str) -> Result<&'static PluginSpec, String> {
    find(id).ok_or_else(|| "该插件尚未提供 Android Toolkit 适配器".into())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Ids and provider prefixes are the keys the rest of the module looks up
    /// by; a duplicate would silently alias one plugin onto another's state.
    #[test]
    fn ids_and_prefixes_are_unique() {
        let mut seen = std::collections::HashSet::new();
        for spec in SPECS {
            assert!(seen.insert(spec.id), "duplicate plugin id: {}", spec.id);
        }
        let mut prefixes = std::collections::HashSet::new();
        for spec in SPECS {
            assert!(
                prefixes.insert(spec.provider_prefix),
                "duplicate provider prefix: {}",
                spec.provider_prefix
            );
        }
        // `from_provider` returns the FIRST prefix that matches, so one namespace
        // nested inside another would route a plugin's provider ids to the wrong
        // row. Comparing ids (not addresses) keeps this independent of how the
        // const array is materialised.
        for spec in SPECS {
            for other in SPECS {
                assert!(
                    spec.id == other.id
                        || !spec.provider_prefix.starts_with(other.provider_prefix),
                    "prefix {} is nested inside {}",
                    spec.provider_prefix,
                    other.provider_prefix
                );
            }
        }
    }

    /// A row that omits one of these would install a package but fail at
    /// runtime, so the table is checked at test time instead.
    #[test]
    fn every_row_is_complete() {
        for spec in SPECS {
            assert!(!spec.package.is_empty(), "{} has no package", spec.id);
            assert!(!spec.name.is_empty(), "{} has no name", spec.id);
            assert!(!spec.description.is_empty(), "{} has no description", spec.id);
            assert!(!spec.install_hint.is_empty(), "{} has no install hint", spec.id);
            assert!(!spec.source.is_empty(), "{} has no source", spec.id);
            assert!(!spec.bridge.is_empty(), "{} has no bridge", spec.id);
            assert!(
                !spec.required_exports.is_empty(),
                "{} validates no exports",
                spec.id
            );
            assert!(
                !spec.config_fields.is_empty(),
                "{} has no config fields",
                spec.id
            );
            assert!(
                !spec.environment_tools.is_empty(),
                "{} declares no runtime",
                spec.id
            );
            assert_eq!(spec.accepted_line.len(), 2, "{} line is malformed", spec.id);
            assert!(
                spec.provider_prefix.ends_with(':'),
                "{} prefix is not a namespace",
                spec.id
            );
            // `installer::latest_version` falls back to `base_version` whenever the
            // registry cannot be reached, so the fallback must itself stay inside
            // the line whose exports were validated - otherwise an offline install
            // would ship a version the adapter was never checked against.
            let base: Vec<u32> = spec
                .base_version
                .split('.')
                .filter_map(|part| part.parse::<u32>().ok())
                .collect();
            assert!(
                spec.accepts(&base),
                "{} base version {} is outside accepted line {:?} (minimum patch {})",
                spec.id,
                spec.base_version,
                spec.accepted_line,
                spec.minimum_patch
            );
        }
    }

    /// The lookup the command layer relies on must round-trip every row, and
    /// must not match an id that has no row.
    #[test]
    fn provider_lookup_round_trips() {
        for spec in SPECS {
            assert_eq!(from_provider(spec.provider_prefix).map(|s| s.id), Some(spec.id));
            assert_eq!(find(spec.id).map(|s| s.id), Some(spec.id));
        }
        assert!(from_provider("plugin:unknown:cn").is_none());
        assert!(find("unknown").is_none());
        assert!(require("unknown").is_err());
    }
}
