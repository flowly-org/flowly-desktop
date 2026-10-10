//! Flowly API client: several servers (flowly.net or self-hosted), each with its own OAuth 2.1 session
//! (PKCE + loopback redirect, endpoints discovered from the server), and the few endpoints the tray needs.

use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine};
use rand::RngCore;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::path::PathBuf;
use std::sync::Mutex;
use std::time::Duration;

pub const DEFAULT_API: &str = "https://api.flowly.net";
pub const DEFAULT_APP: &str = "https://app.flowly.net";

/// One server's session. (`api` only appears in the pre-multi-server `session.json`.)
#[derive(Clone, Default, Serialize, Deserialize)]
pub struct Stored {
    #[serde(default, skip_serializing)]
    pub api: Option<String>,
    pub client_id: Option<String>,
    pub redirect_uri: Option<String>,
    pub access_token: Option<String>,
    pub refresh_token: Option<String>,
    pub expires_at: Option<i64>,
    pub email: Option<String>,
}

/// A Flowly server (flowly.net or a self-hosted one) with its own session.
#[derive(Clone, Default, Serialize, Deserialize)]
pub struct Server {
    pub id: String,
    pub name: String,
    /// API base without `/api`, e.g. https://api.flowly.net.
    pub api_url: String,
    /// Web app, for "Open Flowly".
    pub app_url: String,
    #[serde(default)]
    pub token_endpoint: Option<String>,
    #[serde(default)]
    pub revocation_endpoint: Option<String>,
    #[serde(default)]
    pub session: Stored,
}

#[derive(Clone, Default, Serialize, Deserialize)]
struct Config {
    active: String,
    servers: Vec<Server>,
}

fn default_server() -> Server {
    Server {
        id: "flowly".into(),
        name: "flowly.net".into(),
        api_url: DEFAULT_API.into(),
        app_url: DEFAULT_APP.into(),
        ..Default::default()
    }
}

pub struct Client {
    path: PathBuf,
    config: Mutex<Config>,
    http: reqwest::Client,
}

#[derive(Debug)]
pub enum ApiError {
    SignedOut,
    Http(String),
}

impl std::fmt::Display for ApiError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ApiError::SignedOut => write!(f, "Not signed in"),
            ApiError::Http(m) => write!(f, "{m}"),
        }
    }
}

fn random_token(bytes: usize) -> String {
    let mut buf = vec![0u8; bytes];
    rand::thread_rng().fill_bytes(&mut buf);
    URL_SAFE_NO_PAD.encode(buf)
}

/// https only, except a local server during development.
fn check_url(raw: &str) -> Result<url::Url, ApiError> {
    let url = url::Url::parse(raw).map_err(|_| ApiError::Http(format!("Not a valid address: {raw}")))?;
    let local = matches!(url.host_str(), Some("localhost") | Some("127.0.0.1") | Some("[::1]"));
    match url.scheme() {
        "https" => Ok(url),
        "http" if local => Ok(url),
        _ => Err(ApiError::Http(format!("Use an https:// address ({raw})."))),
    }
}

fn trim_url(raw: &str) -> String {
    raw.trim().trim_end_matches('/').to_string()
}

impl Client {
    pub fn new(config_dir: PathBuf) -> Self {
        let _ = std::fs::create_dir_all(&config_dir);
        let path = config_dir.join("servers.json");
        let config = std::fs::read_to_string(&path)
            .ok()
            .and_then(|s| serde_json::from_str::<Config>(&s).ok())
            .filter(|c| !c.servers.is_empty())
            .unwrap_or_else(|| Self::migrate(&config_dir));
        let client = Client {
            path,
            config: Mutex::new(config),
            http: reqwest::Client::builder()
                .timeout(Duration::from_secs(20))
                .user_agent("Flowly-Desktop/0.3")
                .build()
                .expect("http client"),
        };
        client.save();
        client
    }

