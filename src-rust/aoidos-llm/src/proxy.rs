//! 代理应用与传输客户端构造：把 [`ProxyConfig`] 翻译成 reqwest 客户端。
//!
//! 三模式互不叠加（枚举天然表达）：system 用启动快照；none 用 `no_proxy()`
//! 显式禁用；manual 用显式 `Proxy::all` 并应用凭据引用解析出的认证。客户端
//! 统一在这里定型：ring provider、`retry(never)`（禁用 reqwest 默认协议
//! NACK 重试）、`redirect(Policy::none)`（SSE 不需要重定向语义，且禁止自动
//! 重定向携带请求）。全程不让 reqwest 隐式读取环境代理——system 模式只用
//! 启动快照，保证「manual 不叠加系统代理」由构造方式保证而非运行时判断。
//! 生产端点必须 HTTPS、socks5 需要 reqwest 的 socks feature。

use secrecy::{ExposeSecret, SecretString};

use crate::config::ProxyConfig;
use crate::credentials::CredentialStore;
use aoidos_store::error::{Result, StoreError};

/// 把代理配置应用到客户端构建器；manual 的认证由调用方解析 authRef 后传入。
/// 快照为空与 none 一样显式 `no_proxy()`——不让 reqwest 在 build 时再读环境，
/// 否则「启动快照」与实际代理会分叉。
///
/// # Errors
/// manual URL 解析失败（已过 profile 校验的 URL 不会到这里，防御性返回）。
pub fn apply_proxy(
    builder: reqwest::ClientBuilder,
    config: &ProxyConfig,
    snapshot: &SystemProxySnapshot,
    auth: Option<&ProxyAuth>,
) -> reqwest::Result<reqwest::ClientBuilder> {
    match config {
        ProxyConfig::System => match &snapshot.https {
            Some(url) => Ok(builder.proxy(reqwest::Proxy::all(url.as_str())?)),
            None => Ok(builder.no_proxy()),
        },
        ProxyConfig::None => Ok(builder.no_proxy()),
        ProxyConfig::Manual { url, .. } => {
            let mut proxy = reqwest::Proxy::all(url.as_str())?;
            if let Some(auth) = auth {
                proxy = proxy.basic_auth(&auth.username, auth.password.expose_secret());
            }
            Ok(builder.proxy(proxy))
        }
    }
}

/// 组装传输客户端的唯一直径：TLS provider、禁用隐式重试、禁用自动重定向、
/// 代理三模式与代理认证。供应商 adapter 与回合装配层都用它构造客户端。
///
/// # Errors
/// 代理 URL 或客户端构建失败（TLS backend 初始化异常）时返回 reqwest 错误。
pub fn build_client(
    config: &ProxyConfig,
    snapshot: &SystemProxySnapshot,
    auth: Option<&ProxyAuth>,
) -> reqwest::Result<reqwest::Client> {
    crate::install_ring_provider();
    apply_proxy(
        reqwest::Client::builder()
            .retry(reqwest::retry::never())
            .redirect(reqwest::redirect::Policy::none()),
        config,
        snapshot,
        auth,
    )?
    .build()
}

/// 默认传输（直连）：测试与未接装配层的场景使用；策略与 [`build_client`]
/// 完全一致（禁隐式重试 / 禁自动重定向），代理配置留待装配层注入。
///
/// # Errors
/// 客户端构建失败（TLS backend 初始化异常）时返回 reqwest 错误。
pub(crate) fn default_transport() -> reqwest::Result<reqwest::Client> {
    build_client(&ProxyConfig::System, &SystemProxySnapshot::default(), None)
}

impl std::fmt::Debug for SystemProxySnapshot {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("SystemProxySnapshot { [REDACTED] }")
    }
}
impl std::fmt::Debug for ProxyAuth {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("ProxyAuth { [REDACTED] }")
    }
}

/// manual 代理认证：凭据条目解析后的用户名 + 密码句柄。
#[derive(Clone)]
pub struct ProxyAuth {
    pub username: String,
    pub password: SecretString,
}

