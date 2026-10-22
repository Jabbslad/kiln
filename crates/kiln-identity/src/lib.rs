mod audit;
mod config;
mod providers;
mod store;
pub mod tokens;
pub use audit::{SignInAuditRow, query_signin_audit, read_signin_audit, signin_audit_path};
use axum::{
    Json, Router,
    extract::{ConnectInfo, Form, FromRequestParts, Path, State},
    http::{HeaderMap, StatusCode, header, request::Parts},
    response::{Html, IntoResponse, Redirect, Response},
    routing::{delete, get, post, put},
};
use base64::Engine;
pub use config::{
    Config, GITHUB_AUTHORIZE_URL, GITHUB_TOKEN_URL, GITHUB_USER_URL, PRODUCTION_IDENTITY_ORIGIN,
    unix_now,
};
use kiln_api::auth::{
    DeviceTokens, MemberRole, Registration, Scope, ServerClaims, TenantMember, TenantSummary,
};
pub use providers::{Provider, ProviderIdentity};
use rusqlite::{OptionalExtension, params};
use serde::{Deserialize, Serialize};
use sha2::Digest;
use std::{
    net::{IpAddr, SocketAddr},
    sync::Arc,
};
pub use store::Store;
use store::{ensure_owner_tenant, hash, secret, set_default_server};
use subtle::ConstantTimeEq;