    /// First run, or an update from the single-server version: its session.json becomes the flowly.net entry.
    fn migrate(config_dir: &std::path::Path) -> Config {
        let mut server = default_server();
        let old = config_dir.join("session.json");
        if let Some(session) = std::fs::read_to_string(&old).ok().and_then(|s| serde_json::from_str::<Stored>(&s).ok()) {
            if let Some(api) = session.api.clone().filter(|a| a.as_str() != DEFAULT_API) {
                server.api_url = trim_url(&api);
            }
            server.session = session;
            let _ = std::fs::rename(&old, config_dir.join("session.json.migrated"));
        }
        Config { active: server.id.clone(), servers: vec![server] }
    }

    fn save(&self) {
        let config = self.config.lock().unwrap().clone();
        if let Ok(text) = serde_json::to_string_pretty(&config) {
            let _ = std::fs::write(&self.path, text);
        }
    }

    fn active(&self) -> Server {
        let c = self.config.lock().unwrap();
        c.servers.iter().find(|s| s.id == c.active).or(c.servers.first()).cloned().unwrap_or_else(default_server)
    }

    fn server(&self, id: &str) -> Option<Server> {
        self.config.lock().unwrap().servers.iter().find(|s| s.id == id).cloned()
    }

    /// Change one server (by id) and save.
    fn update(&self, id: &str, f: impl FnOnce(&mut Server)) {
        {
            let mut c = self.config.lock().unwrap();
            if let Some(s) = c.servers.iter_mut().find(|s| s.id == id) {
                f(s);
            }
        }
        self.save();
    }

    pub fn active_id(&self) -> String {
        self.active().id
    }

    pub fn active_name(&self) -> String {
        self.active().name
    }

    pub fn app_url(&self) -> String {
        self.active().app_url
    }

    pub fn signed_in(&self) -> bool {
        self.active().session.refresh_token.is_some()
    }

    pub fn email(&self) -> Option<String> {
        self.active().session.email
    }

    /// Servers for the window and the tray (no tokens).
    pub fn servers(&self) -> Vec<Value> {
        let c = self.config.lock().unwrap();
        c.servers
            .iter()
            .map(|s| {
                json!({
                    "id": s.id, "name": s.name, "api_url": s.api_url, "app_url": s.app_url,
                    "signed_in": s.session.refresh_token.is_some(), "email": s.session.email, "active": s.id == c.active,
                })
            })
            .collect()
    }

    pub fn set_active(&self, id: &str) -> bool {
        let found = {
            let mut c = self.config.lock().unwrap();
            let found = c.servers.iter().any(|s| s.id == id);
            if found {
                c.active = id.to_string();
            }
            found
        };
        if found {
            self.save();
        }
        found
    }

    async fn metadata(&self, api: &str) -> Option<Value> {
        let res = self.http.get(format!("{api}/.well-known/oauth-authorization-server")).send().await.ok()?;
        if !res.status().is_success() {
            return None;
        }
        let meta: Value = res.json().await.ok()?;
        meta["token_endpoint"].as_str().is_some().then_some(meta)
    }