/// 拆分合并存储的代理认证条目（`username:password`，首个冒号分隔，
/// 密码可含冒号）；用户名为空或没有冒号视为条目损坏。
#[must_use]
pub fn parse_proxy_auth(raw: &SecretString) -> Option<ProxyAuth> {
    let (username, password) = raw.expose_secret().split_once(':')?;
    if username.is_empty() {
        return None;
    }
    Some(ProxyAuth {
        username: username.to_owned(),
        password: SecretString::from(password.to_owned()),
    })
}

/// 解析 manual profile 的 authRef：未引用返回 `None`；引用的条目缺失或形状
/// 不合是凭据数据损坏，显式报错而不是静默直连（会让代理 407 才暴露）。
///
/// # Errors
/// 凭据后端读取失败按 store.* 透传；条目缺失 / 形状不合为
/// `store.not-found` / `store.corrupt`。
pub fn resolve_proxy_auth(
    store: &dyn CredentialStore,
    auth_ref: Option<&str>,
) -> Result<Option<ProxyAuth>> {
    let Some(name) = auth_ref else {
        return Ok(None);
    };
    match store.get_key(name)? {
        Some(raw) => parse_proxy_auth(&raw)
            .map(Some)
            .ok_or_else(err_proxy_auth_shape),
        None => Err(StoreError::Io {
            code: "not-found",
            source: std::io::Error::other("proxy auth reference not found"),
        }),
    }
}

/// 凭据条目不是 `username:password` 形状（外部写入或版本不符）。
fn err_proxy_auth_shape() -> StoreError {
    StoreError::Corrupt("proxy auth entry is not username:password".into())
}

/// 环境代理快照：应用启动时一次性读取（llm.md「启动时形成快照」），之后
/// 不再读环境。快照可注入（测试传入确定值，装配层传
/// [`SystemProxySnapshot::capture`] 的结果）。
#[derive(Clone, PartialEq, Eq, Default)]
pub struct SystemProxySnapshot {
    /// `https_proxy` / `HTTPS_PROXY` / `all_proxy` 依序取第一个。
    pub https: Option<String>,
}

impl SystemProxySnapshot {
    /// 从环境变量读取。reqwest 认的环境变量名依序尝试：
    /// `https_proxy` / `HTTPS_PROXY` / `all_proxy` / `ALL_PROXY`。
    #[must_use]
    pub fn capture() -> Self {
        Self::from_getter(|name| std::env::var(name).ok())
    }