#[derive(Clone)]
struct App {
    cfg: Config,
    store: Store,
    signer: tokens::Signer,
    http: reqwest::Client,
}
pub fn router(mut cfg: Config, store: Store) -> anyhow::Result<Router> {
    let u = url::Url::parse(&cfg.public_origin)?;
    anyhow::ensure!(
        u.scheme() == "https"
            && u.host_str().is_some()
            && u.username().is_empty()
            && u.password().is_none()
            && u.path() == "/"
            && u.query().is_none()
            && u.fragment().is_none(),
        "public origin must be an HTTPS origin"
    );
    cfg.public_origin = u.origin().ascii_serialization();
    for endpoint in [
        &cfg.github_authorize_url,
        &cfg.github_token_url,
        &cfg.github_user_url,
    ] {
        let endpoint = url::Url::parse(endpoint)?;
        anyhow::ensure!(
            endpoint.scheme() == "https"
                && endpoint.host_str().is_some()
                && endpoint.username().is_empty()
                && endpoint.password().is_none(),
            "provider URL must be HTTPS"
        );
    }
    anyhow::ensure!(
        cfg.production_github()
            || (cfg!(feature = "lab-provider") && cfg.public_origin != PRODUCTION_IDENTITY_ORIGIN),
        "{}",
        if cfg.public_origin == PRODUCTION_IDENTITY_ORIGIN {
            "https://dark-forge.dev cannot use a substitute identity provider"
        } else {
            "this identity build accepts only the production GitHub endpoints"
        }
    );
    check_private_file(&cfg.signing_key_file)?;
    check_private_file(&cfg.github_secret_file)?;
    check_private_file(&cfg.google_secret_file)?;
    let mut signer =
        tokens::Signer::from_pem(&cfg.signing_key_id, &std::fs::read(&cfg.signing_key_file)?)?;
    if let Some(path) = &cfg.retiring_jwks_file {
        check_private_file(path)?;
        anyhow::ensure!(
            std::fs::metadata(path)?.len() <= 65536,
            "retiring JWKS too large"
        );
        signer = signer.with_retiring(serde_json::from_slice(&std::fs::read(path)?)?)?;
    }
    let http = providers::http_client(cfg.provider_ca_pem.as_deref())?;
    let app = Arc::new(App {
        cfg,
        store,
        signer,
        http,
    });
    let cleanup = Arc::downgrade(&app);
    tokio::spawn(async move {
        let mut interval = tokio::time::interval(std::time::Duration::from_secs(60));
        loop {
            interval.tick().await;
            let Some(cleanup) = cleanup.upgrade() else {
                break;
            };
            let _ = cleanup.store.prune((cleanup.cfg.now)());
        }
    });
    Ok(Router::new()
        .route("/oauth/device/code", post(device_code))
        .route("/oauth/token", post(oauth_token))
        .route("/login", get(login))
        .route("/logout", post(logout))
        .route("/static/auth.css", get(auth_css))
        .route(
            "/static/fonts/geist-latin-400-normal.woff2",
            get(auth_font_geist),
        )
        .route(
            "/static/fonts/geist-mono-latin-400-normal.woff2",
            get(auth_font_geist_mono),
        )
        .route("/device", get(device_page))
        .route("/device/approve", post(approve))
        .route("/oauth/server", get(oauth_server))
        .route("/oauth/server/token", post(server_token))
        .route("/oauth/server/revoke", post(revoke_reader))
        .route("/oauth/{provider}", get(oauth_begin))
        .route("/oauth/{provider}/callback", get(oauth_callback))
        .route("/.well-known/jwks.json", get(jwks))
        .route("/v1/me", get(me))
        .route("/v1/servers", get(servers))
        .route(
            "/v1/servers/{id}",
            put(update_server)
                .delete(delete_server)
                .layer(axum::extract::DefaultBodyLimit::max(70 * 1024)),
        )
        .route("/v1/servers/{id}/app-login", post(app_login))
        .route("/v1/access", post(access))
        .route("/v1/server-status", post(server_status))
        .route("/v1/devices", get(devices))
        .route("/v1/devices/{id}", delete(revoke_device))
        .route(
            "/v1/registrations",
            post(registration).layer(axum::extract::DefaultBodyLimit::max(70 * 1024)),
        )
        .route("/v1/registrations/poll", post(registration_poll))
        .route("/v1/registrations/activate", post(registration_activate))
        .route("/v1/keys", get(keys).post(create_key))
        .route("/v1/keys/{id}", delete(revoke_key))
        .route("/v1/key-token", post(key_token))
        .route("/v1/tenants", get(tenant_list))
        .route(
            "/v1/tenants/{id}/members",
            get(list_tenant_members).post(add_tenant_member),
        )
        .route(
            "/v1/tenants/{id}/members/{name}",
            delete(remove_tenant_member),
        )
        .route("/v1/tenants/{id}/servers", get(tenant_servers))
        .with_state(app)
        .layer(axum::extract::DefaultBodyLimit::max(16 * 1024))
        .layer(axum::middleware::from_fn(
            |request: axum::extract::Request, next: axum::middleware::Next| async move {
                secure(nostore(next.run(request).await))
            },
        )))
}
fn page(body: &str) -> Html<String> {
    Html(format!(
        r#"<!doctype html><html lang=en><head><meta charset=utf-8><meta name=viewport content="width=device-width,initial-scale=1"><meta name=theme-color content=#0e0e0e><title>Kiln identity</title><link rel=stylesheet href=/static/auth.css></head><body><main class=card><div class=mark>Kiln identity</div>{body}</main></body></html>"#
    ))
}
async fn auth_css() -> Response {
    let mut response = include_str!("../static/auth.css").into_response();
    response.headers_mut().insert(
        header::CONTENT_TYPE,
        "text/css; charset=utf-8".parse().expect("static header"),
    );
    secure(response)
}
async fn auth_font_geist() -> Response {
    static_font(include_bytes!(
        "../static/fonts/geist-latin-400-normal.woff2"
    ))
}
async fn auth_font_geist_mono() -> Response {
    static_font(include_bytes!(
        "../static/fonts/geist-mono-latin-400-normal.woff2"
    ))
}
fn static_font(body: &'static [u8]) -> Response {
    let mut response = body.into_response();
    response.headers_mut().insert(
        header::CONTENT_TYPE,
        "font/woff2".parse().expect("static header"),
    );
    secure(response)
}
async fn login() -> impl IntoResponse {
    page(
        r#"<h1>Light the way in.</h1><p>Choose the same identity provider you used before. Accounts are never merged by email.</p><a class=button href=/oauth/github>Continue with GitHub</a><a class="button secondary" href=/oauth/google>Continue with Google</a>"#,
    )
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct LogoutForm {
    csrf: String,
}
async fn logout(
    State(a): State<Arc<App>>,
    peer: Peer,
    h: HeaderMap,
    Form(form): Form<LogoutForm>,
) -> Response {
    let session = cookie(&h, "__Host-kiln_session").unwrap_or_default();
    let revoke = |outcome: &'static str,
                  reason: &'static str,
                  account: &str,
                  tenant: &str,
                  response: Response| {
        auth_note(
            &a,
            peer.0,
            &h,
            audit::REVOCATION,
            "session",
            outcome,
            reason,
            account,
            tenant,
            "",
            &session,
            "",
            response,
        )
    };
    if h.get(header::ORIGIN).and_then(|v| v.to_str().ok()) != Some(a.cfg.public_origin.as_str()) {
        return revoke(
            "failure",
            "invalid_origin",
            "",
            "",
            err(StatusCode::FORBIDDEN, "invalid_origin"),
        );
    }
    if session.is_empty() {
        return revoke(
            "failure",
            "login_required",
            "",
            "",
            err(StatusCode::UNAUTHORIZED, "login_required"),
        );
    }
    let valid = a.store.db.lock().ok().and_then(|c| c.query_row(
        "SELECT EXISTS(SELECT 1 FROM browser_sessions WHERE hash=?1 AND csrf_hash=?2 AND expires_at>?3)",
        params![hash(&session), hash(&form.csrf), (a.cfg.now)()], |r| r.get::<_, bool>(0)).ok()).unwrap_or(false);
    if !valid {
        return revoke(
            "failure",
            "invalid_csrf",
            "",
            "",
            err(StatusCode::FORBIDDEN, "invalid_csrf"),
        );
    }
    let account = a
        .store
        .db
        .lock()
        .ok()
        .and_then(|c| {
            c.query_row(
                "SELECT IFNULL(account_id, '') FROM browser_sessions WHERE hash=?1",
                params![hash(&session)],
                |r| r.get::<_, String>(0),
            )
            .ok()
        })
        .unwrap_or_default();
    let tenant = a
        .store
        .db
        .lock()
        .ok()
        .map(|c| tenant_on(&c, &account))
        .unwrap_or_default();
    let removed = a.store.db.lock().ok().and_then(|c| {
        c.execute(
            "DELETE FROM browser_sessions WHERE hash=?1",
            [hash(&session)],
        )
        .ok()
    });
    if removed != Some(1) {
        return revoke(
            "failure",
            "server_error",
            &account,
            &tenant,
            err(StatusCode::INTERNAL_SERVER_ERROR, "server_error"),
        );
    }
    let mut response = Redirect::to("/login").into_response();
    response.headers_mut().insert(
        header::SET_COOKIE,
        "__Host-kiln_session=; Secure; HttpOnly; SameSite=Lax; Path=/; Max-Age=0"
            .parse()
            .expect("static cookie"),
    );
    revoke("success", "", &account, &tenant, secure(nostore(response)))
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct DeviceForm {
    client_id: String,
    device_name: String,
}
async fn device_code(
    State(a): State<Arc<App>>,
    ConnectInfo(peer): ConnectInfo<SocketAddr>,
    h: HeaderMap,
    Form(f): Form<DeviceForm>,
) -> Response {
    if f.client_id != "kiln-cli" {
        return err(StatusCode::BAD_REQUEST, "invalid_client");
    }
    let ip = client_ip(&a.cfg, peer, &h);
    match a
        .store
        .create_device(&f.device_name, &ip.to_string(), (a.cfg.now)())
    {
        Ok(mut x) => {
            x.verification_uri = format!("{}/device", a.cfg.public_origin.trim_end_matches('/'));
            x.verification_uri_complete =
                format!("{}?user_code={}", x.verification_uri, x.user_code);
            nostore(Json(x).into_response())
        }
        Err(_) => err(StatusCode::TOO_MANY_REQUESTS, "rate_limited"),
    }
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct TokenForm {
    grant_type: String,
    device_code: Option<String>,
    refresh_token: Option<String>,
}
async fn oauth_token(
    State(a): State<Arc<App>>,
    ConnectInfo(peer): ConnectInfo<SocketAddr>,
    h: HeaderMap,
    Form(f): Form<TokenForm>,
) -> Response {
    let now = (a.cfg.now)();
    const DEVICE_POLL_SECONDS: i64 = 5;
    const TOKEN_RATE_WINDOW: i64 = 60;
    const TOKEN_RATE_MAX: i64 = TOKEN_RATE_WINDOW / DEVICE_POLL_SECONDS * 2;
    let limited = (|| -> anyhow::Result<()> {
        let mut c = a.store.db.lock().unwrap();
        let tx = c.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
        store::rate(
            &tx,
            &format!("token:{}", client_ip(&a.cfg, peer, &h)),
            now,
            TOKEN_RATE_MAX,
            TOKEN_RATE_WINDOW,
        )?;
        tx.commit()?;
        Ok(())
    })();
    let event = if f.grant_type == "refresh_token" {
        audit::TOKEN_REFRESH
    } else {
        audit::TOKEN_ISSUE
    };
    let method = if f.grant_type == "refresh_token" {
        "refresh_token"
    } else {
        "device_code"
    };
    let audit = |outcome: &'static str,
                 reason: &'static str,
                 account: &str,
                 tenant: &str,
                 token: &str,
                 response: Response| {
        auth_note(
            &a, peer, &h, event, method, outcome, reason, account, tenant, "kiln-cli", "", token,
            response,
        )
    };
    if limited.is_err() {
        return audit(
            "failure",
            "rate_limited",
            "",
            "",
            "",
            err(StatusCode::TOO_MANY_REQUESTS, "rate_limited"),
        );
    }
    let mut c = a.store.db.lock().unwrap();
    let tx = match c.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate) {
        Ok(x) => x,
        Err(_) => {
            return audit(
                "failure",
                "server_error",
                "",
                "",
                "",
                err(StatusCode::INTERNAL_SERVER_ERROR, "server_error"),
            );
        }
    };
    if f.grant_type == "urn:ietf:params:oauth:grant-type:device_code" {
        let raw = match f.device_code {
            Some(x) => x,
            None => {
                return audit(
                    "failure",
                    "invalid_request",
                    "",
                    "",
                    "",
                    err(StatusCode::BAD_REQUEST, "invalid_request"),
                );
            }
        };
        let row = match tx.query_row("SELECT name,state,expires_at,next_poll,interval,account_id FROM pending_devices WHERE secret_hash=?1",[hash(&raw)],|r|Ok((r.get::<_,String>(0)?,r.get::<_,String>(1)?,r.get::<_,i64>(2)?,r.get::<_,i64>(3)?,r.get::<_,i64>(4)?,r.get::<_,Option<String>>(5)?))).optional() {
            Ok(row) => row,
            Err(_) => return audit("failure", "server_error", "", "", &raw, err(StatusCode::INTERNAL_SERVER_ERROR, "server_error")),
        };
        let Some((name, state, exp, next, interval, account)) = row else {
            return audit(
                "failure",
                "invalid_grant",
                "",
                "",
                &raw,
                err(StatusCode::BAD_REQUEST, "invalid_grant"),
            );
        };
        if now >= exp {
            return audit(
                "failure",
                "expired_token",
                account.as_deref().unwrap_or(""),
                &tenant_on(&tx, account.as_deref().unwrap_or("")),
                &raw,
                err(StatusCode::BAD_REQUEST, "expired_token"),
            );
        }
        if now < next {
            let _ = tx.execute(
                "UPDATE pending_devices SET interval=interval+5,next_poll=?1 WHERE secret_hash=?2",
                params![now + interval + 5, hash(&raw)],
            );
            let _ = tx.commit();
            return err(StatusCode::BAD_REQUEST, "slow_down");
        }
        if state == "pending" {
            let _ = tx.execute(
                "UPDATE pending_devices SET next_poll=?1 WHERE secret_hash=?2",
                params![now + interval, hash(&raw)],
            );
            let _ = tx.commit();
            return err(StatusCode::BAD_REQUEST, "authorization_pending");
        }
        if state == "denied" {
            return audit(
                "failure",
                "access_denied",
                account.as_deref().unwrap_or(""),
                &tenant_on(&tx, account.as_deref().unwrap_or("")),
                &raw,
                err(StatusCode::BAD_REQUEST, "access_denied"),
            );
        }
        let Some(account) = account.filter(|_| state == "approved") else {
            return audit(
                "failure",
                "invalid_grant",
                "",
                "",
                &raw,
                err(StatusCode::BAD_REQUEST, "invalid_grant"),
            );
        };
        let live: bool = match tx.query_row(
            "SELECT EXISTS(SELECT 1 FROM accounts WHERE id=?1 AND revoked_at IS NULL)",
            [&account],
            |r| r.get(0),
        ) {
            Ok(value) => value,
            Err(_) => {
                return audit(
                    "failure",
                    "server_error",
                    &account,
                    &tenant_on(&tx, &account),
                    &raw,
                    err(StatusCode::INTERNAL_SERVER_ERROR, "server_error"),
                );
            }
        };
        if !live {
            return audit(
                "failure",
                "invalid_grant",
                &account,
                &tenant_on(&tx, &account),
                &raw,
                err(StatusCode::BAD_REQUEST, "invalid_grant"),
            );
        }
        let did = format!("dev_{}", uuid::Uuid::new_v4().simple());
        let family = secret();
        let refresh = secret();
        let central = secret();
        let tenant = tenant_on(&tx, &account);
        let ok=tx.execute("DELETE FROM pending_devices WHERE secret_hash=?1",[hash(&raw)]).and_then(|_|tx.execute("INSERT INTO devices(id,account_id,name,created_at,last_used)VALUES(?1,?2,?3,?4,?4)",params![did,account,name,now])).and_then(|_|tx.execute("INSERT INTO refresh_tokens(hash,family_id,device_id,created_at,last_used,absolute_expiry)VALUES(?1,?2,?3,?4,?4,?5)",params![hash(&refresh),family,did,now,now+90*86400])).and_then(|_|tx.execute("INSERT INTO central_tokens(hash,device_id,family_id,expires_at)VALUES(?1,?2,?3,?4)",params![hash(&central),did,family,now+300]));
        if ok.is_err() || tx.commit().is_err() {
            return audit(
                "failure",
                "server_error",
                &account,
                &tenant,
                &raw,
                err(StatusCode::INTERNAL_SERVER_ERROR, "server_error"),
            );
        }
        let issued = central.clone();
        return audit(
            "success",
            "",
            &account,
            &tenant,
            &issued,
            nostore(
                Json(DeviceTokens {
                    access_token: central,
                    token_type: "Bearer".into(),
                    expires_in: 300,
                    refresh_token: refresh,
                })
                .into_response(),
            ),
        );
    }
    if f.grant_type == "refresh_token" {
        let raw = match f.refresh_token {
            Some(x) => x,
            None => {
                return audit(
                    "failure",
                    "invalid_request",
                    "",
                    "",
                    "",
                    err(StatusCode::BAD_REQUEST, "invalid_request"),
                );
            }
        };
        let row = match tx.query_row("SELECT rowid,family_id,device_id,created_at,last_used,absolute_expiry,spent_at,family_revoked_at FROM refresh_tokens WHERE hash=?1",[hash(&raw)],|r|Ok((r.get::<_,i64>(0)?,r.get::<_,String>(1)?,r.get::<_,String>(2)?,r.get::<_,i64>(3)?,r.get::<_,i64>(4)?,r.get::<_,i64>(5)?,r.get::<_,Option<i64>>(6)?,r.get::<_,Option<i64>>(7)?))).optional() {
            Ok(row) => row,
            Err(_) => return audit("failure", "server_error", "", "", &raw, err(StatusCode::INTERNAL_SERVER_ERROR, "server_error")),
        };
        let Some((rowid, family, did, _, last, absolute, spent, revoked)) = row else {
            return audit(
                "failure",
                "invalid_grant",
                "",
                "",
                &raw,
                err(StatusCode::BAD_REQUEST, "invalid_grant"),
            );
        };
        let account: String = tx
            .query_row("SELECT account_id FROM devices WHERE id=?1", [&did], |r| {
                r.get(0)
            })
            .unwrap_or_default();
        let tenant = tenant_on(&tx, &account);
        if spent.is_some() {
            if tx
                .execute(
                    "UPDATE refresh_tokens SET family_revoked_at=?1 WHERE family_id=?2",
                    params![now, family],
                )
                .is_err()
                || tx.commit().is_err()
            {
                return audit(
                    "failure",
                    "server_error",
                    &account,
                    &tenant,
                    &raw,
                    err(StatusCode::INTERNAL_SERVER_ERROR, "server_error"),
                );
            }
            return audit(
                "failure",
                "invalid_grant",
                &account,
                &tenant,
                &raw,
                err(StatusCode::BAD_REQUEST, "invalid_grant"),
            );
        }
        let live: bool = match tx.query_row("SELECT EXISTS(SELECT 1 FROM devices d JOIN accounts a ON a.id=d.account_id WHERE d.id=?1 AND d.revoked_at IS NULL AND a.revoked_at IS NULL)",[&did],|r|r.get(0)) {
            Ok(value) => value,
            Err(_) => return audit("failure", "server_error", &account, &tenant, &raw, err(StatusCode::INTERNAL_SERVER_ERROR, "server_error")),
        };
        if !live || revoked.is_some() || now - last >= 30 * 86400 || now >= absolute {
            return audit(
                "failure",
                "invalid_grant",
                &account,
                &tenant,
                &raw,
                err(StatusCode::BAD_REQUEST, "invalid_grant"),
            );
        }
        let refresh = secret();
        let central = secret();
        let ok=tx.execute("UPDATE refresh_tokens SET spent_at=?1 WHERE rowid=?2 AND spent_at IS NULL",params![now,rowid]).and_then(|n|if n==1{Ok(n)}else{Err(rusqlite::Error::ExecuteReturnedResults)}).and_then(|_|tx.execute("INSERT INTO refresh_tokens(hash,family_id,device_id,created_at,last_used,absolute_expiry)VALUES(?1,?2,?3,?4,?4,?5)",params![hash(&refresh),family,did,now,absolute])).and_then(|_|tx.execute("INSERT INTO central_tokens(hash,device_id,family_id,expires_at)VALUES(?1,?2,?3,?4)",params![hash(&central),did,family,now+300]));
        let ok = ok.and_then(|_| {
            tx.execute(
                "UPDATE devices SET last_used=?1 WHERE id=?2",
                params![now, did],
            )
        });
        if ok.is_err() || tx.commit().is_err() {
            return audit(
                "failure",
                "invalid_grant",
                &account,
                &tenant,
                &raw,
                err(StatusCode::BAD_REQUEST, "invalid_grant"),
            );
        }
        let issued = central.clone();
        return audit(
            "success",
            "",
            &account,
            &tenant,
            &issued,
            nostore(
                Json(DeviceTokens {
                    access_token: central,
                    token_type: "Bearer".into(),
                    expires_in: 300,
                    refresh_token: refresh,
                })
                .into_response(),
            ),
        );
    }
    audit(
        "failure",
        "unsupported_grant_type",
        "",
        "",
        "",
        err(StatusCode::BAD_REQUEST, "unsupported_grant_type"),
    )
}
fn bearer(h: &HeaderMap) -> Option<&str> {
    h.get(header::AUTHORIZATION)?
        .to_str()
        .ok()?
        .strip_prefix("Bearer ")
}
// Authorization and use share a SQLite write transaction, including across processes.
fn authorized(
    a: &App,
    h: &HeaderMap,
    action: impl FnOnce(&rusqlite::Transaction<'_>, String, String) -> anyhow::Result<Response>,
) -> Response {
    let Some(raw) = bearer(h) else {
        return err(StatusCode::UNAUTHORIZED, "invalid_token");
    };
    let now = (a.cfg.now)();
    let mut c = a.store.db.lock().unwrap();
    let tx = match c.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate) {
        Ok(tx) => tx,
        Err(_) => return err(StatusCode::INTERNAL_SERVER_ERROR, "server_error"),
    };
    let principal = tx.query_row("SELECT d.account_id,d.id FROM central_tokens t JOIN devices d ON d.id=t.device_id JOIN accounts x ON x.id=d.account_id WHERE t.hash=?1 AND t.expires_at>?2 AND d.revoked_at IS NULL AND x.revoked_at IS NULL AND EXISTS(SELECT 1 FROM refresh_tokens r WHERE r.family_id=t.family_id AND r.family_revoked_at IS NULL AND r.spent_at IS NULL AND r.absolute_expiry>?2 AND r.last_used>?2-2592000)",params![hash(raw),now],|r|Ok((r.get::<_,String>(0)?,r.get::<_,String>(1)?)));
    let Ok((owner, device)) = principal else {
        return err(StatusCode::UNAUTHORIZED, "invalid_token");
    };
    match action(&tx, owner, device) {
        Ok(response) if !response.status().is_success() => response,
        Ok(response) => match tx.commit() {
            Ok(()) => nostore(response),
            Err(_) => err(StatusCode::INTERNAL_SERVER_ERROR, "server_error"),
        },
        Err(_) => err(StatusCode::INTERNAL_SERVER_ERROR, "server_error"),
    }
}
async fn me(State(a): State<Arc<App>>, h: HeaderMap) -> Response {
    authorized(&a, &h, |_, account, device| {
        Ok(Json(serde_json::json!({"account_id":account,"device_id":device})).into_response())
    })
}
async fn servers(State(a): State<Arc<App>>, h: HeaderMap) -> Response {
    authorized(&a, &h, |tx, account, _| {
        let mut q = tx.prepare(
            "SELECT s.id,s.name,s.origin,s.ca_pem,s.revision FROM servers s
             JOIN tenant_members m ON m.tenant_id=s.tenant_id
             WHERE m.account_id=?1 AND s.active=1 ORDER BY s.created_at",
        )?;
        let values = q
            .query_map([account], |r| {
                Ok(kiln_api::auth::ServerDescriptor {
                    id: r.get(0)?,
                    name: r.get(1)?,
                    origin: r.get(2)?,
                    ca_pem: r.get(3)?,
                    revision: r.get::<_, i64>(4)? as u64,
                })
            })?
            .collect::<Result<Vec<_>, _>>()?;
        Ok(Json(values).into_response())
    })
}
fn member_role(raw: &str) -> Option<MemberRole> {
    match raw {
        "owner" => Some(MemberRole::Owner),
        "member" => Some(MemberRole::Member),
        _ => None,
    }
}
fn local_member_name(name: &str) -> bool {
    (1..=32).contains(&name.len())
        && name
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
        && name
            .as_bytes()
            .first()
            .is_some_and(|b| b.is_ascii_lowercase())
}
fn tenant_owner(
    tx: &rusqlite::Transaction<'_>,
    tenant_id: &str,
    account: &str,
) -> rusqlite::Result<bool> {
    tx.query_row(
        "SELECT EXISTS(SELECT 1 FROM tenant_members WHERE tenant_id=?1 AND account_id=?2 AND role='owner')",
        params![tenant_id, account],
        |r| r.get(0),
    )
}
fn tenant_member_rows(
    tx: &rusqlite::Transaction<'_>,
    tenant_id: &str,
) -> rusqlite::Result<Vec<TenantMember>> {
    let mut q = tx.prepare(
        "SELECT account_id,role,local_name FROM tenant_members WHERE tenant_id=?1 ORDER BY role,local_name,account_id",
    )?;
    q.query_map([tenant_id], |r| {
        let role: String = r.get(1)?;
        Ok(TenantMember {
            account_id: r.get(0)?,
            role: member_role(&role).unwrap_or(MemberRole::Member),
            name: r.get(2)?,
        })
    })?
    .collect()
}
fn tenant_servers_rows(
    tx: &rusqlite::Transaction<'_>,
    tenant_id: &str,
) -> rusqlite::Result<Vec<kiln_api::auth::ServerDescriptor>> {
    let mut q = tx.prepare(
        "SELECT id,name,origin,ca_pem,revision FROM servers WHERE tenant_id=?1 AND active=1 ORDER BY created_at",
    )?;
    q.query_map([tenant_id], |r| {
        Ok(kiln_api::auth::ServerDescriptor {
            id: r.get(0)?,
            name: r.get(1)?,
            origin: r.get(2)?,
            ca_pem: r.get(3)?,
            revision: r.get::<_, i64>(4)? as u64,
        })
    })?
    .collect()
}
fn directory_in_tenant(
    tx: &rusqlite::Transaction<'_>,
    tenant_id: &str,
    raw: &str,
) -> rusqlite::Result<bool> {
    tx.query_row(
        "SELECT EXISTS(SELECT 1 FROM servers WHERE tenant_id=?1 AND directory_hash=?2 AND active=1)",
        params![tenant_id, hash(raw)],
        |r| r.get(0),
    )
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct AccessReq {
    server_id: String,
}
async fn access(
    State(a): State<Arc<App>>,
    peer: Peer,
    h: HeaderMap,
    Json(f): Json<AccessReq>,
) -> Response {
    authorized(&a, &h, |tx, account, did| {
        let row: Option<(String, String)> = tx
            .query_row(
                "SELECT s.tenant_id,m.role FROM servers s JOIN tenant_members m ON m.tenant_id=s.tenant_id AND m.account_id=?2 WHERE s.id=?1 AND s.active=1",
                params![f.server_id, account],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .optional()?;
        let Some((tenant_id, role_raw)) = row else {
            return Ok(err(StatusCode::NOT_FOUND, "not_found"));
        };
        let Some(role) = member_role(&role_raw) else {
            return Ok(err(StatusCode::NOT_FOUND, "not_found"));
        };
        let now = (a.cfg.now)() as u64;
        let claims = ServerClaims {
            iss: a.cfg.public_origin.clone(),
            sub: account,
            aud: format!("kiln:server:{}", f.server_id),
            tenant_id: Some(tenant_id),
            iat: now,
            nbf: now,
            exp: now + 300,
            credential_id: did,
            scope: Scope::Operate,
            role: Some(role),
        };
        let token = a.signer.issue(&claims)?;
        let issued = token.clone();
        Ok(auth_note(
            &a,
            peer.0,
            &h,
            audit::TOKEN_ISSUE,
            "device",
            "success",
            "",
            &claims.sub,
            claims.tenant_id.as_deref().unwrap_or(""),
            &f.server_id,
            "",
            &issued,
            Json(tokens::Grant {
                access_token: token,
                token_type: "Bearer",
                expires_in: 300,
            })
            .into_response(),
        ))
    })
}
async fn jwks(State(a): State<Arc<App>>) -> impl IntoResponse {
    Json(a.signer.jwks())
}
async fn devices(State(a): State<Arc<App>>, h: HeaderMap) -> Response {
    authorized(&a, &h, |tx, owner, _| {
        let mut q=tx.prepare("SELECT id,name,last_used,created_at+7776000 FROM devices WHERE account_id=?1 AND revoked_at IS NULL")?;
        let v=q.query_map([owner],|r|Ok(serde_json::json!({"id":r.get::<_,String>(0)?,"name":r.get::<_,String>(1)?,"last_used":r.get::<_,i64>(2)?,"expires_at":r.get::<_,i64>(3)?})))?.collect::<Result<Vec<_>,_>>()?;
        Ok(Json(v).into_response())
    })
}
async fn revoke_device(
    State(a): State<Arc<App>>,
    peer: Peer,
    h: HeaderMap,
    Path(id): Path<String>,
) -> Response {
    let token = bearer(&h).unwrap_or("").to_owned();
    let saw = std::cell::Cell::new(false);
    let response = authorized(&a, &h, |tx, owner, _| {
        saw.set(true);
        let tenant = tenant_on(tx, &owner);
        let n = tx.execute(
            "UPDATE devices SET revoked_at=?1 WHERE id=?2 AND account_id=?3 AND revoked_at IS NULL",
            params![(a.cfg.now)(), id, owner],
        )?;
        if n == 1 {
            tx.execute("INSERT INTO audit_events(event,actor_id,resource_id,outcome,created_at) VALUES('device_revoked',?1,?2,'success',?3)", params![owner,id,(a.cfg.now)()])?;
            Ok(auth_note(
                &a,
                peer.0,
                &h,
                audit::REVOCATION,
                "device",
                "success",
                "",
                &owner,
                &tenant,
                &id,
                "",
                &token,
                StatusCode::NO_CONTENT.into_response(),
            ))
        } else {
            Ok(auth_note(
                &a,
                peer.0,
                &h,
                audit::REVOCATION,
                "device",
                "failure",
                "not_found",
                &owner,
                &tenant,
                &id,
                "",
                &token,
                err(StatusCode::NOT_FOUND, "not_found"),
            ))
        }
    });
    if saw.get() {
        response
    } else {
        let reason = if response.status() == StatusCode::UNAUTHORIZED {
            "invalid_token"
        } else {
            "server_error"
        };
        auth_note(
            &a,
            peer.0,
            &h,
            audit::REVOCATION,
            "device",
            "failure",
            reason,
            "",
            "",
            &id,
            "",
            &token,
            response,
        )
    }
}

#[derive(Deserialize)]
struct CodeQuery {
    user_code: Option<String>,
}
async fn device_page(
    State(a): State<Arc<App>>,
    axum::extract::Query(q): axum::extract::Query<CodeQuery>,
    h: HeaderMap,
) -> Response {
    let explicit_code = q.user_code.is_some();
    let mut code = q.user_code.unwrap_or_default().trim().to_uppercase();
    if code.len() > 16 {
        return err(StatusCode::BAD_REQUEST, "invalid_code");
    }
    let session = cookie(&h, "__Host-kiln_session");
    if !explicit_code && let Some(raw) = &session {
        code = a
            .store
            .db
            .lock()
            .ok()
            .and_then(|c| {
                c.query_row(
                    "SELECT requested_code FROM browser_sessions WHERE hash=?1 AND expires_at>?2",
                    params![hash(raw), (a.cfg.now)()],
                    |r| r.get::<_, Option<String>>(0),
                )
                .ok()
                .flatten()
            })
            .unwrap_or_default();
    }
    let csrf = cookie(&h, "__Host-kiln_csrf").unwrap_or_default();
    let logged_in = session.as_ref().is_some_and(|raw| a.store.db.lock().ok().and_then(|c| c.query_row("SELECT EXISTS(SELECT 1 FROM browser_sessions s JOIN accounts a ON a.id=s.account_id WHERE s.hash=?1 AND s.expires_at>?2 AND a.revoked_at IS NULL)",params![hash(raw),(a.cfg.now)()],|r|r.get::<_,bool>(0)).ok()).unwrap_or(false));
    let details = if code.is_empty() {
        None
    } else {
        request_details(&a.store, &code, (a.cfg.now)())
    };
    // Resume pending OAuth requests, but never trap code entry on a stale one.
    if code.is_empty() || (!explicit_code && details.is_none()) {
        return secure(page("<h1>Enter your code.</h1><form method=get action=/device><label for=user_code>Code from your terminal</label><input id=user_code name=user_code maxlength=16 required autocomplete=one-time-code><button>Continue</button></form>").into_response());
    }
    let Some(details) = details else {
        return page("<h1>Request unavailable.</h1><p>This code is invalid, expired, or already used. Start a new request from your terminal.</p><a href=\"/device?user_code=\">Enter another code</a>").into_response();
    };
    let body = format!(
        "<h1>Approve this request.</h1><p>Confirm that this code and request details match your terminal.</p><div class=code>{}</div>{}{}",
        esc(&code),
        details,
        if logged_in {
            format!(
                r#"<form method=post action=/device/approve><input type=hidden name=csrf value="{}"><input type=hidden name=user_code value="{}"><input type=hidden name=decision value=approve><button>Approve request</button></form><form method=post action=/device/approve><input type=hidden name=csrf value="{}"><input type=hidden name=user_code value="{}"><input type=hidden name=decision value=deny><button class=secondary>Deny</button></form>"#,
                esc(&csrf),
                esc(&code),
                esc(&csrf),
                esc(&code)
            )
        } else {
            format!(
                "<a class=button href=/oauth/github?user_code={}>Continue with GitHub</a><a class=\"button secondary\" href=/oauth/google?user_code={}>Continue with Google</a>",
                enc(&code),
                enc(&code)
            )
        }
    );
    secure(page(&body).into_response())
}
fn request_details(store: &Store, code: &str, now: i64) -> Option<String> {
    let c = store.db.lock().ok()?;
    if let Ok(name) = c.query_row(
        "SELECT name FROM pending_devices WHERE user_code=?1 AND state='pending' AND expires_at>?2",
        params![code, now],
        |r| r.get::<_, String>(0),
    ) {
        return Some(format!(
            "<h2>Sign in a device</h2><p>{}</p><p>This device can manage the servers owned by your account.</p>",
            esc(&name)
        ));
    }
    let (name, origin, pem) = c.query_row(
        "SELECT name,origin,ca_pem FROM pending_registrations WHERE user_code=?1 AND state='pending' AND expires_at>?2",
        params![code,now], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?, r.get::<_, String>(2)?))).ok()?;
    let fingerprint = x509_parser::pem::parse_x509_pem(pem.as_bytes())
        .ok()
        .map(|(_, p)| {
            sha2::Sha256::digest(&p.contents)
                .iter()
                .map(|b| format!("{b:02X}"))
                .collect::<Vec<_>>()
                .join(":")
        })
        .unwrap_or_else(|| "invalid certificate".into());
    Some(format!(
        "<h2>Enroll a server</h2><dl><dt>Server</dt><dd>{}</dd><dt>Origin</dt><dd>{}</dd><dt>CA SHA-256</dt><dd class=fingerprint>{}</dd></dl>",
        esc(&name),
        esc(&origin),
        esc(&fingerprint)
    ))
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Approval {
    user_code: String,
    decision: String,
    csrf: String,
}
async fn approve(
    State(a): State<Arc<App>>,
    ConnectInfo(peer): ConnectInfo<SocketAddr>,
    h: HeaderMap,
    Form(f): Form<Approval>,
) -> Response {
    let decision = |outcome: &'static str,
                    reason: &'static str,
                    account: &str,
                    tenant: &str,
                    response: Response| {
        auth_note(
            &a,
            peer,
            &h,
            audit::DEVICE_APPROVAL,
            "device_code",
            outcome,
            reason,
            account,
            tenant,
            "kiln-cli",
            "",
            &f.user_code,
            response,
        )
    };
    if h.get(header::ORIGIN).and_then(|v| v.to_str().ok())
        != Some(a.cfg.public_origin.trim_end_matches('/'))
    {
        return decision(
            "failure",
            "invalid_origin",
            "",
            "",
            err(StatusCode::FORBIDDEN, "invalid_origin"),
        );
    }
    let Some(raw) = cookie(&h, "__Host-kiln_session") else {
        return decision(
            "failure",
            "login_required",
            "",
            "",
            err(StatusCode::UNAUTHORIZED, "login_required"),
        );
    };
    let now = (a.cfg.now)();
    let Some(csrf_cookie) = cookie(&h, "__Host-kiln_csrf") else {
        return decision(
            "failure",
            "invalid_csrf",
            "",
            "",
            err(StatusCode::FORBIDDEN, "invalid_csrf"),
        );
    };
    if f.csrf != csrf_cookie {
        return decision(
            "failure",
            "invalid_csrf",
            "",
            "",
            err(StatusCode::FORBIDDEN, "invalid_csrf"),
        );
    }
    if !matches!(f.decision.as_str(), "approve" | "deny") || f.user_code.len() != 12 {
        return decision(
            "failure",
            "invalid_request",
            "",
            "",
            err(StatusCode::BAD_REQUEST, "invalid_request"),
        );
    }
    let limited = (|| -> anyhow::Result<()> {
        let mut c = a.store.db.lock().unwrap();
        let tx = c.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
        store::rate(
            &tx,
            &format!("approval:{}", client_ip(&a.cfg, peer, &h)),
            now,
            5,
            60,
        )?;
        store::rate(&tx, &format!("code:{}", f.user_code), now, 20, 600)?;
        tx.commit()?;
        Ok(())
    })();
    if limited.is_err() {
        return decision(
            "failure",
            "rate_limited",
            "",
            "",
            err(StatusCode::TOO_MANY_REQUESTS, "rate_limited"),
        );
    }
    let mut c = a.store.db.lock().unwrap();
    let tx = match c.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate) {
        Ok(tx) => tx,
        Err(_) => {
            return decision(
                "failure",
                "server_error",
                "",
                "",
                err(StatusCode::INTERNAL_SERVER_ERROR, "server_error"),
            );
        }
    };
    let account = match tx
        .query_row(
            "SELECT s.account_id FROM browser_sessions s JOIN accounts a ON a.id=s.account_id WHERE s.hash=?1 AND s.csrf_hash=?2 AND s.expires_at>?3 AND a.revoked_at IS NULL",
            params![hash(&raw), hash(&f.csrf), now],
            |r| r.get::<_, Option<String>>(0),
        )
        .optional()
    {
        Ok(row) => row.flatten(),
        Err(_) => {
            return decision(
                "failure",
                "server_error",
                "",
                "",
                err(StatusCode::INTERNAL_SERVER_ERROR, "server_error"),
            );
        }
    };
    let Some(account) = account else {
        return decision(
            "failure",
            "login_required",
            "",
            "",
            err(StatusCode::UNAUTHORIZED, "login_required"),
        );
    };
    let state = if f.decision == "approve" {
        "approved"
    } else {
        "denied"
    };
    let result = (|| -> anyhow::Result<usize> {
        let n = tx.execute("UPDATE pending_devices SET state=?1,account_id=?2 WHERE user_code=?3 AND state='pending' AND expires_at>?4", params![state,account,f.user_code,now])?;
        if n == 1 {
            return Ok(n);
        }
        Ok(tx.execute(
            "UPDATE pending_registrations SET state=?1,owner_id=?2 WHERE user_code=?3 AND state='pending' AND expires_at>?4",
            params![state, account, f.user_code, now],
        )?)
    })();
    let tenant = tenant_on(&tx, &account);
    let outcome = if state == "approved" {
        "success"
    } else {
        "failure"
    };
    let reason = if state == "approved" {
        ""
    } else {
        "access_denied"
    };
    match result {
        Ok(1) => {
            if tx.execute("INSERT INTO audit_events(event,actor_id,resource_id,outcome,created_at) VALUES('request_decision',?1,?2,?3,?4)", params![account,f.user_code,state,now]).is_err() || tx.commit().is_err() {
                return decision(
                    "failure",
                    "server_error",
                    &account,
                    &tenant,
                    err(StatusCode::INTERNAL_SERVER_ERROR, "server_error"),
                );
            }
            decision(
                outcome,
                reason,
                &account,
                &tenant,
                page("<h1>Decision saved.</h1><p>You can return to your terminal.</p>")
                    .into_response(),
            )
        }
        Ok(_) => {
            let _ = tx.commit();
            decision(
                "failure",
                "invalid_code",
                &account,
                &tenant,
                err(StatusCode::BAD_REQUEST, "invalid_code"),
            )
        }
        Err(_) => decision(
            "failure",
            "server_error",
            &account,
            &tenant,
            err(StatusCode::INTERNAL_SERVER_ERROR, "server_error"),
        ),
    }
}
async fn oauth_begin(
    State(a): State<Arc<App>>,
    ConnectInfo(peer): ConnectInfo<SocketAddr>,
    Path(p): Path<String>,
    axum::extract::Query(q): axum::extract::Query<CodeQuery>,
    h: HeaderMap,
) -> Response {
    let provider = match p.as_str() {
        "github" => Provider::Github,
        "google" => Provider::Google,
        _ => return err(StatusCode::NOT_FOUND, "not_found"),
    };
    if q.user_code.as_ref().is_some_and(|s| s.len() > 16) {
        return err(StatusCode::BAD_REQUEST, "invalid_code");
    }
    // Persist the limit before provider I/O, including failed exchanges.
    let limited = (|| -> anyhow::Result<()> {
        let mut c = a.store.db.lock().unwrap();
        let tx = c.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
        store::rate(
            &tx,
            &format!("oauth:{}", client_ip(&a.cfg, peer, &h)),
            (a.cfg.now)(),
            10,
            60,
        )?;
        let pending: i64 = tx.query_row(
            "SELECT count(*) FROM oauth_attempts WHERE expires_at>?1",
            [(a.cfg.now)()],
            |r| r.get(0),
        )?;
        anyhow::ensure!(pending < 10000, "too many pending authorizations");
        tx.commit()?;
        Ok(())
    })();
    if limited.is_err() {
        return err(StatusCode::TOO_MANY_REQUESTS, "rate_limited");
    }
    let session = cookie(&h, "__Host-kiln_session")
        .filter(|s| {
            s.len() == 43
                && s.bytes()
                    .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
        })
        .unwrap_or_else(secret);
    let csrf = secret();
    let redirect = format!(
        "{}/oauth/{}/callback",
        a.cfg.public_origin.trim_end_matches('/'),
        provider.as_str()
    );
    let authorization = match provider {
        Provider::Github => providers::github_begin(
            &a.cfg.github_client_id,
            &redirect,
            &a.cfg.github_authorize_url,
            &a.cfg.github_token_url,
        ),
        Provider::Google => {
            providers::google_begin(&a.http, &a.cfg.google_client_id, &redirect).await
        }
    };
    let authorization = match authorization {
        Ok(value) => value,
        Err(_) => return err(StatusCode::SERVICE_UNAVAILABLE, "provider_unavailable"),
    };
    let now = (a.cfg.now)();
    let write = a.store.db.lock().map_err(|_| ()).and_then(|mut c| {
        let tx = c.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate).map_err(|_| ())?;
        tx.execute("INSERT INTO browser_sessions(hash,csrf_hash,requested_code,expires_at)VALUES(?1,?2,?3,?4) ON CONFLICT(hash) DO UPDATE SET csrf_hash=?2,requested_code=COALESCE(?3,requested_code),expires_at=?4,account_id=CASE WHEN browser_sessions.expires_at>?5 THEN browser_sessions.account_id ELSE NULL END",params![hash(&session),hash(&csrf),q.user_code.map(|x|x.trim().to_uppercase()),now+43200,now]).map_err(|_| ())?;
        tx.execute("INSERT INTO oauth_attempts(state_hash,session_hash,provider,pkce,nonce,expires_at)VALUES(?1,?2,?3,?4,?5,?6)",params![hash(&authorization.state),hash(&session),provider.as_str(),authorization.pkce,authorization.nonce,now+600]).map_err(|_| ())?;
        tx.commit().map_err(|_| ())
    });
    if write.is_err() {
        return err(StatusCode::INTERNAL_SERVER_ERROR, "server_error");
    }
    let mut r = Redirect::to(&authorization.url).into_response();
    r.headers_mut().insert(
        header::SET_COOKIE,
        format!(
            "__Host-kiln_session={session}; Secure; HttpOnly; SameSite=Lax; Path=/; Max-Age=43200"
        )
        .parse()
        .unwrap(),
    );
    r.headers_mut().append(
        header::SET_COOKIE,
        format!("__Host-kiln_csrf={csrf}; Secure; HttpOnly; SameSite=Lax; Path=/; Max-Age=43200")
            .parse()
            .expect("cookie"),
    );
    secure(r)
}
#[derive(Deserialize)]
struct Callback {
    code: String,
    state: String,
}
async fn oauth_callback(
    State(a): State<Arc<App>>,
    peer: Peer,
    Path(p): Path<String>,
    h: HeaderMap,
    axum::extract::Query(q): axum::extract::Query<Callback>,
) -> Response {
    let method = if p == "google" { "google" } else { "github" };
    let client = if p == "google" {
        a.cfg.google_client_id.clone()
    } else {
        a.cfg.github_client_id.clone()
    };
    let sign = |outcome: &'static str,
                reason: &'static str,
                account: &str,
                tenant: &str,
                session: &str,
                response: Response| {
        auth_note(
            &a,
            peer.0,
            &h,
            audit::SIGN_IN,
            method,
            outcome,
            reason,
            account,
            tenant,
            &client,
            session,
            "",
            response,
        )
    };
    let Some(session) = cookie(&h, "__Host-kiln_session") else {
        return sign(
            "failure",
            "invalid_state",
            "",
            "",
            "",
            err(StatusCode::BAD_REQUEST, "invalid_state"),
        );
    };
    let now = (a.cfg.now)();
    let row = {
        let mut c = a.store.db.lock().unwrap();
        let tx = match c.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate) {
            Ok(tx) => tx,
            Err(_) => {
                return sign(
                    "failure",
                    "server_error",
                    "",
                    "",
                    &session,
                    err(StatusCode::INTERNAL_SERVER_ERROR, "server_error"),
                );
            }
        };
        let row=tx.query_row("SELECT provider,pkce,nonce FROM oauth_attempts WHERE state_hash=?1 AND session_hash=?2 AND expires_at>?3",params![hash(&q.state),hash(&session),now],|r|Ok((r.get::<_,String>(0)?,r.get::<_,String>(1)?,r.get::<_,Option<String>>(2)?))).optional().ok().flatten();
        if tx
            .execute(
                "DELETE FROM oauth_attempts WHERE state_hash=?1",
                [hash(&q.state)],
            )
            .is_err()
            || tx.commit().is_err()
        {
            return sign(
                "failure",
                "server_error",
                "",
                "",
                &session,
                err(StatusCode::INTERNAL_SERVER_ERROR, "server_error"),
            );
        }
        row
    };
    let Some((provider, pkce, nonce)) = row else {
        return sign(
            "failure",
            "invalid_state",
            "",
            "",
            &session,
            err(StatusCode::BAD_REQUEST, "invalid_state"),
        );
    };
    if provider != p {
        return sign(
            "failure",
            "provider_mismatch",
            "",
            "",
            &session,
            err(StatusCode::BAD_REQUEST, "provider_mismatch"),
        );
    }
    let redirect = format!(
        "{}/oauth/{}/callback",
        a.cfg.public_origin.trim_end_matches('/'),
        p
    );
    let ident = if p == "github" {
        let s = match std::fs::read_to_string(&a.cfg.github_secret_file) {
            Ok(x) => x,
            Err(_) => {
                return sign(
                    "failure",
                    "provider_unavailable",
                    "",
                    "",
                    &session,
                    err(StatusCode::SERVICE_UNAVAILABLE, "provider_unavailable"),
                );
            }
        };
        providers::github_finish(
            &a.http,
            &a.cfg.github_client_id,
            s.trim(),
            &q.code,
            &redirect,
            &pkce,
            &a.cfg.github_token_url,
            &a.cfg.github_user_url,
        )
        .await
    } else {
        let s = match std::fs::read_to_string(&a.cfg.google_secret_file) {
            Ok(x) => x,
            Err(_) => {
                return sign(
                    "failure",
                    "provider_unavailable",
                    "",
                    "",
                    &session,
                    err(StatusCode::SERVICE_UNAVAILABLE, "provider_unavailable"),
                );
            }
        };
        providers::google_finish(
            &a.http,
            &a.cfg.google_client_id,
            s.trim(),
            &redirect,
            &q.code,
            nonce.as_deref().unwrap_or(""),
            &pkce,
        )
        .await
    };
    let ident = match ident {
        Ok(x) => x,
        Err(_) => {
            return sign(
                "failure",
                "provider_failed",
                "",
                "",
                &session,
                err(StatusCode::BAD_GATEWAY, "provider_failed"),
            );
        }
    };
    let account = match a.store.seed_account(
        ident.provider.as_str(),
        &ident.subject,
        &ident.display_name,
        now,
    ) {
        Ok(x) => x,
        Err(_) => {
            return sign(
                "failure",
                "server_error",
                "",
                "",
                &session,
                err(StatusCode::INTERNAL_SERVER_ERROR, "server_error"),
            );
        }
    };
    let rotated = secret();
    let updated = a.store.db.lock().ok().and_then(|c| c.execute(
        "UPDATE browser_sessions SET hash=?1,account_id=?2,expires_at=?3 WHERE hash=?4 AND expires_at>?5",
        params![hash(&rotated), account, now + 43200, hash(&session), now],
    ).ok()).unwrap_or(0);
    if updated != 1 {
        return sign(
            "failure",
            "server_error",
            &account,
            "",
            &session,
            err(StatusCode::INTERNAL_SERVER_ERROR, "server_error"),
        );
    }
    let tenant = a
        .store
        .db
        .lock()
        .ok()
        .map(|c| tenant_on(&c, &account))
        .unwrap_or_default();
    let pending = take_server_authorization(&a, &session);
    let mut response = match pending {
        Ok(Some(pending)) => match finish_server_authorization(&a, &account, &pending, now) {
            Ok(response) => response,
            Err(()) => {
                return sign(
                    "failure",
                    "server_error",
                    &account,
                    &tenant,
                    &rotated,
                    err(StatusCode::INTERNAL_SERVER_ERROR, "server_error"),
                );
            }
        },
        Ok(None) => Redirect::to("/device").into_response(),
        Err(()) => {
            return sign(
                "failure",
                "server_error",
                &account,
                &tenant,
                &rotated,
                err(StatusCode::INTERNAL_SERVER_ERROR, "server_error"),
            );
        }
    };
    response.headers_mut().insert(
        header::SET_COOKIE,
        format!(
            "__Host-kiln_session={rotated}; Secure; HttpOnly; SameSite=Lax; Path=/; Max-Age=43200"
        )
        .parse()
        .expect("cookie"),
    );
    sign(
        "success",
        "",
        &account,
        &tenant,
        &rotated,
        secure(nostore(response)),
    )
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RegReq {
    name: String,
    origin: String,
    ca_pem: String,
}
#[derive(Serialize)]
struct RegAuth {
    registration_code: String,
    user_code: String,
    verification_uri: String,
    expires_in: u64,
    interval: u64,
}
fn validate_reg(f: &RegReq) -> anyhow::Result<()> {
    anyhow::ensure!(
        !f.name.is_empty()
            && f.name.len() <= 128
            && f.origin.len() <= 2048
            && f.ca_pem.len() <= 65536,
        "invalid registration"
    );
    let u = url::Url::parse(&f.origin)?;
    anyhow::ensure!(
        u.scheme() == "https"
            && u.username().is_empty()
            && u.password().is_none()
            && u.host_str().is_some()
            && u.path() == "/"
            && u.query().is_none()
            && u.fragment().is_none(),
        "invalid origin"
    );
    let (remaining, pem) = x509_parser::pem::parse_x509_pem(f.ca_pem.as_bytes())
        .map_err(|_| anyhow::anyhow!("invalid certificate"))?;
    anyhow::ensure!(
        remaining.iter().all(u8::is_ascii_whitespace),
        "only one certificate is allowed"
    );
    pem.parse_x509()
        .map_err(|_| anyhow::anyhow!("invalid certificate"))?;
    Ok(())
}
async fn registration(
    State(a): State<Arc<App>>,
    ConnectInfo(peer): ConnectInfo<SocketAddr>,
    h: HeaderMap,
    Json(f): Json<RegReq>,
) -> Response {
    if validate_reg(&f).is_err() {
        return err(StatusCode::BAD_REQUEST, "invalid_registration");
    }
    let raw = secret();
    let user = store::code();
    let now = (a.cfg.now)();
    let mut c = a.store.db.lock().unwrap();
    let tx = match c.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate) {
        Ok(tx) => tx,
        Err(_) => return err(StatusCode::INTERNAL_SERVER_ERROR, "server_error"),
    };
    if store::rate(
        &tx,
        &format!("registration:{}", client_ip(&a.cfg, peer, &h)),
        now,
        10,
        60,
    )
    .is_err()
    {
        return err(StatusCode::TOO_MANY_REQUESTS, "rate_limited");
    }
    let count: i64 = tx.query_row("SELECT (SELECT count(*) FROM pending_devices WHERE expires_at>?1)+(SELECT count(*) FROM pending_registrations WHERE expires_at>?1)", [now], |r| r.get(0)).unwrap_or(10000);
    if count >= 10000 {
        return err(StatusCode::TOO_MANY_REQUESTS, "rate_limited");
    }
    let n=tx.execute("INSERT INTO pending_registrations(secret_hash,user_code,name,origin,ca_pem,state,expires_at,next_poll,interval)VALUES(?1,?2,?3,?4,?5,'pending',?6,?7,5)",params![hash(&raw),user,f.name,f.origin,f.ca_pem,now+600,now+5]);
    if n.is_err() || tx.commit().is_err() {
        return err(StatusCode::INTERNAL_SERVER_ERROR, "server_error");
    }
    nostore(
        Json(RegAuth {
            registration_code: raw,
            user_code: user,
            verification_uri: format!("{}/device", a.cfg.public_origin.trim_end_matches('/')),
            expires_in: 600,
            interval: 5,
        })
        .into_response(),
    )
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct SecretReq {
    registration_code: String,
}
async fn registration_poll(State(a): State<Arc<App>>, Json(f): Json<SecretReq>) -> Response {
    let now = (a.cfg.now)();
    let mut c = a.store.db.lock().unwrap();
    let tx = match c.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate) {
        Ok(tx) => tx,
        Err(_) => return err(StatusCode::INTERNAL_SERVER_ERROR, "server_error"),
    };
    let row=tx.query_row("SELECT name,origin,ca_pem,state,owner_id,expires_at,poll_consumed,next_poll,interval FROM pending_registrations WHERE secret_hash=?1",[hash(&f.registration_code)],|r|Ok((r.get::<_,String>(0)?,r.get::<_,String>(1)?,r.get::<_,String>(2)?,r.get::<_,String>(3)?,r.get::<_,Option<String>>(4)?,r.get::<_,i64>(5)?,r.get::<_,i64>(6)?,r.get::<_,i64>(7)?,r.get::<_,i64>(8)?))).optional().ok().flatten();
    let Some((name, origin, ca, state, owner, exp, consumed, next_poll, interval)) = row else {
        return err(StatusCode::BAD_REQUEST, "invalid_grant");
    };
    if now >= exp {
        return err(StatusCode::BAD_REQUEST, "expired_token");
    }
    if now < next_poll {
        let _ = tx.execute("UPDATE pending_registrations SET interval=interval+5,next_poll=?1 WHERE secret_hash=?2", params![now+interval+5,hash(&f.registration_code)]);
        let _ = tx.commit();
        return err(StatusCode::TOO_MANY_REQUESTS, "slow_down");
    }
    let _ = tx.execute(
        "UPDATE pending_registrations SET next_poll=?1 WHERE secret_hash=?2",
        params![now + interval, hash(&f.registration_code)],
    );
    if state == "pending" {
        let _ = tx.commit();
        return nostore(Json(serde_json::json!({"status":"pending"})).into_response());
    }
    if state == "denied" {
        let _ = tx.commit();
        return nostore(Json(serde_json::json!({"status":"denied"})).into_response());
    }
    if consumed != 0 {
        return err(StatusCode::BAD_REQUEST, "invalid_grant");
    }
    let owner = owner.unwrap();
    let owner_live = tx
        .query_row(
            "SELECT EXISTS(SELECT 1 FROM accounts WHERE id=?1 AND revoked_at IS NULL)",
            [&owner],
            |r| r.get::<_, bool>(0),
        )
        .unwrap_or(false);
    if !owner_live {
        return err(StatusCode::BAD_REQUEST, "invalid_grant");
    }
    let sid = format!("srv_{}", uuid::Uuid::new_v4().simple());
    let dir = secret();
    let tenant = match ensure_owner_tenant(&tx, &owner, now) {
        Ok(tenant) => tenant,
        Err(_) => return err(StatusCode::INTERNAL_SERVER_ERROR, "server_error"),
    };
    let ok = tx
        .execute(
            "INSERT INTO servers(id,owner_id,tenant_id,name,origin,ca_pem,revision,active,directory_hash,created_at)VALUES(?1,?2,?3,?4,?5,?6,1,0,?7,?8)",
            params![sid, owner, tenant, name, origin, ca, hash(&dir), now],
        )
        .and_then(|_| set_default_server(&tx, &tenant, &sid))
        .and_then(|_| {
            tx.execute(
                "UPDATE pending_registrations SET poll_consumed=1 WHERE secret_hash=?1 AND poll_consumed=0",
                [hash(&f.registration_code)],
            )
        });
    if ok.is_err() || tx.commit().is_err() {
        return err(StatusCode::BAD_REQUEST, "invalid_grant");
    }
    nostore(
        Json(Registration {
            server_id: sid,
            owner_id: owner,
            directory_token: dir,
        })
        .into_response(),
    )
}
async fn registration_activate(State(a): State<Arc<App>>, h: HeaderMap) -> Response {
    let Some(raw) = bearer(&h) else {
        return err(StatusCode::UNAUTHORIZED, "invalid_token");
    };
    let n = a
        .store
        .db
        .lock()
        .unwrap()
        .execute(
            "UPDATE servers SET active=1 WHERE directory_hash=?1 AND (active=1 OR created_at>?2) AND EXISTS(SELECT 1 FROM accounts WHERE id=owner_id AND revoked_at IS NULL)",
            params![hash(raw), (a.cfg.now)()-3600],
        )
        .unwrap_or(0);
    if n == 1 {
        StatusCode::NO_CONTENT.into_response()
    } else {
        err(StatusCode::UNAUTHORIZED, "invalid_token")
    }
}
async fn update_server(
    State(a): State<Arc<App>>,
    h: HeaderMap,
    Path(id): Path<String>,
    Json(f): Json<RegReq>,
) -> Response {
    if validate_reg(&f).is_err() {
        return err(StatusCode::BAD_REQUEST, "invalid_registration");
    }
    let Some(raw) = bearer(&h) else {
        return err(StatusCode::UNAUTHORIZED, "invalid_token");
    };
    let n=a.store.db.lock().unwrap().execute("UPDATE servers SET name=?1,origin=?2,ca_pem=?3,revision=revision+1 WHERE id=?4 AND directory_hash=?5 AND active=1",params![f.name,f.origin,f.ca_pem,id,hash(raw)]).unwrap_or(0);
    if n == 1 {
        StatusCode::NO_CONTENT.into_response()
    } else {
        err(StatusCode::UNAUTHORIZED, "invalid_token")
    }
}
async fn delete_server(
    State(a): State<Arc<App>>,
    h: HeaderMap,
    Path(id): Path<String>,
) -> Response {
    let Some(raw) = bearer(&h) else {
        return err(StatusCode::UNAUTHORIZED, "invalid_token");
    };
    let n = a
        .store
        .db
        .lock()
        .unwrap()
        .execute(
            "UPDATE servers SET active=0,directory_hash=?1 WHERE id=?2 AND directory_hash=?3 AND active=1",
            params![hash(&secret()), id, hash(raw)],
        )
        .unwrap_or(0);
    if n == 1 {
        StatusCode::NO_CONTENT.into_response()
    } else {
        err(StatusCode::UNAUTHORIZED, "invalid_token")
    }
}