    /// Add a server from the address of its API or of its web app, and make it active.
    pub async fn add_server(&self, input: &str) -> Result<Value, ApiError> {
        let mut typed = trim_url(input);
        if !typed.contains("://") {
            typed = format!("https://{typed}");
        }
        let typed_url = check_url(&typed)?;
        // The API itself, or a web app whose config.json says where its API is.
        let (api, meta) = match self.metadata(&typed).await {
            Some(meta) => (typed.clone(), meta),
            None => {
                let not_flowly = || ApiError::Http(format!("No Flowly server found at {typed}."));
                let cfg: Value = self
                    .http
                    .get(format!("{typed}/config.json"))
                    .send()
                    .await
                    .map_err(|e| ApiError::Http(e.to_string()))?
                    .json()
                    .await
                    .map_err(|_| not_flowly())?;
                let api_url = cfg["apiUrl"].as_str().ok_or_else(not_flowly)?;
                let joined = typed_url.join(api_url).map_err(|_| not_flowly())?;
                let api = trim_url(joined.as_str());
                let api = api.strip_suffix("/api").unwrap_or(&api).to_string();
                check_url(&api)?;
                let meta = self.metadata(&api).await.ok_or_else(not_flowly)?;
                (api, meta)
            }
        };
        let app = meta["authorization_endpoint"]
            .as_str()
            .and_then(|a| url::Url::parse(a).ok())
            .map(|u| u.origin().ascii_serialization())
            .unwrap_or_else(|| typed_url.origin().ascii_serialization());
        check_url(&app)?;
        let existing = self.config.lock().unwrap().servers.iter().find(|s| s.api_url == api).map(|s| s.id.clone());
        let id = match existing {
            Some(id) => id,
            None => {
                let name = url::Url::parse(&app).ok().and_then(|u| u.host_str().map(String::from)).unwrap_or_else(|| app.clone());
                let server = Server {
                    id: random_token(6),
                    name: name.trim_start_matches("app.").to_string(),
                    api_url: api,
                    app_url: app,
                    token_endpoint: meta["token_endpoint"].as_str().map(String::from),
                    revocation_endpoint: meta["revocation_endpoint"].as_str().map(String::from),
                    session: Stored::default(),
                };
                let id = server.id.clone();
                self.config.lock().unwrap().servers.push(server);
                id
            }
        };
        self.set_active(&id);
        Ok(json!({"id": id, "name": self.active_name()}))
    }

    /// Forget a server (signing out of it first). The list never ends up empty.
    pub fn remove_server(&self, id: &str) {
        if let Some(server) = self.server(id) {
            self.revoke(&server);
        }
        {
            let mut c = self.config.lock().unwrap();
            c.servers.retain(|s| s.id != id);
            if c.servers.is_empty() {
                c.servers.push(default_server());
            }
            if !c.servers.iter().any(|s| s.id == c.active) {
                c.active = c.servers[0].id.clone();
            }
        }
        self.save();
    }

    fn revoke(&self, server: &Server) {
        if let Some(token) = server.session.refresh_token.clone() {
            let url = server.revocation_endpoint.clone().unwrap_or_else(|| format!("{}/api/oauth/revoke", server.api_url));
            let http = self.http.clone();
            tauri::async_runtime::spawn(async move {
                let _ = http.post(url).form(&[("token", token)]).send().await;
            });
        }
    }

    pub fn sign_out(&self) {
        let server = self.active();
        self.sign_out_of(&server.id);
    }

    fn sign_out_of(&self, id: &str) {
        let Some(server) = self.server(id) else { return };
        self.revoke(&server);
        self.update(id, |s| {
            s.session.access_token = None;
            s.session.refresh_token = None;
            s.session.expires_at = None;
            s.session.email = None;
        });
    }