    /// 可注入变体：测试用确定 getter，不污染进程环境。
    pub fn from_getter(get: impl Fn(&str) -> Option<String>) -> Self {
        let https = ["https_proxy", "HTTPS_PROXY", "all_proxy", "ALL_PROXY"]
            .iter()
            .find_map(|name| get(name).filter(|value| !value.trim().is_empty()));
        Self { https }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testserver::{
        ProxyExpectation, ScriptedProxy, ScriptedServer, Segment, sse_ok_script,
    };
    use std::collections::BTreeMap;

    fn getter(map: BTreeMap<&str, &str>) -> impl Fn(&str) -> Option<String> {
        move |name| map.get(name).map(|v| v.to_string())
    }

    fn manual(url: &str) -> ProxyConfig {
        ProxyConfig::Manual {
            url: url.to_owned(),
            auth_ref: None,
        }
    }

    #[test]
    fn proxy_debug_never_exposes_environment_or_authentication() {
        let snapshot = SystemProxySnapshot {
            https: Some("http://user:secret@proxy:1".into()),
        };
        assert!(!format!("{snapshot:?}").contains("secret"));
        let auth = ProxyAuth {
            username: "private-user".into(),
            password: SecretString::from("secret".to_owned()),
        };
        let debug = format!("{auth:?}");
        assert!(!debug.contains("private-user") && !debug.contains("secret"));
    }

    #[test]
    fn socks5_configuration_constructs_a_real_transport() {
        build_client(
            &manual("socks5://127.0.0.1:1080"),
            &SystemProxySnapshot::default(),
            None,
        )
        .unwrap();
    }

    #[test]
    fn snapshot_prefers_https_proxy_then_all_proxy() {
        let both = getter(BTreeMap::from([
            ("https_proxy", "http://a:1"),
            ("all_proxy", "http://b:2"),
        ]));
        assert_eq!(
            SystemProxySnapshot::from_getter(both).https.as_deref(),
            Some("http://a:1")
        );
        let only_all = getter(BTreeMap::from([("ALL_PROXY", "socks5://c:3")]));
        assert_eq!(
            SystemProxySnapshot::from_getter(only_all).https.as_deref(),
            Some("socks5://c:3")
        );
        let empty = getter(BTreeMap::from([("https_proxy", "  ")]));
        assert_eq!(SystemProxySnapshot::from_getter(empty).https, None);
    }

    #[test]
    fn snapshot_case_insensitive_fallback_chain() {
        let upper = getter(BTreeMap::from([("HTTPS_PROXY", "http://upper:9")]));
        assert_eq!(
            SystemProxySnapshot::from_getter(upper).https.as_deref(),
            Some("http://upper:9")
        );
    }

    #[test]
    fn capture_reads_process_environment() {
        // 只读进程环境；不在并行 HTTP/TLS 测试期间用 unsafe 修改全局环境。
        let snapshot = SystemProxySnapshot::capture();
        let expected = SystemProxySnapshot::from_getter(|name| std::env::var(name).ok());
        assert_eq!(snapshot, expected);
    }

    #[test]
    fn parse_proxy_auth_splits_at_first_colon() {
        let auth = parse_proxy_auth(&SecretString::from(String::from("user:p@ss:word"))).unwrap();
        assert_eq!(auth.username, "user");
        assert_eq!(auth.password.expose_secret(), "p@ss:word");
        // 无冒号 / 空用户名都是损坏条目。
        assert!(parse_proxy_auth(&SecretString::from(String::from("no-colon"))).is_none());
        assert!(parse_proxy_auth(&SecretString::from(String::from(":secret"))).is_none());
    }

    #[test]
    fn resolve_proxy_auth_maps_missing_and_malformed_entries() {
        let store = crate::credentials::MemoryStore::default();
        assert!(resolve_proxy_auth(&store, None).unwrap().is_none());
        // 引用的条目缺失：not-found 而非静默无认证。
        let err = resolve_proxy_auth(&store, Some("ghost")).unwrap_err();
        assert_eq!(err.code(), "not-found");
        store
            .set_key("proxy-auth", SecretString::from(String::from("malformed")))
            .unwrap();
        assert_eq!(
            resolve_proxy_auth(&store, Some("proxy-auth"))
                .unwrap_err()
                .code(),
            "corrupt"
        );
        store
            .set_key("proxy-auth", SecretString::from(String::from("u:p")))
            .unwrap();
        let auth = resolve_proxy_auth(&store, Some("proxy-auth"))
            .unwrap()
            .unwrap();
        assert_eq!(auth.username, "u");
        // 后端读取失败如实透传。
        assert!(
            FailingAuthBackend
                .set_key("x", SecretString::from(String::from("v")))
                .is_err()
        );
        assert!(FailingAuthBackend.clear_key("x").is_err());
        assert!(resolve_proxy_auth(&FailingAuthBackend, Some("any")).is_err());
    }

    /// 永远失败的凭据后端（resolve 的读取失败路径直测）。
    struct FailingAuthBackend;

    impl FailingAuthBackend {
        fn fail<T>() -> Result<T> {
            Err(aoidos_store::error::StoreError::Corrupt(
                "backend unavailable".into(),
            ))
        }
    }

    impl CredentialStore for FailingAuthBackend {
        fn set_key(&self, _provider_id: &str, _secret: SecretString) -> Result<()> {
            Self::fail()
        }

        fn get_key(&self, _provider_id: &str) -> Result<Option<SecretString>> {
            Self::fail()
        }

        fn clear_key(&self, _provider_id: &str) -> Result<()> {
            Self::fail()
        }
    }

    #[test]
    fn manual_url_parse_failure_is_defensive_error() {
        let snapshot = SystemProxySnapshot::default();
        assert!(build_client(&manual("not a url"), &snapshot, None).is_err());
    }

    #[tokio::test]
    async fn manual_mode_routes_through_configured_proxy_only() {
        let proxy = ScriptedProxy::start(ProxyExpectation::Allow).await;
        // 快照里有「另一个代理」也必须被无视：manual 不叠加系统代理。
        let snapshot = SystemProxySnapshot {
            https: Some("http://127.0.0.1:1".to_owned()),
        };
        let client = build_client(&manual(&proxy.url()), &snapshot, None).unwrap();
        let response = client
            .get("http://narration.example/x")
            .send()
            .await
            .expect("manual proxy exchange");
        assert_eq!(response.status().as_u16(), 200);
        let records = proxy.records();
        assert_eq!(records.len(), 1);
        assert_eq!(records[0].target, "http://narration.example/x");
        assert_eq!(records[0].proxy_authorization, None);
        proxy.shutdown().await;
    }

    #[tokio::test]
    async fn none_mode_bypasses_snapshot_proxy() {
        let proxy = ScriptedProxy::start(ProxyExpectation::Allow).await;
        let origin = ScriptedServer::start(sse_ok_script("ok")).await;
        let snapshot = SystemProxySnapshot {
            https: Some(proxy.url()),
        };
        let client = build_client(&ProxyConfig::None, &snapshot, None).unwrap();
        let response = client
            .get(format!("http://127.0.0.1:{}/direct", origin.addr.port()))
            .send()
            .await
            .expect("direct exchange");
        assert_eq!(response.status().as_u16(), 200);
        assert_eq!(origin.request_count(), 1, "必须直连源站");
        assert!(proxy.records().is_empty(), "none 模式不得碰代理");
        proxy.shutdown().await;
        origin.shutdown().await;
    }

    #[tokio::test]
    async fn system_mode_uses_snapshot_and_empty_snapshot_is_direct() {
        let proxy = ScriptedProxy::start(ProxyExpectation::Allow).await;
        // 快照有值：走快照代理。
        let snapshot = SystemProxySnapshot {
            https: Some(proxy.url()),
        };
        let client = build_client(&ProxyConfig::System, &snapshot, None).unwrap();
        client
            .get("http://narration.example/via-system")
            .send()
            .await
            .expect("system snapshot proxy exchange");
        assert_eq!(
            proxy.records()[0].target,
            "http://narration.example/via-system"
        );
        proxy.shutdown().await;

        // 快照为空：显式直连（不得退回隐式环境探测）。
        let origin = ScriptedServer::start(sse_ok_script("ok")).await;
        let client =
            build_client(&ProxyConfig::System, &SystemProxySnapshot::default(), None).unwrap();
        let response = client
            .get(format!("http://127.0.0.1:{}/direct", origin.addr.port()))
            .send()
            .await
            .expect("direct exchange");
        assert_eq!(response.status().as_u16(), 200);
        assert_eq!(origin.request_count(), 1);
        origin.shutdown().await;
    }

    #[tokio::test]
    async fn manual_proxy_sends_basic_auth_from_resolved_credential() {
        let proxy =
            ScriptedProxy::start(ProxyExpectation::BasicAuth("Basic dXNlcjpwYXNz".into())).await;
        let client = build_client(
            &manual(&proxy.url()),
            &SystemProxySnapshot::default(),
            Some(&ProxyAuth {
                username: "user".into(),
                password: SecretString::from(String::from("pass")),
            }),
        )
        .unwrap();
        let response = client
            .get("http://narration.example/authed")
            .send()
            .await
            .expect("authed proxy exchange");
        assert_eq!(response.status().as_u16(), 200);
        assert_eq!(
            proxy.records()[0].proxy_authorization.as_deref(),
            Some("Basic dXNlcjpwYXNz")
        );
        proxy.shutdown().await;
    }

    #[tokio::test]
    async fn https_requests_tunnel_through_proxy_via_connect() {
        crate::install_ring_provider();
        let origin = ScriptedServer::start_tls(sse_ok_script("ok")).await;
        let proxy = ScriptedProxy::start(ProxyExpectation::Allow).await;
        let cert = reqwest::tls::Certificate::from_der(
            origin
                .certificate()
                .expect("tls fixture has a certificate")
                .as_ref(),
        )
        .unwrap();
        // build_client 不带自定义信任根（生产只信系统根）；需要信任夹具证书的
        // 测试按同一顺序组合 builder：retry/redirect 先定，代理经 apply_proxy 注入。
        let builder = reqwest::Client::builder()
            .retry(reqwest::retry::never())
            .redirect(reqwest::redirect::Policy::none())
            .add_root_certificate(cert);
        let client = apply_proxy(
            builder,
            &manual(&proxy.url()),
            &SystemProxySnapshot::default(),
            None,
        )
        .unwrap()
        .build()
        .unwrap();
        let response = client
            .get(format!("https://localhost:{}/tunneled", origin.addr.port()))
            .send()
            .await
            .expect("tunneled https exchange");
        assert_eq!(response.status().as_u16(), 200);
        let records = proxy.records();
        assert_eq!(records.len(), 1);
        assert_eq!(
            records[0].connect.as_deref(),
            Some(&*format!("localhost:{}", origin.addr.port()))
        );
        assert_eq!(origin.request_count(), 1);
        assert_eq!(origin.requests()[0].path, "/tunneled");
        proxy.shutdown().await;
        origin.shutdown().await;
    }

    #[tokio::test]
    async fn untrusted_tls_through_proxy_is_classified_as_tls() {
        crate::install_ring_provider();
        let origin = ScriptedServer::start_tls(sse_ok_script("ok")).await;
        let proxy = ScriptedProxy::start(ProxyExpectation::Allow).await;
        let client =
            build_client(&manual(&proxy.url()), &SystemProxySnapshot::default(), None).unwrap();
        let error = client
            .get(format!("https://localhost:{}/x", origin.addr.port()))
            .send()
            .await
            .expect_err("untrusted origin certificate must fail");
        let classified =
            crate::error::ProviderError::from_reqwest(&error, crate::error::RequestPhase::Send);
        assert_eq!(classified.code(), "tls", "实际分类：{classified}");
        // 隧道已建立（CONNECT 成功），失败发生在隧道内的 TLS 握手。
        assert!(proxy.records()[0].connect.is_some());
        proxy.shutdown().await;
        origin.shutdown().await;
    }

    #[tokio::test]
    async fn redirect_is_never_followed() {
        let origin = ScriptedServer::start(vec![
            Segment::Status(302),
            Segment::Header("Location: http://narration.example/moved".into()),
            Segment::Header("Connection: close".into()),
            Segment::Header(String::new()),
        ])
        .await;
        let client =
            build_client(&ProxyConfig::System, &SystemProxySnapshot::default(), None).unwrap();
        let response = client
            .get(format!("http://127.0.0.1:{}/old", origin.addr.port()))
            .send()
            .await
            .expect("redirect response");
        assert_eq!(response.status().as_u16(), 302, "3xx 必须原样返回");
        assert_eq!(origin.request_count(), 1, "不得跟随重定向再发请求");
        origin.shutdown().await;
    }

    #[test]
    fn proxy_expectation_rejects_wrong_credentials() {
        // ProxyExpectation 的匹配逻辑是纯字符串判断，直接断言。
        let expectation = ProxyExpectation::BasicAuth("Basic good".into());
        assert!(expectation.satisfied(Some("Basic good")));
        assert!(!expectation.satisfied(Some("Basic evil")));
        assert!(!expectation.satisfied(None));
        assert!(ProxyExpectation::Allow.satisfied(None));
        assert!(ProxyExpectation::Allow.satisfied(Some("anything")));
    }
}
