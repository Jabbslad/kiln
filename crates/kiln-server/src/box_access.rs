use crate::Failure;
use axum::http::{HeaderValue, StatusCode, header};
use kiln_api::{BoxView, Caller, Role, may_access_box};
use reqwest::Url;

/// Members may only reach boxes they own; owners skip the metadata read.
pub async fn ensure_member_box_access(
    client: &reqwest::Client,
    upstream: &Url,
    authorization: &HeaderValue,
    box_id: &str,
    caller: &Caller,
    tenant_owner: &str,
) -> Result<(), Failure> {
    if caller.role == Role::Owner {
        return Ok(());
    }
    let mut url = upstream.clone();
    url.set_path(&format!("/v1/boxes/{box_id}"));
    let response = client
        .get(url)
        .header(header::AUTHORIZATION, authorization.clone())
        .send()
        .await
        .map_err(|_| Failure::missing())?;
    if response.status() == StatusCode::NOT_FOUND {
        return Err(Failure::missing());
    }
    if !response.status().is_success() {
        return Err(Failure::missing());
    }
    let bytes = response.bytes().await.map_err(|_| Failure::missing())?;
    if bytes.len() > kiln_api::MAX_RESPONSE_BYTES {
        return Err(Failure::missing());
    }
    let view: BoxView = serde_json::from_slice(&bytes).map_err(|_| Failure::missing())?;
    if view.id != box_id {
        return Err(Failure::missing());
    }
    if may_access_box(caller, view.owner.as_deref(), tenant_owner) {
        Ok(())
    } else {
        Err(Failure::missing())
    }
}
