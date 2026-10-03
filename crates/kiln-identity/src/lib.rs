mod config;
mod providers;
mod store;
pub mod tokens;
use axum::{
    Json, Router,
    extract::{ConnectInfo, Form, Path, State},
    http::{HeaderMap, StatusCode, header},
    response::{Html, IntoResponse, Redirect, Response},
    routing::{delete, get, post, put},
};
pub use config::{Config, unix_now};
use kiln_api::auth::{DeviceTokens, Registration, Scope, ServerClaims};
pub use providers::{Provider, ProviderIdentity};
use rusqlite::{OptionalExtension, params};
use serde::{Deserialize, Serialize};
use sha2::Digest;
use std::{
    net::{IpAddr, SocketAddr},
    sync::Arc,
};
pub use store::Store;
use store::{hash, secret};

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
    let app = Arc::new(App {
        cfg,
        store,
        signer,
        http: providers::http_client()?,
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
        .route("/device", get(device_page))
        .route("/device/approve", post(approve))
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
        .route("/v1/access", post(access))
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
        r#"<!doctype html><html><head><meta charset=utf-8><meta name=viewport content="width=device-width"><title>Kiln identity</title><link rel=stylesheet href=/static/auth.css></head><body><main class=card><div class=mark>Kiln identity</div>{body}</main></body></html>"#
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
async fn logout(State(a): State<Arc<App>>, h: HeaderMap, Form(form): Form<LogoutForm>) -> Response {
    if h.get(header::ORIGIN).and_then(|v| v.to_str().ok()) != Some(a.cfg.public_origin.as_str()) {
        return err(StatusCode::FORBIDDEN, "invalid_origin");
    }
    let Some(raw) = cookie(&h, "__Host-kiln_session") else {
        return err(StatusCode::UNAUTHORIZED, "login_required");
    };
    let valid = a.store.db.lock().ok().and_then(|c| c.query_row(
        "SELECT EXISTS(SELECT 1 FROM browser_sessions WHERE hash=?1 AND csrf_hash=?2 AND expires_at>?3)",
        params![hash(&raw), hash(&form.csrf), (a.cfg.now)()], |r| r.get::<_, bool>(0)).ok()).unwrap_or(false);
    if !valid {
        return err(StatusCode::FORBIDDEN, "invalid_csrf");
    }
    let removed = a.store.db.lock().ok().and_then(|c| {
        c.execute("DELETE FROM browser_sessions WHERE hash=?1", [hash(&raw)])
            .ok()
    });
    if removed != Some(1) {
        return err(StatusCode::INTERNAL_SERVER_ERROR, "server_error");
    }
    let mut response = Redirect::to("/login").into_response();
    response.headers_mut().insert(
        header::SET_COOKIE,
        "__Host-kiln_session=; Secure; HttpOnly; SameSite=Lax; Path=/; Max-Age=0"
            .parse()
            .expect("static cookie"),
    );
    secure(nostore(response))
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
async fn oauth_token(State(a): State<Arc<App>>, Form(f): Form<TokenForm>) -> Response {
    let now = (a.cfg.now)();
    let mut c = a.store.db.lock().unwrap();
    let tx = match c.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate) {
        Ok(x) => x,
        Err(_) => return err(StatusCode::INTERNAL_SERVER_ERROR, "server_error"),
    };
    if f.grant_type == "urn:ietf:params:oauth:grant-type:device_code" {
        let raw = match f.device_code {
            Some(x) => x,
            None => return err(StatusCode::BAD_REQUEST, "invalid_request"),
        };
        let row=tx.query_row("SELECT name,state,expires_at,next_poll,interval,account_id FROM pending_devices WHERE secret_hash=?1",[hash(&raw)],|r|Ok((r.get::<_,String>(0)?,r.get::<_,String>(1)?,r.get::<_,i64>(2)?,r.get::<_,i64>(3)?,r.get::<_,i64>(4)?,r.get::<_,Option<String>>(5)?))).optional().ok().flatten();
        let Some((name, state, exp, next, interval, account)) = row else {
            return err(StatusCode::BAD_REQUEST, "invalid_grant");
        };
        if now >= exp {
            return err(StatusCode::BAD_REQUEST, "expired_token");
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
            return err(StatusCode::BAD_REQUEST, "access_denied");
        }
        let Some(account) = account.filter(|_| state == "approved") else {
            return err(StatusCode::BAD_REQUEST, "invalid_grant");
        };
        let live: bool = tx
            .query_row(
                "SELECT EXISTS(SELECT 1 FROM accounts WHERE id=?1 AND revoked_at IS NULL)",
                [&account],
                |r| r.get(0),
            )
            .unwrap_or(false);
        if !live {
            return err(StatusCode::BAD_REQUEST, "invalid_grant");
        }
        let did = format!("dev_{}", uuid::Uuid::new_v4().simple());
        let family = secret();
        let refresh = secret();
        let central = secret();
        let ok=tx.execute("DELETE FROM pending_devices WHERE secret_hash=?1",[hash(&raw)]).and_then(|_|tx.execute("INSERT INTO devices(id,account_id,name,created_at,last_used)VALUES(?1,?2,?3,?4,?4)",params![did,account,name,now])).and_then(|_|tx.execute("INSERT INTO refresh_tokens(hash,family_id,device_id,created_at,last_used,absolute_expiry)VALUES(?1,?2,?3,?4,?4,?5)",params![hash(&refresh),family,did,now,now+90*86400])).and_then(|_|tx.execute("INSERT INTO central_tokens(hash,device_id,family_id,expires_at)VALUES(?1,?2,?3,?4)",params![hash(&central),did,family,now+300]));
        if ok.is_err() || tx.commit().is_err() {
            return err(StatusCode::INTERNAL_SERVER_ERROR, "server_error");
        }
        return nostore(
            Json(DeviceTokens {
                access_token: central,
                token_type: "Bearer".into(),
                expires_in: 300,
                refresh_token: refresh,
            })
            .into_response(),
        );
    }
    if f.grant_type == "refresh_token" {
        let raw = match f.refresh_token {
            Some(x) => x,
            None => return err(StatusCode::BAD_REQUEST, "invalid_request"),
        };
        let row=tx.query_row("SELECT rowid,family_id,device_id,created_at,last_used,absolute_expiry,spent_at,family_revoked_at FROM refresh_tokens WHERE hash=?1",[hash(&raw)],|r|Ok((r.get::<_,i64>(0)?,r.get::<_,String>(1)?,r.get::<_,String>(2)?,r.get::<_,i64>(3)?,r.get::<_,i64>(4)?,r.get::<_,i64>(5)?,r.get::<_,Option<i64>>(6)?,r.get::<_,Option<i64>>(7)?))).optional().ok().flatten();
        let Some((rowid, family, did, _, last, absolute, spent, revoked)) = row else {
            return err(StatusCode::BAD_REQUEST, "invalid_grant");
        };
        if spent.is_some() {
            if tx
                .execute(
                    "UPDATE refresh_tokens SET family_revoked_at=?1 WHERE family_id=?2",
                    params![now, family],
                )
                .is_err()
                || tx.commit().is_err()
            {
                return err(StatusCode::INTERNAL_SERVER_ERROR, "server_error");
            }
            return err(StatusCode::BAD_REQUEST, "invalid_grant");
        }
        let live:bool=tx.query_row("SELECT EXISTS(SELECT 1 FROM devices d JOIN accounts a ON a.id=d.account_id WHERE d.id=?1 AND d.revoked_at IS NULL AND a.revoked_at IS NULL)",[&did],|r|r.get(0)).unwrap_or(false);
        if !live || revoked.is_some() || now - last >= 30 * 86400 || now >= absolute {
            return err(StatusCode::BAD_REQUEST, "invalid_grant");
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
            return err(StatusCode::BAD_REQUEST, "invalid_grant");
        }
        return nostore(
            Json(DeviceTokens {
                access_token: central,
                token_type: "Bearer".into(),
                expires_in: 300,
                refresh_token: refresh,
            })
            .into_response(),
        );
    }
    err(StatusCode::BAD_REQUEST, "unsupported_grant_type")
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
    authorized(&a, &h, |tx, owner, _| {
        let mut q = tx.prepare(
            "SELECT id,name,origin,ca_pem,revision FROM servers WHERE owner_id=?1 AND active=1",
        )?;
        let values = q
            .query_map([owner], |r| {
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
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct AccessReq {
    server_id: String,
}
async fn access(State(a): State<Arc<App>>, h: HeaderMap, Json(f): Json<AccessReq>) -> Response {
    authorized(&a, &h, |tx, owner, did| {
        let owns: bool = tx.query_row(
            "SELECT EXISTS(SELECT 1 FROM servers WHERE id=?1 AND owner_id=?2 AND active=1)",
            params![f.server_id, owner],
            |r| r.get(0),
        )?;
        if !owns {
            return Ok(err(StatusCode::NOT_FOUND, "not_found"));
        }
        let now = (a.cfg.now)() as u64;
        let claims = ServerClaims {
            iss: a.cfg.public_origin.clone(),
            sub: owner,
            aud: format!("kiln:server:{}", f.server_id),
            iat: now,
            nbf: now,
            exp: now + 300,
            credential_id: did,
            scope: Scope::Operate,
        };
        let token = a.signer.issue(&claims)?;
        Ok(Json(tokens::Grant {
            access_token: token,
            token_type: "Bearer",
            expires_in: 300,
        })
        .into_response())
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
    h: HeaderMap,
    Path(id): Path<String>,
) -> Response {
    authorized(&a, &h, |tx, owner, _| {
        let n = tx.execute(
            "UPDATE devices SET revoked_at=?1 WHERE id=?2 AND account_id=?3 AND revoked_at IS NULL",
            params![(a.cfg.now)(), id, owner],
        )?;
        if n == 1 {
            tx.execute("INSERT INTO audit_events(event,actor_id,resource_id,outcome,created_at) VALUES('device_revoked',?1,?2,'success',?3)", params![owner,id,(a.cfg.now)()])?;
            Ok(StatusCode::NO_CONTENT.into_response())
        } else {
            Ok(err(StatusCode::NOT_FOUND, "not_found"))
        }
    })
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
    let mut code = q.user_code.unwrap_or_default().trim().to_uppercase();
    if code.len() > 16 {
        return err(StatusCode::BAD_REQUEST, "invalid_code");
    }
    let session = cookie(&h, "__Host-kiln_session");
    if code.is_empty()
        && let Some(raw) = &session
    {
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
    if code.is_empty() {
        return secure(page("<h1>Enter your code.</h1><form method=get action=/device><label for=user_code>Code from your terminal</label><input id=user_code name=user_code maxlength=16 required autocomplete=one-time-code><button>Continue</button></form>").into_response());
    }
    let Some(details) = request_details(&a.store, &code, (a.cfg.now)()) else {
        return page("<h1>Request unavailable.</h1><p>This code is invalid, expired, or already used. Start a new request from your terminal.</p><a href=/device>Enter another code</a>").into_response();
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
    if h.get(header::ORIGIN).and_then(|v| v.to_str().ok())
        != Some(a.cfg.public_origin.trim_end_matches('/'))
    {
        return err(StatusCode::FORBIDDEN, "invalid_origin");
    }
    let Some(raw) = cookie(&h, "__Host-kiln_session") else {
        return err(StatusCode::UNAUTHORIZED, "login_required");
    };
    let now = (a.cfg.now)();
    let Some(csrf_cookie) = cookie(&h, "__Host-kiln_csrf") else {
        return err(StatusCode::FORBIDDEN, "invalid_csrf");
    };
    if f.csrf != csrf_cookie {
        return err(StatusCode::FORBIDDEN, "invalid_csrf");
    }
    if !matches!(f.decision.as_str(), "approve" | "deny") || f.user_code.len() != 12 {
        return err(StatusCode::BAD_REQUEST, "invalid_request");
    }
    let mut c = a.store.db.lock().unwrap();
    let tx = match c.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate) {
        Ok(tx) => tx,
        Err(_) => return err(StatusCode::INTERNAL_SERVER_ERROR, "server_error"),
    };
    if store::rate(
        &tx,
        &format!("approval:{}", client_ip(&a.cfg, peer, &h)),
        now,
        5,
        60,
    )
    .is_err()
        || store::rate(&tx, &format!("code:{}", f.user_code), now, 20, 600).is_err()
    {
        return err(StatusCode::TOO_MANY_REQUESTS, "rate_limited");
    }
    let account = tx.query_row(
            "SELECT s.account_id FROM browser_sessions s JOIN accounts a ON a.id=s.account_id WHERE s.hash=?1 AND s.csrf_hash=?2 AND s.expires_at>?3 AND a.revoked_at IS NULL",
            params![hash(&raw), hash(&f.csrf), now],
            |r| r.get::<_, Option<String>>(0),
        )
        .optional()
        .ok()
        .flatten()
        .flatten();
    let Some(account) = account else {
        return err(StatusCode::UNAUTHORIZED, "login_required");
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
    match result {
        Ok(1) => {
            if tx.execute("INSERT INTO audit_events(event,actor_id,resource_id,outcome,created_at) VALUES('request_decision',?1,?2,?3,?4)", params![account,f.user_code,state,now]).is_err() || tx.commit().is_err() {
                return err(StatusCode::INTERNAL_SERVER_ERROR, "server_error");
            }
            page("<h1>Decision saved.</h1><p>You can return to your terminal.</p>").into_response()
        }
        Ok(_) => {
            let _ = tx.commit();
            err(StatusCode::BAD_REQUEST, "invalid_code")
        }
        Err(_) => err(StatusCode::INTERNAL_SERVER_ERROR, "server_error"),
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
        Provider::Github => providers::github_begin(&a.cfg.github_client_id, &redirect),
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
        tx.execute("INSERT INTO browser_sessions(hash,csrf_hash,requested_code,expires_at)VALUES(?1,?2,?3,?4) ON CONFLICT(hash) DO UPDATE SET csrf_hash=?2,requested_code=COALESCE(?3,requested_code),expires_at=?4",params![hash(&session),hash(&csrf),q.user_code.map(|x|x.trim().to_uppercase()),now+43200]).map_err(|_| ())?;
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
    Path(p): Path<String>,
    h: HeaderMap,
    axum::extract::Query(q): axum::extract::Query<Callback>,
) -> Response {
    let Some(session) = cookie(&h, "__Host-kiln_session") else {
        return err(StatusCode::BAD_REQUEST, "invalid_state");
    };
    let now = (a.cfg.now)();
    let row = {
        let mut c = a.store.db.lock().unwrap();
        let tx = match c.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate) {
            Ok(tx) => tx,
            Err(_) => return err(StatusCode::INTERNAL_SERVER_ERROR, "server_error"),
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
            return err(StatusCode::INTERNAL_SERVER_ERROR, "server_error");
        }
        row
    };
    let Some((provider, pkce, nonce)) = row else {
        return err(StatusCode::BAD_REQUEST, "invalid_state");
    };
    if provider != p {
        return err(StatusCode::BAD_REQUEST, "provider_mismatch");
    }
    let redirect = format!(
        "{}/oauth/{}/callback",
        a.cfg.public_origin.trim_end_matches('/'),
        p
    );
    let ident = if p == "github" {
        let s = match std::fs::read_to_string(&a.cfg.github_secret_file) {
            Ok(x) => x,
            Err(_) => return err(StatusCode::SERVICE_UNAVAILABLE, "provider_unavailable"),
        };
        providers::github_finish(
            &a.http,
            &a.cfg.github_client_id,
            s.trim(),
            &q.code,
            &redirect,
            &pkce,
        )
        .await
    } else {
        let s = match std::fs::read_to_string(&a.cfg.google_secret_file) {
            Ok(x) => x,
            Err(_) => return err(StatusCode::SERVICE_UNAVAILABLE, "provider_unavailable"),
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
        Err(_) => return err(StatusCode::BAD_GATEWAY, "provider_failed"),
    };
    let account = match a.store.seed_account(
        ident.provider.as_str(),
        &ident.subject,
        &ident.display_name,
        now,
    ) {
        Ok(x) => x,
        Err(_) => return err(StatusCode::INTERNAL_SERVER_ERROR, "server_error"),
    };
    let rotated = secret();
    let updated = a.store.db.lock().ok().and_then(|c| c.execute(
        "UPDATE browser_sessions SET hash=?1,account_id=?2,expires_at=?3 WHERE hash=?4 AND expires_at>?5",
        params![hash(&rotated), account, now + 43200, hash(&session), now],
    ).ok()).unwrap_or(0);
    if updated != 1 {
        return err(StatusCode::INTERNAL_SERVER_ERROR, "server_error");
    }
    let mut response = Redirect::to("/device").into_response();
    response.headers_mut().insert(
        header::SET_COOKIE,
        format!(
            "__Host-kiln_session={rotated}; Secure; HttpOnly; SameSite=Lax; Path=/; Max-Age=43200"
        )
        .parse()
        .expect("cookie"),
    );
    secure(nostore(response))
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
    let ok=tx.execute("INSERT INTO servers(id,owner_id,name,origin,ca_pem,revision,active,directory_hash,created_at)VALUES(?1,?2,?3,?4,?5,1,0,?6,?7)",params![sid,owner,name,origin,ca,hash(&dir),now]).and_then(|_|tx.execute("UPDATE pending_registrations SET poll_consumed=1 WHERE secret_hash=?1 AND poll_consumed=0",[hash(&f.registration_code)]));
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

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct NewKey {
    server_id: String,
    scope: Scope,
    expires_in: Option<u64>,
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
        let n=tx.execute("INSERT INTO automation_keys(id,owner_id,server_id,hash,scope,expires_at) SELECT ?1,?2,id,?3,?4,?5 FROM servers WHERE id=?6 AND owner_id=?2 AND active=1",params![id,owner,hash(&raw),scope,now+life as i64,f.server_id])?;
        if n != 1 {
            return Ok(err(StatusCode::NOT_FOUND, "not_found"));
        }
        tx.execute("INSERT INTO audit_events(event,actor_id,resource_id,outcome,created_at) VALUES('key_created',?1,?2,'success',?3)", params![owner,id,now])?;
        Ok(Json(serde_json::json!({"id":id,"key":raw,"server_id":f.server_id,"scope":scope,"expires_at":now+life as i64})).into_response())
    })
}
async fn revoke_key(State(a): State<Arc<App>>, h: HeaderMap, Path(id): Path<String>) -> Response {
    authorized(&a, &h, |tx, owner, _| {
        let n=tx.execute("UPDATE automation_keys SET revoked_at=?1 WHERE id=?2 AND owner_id=?3 AND revoked_at IS NULL",params![(a.cfg.now)(),id,owner])?;
        if n == 1 {
            tx.execute("INSERT INTO audit_events(event,actor_id,resource_id,outcome,created_at) VALUES('key_revoked',?1,?2,'success',?3)", params![owner,id,(a.cfg.now)()])?;
            Ok(StatusCode::NO_CONTENT.into_response())
        } else {
            Ok(err(StatusCode::NOT_FOUND, "not_found"))
        }
    })
}
async fn key_token(State(a): State<Arc<App>>, h: HeaderMap) -> Response {
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
    let scope = match scope.as_str() {
        "read" => Scope::Read,
        "operate" => Scope::Operate,
        _ => return err(StatusCode::UNAUTHORIZED, "invalid_token"),
    };
    let n = now as u64;
    let jwt = match a.signer.issue(&ServerClaims {
        iss: a.cfg.public_origin.clone(),
        sub: owner,
        aud: format!("kiln:server:{sid}"),
        iat: n,
        nbf: n,
        exp: n + 300,
        credential_id: kid,
        scope,
    }) {
        Ok(x) => x,
        Err(_) => return err(StatusCode::INTERNAL_SERVER_ERROR, "server_error"),
    };
    if tx.commit().is_err() {
        return err(StatusCode::INTERNAL_SERVER_ERROR, "server_error");
    }
    nostore(Json(serde_json::json!({"token":{"access_token":jwt,"token_type":"Bearer","expires_in":300},"server":{"id":sid,"name":name,"origin":origin,"ca_pem":ca,"revision":rev}})).into_response())
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