    /// Browser sign-in. Opens `open_url` with the authorization URL and waits for the redirect
    /// on a loopback port (RFC 8252).
    pub async fn sign_in(&self, open_url: impl Fn(String)) -> Result<(), ApiError> {
        let server = self.active();
        let (id, api) = (server.id.clone(), server.api_url.clone());
        let meta = self.metadata(&api).await.unwrap_or(Value::Null);
        let endpoint = |key: &str, fallback: String| -> Result<String, ApiError> {
            let url = meta[key].as_str().map(String::from).unwrap_or(fallback);
            check_url(&url)?;
            Ok(url)
        };
        let register = endpoint("registration_endpoint", format!("{api}/api/oauth/register"))?;
        let authorize = endpoint("authorization_endpoint", format!("{}/oauth/authorize", server.app_url))?;
        let token_url = endpoint("token_endpoint", format!("{api}/api/oauth/token"))?;
        let revoke_url = meta["revocation_endpoint"].as_str().map(String::from);
        let server = tiny_http::Server::http("127.0.0.1:0").map_err(|e| ApiError::Http(e.to_string()))?;
        let port = server.server_addr().to_ip().map(|a| a.port()).unwrap_or(0);
        let redirect_uri = format!("http://127.0.0.1:{port}/callback");

        // Register (again) when the redirect port changes; registration is cheap and anonymous.
        let client_id = {
            let res: Value = self
                .http
                .post(&register)
                .json(&json!({
                    "client_name": "Flowly Desktop",
                    "redirect_uris": [redirect_uri],
                    "token_endpoint_auth_method": "none",
                    "client_uri": "https://github.com/flowly-org/flowly-desktop"
                }))
                .send()
                .await
                .map_err(|e| ApiError::Http(e.to_string()))?
                .json()
                .await
                .map_err(|e| ApiError::Http(e.to_string()))?;
            res["client_id"].as_str().ok_or_else(|| ApiError::Http(format!("Registration failed: {res}")))?.to_string()
        };

        let verifier = random_token(48);
        let challenge = URL_SAFE_NO_PAD.encode(Sha256::digest(verifier.as_bytes()));
        let state_param = random_token(16);
        let mut url = url::Url::parse(&authorize).map_err(|e| ApiError::Http(e.to_string()))?;
        url.query_pairs_mut()
            .append_pair("response_type", "code")
            .append_pair("client_id", &client_id)
            .append_pair("redirect_uri", &redirect_uri)
            .append_pair("code_challenge", &challenge)
            .append_pair("code_challenge_method", "S256")
            .append_pair("scope", "api mcp")
            .append_pair("state", &state_param);
        open_url(url.to_string());

        // Wait up to 5 minutes for the browser to come back.
        let expected_state = state_param.clone();
        let code = tokio::task::spawn_blocking(move || -> Result<String, String> {
            let deadline = std::time::Instant::now() + Duration::from_secs(300);
            while std::time::Instant::now() < deadline {
                let Ok(Some(req)) = server.recv_timeout(Duration::from_secs(1)) else { continue };
                let full = format!("http://127.0.0.1{}", req.url());
                let parsed = url::Url::parse(&full).map_err(|e| e.to_string())?;
                let params: std::collections::HashMap<_, _> = parsed.query_pairs().into_owned().collect();
                let page = |msg: &str| {
                    tiny_http::Response::from_string(format!(
                        "<!doctype html><meta charset=utf-8><title>Flowly</title><body style=\"font:16px system-ui;display:grid;place-items:center;height:90vh;color:#333\"><p>{msg}</p></body>"
                    ))
                    .with_header("Content-Type: text/html; charset=utf-8".parse::<tiny_http::Header>().unwrap())
                };
                if parsed.path() != "/callback" {
                    let _ = req.respond(tiny_http::Response::empty(404));
                    continue;
                }
                if params.get("state") != Some(&expected_state) {
                    let _ = req.respond(page("This sign-in link doesn't match. Try again from the Flowly tray app."));
                    return Err("State mismatch".into());
                }
                if let Some(code) = params.get("code") {
                    let _ = req.respond(page("Signed in to Flowly. You can close this tab."));
                    return Ok(code.clone());
                }
                let _ = req.respond(page("Sign-in was cancelled. You can close this tab."));
                return Err("Sign-in was cancelled".into());
            }
            Err("Sign-in timed out".into())
        })
        .await
        .map_err(|e| ApiError::Http(e.to_string()))?
        .map_err(ApiError::Http)?;

        let tok: Value = self
            .http
            .post(&token_url)
            .form(&[
                ("grant_type", "authorization_code"),
                ("code", &code),
                ("client_id", &client_id),
                ("redirect_uri", &redirect_uri),
                ("code_verifier", &verifier),
            ])
            .send()
            .await
            .map_err(|e| ApiError::Http(e.to_string()))?
            .json()
            .await
            .map_err(|e| ApiError::Http(e.to_string()))?;
        self.update(&id, |s| {
            s.token_endpoint = Some(token_url);
            s.revocation_endpoint = revoke_url.or(s.revocation_endpoint.take());
        });
        self.store_tokens(&id, &tok, Some(client_id), Some(redirect_uri))?;
        if let Ok(me) = self.request(&id, reqwest::Method::GET, "/api/auth/me/", None).await {
            self.update(&id, |s| s.session.email = me["email"].as_str().map(String::from));
        }
        Ok(())
    }

