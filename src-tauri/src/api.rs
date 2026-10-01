//! Flowly API client: OAuth 2.1 sign-in (PKCE + loopback redirect), token storage and refresh, and
//! the few endpoints the tray needs (timer, today's tasks, workspaces).

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

#[derive(Clone, Default, Serialize, Deserialize)]
pub struct Stored {
    pub api: Option<String>,
    pub client_id: Option<String>,
    pub redirect_uri: Option<String>,
    pub access_token: Option<String>,
    pub refresh_token: Option<String>,
    pub expires_at: Option<i64>,
    pub email: Option<String>,
}

pub struct Client {
    path: PathBuf,
    state: Mutex<Stored>,
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

impl Client {
    pub fn new(config_dir: PathBuf) -> Self {
        let _ = std::fs::create_dir_all(&config_dir);
        let path = config_dir.join("session.json");
        let state = std::fs::read_to_string(&path)
            .ok()
            .and_then(|s| serde_json::from_str(&s).ok())
            .unwrap_or_default();
        Client {
            path,
            state: Mutex::new(state),
            http: reqwest::Client::builder()
                .timeout(Duration::from_secs(20))
                .user_agent("Flowly-Desktop/0.1")
                .build()
                .expect("http client"),
        }
    }

    fn save(&self) {
        let state = self.state.lock().unwrap().clone();
        if let Ok(text) = serde_json::to_string_pretty(&state) {
            let _ = std::fs::write(&self.path, text);
        }
    }

    pub fn api_base(&self) -> String {
        self.state.lock().unwrap().api.clone().unwrap_or_else(|| DEFAULT_API.to_string())
    }

    pub fn signed_in(&self) -> bool {
        self.state.lock().unwrap().refresh_token.is_some()
    }

    pub fn email(&self) -> Option<String> {
        self.state.lock().unwrap().email.clone()
    }

    pub fn sign_out(&self) {
        let token = self.state.lock().unwrap().refresh_token.clone();
        if let Some(token) = token {
            let url = format!("{}/api/oauth/revoke", self.api_base());
            let http = self.http.clone();
            tauri::async_runtime::spawn(async move {
                let _ = http.post(url).form(&[("token", token)]).send().await;
            });
        }
        let mut s = self.state.lock().unwrap();
        s.access_token = None;
        s.refresh_token = None;
        s.expires_at = None;
        s.email = None;
        drop(s);
        self.save();
    }

    /// Browser sign-in. Opens `open_url` with the authorization URL and waits for the redirect
    /// on a loopback port (RFC 8252).
    pub async fn sign_in(&self, open_url: impl Fn(String)) -> Result<(), ApiError> {
        let api = self.api_base();
        let server = tiny_http::Server::http("127.0.0.1:0").map_err(|e| ApiError::Http(e.to_string()))?;
        let port = server.server_addr().to_ip().map(|a| a.port()).unwrap_or(0);
        let redirect_uri = format!("http://127.0.0.1:{port}/callback");

        // Register (again) when the redirect port changes; registration is cheap and anonymous.
        let client_id = {
            let res: Value = self
                .http
                .post(format!("{api}/api/oauth/register"))
                .json(&json!({
                    "client_name": "Flowly Desktop",
                    "redirect_uris": [redirect_uri],
                    "token_endpoint_auth_method": "none",
                    "client_uri": "https://flowly.net"
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
        let meta: Value = self
            .http
            .get(format!("{api}/.well-known/oauth-authorization-server"))
            .send()
            .await
            .map_err(|e| ApiError::Http(e.to_string()))?
            .json()
            .await
            .map_err(|e| ApiError::Http(e.to_string()))?;
        let authorize = meta["authorization_endpoint"].as_str().unwrap_or("https://app.flowly.net/oauth/authorize");
        let mut url = url::Url::parse(authorize).map_err(|e| ApiError::Http(e.to_string()))?;
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
            .post(format!("{api}/api/oauth/token"))
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
        self.store_tokens(&tok, Some(client_id), Some(redirect_uri))?;
        if let Ok(me) = self.get("/api/auth/me/").await {
            self.state.lock().unwrap().email = me["email"].as_str().map(String::from);
            self.save();
        }
        Ok(())
    }

    fn store_tokens(&self, tok: &Value, client_id: Option<String>, redirect: Option<String>) -> Result<(), ApiError> {
        let access = tok["access_token"].as_str().ok_or_else(|| ApiError::Http(format!("Token error: {tok}")))?;
        let mut s = self.state.lock().unwrap();
        s.access_token = Some(access.to_string());
        s.refresh_token = tok["refresh_token"].as_str().map(String::from).or(s.refresh_token.take());
        s.expires_at = Some(chrono::Utc::now().timestamp() + tok["expires_in"].as_i64().unwrap_or(3600) - 60);
        if client_id.is_some() {
            s.client_id = client_id;
        }
        if redirect.is_some() {
            s.redirect_uri = redirect;
        }
        drop(s);
        self.save();
        Ok(())
    }

    async fn access_token(&self) -> Result<String, ApiError> {
        let (access, refresh, expires, client_id) = {
            let s = self.state.lock().unwrap();
            (s.access_token.clone(), s.refresh_token.clone(), s.expires_at.unwrap_or(0), s.client_id.clone())
        };
        if let Some(a) = access {
            if expires > chrono::Utc::now().timestamp() {
                return Ok(a);
            }
        }
        let (Some(refresh), Some(client_id)) = (refresh, client_id) else { return Err(ApiError::SignedOut) };
        let res = self
            .http
            .post(format!("{}/api/oauth/token", self.api_base()))
            .form(&[("grant_type", "refresh_token"), ("refresh_token", &refresh), ("client_id", &client_id)])
            .send()
            .await
            .map_err(|e| ApiError::Http(e.to_string()))?;
        if res.status().as_u16() >= 400 {
            // Revoked or expired: the user has to sign in again.
            self.sign_out();
            return Err(ApiError::SignedOut);
        }
        let tok: Value = res.json().await.map_err(|e| ApiError::Http(e.to_string()))?;
        self.store_tokens(&tok, None, None)?;
        Ok(tok["access_token"].as_str().unwrap_or_default().to_string())
    }

    async fn send(&self, method: reqwest::Method, path: &str, body: Option<Value>) -> Result<Value, ApiError> {
        let token = self.access_token().await?;
        let mut req = self.http.request(method, format!("{}{}", self.api_base(), path)).bearer_auth(token);
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

    pub async fn get(&self, path: &str) -> Result<Value, ApiError> {
        self.send(reqwest::Method::GET, path, None).await
    }

    pub async fn post(&self, path: &str, body: Value) -> Result<Value, ApiError> {
        self.send(reqwest::Method::POST, path, Some(body)).await
    }
}