async fn tenant_list(State(a): State<Arc<App>>, h: HeaderMap) -> Response {
    authorized(&a, &h, |tx, account, _| {
        let mut q = tx.prepare(
            "SELECT t.id,m.role,t.default_server_id FROM tenants t
             JOIN tenant_members m ON m.tenant_id=t.id
             WHERE m.account_id=?1 ORDER BY t.created_at",
        )?;
        let tenants = q
            .query_map([account], |r| {
                Ok(TenantSummary {
                    id: r.get(0)?,
                    role: member_role(&r.get::<_, String>(1)?).unwrap_or(MemberRole::Member),
                    default_server_id: r.get(2)?,
                })
            })?
            .collect::<Result<Vec<_>, _>>()?;
        Ok(Json(tenants).into_response())
    })
}

#[derive(Serialize)]
struct TenantMembersResponse {
    tenant_id: String,
    members: Vec<TenantMember>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct AddTenantMemberReq {
    name: String,
}

async fn list_tenant_members(
    State(a): State<Arc<App>>,
    h: HeaderMap,
    Path(id): Path<String>,
) -> Response {
    if id.is_empty() || id.len() > 128 {
        return err(StatusCode::NOT_FOUND, "not_found");
    }
    let Some(raw) = bearer(&h) else {
        return err(StatusCode::UNAUTHORIZED, "invalid_token");
    };
    let mut c = match a.store.db.lock() {
        Ok(c) => c,
        Err(_) => return err(StatusCode::INTERNAL_SERVER_ERROR, "server_error"),
    };
    let tx = match c.transaction_with_behavior(rusqlite::TransactionBehavior::Deferred) {
        Ok(tx) => tx,
        Err(_) => return err(StatusCode::INTERNAL_SERVER_ERROR, "server_error"),
    };
    let directory = match directory_in_tenant(&tx, &id, raw) {
        Ok(v) => v,
        Err(_) => return err(StatusCode::INTERNAL_SERVER_ERROR, "server_error"),
    };
    if directory {
        let members = match tenant_member_rows(&tx, &id) {
            Ok(v) => v,
            Err(_) => return err(StatusCode::INTERNAL_SERVER_ERROR, "server_error"),
        };
        if tx.commit().is_err() {
            return err(StatusCode::INTERNAL_SERVER_ERROR, "server_error");
        }
        return nostore(
            Json(TenantMembersResponse {
                tenant_id: id,
                members,
            })
            .into_response(),
        );
    }
    drop(tx);
    drop(c);
    authorized(&a, &h, |tx, caller, _| {
        if !tenant_owner(tx, &id, &caller)? {
            return Ok(err(StatusCode::NOT_FOUND, "not_found"));
        }
        Ok(nostore(
            Json(TenantMembersResponse {
                tenant_id: id.clone(),
                members: tenant_member_rows(tx, &id)?,
            })
            .into_response(),
        ))
    })
}

async fn add_tenant_member(
    State(a): State<Arc<App>>,
    h: HeaderMap,
    Path(id): Path<String>,
    Json(f): Json<AddTenantMemberReq>,
) -> Response {
    if id.is_empty() || id.len() > 128 {
        return err(StatusCode::NOT_FOUND, "not_found");
    }
    if !local_member_name(&f.name) {
        return err(StatusCode::BAD_REQUEST, "invalid_name");
    }
    authorized(&a, &h, |tx, caller, _| {
        if !tenant_owner(tx, &id, &caller)? {
            return Ok(err(StatusCode::NOT_FOUND, "not_found"));
        }
        let exists: bool = tx.query_row(
            "SELECT EXISTS(SELECT 1 FROM tenant_members WHERE tenant_id=?1 AND local_name=?2)",
            params![id, f.name],
            |r| r.get(0),
        )?;
        if exists {
            return Ok(err(StatusCode::CONFLICT, "name_in_use"));
        }
        let account = format!("acct_{}", uuid::Uuid::new_v4().simple());
        let now = (a.cfg.now)();
        tx.execute(
            "INSERT INTO accounts(id,created_at)VALUES(?1,?2)",
            params![account, now],
        )?;
        tx.execute(
            "INSERT INTO tenant_members(tenant_id,account_id,role,local_name)VALUES(?1,?2,'member',?3)",
            params![id, account, f.name],
        )?;
        tx.execute(
            "INSERT INTO audit_events(event,actor_id,resource_id,outcome,created_at) VALUES('tenant_member_added',?1,?2,'success',?3)",
            params![caller, account, now],
        )?;
        Ok(Json(serde_json::json!({
            "tenant_id": id,
            "account_id": account,
            "name": f.name,
            "role": "member",
        }))
        .into_response())
    })
}

async fn remove_tenant_member(
    State(a): State<Arc<App>>,
    h: HeaderMap,
    Path((id, name)): Path<(String, String)>,
) -> Response {
    if id.is_empty() || id.len() > 128 || !local_member_name(&name) {
        return err(StatusCode::NOT_FOUND, "not_found");
    }
    authorized(&a, &h, |tx, caller, _| {
        if !tenant_owner(tx, &id, &caller)? {
            return Ok(err(StatusCode::NOT_FOUND, "not_found"));
        }
        let account: Option<String> = tx
            .query_row(
                "SELECT account_id FROM tenant_members WHERE tenant_id=?1 AND local_name=?2 AND role='member'",
                params![id, name],
                |r| r.get(0),
            )
            .optional()?;
        let Some(account) = account else {
            return Ok(err(StatusCode::NOT_FOUND, "not_found"));
        };
        let now = (a.cfg.now)();
        tx.execute(
            "UPDATE automation_keys SET revoked_at=?1 WHERE owner_id=?2 AND server_id IN (SELECT id FROM servers WHERE tenant_id=?3) AND revoked_at IS NULL",
            params![now, account, id],
        )?;
        tx.execute(
            "DELETE FROM automation_keys WHERE owner_id=?1 AND server_id IN (SELECT id FROM servers WHERE tenant_id=?2)",
            params![account, id],
        )?;
        tx.execute(
            "DELETE FROM tenant_members WHERE tenant_id=?1 AND account_id=?2",
            params![id, account],
        )?;
        tx.execute(
            "DELETE FROM accounts WHERE id=?1 AND NOT EXISTS(SELECT 1 FROM provider_identities WHERE account_id=?1) AND NOT EXISTS(SELECT 1 FROM devices WHERE account_id=?1)",
            [&account],
        )?;
        tx.execute(
            "INSERT INTO audit_events(event,actor_id,resource_id,outcome,created_at) VALUES('tenant_member_removed',?1,?2,'success',?3)",
            params![caller, account, now],
        )?;
        Ok(StatusCode::NO_CONTENT.into_response())
    })
}

async fn tenant_servers(
    State(a): State<Arc<App>>,
    h: HeaderMap,
    Path(id): Path<String>,
) -> Response {
    if id.is_empty() || id.len() > 128 {
        return err(StatusCode::NOT_FOUND, "not_found");
    }
    authorized(&a, &h, |tx, account, _| {
        let member: bool = tx.query_row(
            "SELECT EXISTS(SELECT 1 FROM tenant_members WHERE tenant_id=?1 AND account_id=?2)",
            params![id, account],
            |r| r.get(0),
        )?;
        if !member {
            return Ok(err(StatusCode::NOT_FOUND, "not_found"));
        }
        Ok(Json(tenant_servers_rows(tx, &id)?).into_response())
    })
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct NewKey {
    server_id: String,
    scope: Scope,
    expires_in: Option<u64>,
    account: Option<String>,
}
async fn keys(State(a): State<Arc<App>>, h: HeaderMap) -> Response {
    authorized(&a, &h, |tx, owner, _| {
        let mut q=tx.prepare("SELECT id,server_id,scope,expires_at FROM automation_keys WHERE owner_id=?1 AND revoked_at IS NULL")?;
        let v=q.query_map([owner],|r|Ok(serde_json::json!({"id":r.get::<_,String>(0)?,"server_id":r.get::<_,String>(1)?,"scope":r.get::<_,String>(2)?,"expires_at":r.get::<_,i64>(3)?})))?.collect::<Result<Vec<_>,_>>()?;
        Ok(Json(v).into_response())
    })
}
async fn create_key(State(a): State<Arc<App>>, h: HeaderMap, Json(f): Json<NewKey>) -> Response {
    authorized(&a, &h, |tx, owner, _| {
        let tenant_id: Option<String> = tx
            .query_row(
                "SELECT tenant_id FROM servers WHERE id=?1 AND active=1",
                [&f.server_id],
                |r| r.get(0),
            )
            .optional()?;
        let Some(tenant_id) = tenant_id else {
            return Ok(err(StatusCode::NOT_FOUND, "not_found"));
        };
        if !tenant_owner(tx, &tenant_id, &owner)? {
            return Ok(err(StatusCode::NOT_FOUND, "not_found"));
        }
        let key_account = if let Some(name) = f.account.as_deref() {
            if !local_member_name(name) {
                return Ok(err(StatusCode::BAD_REQUEST, "invalid_name"));
            }
            match tx
                .query_row(
                    "SELECT account_id FROM tenant_members WHERE tenant_id=?1 AND local_name=?2 AND role='member'",
                    params![tenant_id, name],
                    |r| r.get::<_, String>(0),
                )
                .optional()?
            {
                Some(account) => account,
                None => return Ok(err(StatusCode::NOT_FOUND, "not_found")),
            }
        } else {
            owner.clone()
        };
        let raw = secret();
        let id = format!("key_{}", uuid::Uuid::new_v4().simple());
        let now = (a.cfg.now)();
        let life = f.expires_in.unwrap_or(90 * 86400);
        if life == 0 || life > 90 * 86400 {
            return Ok(err(StatusCode::BAD_REQUEST, "invalid_expiry"));
        }
        let scope = match f.scope {
            Scope::Read => "read",
            Scope::Operate => "operate",
        };
        let n = tx.execute(
            "INSERT INTO automation_keys(id,owner_id,server_id,hash,scope,expires_at)
             SELECT ?1,?2,s.id,?3,?4,?5 FROM servers s
             JOIN tenant_members m ON m.tenant_id=s.tenant_id AND m.account_id=?2
             WHERE s.id=?6 AND s.active=1",
            params![
                id,
                key_account,
                hash(&raw),
                scope,
                now + life as i64,
                f.server_id
            ],
        )?;
        if n != 1 {
            return Ok(err(StatusCode::NOT_FOUND, "not_found"));
        }
        tx.execute("INSERT INTO audit_events(event,actor_id,resource_id,outcome,created_at) VALUES('key_created',?1,?2,'success',?3)", params![owner,id,now])?;
        Ok(Json(serde_json::json!({"id":id,"key":raw,"server_id":f.server_id,"scope":scope,"expires_at":now+life as i64})).into_response())
    })
}
async fn revoke_key(
    State(a): State<Arc<App>>,
    peer: Peer,
    h: HeaderMap,
    Path(id): Path<String>,
) -> Response {
    let token = bearer(&h).unwrap_or("").to_owned();
    let saw = std::cell::Cell::new(false);
    let response = authorized(&a, &h, |tx, owner, _| {
        saw.set(true);
        let tenant = tenant_on(tx, &owner);
        let n=tx.execute("UPDATE automation_keys SET revoked_at=?1 WHERE id=?2 AND owner_id=?3 AND revoked_at IS NULL",params![(a.cfg.now)(),id,owner])?;
        if n == 1 {
            tx.execute("INSERT INTO audit_events(event,actor_id,resource_id,outcome,created_at) VALUES('key_revoked',?1,?2,'success',?3)", params![owner,id,(a.cfg.now)()])?;
            Ok(auth_note(
                &a,
                peer.0,
                &h,
                audit::REVOCATION,
                "automation_key",
                "success",
                "",
                &owner,
                &tenant,
                &id,
                "",
                &token,
                StatusCode::NO_CONTENT.into_response(),
            ))
        } else {
            Ok(auth_note(
                &a,
                peer.0,
                &h,
                audit::REVOCATION,
                "automation_key",
                "failure",
                "not_found",
                &owner,
                &tenant,
                &id,
                "",
                &token,
                err(StatusCode::NOT_FOUND, "not_found"),
            ))
        }
    });
    if saw.get() {
        response
    } else {
        let reason = if response.status() == StatusCode::UNAUTHORIZED {
            "invalid_token"
        } else {
            "server_error"
        };
        auth_note(
            &a,
            peer.0,
            &h,
            audit::REVOCATION,
            "automation_key",
            "failure",
            reason,
            "",
            "",
            &id,
            "",
            &token,
            response,
        )
    }
}
async fn key_token(State(a): State<Arc<App>>, peer: Peer, h: HeaderMap) -> Response {
    let Some(raw) = bearer(&h) else {
        return err(StatusCode::UNAUTHORIZED, "invalid_token");
    };
    let now = (a.cfg.now)();
    let mut c = a.store.db.lock().unwrap();
    let tx = match c.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate) {
        Ok(tx) => tx,
        Err(_) => return err(StatusCode::INTERNAL_SERVER_ERROR, "server_error"),
    };
    let row=tx.query_row("SELECT k.id,k.owner_id,k.server_id,k.scope,s.name,s.origin,s.ca_pem,s.revision FROM automation_keys k JOIN servers s ON s.id=k.server_id JOIN accounts a ON a.id=k.owner_id WHERE k.hash=?1 AND k.revoked_at IS NULL AND k.expires_at>?2 AND s.active=1 AND a.revoked_at IS NULL",params![hash(raw),now],|r|Ok((r.get::<_,String>(0)?,r.get::<_,String>(1)?,r.get::<_,String>(2)?,r.get::<_,String>(3)?,r.get::<_,String>(4)?,r.get::<_,String>(5)?,r.get::<_,String>(6)?,r.get::<_,i64>(7)?))).optional().ok().flatten();
    let Some((kid, owner, sid, scope, name, origin, ca, rev)) = row else {
        return err(StatusCode::UNAUTHORIZED, "invalid_token");
    };
    let row: Option<(String, String)> = tx
        .query_row(
            "SELECT s.tenant_id,m.role FROM servers s JOIN tenant_members m ON m.tenant_id=s.tenant_id AND m.account_id=?2 WHERE s.id=?1 AND s.active=1",
            params![sid, owner],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .optional()
        .ok()
        .flatten();
    let Some((tenant_id, role_raw)) = row else {
        return err(StatusCode::UNAUTHORIZED, "invalid_token");
    };
    let Some(role) = member_role(&role_raw) else {
        return err(StatusCode::UNAUTHORIZED, "invalid_token");
    };
    let scope = match scope.as_str() {
        "read" => Scope::Read,
        "operate" => Scope::Operate,
        _ => return err(StatusCode::UNAUTHORIZED, "invalid_token"),
    };
    let n = now as u64;
    let jwt = match a.signer.issue(&ServerClaims {
        iss: a.cfg.public_origin.clone(),
        sub: owner.clone(),
        aud: format!("kiln:server:{sid}"),
        tenant_id: Some(tenant_id.clone()),
        iat: n,
        nbf: n,
        exp: n + 300,
        credential_id: kid,
        scope,
        role: Some(role),
    }) {
        Ok(x) => x,
        Err(_) => {
            return auth_note(
                &a,
                peer.0,
                &h,
                audit::TOKEN_ISSUE,
                "automation_key",
                "failure",
                "server_error",
                &owner,
                &tenant_id,
                &sid,
                "",
                raw,
                err(StatusCode::INTERNAL_SERVER_ERROR, "server_error"),
            );
        }
    };
    if tx.commit().is_err() {
        return auth_note(
            &a,
            peer.0,
            &h,
            audit::TOKEN_ISSUE,
            "automation_key",
            "failure",
            "server_error",
            &owner,
            &tenant_id,
            &sid,
            "",
            raw,
            err(StatusCode::INTERNAL_SERVER_ERROR, "server_error"),
        );
    }
    let issued = jwt.clone();
    auth_note(
        &a,
        peer.0,
        &h,
        audit::TOKEN_ISSUE,
        "automation_key",
        "success",
        "",
        &owner,
        &tenant_id,
        &sid,
        "",
        &issued,
        nostore(Json(serde_json::json!({"token":{"access_token":jwt,"token_type":"Bearer","expires_in":300},"server":{"id":sid,"name":name,"origin":origin,"ca_pem":ca,"revision":rev}})).into_response()),
    )
}
#[derive(Deserialize)]
struct ServerQuery {
    redirect_uri: String,
    state: String,
    code_challenge: String,
    code_challenge_method: String,
    server_id: String,
}
struct PendingServerAuth {
    server_id: String,
    redirect_uri: String,
    state: String,
    challenge: String,
}
fn pkce_s256(verifier: &str) -> String {
    base64::engine::general_purpose::URL_SAFE_NO_PAD
        .encode(sha2::Sha256::digest(verifier.as_bytes()))
}
fn pkce_ok(verifier: &str, challenge: &str) -> bool {
    if !(43..=128).contains(&verifier.len())
        || !verifier
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_' | b'.' | b'~'))
    {
        return false;
    }
    let actual = pkce_s256(verifier);
    actual.len() == challenge.len() && bool::from(actual.as_bytes().ct_eq(challenge.as_bytes()))
}
fn challenge_ok(challenge: &str) -> bool {
    challenge.len() == 43
        && challenge
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_'))
}
fn state_ok(state: &str) -> bool {
    (1..=256).contains(&state.len())
        && state
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_' | b'.' | b'~'))
}
fn code_location(redirect: &str, code: &str, state: &str) -> anyhow::Result<String> {
    let mut url = url::Url::parse(redirect)?;
    url.query_pairs_mut()
        .clear()
        .append_pair("code", code)
        .append_pair("state", state);
    Ok(url.into())
}
fn provider_page(redirect: &str) -> String {
    format!(
        "<h1>Light the way in.</h1><p>Continue to {}.</p><a class=button href=/oauth/github>Continue with GitHub</a><a class=\"button secondary\" href=/oauth/google>Continue with Google</a>",
        esc(redirect)
    )
}
fn refusal_page(redirect: &str) -> String {
    format!(
        "<h1>This account cannot continue.</h1><p>Sign-in to {} was refused.</p>",
        esc(redirect)
    )
}
fn oauth_limited(a: &App, peer: SocketAddr, headers: &HeaderMap) -> bool {
    let limited = (|| -> anyhow::Result<()> {
        let mut c = a.store.db.lock().unwrap();
        let tx = c.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
        store::rate(
            &tx,
            &format!("oauth:{}", client_ip(&a.cfg, peer, headers)),
            (a.cfg.now)(),
            10,
            60,
        )?;
        tx.commit()?;
        Ok(())
    })();
    limited.is_err()
}
fn enrolled_server(
    tx: &rusqlite::Transaction<'_>,
    server_id: &str,
    redirect: &str,
) -> anyhow::Result<Option<String>> {
    if server_id.is_empty() || server_id.len() > 128 {
        return Ok(None);
    }
    Ok(tx
        .query_row(
            "SELECT owner_id FROM servers WHERE id=?1 AND redirect_uri=?2 AND active=1 AND EXISTS(SELECT 1 FROM accounts WHERE id=owner_id AND revoked_at IS NULL)",
            params![server_id, redirect],
            |row| row.get(0),
        )
        .optional()?)
}
fn insert_server_code(
    tx: &rusqlite::Transaction<'_>,
    server: &str,
    account: &str,
    redirect: &str,
    challenge: &str,
    now: i64,
) -> anyhow::Result<String> {
    let raw = secret();
    tx.execute(
        "INSERT INTO server_codes(hash,server_id,account_id,redirect_uri,challenge,expires_at) VALUES(?1,?2,?3,?4,?5,?6)",
        params![hash(&raw), server, account, redirect, challenge, now + 60],
    )?;
    Ok(raw)
}
fn issue_app(
    a: &App,
    owner: &str,
    server: &str,
    tenant_id: &str,
    now: i64,
) -> anyhow::Result<String> {
    let credential = format!("app_{server}");
    anyhow::ensure!(credential.len() <= 128, "invalid server id");
    let now = u64::try_from(now)?;
    a.signer.issue(&ServerClaims {
        iss: a.cfg.public_origin.clone(),
        sub: owner.to_owned(),
        aud: format!("kiln:server:{server}"),
        tenant_id: Some(tenant_id.to_owned()),
        iat: now,
        nbf: now,
        exp: now + 300,
        credential_id: credential,
        scope: Scope::Operate,
        role: Some(MemberRole::Owner),
    })
}
async fn oauth_server(
    State(a): State<Arc<App>>,
    ConnectInfo(peer): ConnectInfo<SocketAddr>,
    axum::extract::Query(q): axum::extract::Query<ServerQuery>,
    h: HeaderMap,
) -> Response {
    if q.code_challenge_method != "S256" || !challenge_ok(&q.code_challenge) || !state_ok(&q.state)
    {
        return err(StatusCode::BAD_REQUEST, "invalid_request");
    }
    if oauth_limited(&a, peer, &h) {
        return err(StatusCode::TOO_MANY_REQUESTS, "rate_limited");
    }
    let now = (a.cfg.now)();
    let mut c = a.store.db.lock().unwrap();
    let tx = match c.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate) {
        Ok(tx) => tx,
        Err(_) => return err(StatusCode::INTERNAL_SERVER_ERROR, "server_error"),
    };
    let owner = match enrolled_server(&tx, &q.server_id, &q.redirect_uri) {
        Ok(row) => row,
        Err(_) => return err(StatusCode::INTERNAL_SERVER_ERROR, "server_error"),
    };
    let Some(owner) = owner else {
        return err(StatusCode::BAD_REQUEST, "invalid_redirect");
    };
    let server = q.server_id.as_str();
    let session = cookie(&h, "__Host-kiln_session");
    let account = match session.as_deref() {
        Some(raw) => match tx
            .query_row(
                "SELECT account_id FROM browser_sessions WHERE hash=?1 AND expires_at>?2",
                params![hash(raw), now],
                |row| row.get::<_, Option<String>>(0),
            )
            .optional()
        {
            Ok(row) => row.flatten(),
            Err(_) => return err(StatusCode::INTERNAL_SERVER_ERROR, "server_error"),
        },
        None => None,
    };
    if account.as_deref() == Some(owner.as_str()) {
        let code = match insert_server_code(
            &tx,
            server,
            &owner,
            &q.redirect_uri,
            &q.code_challenge,
            now,
        ) {
            Ok(code) => code,
            Err(_) => return err(StatusCode::INTERNAL_SERVER_ERROR, "server_error"),
        };
        if tx.commit().is_err() {
            return err(StatusCode::INTERNAL_SERVER_ERROR, "server_error");
        }
        let location = match code_location(&q.redirect_uri, &code, &q.state) {
            Ok(location) => location,
            Err(_) => return err(StatusCode::INTERNAL_SERVER_ERROR, "server_error"),
        };
        return secure(nostore(Redirect::to(&location).into_response()));
    }
    if account.is_some() {
        let _ = tx.commit();
        return secure(page(&refusal_page(&q.redirect_uri)).into_response());
    }
    let raw = session
        .filter(|value| {
            value.len() == 43
                && value
                    .bytes()
                    .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
        })
        .unwrap_or_else(secret);
    let csrf = secret();
    let wrote = tx
        .execute(
            "INSERT INTO browser_sessions(hash,csrf_hash,expires_at) VALUES(?1,?2,?3) ON CONFLICT(hash) DO UPDATE SET csrf_hash=?2,expires_at=?3,account_id=CASE WHEN browser_sessions.expires_at>?4 THEN browser_sessions.account_id ELSE NULL END",
            params![hash(&raw), hash(&csrf), now + 43200, now],
        )
        .and_then(|_| {
            tx.execute(
                "INSERT INTO server_authorizations(session_hash,server_id,redirect_uri,state,challenge,expires_at) VALUES(?1,?2,?3,?4,?5,?6) ON CONFLICT(session_hash) DO UPDATE SET server_id=?2,redirect_uri=?3,state=?4,challenge=?5,expires_at=?6",
                params![hash(&raw), server, q.redirect_uri, q.state, q.code_challenge, now + 600],
            )
        });
    if wrote.is_err() || tx.commit().is_err() {
        return err(StatusCode::INTERNAL_SERVER_ERROR, "server_error");
    }
    let mut response = page(&provider_page(&q.redirect_uri)).into_response();
    response.headers_mut().insert(
        header::SET_COOKIE,
        format!("__Host-kiln_session={raw}; Secure; HttpOnly; SameSite=Lax; Path=/; Max-Age=43200")
            .parse()
            .expect("cookie"),
    );
    response.headers_mut().append(
        header::SET_COOKIE,
        format!("__Host-kiln_csrf={csrf}; Secure; HttpOnly; SameSite=Lax; Path=/; Max-Age=43200")
            .parse()
            .expect("cookie"),
    );
    secure(response)
}
fn take_server_authorization(a: &App, session: &str) -> Result<Option<PendingServerAuth>, ()> {
    let mut c = a.store.db.lock().map_err(|_| ())?;
    let tx = c
        .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)
        .map_err(|_| ())?;
    let row = tx
        .query_row(
            "SELECT server_id,redirect_uri,state,challenge FROM server_authorizations WHERE session_hash=?1 AND expires_at>?2",
            params![hash(session), (a.cfg.now)()],
            |r| {
                Ok(PendingServerAuth {
                    server_id: r.get(0)?,
                    redirect_uri: r.get(1)?,
                    state: r.get(2)?,
                    challenge: r.get(3)?,
                })
            },
        )
        .optional()
        .map_err(|_| ())?;
    tx.execute(
        "DELETE FROM server_authorizations WHERE session_hash=?1",
        params![hash(session)],
    )
    .map_err(|_| ())?;
    tx.commit().map_err(|_| ())?;
    Ok(row)
}
fn finish_server_authorization(
    a: &App,
    account: &str,
    pending: &PendingServerAuth,
    now: i64,
) -> Result<Response, ()> {
    let mut c = a.store.db.lock().map_err(|_| ())?;
    let tx = c
        .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)
        .map_err(|_| ())?;
    let owns: bool = tx
        .query_row(
            "SELECT EXISTS(SELECT 1 FROM servers WHERE id=?1 AND owner_id=?2 AND redirect_uri=?3 AND active=1 AND EXISTS(SELECT 1 FROM accounts WHERE id=?2 AND revoked_at IS NULL))",
            params![pending.server_id, account, pending.redirect_uri],
            |row| row.get(0),
        )
        .map_err(|_| ())?;
    if !owns {
        let _ = tx.commit();
        return Ok(page(&refusal_page(&pending.redirect_uri)).into_response());
    }
    let code = insert_server_code(
        &tx,
        &pending.server_id,
        account,
        &pending.redirect_uri,
        &pending.challenge,
        now,
    )
    .map_err(|_| ())?;
    tx.commit().map_err(|_| ())?;
    let location = code_location(&pending.redirect_uri, &code, &pending.state).map_err(|_| ())?;
    Ok(Redirect::to(&location).into_response())
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct CodeExchange {
    code: String,
    code_verifier: String,
    redirect_uri: String,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ReaderExchange {
    reader: String,
}
async fn server_token(
    State(a): State<Arc<App>>,
    peer: Peer,
    h: HeaderMap,
    body: String,
) -> Response {
    let value: serde_json::Value = match serde_json::from_str(&body) {
        Ok(value) => value,
        Err(_) => {
            return auth_note(
                &a,
                peer.0,
                &h,
                audit::OAUTH_CODE_EXCHANGE,
                "authorization_code",
                "failure",
                "invalid_request",
                "",
                "",
                "",
                "",
                "",
                err(StatusCode::BAD_REQUEST, "invalid_request"),
            );
        }
    };
    if value.get("reader").is_some() {
        let exchange: ReaderExchange = match serde_json::from_value(value) {
            Ok(exchange) => exchange,
            Err(_) => {
                return auth_note(
                    &a,
                    peer.0,
                    &h,
                    audit::APP_READER_EXCHANGE,
                    "app_reader",
                    "failure",
                    "invalid_request",
                    "",
                    "",
                    "",
                    "",
                    "",
                    err(StatusCode::BAD_REQUEST, "invalid_request"),
                );
            }
        };
        let Some(presented) = bearer(&h) else {
            return auth_note(
                &a,
                peer.0,
                &h,
                audit::APP_READER_EXCHANGE,
                "app_reader",
                "failure",
                "invalid_token",
                "",
                "",
                "",
                "",
                "",
                err(StatusCode::UNAUTHORIZED, "invalid_token"),
            );
        };
        return reader_token(&a, peer.0, &h, &exchange.reader, presented);
    }
    let exchange: CodeExchange = match serde_json::from_value(value) {
        Ok(exchange) => exchange,
        Err(_) => {
            return auth_note(
                &a,
                peer.0,
                &h,
                audit::OAUTH_CODE_EXCHANGE,
                "authorization_code",
                "failure",
                "invalid_request",
                "",
                "",
                "",
                "",
                "",
                err(StatusCode::BAD_REQUEST, "invalid_request"),
            );
        }
    };
    let Some(presented) = bearer(&h) else {
        return auth_note(
            &a,
            peer.0,
            &h,
            audit::OAUTH_CODE_EXCHANGE,
            "authorization_code",
            "failure",
            "invalid_token",
            "",
            "",
            "",
            "",
            "",
            err(StatusCode::UNAUTHORIZED, "invalid_token"),
        );
    };
    let now = (a.cfg.now)();
    let mut c = a.store.db.lock().unwrap();
    let tx = match c.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate) {
        Ok(tx) => tx,
        Err(_) => return err(StatusCode::INTERNAL_SERVER_ERROR, "server_error"),
    };
    let row = match tx
        .query_row(
            "SELECT server_id,account_id,redirect_uri,challenge,expires_at FROM server_codes WHERE hash=?1",
            params![hash(&exchange.code)],
            |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, String>(3)?,
                    row.get::<_, i64>(4)?,
                ))
            },
        )
        .optional()
    {
        Ok(row) => row,
        Err(_) => {
            return auth_note(
                &a,
                peer.0,
                &h,
                audit::OAUTH_CODE_EXCHANGE,
                "authorization_code",
                "failure",
                "server_error",
                "",
                "",
                "",
                "",
                &exchange.code,
                err(StatusCode::INTERNAL_SERVER_ERROR, "server_error"),
            );
        }
    };
    let Some((server, account, redirect, challenge, expires)) = row else {
        return auth_note(
            &a,
            peer.0,
            &h,
            audit::OAUTH_CODE_EXCHANGE,
            "authorization_code",
            "failure",
            "invalid_grant",
            "",
            "",
            "",
            "",
            &exchange.code,
            err(StatusCode::BAD_REQUEST, "invalid_grant"),
        );
    };
    let secret_ok: bool = match tx.query_row(
        "SELECT EXISTS(SELECT 1 FROM servers s JOIN accounts a ON a.id=s.owner_id WHERE s.id=?1 AND s.owner_id=?2 AND s.login_secret_hash=?3 AND s.active=1 AND a.revoked_at IS NULL)",
        params![server, account, hash(presented)],
        |row| row.get(0),
    ) {
        Ok(value) => value,
        Err(_) => {
            return auth_note(
                &a,
                peer.0,
                &h,
                audit::OAUTH_CODE_EXCHANGE,
                "authorization_code",
                "failure",
                "server_error",
                &account,
                "",
                &server,
                "",
                presented,
                err(StatusCode::INTERNAL_SERVER_ERROR, "server_error"),
            );
        }
    };
    let client_id = server.clone();
    let exchange_note = |outcome: &'static str,
                         reason: &'static str,
                         account: &str,
                         tenant: &str,
                         token: &str,
                         response: Response| {
        auth_note(
            &a,
            peer.0,
            &h,
            audit::OAUTH_CODE_EXCHANGE,
            "authorization_code",
            outcome,
            reason,
            account,
            tenant,
            &client_id,
            "",
            token,
            response,
        )
    };
    if !secret_ok {
        return exchange_note(
            "failure",
            "invalid_token",
            &account,
            "",
            presented,
            err(StatusCode::UNAUTHORIZED, "invalid_token"),
        );
    }
    let consume = tx.execute(
        "DELETE FROM server_codes WHERE hash=?1",
        params![hash(&exchange.code)],
    );
    if consume.is_err() {
        return exchange_note(
            "failure",
            "server_error",
            &account,
            "",
            presented,
            err(StatusCode::INTERNAL_SERVER_ERROR, "server_error"),
        );
    }
    if now >= expires {
        let _ = tx.commit();
        return exchange_note(
            "failure",
            "expired_token",
            &account,
            "",
            &exchange.code,
            err(StatusCode::BAD_REQUEST, "expired_token"),
        );
    }
    if exchange.redirect_uri != redirect || !pkce_ok(&exchange.code_verifier, &challenge) {
        let _ = tx.commit();
        return exchange_note(
            "failure",
            "invalid_grant",
            &account,
            "",
            &exchange.code,
            err(StatusCode::BAD_REQUEST, "invalid_grant"),
        );
    }
    let tenant_id: String = match tx.query_row(
        "SELECT tenant_id FROM servers WHERE id=?1 AND active=1",
        [&server],
        |r| r.get(0),
    ) {
        Ok(v) => v,
        Err(_) => {
            return exchange_note(
                "failure",
                "server_error",
                &account,
                "",
                presented,
                err(StatusCode::INTERNAL_SERVER_ERROR, "server_error"),
            );
        }
    };
    let reader = secret();
    let jwt = match issue_app(&a, &account, &server, &tenant_id, now) {
        Ok(jwt) => jwt,
        Err(_) => {
            return exchange_note(
                "failure",
                "server_error",
                &account,
                &tenant_id,
                presented,
                err(StatusCode::INTERNAL_SERVER_ERROR, "server_error"),
            );
        }
    };
    if tx
        .execute(
            "INSERT INTO app_readers(hash,server_id,account_id,expires_at) VALUES(?1,?2,?3,?4)",
            params![hash(&reader), server, account, now + 43200],
        )
        .is_err()
        || tx.commit().is_err()
    {
        return exchange_note(
            "failure",
            "server_error",
            &account,
            &tenant_id,
            presented,
            err(StatusCode::INTERNAL_SERVER_ERROR, "server_error"),
        );
    }
    let issued = jwt.clone();
    exchange_note(
        "success",
        "",
        &account,
        &tenant_id,
        &issued,
        nostore(
            Json(serde_json::json!({
                "access_token": jwt,
                "token_type": "Bearer",
                "expires_in": 300,
                "reader": reader,
            }))
            .into_response(),
        ),
    )
}
fn reader_token(
    a: &App,
    peer: SocketAddr,
    headers: &HeaderMap,
    reader: &str,
    presented: &str,
) -> Response {
    let now = (a.cfg.now)();
    let mut c = a.store.db.lock().unwrap();
    let tx = match c.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate) {
        Ok(tx) => tx,
        Err(_) => {
            return auth_note(
                a,
                peer,
                headers,
                audit::APP_READER_EXCHANGE,
                "app_reader",
                "failure",
                "server_error",
                "",
                "",
                "",
                "",
                presented,
                err(StatusCode::INTERNAL_SERVER_ERROR, "server_error"),
            );
        }
    };
    let row = match tx.query_row(
        "SELECT r.server_id,r.account_id FROM app_readers r JOIN servers s ON s.id=r.server_id JOIN accounts a ON a.id=r.account_id WHERE r.hash=?1 AND r.expires_at>?2 AND s.login_secret_hash=?3 AND s.active=1 AND a.revoked_at IS NULL AND s.owner_id=r.account_id",
        params![hash(reader), now, hash(presented)],
        |row| Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?)),
    ).optional() {
        Ok(row) => row,
        Err(_) => {
            return auth_note(
                a,
                peer,
                headers,
                audit::APP_READER_EXCHANGE,
                "app_reader",
                "failure",
                "server_error",
                "",
                "",
                "",
                "",
                presented,
                err(StatusCode::INTERNAL_SERVER_ERROR, "server_error"),
            );
        }
    };
    let Some((server, account)) = row else {
        return auth_note(
            a,
            peer,
            headers,
            audit::APP_READER_EXCHANGE,
            "app_reader",
            "failure",
            "invalid_token",
            "",
            "",
            "",
            "",
            presented,
            err(StatusCode::UNAUTHORIZED, "invalid_token"),
        );
    };
    let tenant_id: String = match tx.query_row(
        "SELECT tenant_id FROM servers WHERE id=?1 AND active=1",
        [&server],
        |r| r.get(0),
    ) {
        Ok(v) => v,
        Err(_) => {
            return auth_note(
                a,
                peer,
                headers,
                audit::APP_READER_EXCHANGE,
                "app_reader",
                "failure",
                "server_error",
                &account,
                "",
                &server,
                "",
                presented,
                err(StatusCode::INTERNAL_SERVER_ERROR, "server_error"),
            );
        }
    };
    let jwt = match issue_app(a, &account, &server, &tenant_id, now) {
        Ok(jwt) => jwt,
        Err(_) => {
            return auth_note(
                a,
                peer,
                headers,
                audit::APP_READER_EXCHANGE,
                "app_reader",
                "failure",
                "server_error",
                &account,
                &tenant_id,
                &server,
                "",
                presented,
                err(StatusCode::INTERNAL_SERVER_ERROR, "server_error"),
            );
        }
    };
    if tx.commit().is_err() {
        return auth_note(
            a,
            peer,
            headers,
            audit::APP_READER_EXCHANGE,
            "app_reader",
            "failure",
            "server_error",
            &account,
            &tenant_id,
            &server,
            "",
            presented,
            err(StatusCode::INTERNAL_SERVER_ERROR, "server_error"),
        );
    }
    let issued = jwt.clone();
    auth_note(
        a,
        peer,
        headers,
        audit::APP_READER_EXCHANGE,
        "app_reader",
        "success",
        "",
        &account,
        &tenant_id,
        &server,
        "",
        &issued,
        nostore(
            Json(tokens::Grant {
                access_token: jwt,
                token_type: "Bearer",
                expires_in: 300,
            })
            .into_response(),
        ),
    )
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct StatusReq {
    server_id: String,
}
#[derive(Serialize)]
struct StatusBody {
    account_active: bool,
    server_active: bool,
}
async fn revoke_reader(
    State(a): State<Arc<App>>,
    peer: Peer,
    h: HeaderMap,
    Json(body): Json<ReaderExchange>,
) -> Response {
    let Some(presented) = bearer(&h) else {
        return auth_note(
            &a,
            peer.0,
            &h,
            audit::REVOCATION,
            "app_reader",
            "failure",
            "invalid_token",
            "",
            "",
            "",
            "",
            "",
            err(StatusCode::UNAUTHORIZED, "invalid_token"),
        );
    };
    let deleted = a.store.db.lock().unwrap().execute(
        "DELETE FROM app_readers WHERE hash=?1 AND EXISTS(SELECT 1 FROM servers s WHERE s.id=app_readers.server_id AND s.login_secret_hash=?2)",
        params![hash(&body.reader), hash(presented)],
    );
    let (outcome, reason, response) = match deleted {
        Ok(1) => (
            "success",
            "",
            nostore(StatusCode::NO_CONTENT.into_response()),
        ),
        Ok(_) => (
            "failure",
            "invalid_token",
            err(StatusCode::UNAUTHORIZED, "invalid_token"),
        ),
        Err(_) => (
            "failure",
            "server_error",
            err(StatusCode::INTERNAL_SERVER_ERROR, "server_error"),
        ),
    };
    auth_note(
        &a,
        peer.0,
        &h,
        audit::REVOCATION,
        "app_reader",
        outcome,
        reason,
        "",
        "",
        "",
        "",
        presented,
        response,
    )
}
async fn server_status(
    State(a): State<Arc<App>>,
    h: HeaderMap,
    Json(f): Json<StatusReq>,
) -> Response {
    let Some(secret) = bearer(&h) else {
        return err(StatusCode::UNAUTHORIZED, "invalid_token");
    };
    let row = a.store.db.lock().unwrap().query_row(
        "SELECT a.revoked_at IS NULL, s.active=1 FROM servers s JOIN accounts a ON a.id=s.owner_id WHERE s.id=?1 AND s.login_secret_hash=?2",
        params![f.server_id, hash(secret)],
        |row| Ok((row.get::<_, bool>(0)?, row.get::<_, bool>(1)?)),
    );
    match row {
        Ok((account_active, server_active)) => nostore(
            Json(StatusBody {
                account_active,
                server_active,
            })
            .into_response(),
        ),
        Err(rusqlite::Error::QueryReturnedNoRows) => err(StatusCode::UNAUTHORIZED, "invalid_token"),
        Err(_) => err(StatusCode::INTERNAL_SERVER_ERROR, "server_error"),
    }
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct AppLoginReq {
    redirect_uri: String,
}
async fn app_login(
    State(a): State<Arc<App>>,
    peer: Peer,
    h: HeaderMap,
    Path(id): Path<String>,
    Json(f): Json<AppLoginReq>,
) -> Response {
    if f.redirect_uri != kiln_api::auth::APP_OAUTH_REDIRECT {
        return auth_note(
            &a,
            peer.0,
            &h,
            audit::TOKEN_ISSUE,
            "app_login",
            "failure",
            "invalid_redirect",
            "",
            "",
            &id,
            "",
            "",
            err(StatusCode::BAD_REQUEST, "invalid_redirect"),
        );
    }
    let Some(raw) = bearer(&h) else {
        return auth_note(
            &a,
            peer.0,
            &h,
            audit::TOKEN_ISSUE,
            "app_login",
            "failure",
            "invalid_token",
            "",
            "",
            &id,
            "",
            "",
            err(StatusCode::UNAUTHORIZED, "invalid_token"),
        );
    };
    let token = secret();
    let n = a.store.db.lock().unwrap().execute(
        "UPDATE servers SET redirect_uri=?1, login_secret_hash=?2 WHERE id=?3 AND directory_hash=?4 AND login_secret_hash IS NULL",
        params![f.redirect_uri, hash(&token), id, hash(raw)],
    ).unwrap_or(0);
    if n == 1 {
        let issued = token.clone();
        return auth_note(
            &a,
            peer.0,
            &h,
            audit::TOKEN_ISSUE,
            "app_login",
            "success",
            "",
            "",
            "",
            &id,
            "",
            &issued,
            nostore(Json(serde_json::json!({"login_token": token})).into_response()),
        );
    }
    auth_note(
        &a,
        peer.0,
        &h,
        audit::TOKEN_ISSUE,
        "app_login",
        "failure",
        "invalid_token",
        "",
        "",
        &id,
        "",
        raw,
        err(StatusCode::UNAUTHORIZED, "invalid_token"),
    )
}
struct Peer(SocketAddr);

impl<S> FromRequestParts<S> for Peer
where
    S: Send + Sync,
{
    type Rejection = std::convert::Infallible;

    async fn from_request_parts(parts: &mut Parts, _state: &S) -> Result<Self, Self::Rejection> {
        let addr = parts
            .extensions
            .get::<ConnectInfo<SocketAddr>>()
            .map(|info| info.0)
            .unwrap_or_else(|| SocketAddr::from(([0, 0, 0, 0], 0)));
        Ok(Peer(addr))
    }
}

fn tenant_on(c: &rusqlite::Connection, account: &str) -> String {
    if account.is_empty() {
        return String::new();
    }
    c.query_row(
        "SELECT tenant_id FROM tenant_members WHERE account_id=?1 ORDER BY tenant_id LIMIT 1",
        [account],
        |row| row.get(0),
    )
    .optional()
    .ok()
    .flatten()
    .unwrap_or_default()
}

fn user_agent(headers: &HeaderMap) -> String {
    headers
        .get(header::USER_AGENT)
        .and_then(|value| value.to_str().ok())
        .unwrap_or("")
        .chars()
        .take(200)
        .collect()
}

fn note(
    a: &App,
    peer: SocketAddr,
    headers: &HeaderMap,
    mut input: audit::Input,
    response: Response,
) -> Response {
    input.client_ip = client_ip(&a.cfg, peer, headers).to_string();
    input.user_agent = user_agent(headers);
    if input.session_raw.is_empty() {
        input.session_raw = cookie(headers, "__Host-kiln_session").unwrap_or_default();
    }
    a.store.audit().record(input);
    response
}

fn blank(
    event: &'static str,
    outcome: &'static str,
    reason: &'static str,
    auth_method: &'static str,
) -> audit::Input {
    audit::Input {
        event,
        outcome,
        reason,
        auth_method,
        account_id: String::new(),
        tenant_id: String::new(),
        client_ip: String::new(),
        user_agent: String::new(),
        client_id: String::new(),
        session_raw: String::new(),
        token_raw: String::new(),
    }
}

#[allow(clippy::too_many_arguments)]
fn auth_note(
    a: &App,
    peer: SocketAddr,
    headers: &HeaderMap,
    event: &'static str,
    auth_method: &'static str,
    outcome: &'static str,
    reason: &'static str,
    account: &str,
    tenant: &str,
    client_id: &str,
    session: &str,
    token: &str,
    response: Response,
) -> Response {
    let mut input = blank(event, outcome, reason, auth_method);
    input.account_id = account.to_owned();
    input.tenant_id = tenant.to_owned();
    input.client_id = client_id.to_owned();
    input.session_raw = session.to_owned();
    input.token_raw = token.to_owned();
    note(a, peer, headers, input, response)
}

fn err(s: StatusCode, code: &str) -> Response {
    nostore((s, Json(serde_json::json!({"error":code}))).into_response())
}
fn nostore(mut r: Response) -> Response {
    r.headers_mut()
        .insert(header::CACHE_CONTROL, "no-store".parse().unwrap());
    r
}
fn secure(mut r: Response) -> Response {
    for (k, v) in [
        (
            "content-security-policy",
            "default-src 'self'; frame-ancestors 'none'; base-uri 'none'; form-action 'self'",
        ),
        // no-referrer makes native HTML form POSTs send Origin: null. Keep the
        // origin for CSRF checks without exposing any approval/OAuth query string.
        ("referrer-policy", "strict-origin"),
        ("x-content-type-options", "nosniff"),
    ] {
        r.headers_mut().insert(k, v.parse().unwrap());
    }
    r
}
fn cookie(h: &HeaderMap, n: &str) -> Option<String> {
    h.get(header::COOKIE)?
        .to_str()
        .ok()?
        .split(';')
        .find_map(|x| x.trim().strip_prefix(&format!("{n}=")).map(str::to_owned))
}
fn esc(x: &str) -> String {
    x.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}
fn enc(x: &str) -> String {
    url::form_urlencoded::byte_serialize(x.as_bytes()).collect()
}
fn client_ip(cfg: &Config, peer: SocketAddr, headers: &HeaderMap) -> IpAddr {
    let direct = peer.ip();
    if cfg.trusted_proxy_loopback
        && direct.is_loopback()
        && let Some(ip) = headers
            .get("x-forwarded-for")
            .and_then(|v| v.to_str().ok())
            .and_then(|v| v.split(',').next())
            .and_then(|v| v.trim().parse().ok())
    {
        return ip;
    }
    direct
}
fn check_private_file(path: &std::path::Path) -> anyhow::Result<()> {
    let metadata = std::fs::symlink_metadata(path)?;
    anyhow::ensure!(
        metadata.file_type().is_file() && !metadata.file_type().is_symlink(),
        "credential path must be a regular file"
    );
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        anyhow::ensure!(
            metadata.mode() & 0o077 == 0 && metadata.uid() == unsafe { libc::geteuid() },
            "credential file must be owned by this user and owner-only"
        );
    }
    Ok(())
}