    fn store_tokens(&self, id: &str, tok: &Value, client_id: Option<String>, redirect: Option<String>) -> Result<(), ApiError> {
        let access = tok["access_token"].as_str().ok_or_else(|| ApiError::Http(format!("Token error: {tok}")))?.to_string();
        let expires_at = chrono::Utc::now().timestamp() + tok["expires_in"].as_i64().unwrap_or(3600) - 60;
        let refresh = tok["refresh_token"].as_str().map(String::from);
        self.update(id, |s| {
            let session = &mut s.session;
            session.access_token = Some(access);
            session.refresh_token = refresh.or(session.refresh_token.take());
            session.expires_at = Some(expires_at);
            if client_id.is_some() {
                session.client_id = client_id;
            }
            if redirect.is_some() {
                session.redirect_uri = redirect;
            }
        });
        Ok(())
    }

    async fn access_token(&self, id: &str) -> Result<String, ApiError> {
        let server = self.server(id).ok_or(ApiError::SignedOut)?;
        let session = &server.session;
        if let Some(a) = &session.access_token {
            if session.expires_at.unwrap_or(0) > chrono::Utc::now().timestamp() {
                return Ok(a.clone());
            }
        }
        let (Some(refresh), Some(client_id)) = (session.refresh_token.clone(), session.client_id.clone()) else {
            return Err(ApiError::SignedOut);
        };
        let token_url = server.token_endpoint.clone().unwrap_or_else(|| format!("{}/api/oauth/token", server.api_url));
        let res = self
            .http
            .post(token_url)
            .form(&[("grant_type", "refresh_token"), ("refresh_token", &refresh), ("client_id", &client_id)])
            .send()
            .await
            .map_err(|e| ApiError::Http(e.to_string()))?;
        if res.status().as_u16() >= 400 {
            // Revoked or expired: the user has to sign in again.
            self.sign_out_of(id);
            return Err(ApiError::SignedOut);
        }
        let tok: Value = res.json().await.map_err(|e| ApiError::Http(e.to_string()))?;
        self.store_tokens(id, &tok, None, None)?;
        Ok(tok["access_token"].as_str().unwrap_or_default().to_string())
    }

    /// A call to one server, whichever is active by the time it returns.
    async fn request(&self, id: &str, method: reqwest::Method, path: &str, body: Option<Value>) -> Result<Value, ApiError> {
        let token = self.access_token(id).await?;
        let api = self.server(id).map(|s| s.api_url).ok_or(ApiError::SignedOut)?;
        let mut req = self.http.request(method, format!("{api}{path}")).bearer_auth(token);
        if let Some(b) = body {
            req = req.json(&b);
        }
        let res = req.send().await.map_err(|e| ApiError::Http(e.to_string()))?;
        let status = res.status();
        let value: Value = res.json().await.unwrap_or(Value::Null);
        if status.as_u16() == 401 {
            return Err(ApiError::SignedOut);
        }
        if !status.is_success() {
            let detail = value["detail"].as_str().map(String::from).unwrap_or_else(|| value.to_string());
            return Err(ApiError::Http(detail));
        }
        Ok(value)
    }

    async fn send(&self, method: reqwest::Method, path: &str, body: Option<Value>) -> Result<Value, ApiError> {
        let id = self.active_id();
        self.request(&id, method, path, body).await
    }

    pub async fn get(&self, path: &str) -> Result<Value, ApiError> {
        self.send(reqwest::Method::GET, path, None).await
    }

    pub async fn post(&self, path: &str, body: Value) -> Result<Value, ApiError> {
        self.send(reqwest::Method::POST, path, Some(body)).await
    }
}
