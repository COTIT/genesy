//! Assistant rules; existing business handlers retain permission and state checks.

use super::{
    dto::{CompleteInput, ProposalInput, UploadInput},
    repo::{self, Owner, Session},
};
use crate::AppError;
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use sqlx::PgPool;
use std::sync::OnceLock;
use uuid::Uuid;

/// Owned session, including terminal state for history.
pub async fn session(db: &PgPool, o: Owner, id: Uuid) -> Result<Session, AppError> {
    repo::session(db, o, id).await?.ok_or(AppError::NotFound)
}

/// Session resource plus `identity`; `permissions` come from the request's access check.
pub async fn session_with_identity(
    db: &PgPool,
    o: Owner,
    id: Uuid,
    permissions: Vec<String>,
) -> Result<Value, AppError> {
    let (session, workspaces, farm_roles) = repo::session_identity(db, o, id)
        .await?
        .ok_or(AppError::NotFound)?;
    let mut value = session.resource();
    value["identity"] = serde_json::json!({
        "workspaces": workspaces,
        "permissions": permissions,
        "farm_roles": farm_roles,
    });
    Ok(value)
}

/// New assistant session owned by this login.
pub async fn create_session(db: &PgPool, o: Owner, language: &str) -> Result<Value, AppError> {
    let id = repo::create_session(db, o, language).await?;
    Ok(session(db, o, id).await?.resource())
}

/// Create immutable proposal after validating actual documented operation.
pub async fn propose(db: &PgPool, o: Owner, input: ProposalInput) -> Result<Value, AppError> {
    let mut tx = db.begin().await?;
    let owned = repo::lock_session(&mut tx, o, input.session_id)
        .await?
        .ok_or(AppError::NotFound)?;
    require_active(&owned)?;
    if super::batch_repo::active(&mut *tx, o, input.session_id).await? {
        return Err(AppError::Conflict(
            "Finish or cancel the pending fixed batch first.",
        ));
    }
    let id = propose_locked(&mut tx, o, input).await?;
    tx.commit().await?;
    action(db, o, id).await
}

/// Session lock is shared by standalone and atomic batch proposal creation.
#[tracing::instrument(level = "debug", skip_all, fields(operation = %input.operation_id))]
pub(super) async fn propose_locked(
    conn: &mut sqlx::PgConnection,
    o: Owner,
    mut input: ProposalInput,
) -> Result<Uuid, AppError> {
    let path = normalize_path(&input.path)?;
    if let Some(rest) = path.strip_prefix("/v1/workspace/") {
        let actual = rest.split('/').next().ok_or(AppError::NotFound)?;
        if actual != o.workspace_id.to_string() {
            return Err(AppError::NotFound);
        }
    }
    let operation = resolve_operation(&input.method, &path)?;
    if operation != input.operation_id {
        return Err(AppError::field(
            "operation_id",
            "The operation does not match this request.",
        ));
    }
    if !reviewable_method(&input.method, &operation) {
        return Err(AppError::field(
            "method",
            "This read operation does not require an action claim.",
        ));
    }
    input.path = path;
    let hash = match (&input.body_sha256, &input.content_type) {
        (Some(digest), Some(content_type)) => request_hash_raw(
            &input.method,
            &input.path,
            input.query.as_deref(),
            content_type,
            digest,
        ),
        _ => request_hash(
            &input.method,
            &input.path,
            input.query.as_deref(),
            input.body.as_ref(),
        ),
    };
    let proposal = serde_json::to_value(&input)
        .map_err(|_| AppError::Internal("assistant proposal serialization failed".into()))?;
    let review_facts = match crate::modules::products::approval::capture(conn, o, &input).await? {
        Some(facts) => facts,
        None => match crate::modules::invoices::approval::capture(conn, o, &input).await? {
            Some(facts) => facts,
            None => match super::impact::capture(conn, o, &input).await? {
                Some(facts) => facts,
                None => match super::mail_recipients::capture(conn, o, &input).await? {
                    Some(facts) => facts,
                    None => match super::farm_facts::capture(conn, o, &input).await? {
                        Some(facts) => facts,
                        None => {
                            match crate::modules::currency_rates::approval::capture(conn, o, &input)
                                .await?
                            {
                                Some(facts) => facts,
                                None => match crate::modules::storefront_design::approval::capture(
                                    conn, o, &input,
                                )
                                .await?
                                {
                                    Some(facts) => facts,
                                    None => super::preconditions::capture(conn, o, &input).await?,
                                },
                            }
                        }
                    },
                },
            },
        },
    };
    repo::propose(conn, o, &input, &hash, &proposal, &review_facts)
        .await
        .map_err(AppError::from)
}

/// Current bounded action evidence and unexpired draft.
pub async fn action(db: &PgPool, o: Owner, id: Uuid) -> Result<Value, AppError> {
    let row = repo::action(db, o, id).await?.ok_or(AppError::NotFound)?;
    let granted = view_grants(db, o).await?;
    Ok(public_review(row, &granted))
}

/// One permission lookup for bounded history, preserving private execution snapshots.
pub async fn history(
    db: &PgPool,
    o: Owner,
    sid: Uuid,
    before: i64,
) -> Result<Vec<Value>, AppError> {
    session(db, o, sid).await?;
    let granted = view_grants(db, o).await?;
    Ok(repo::history(db, o, sid, before)
        .await?
        .into_iter()
        .map(|row| public_review(row, &granted))
        .collect())
}

#[tracing::instrument(level = "debug", skip_all)]
pub(super) async fn view_grants(
    db: &PgPool,
    o: Owner,
) -> Result<crate::modules::permissions::model::Granted, AppError> {
    let access = crate::modules::permissions::repo::access(db, o.workspace_id, o.user_id)
        .await?
        .ok_or(AppError::NotFound)?;
    if !access.active {
        return Err(AppError::Forbidden("This action is unauthorized."));
    }
    Ok(crate::modules::permissions::model::Granted::new(
        access.owner,
        access.permissions,
    ))
}

pub(super) fn public_review(
    mut row: Value,
    granted: &crate::modules::permissions::model::Granted,
) -> Value {
    let facts = &row["review_facts"];
    if facts.is_null() || facts.as_object().is_some_and(|v| v.is_empty()) {
        return row;
    }
    let allowed = match facts["facts"]["kind"].as_str() {
        Some("order") => {
            granted.allows("order.view")
                && (facts["operation_id"] != "orders_invoice" || granted.allows("invoice.view"))
        }
        Some("stock" | "stock_transfer") => granted.allows("stock.list"),
        Some("product" | "product_variant") => granted.allows("product.view"),
        Some("invoice") => granted.allows("invoice.view"),
        Some("mail" | "contact" | "relation") => granted.allows("relation.view"),
        // Farm roles aren't workspace permissions; farm_facts::capture refused callers
        // without the operation's role before reading any name.
        Some("farm_supply" | "supply_correction" | "harvest" | "dispatch" | "supply_link") => true,
        // Currencies and rates are readable by every active member.
        Some("currency_rate") => true,
        Some("storefront_design") => granted.allows("workspace_admin"),
        _ => false,
    };
    if !allowed {
        row["review_facts"] = json!({"factsHiddenByPermission":true});
    } else {
        for stage in ["locked", "priced"] {
            if let Some(values) = row["review_facts"]["facts"][stage].as_object_mut() {
                values.remove("material_sha256");
            }
        }
    }
    row
}

/// Signed bridge approval, bound to exact session and original login.
pub async fn approve(
    db: &PgPool,
    o: Owner,
    id: Uuid,
    sid: Uuid,
    event: Option<&str>,
) -> Result<Value, AppError> {
    let mut tx = db.begin().await?;
    let owned = repo::lock_session(&mut tx, o, sid)
        .await?
        .ok_or(AppError::NotFound)?;
    require_active(&owned)?;
    if let Some(event) = event
        && super::batch_repo::event_used(&mut *tx, o, sid, event, None).await?
    {
        return Err(AppError::Conflict(
            "This approval event has already been used.",
        ));
    }
    let approved = repo::approve(&mut *tx, o, id, sid, event)
        .await
        .map_err(|e| {
            if e.as_database_error().and_then(|d| d.constraint())
                == Some("assistant_actions_approval_event_idx")
            {
                AppError::Conflict("This approval event has already been used.")
            } else {
                AppError::Database(e)
            }
        })?;
    if approved != 1 {
        return Err(AppError::Conflict("This action cannot be approved."));
    }
    tx.commit().await?;
    action(db, o, id).await
}

/// Cancel only while action remains undispatched.
pub async fn cancel(db: &PgPool, o: Owner, id: Uuid, sid: Uuid) -> Result<Value, AppError> {
    session(db, o, sid).await?;
    if repo::cancel(db, o, id, sid).await? != 1 {
        return Err(AppError::Conflict(
            "This action has already been dispatched or is unavailable.",
        ));
    }
    action(db, o, id).await
}

/// Reserve lifetime and in-flight quotas under session lock.
pub async fn reserve(
    db: &PgPool,
    o: Owner,
    sid: Uuid,
    input: UploadInput,
) -> Result<Value, AppError> {
    let mut tx = db.begin().await?;
    let owned = repo::lock_session(&mut tx, o, sid)
        .await?
        .ok_or(AppError::NotFound)?;
    require_active(&owned)?;
    let (total, reserved) = repo::upload_counts(&mut tx, o.workspace_id, sid).await?;
    if total >= 10 {
        return Err(AppError::Conflict(
            "This session has reached its ten-file limit.",
        ));
    }
    if reserved >= 2 {
        return Err(AppError::Conflict(
            "Two files are already uploading in this session.",
        ));
    }
    let data = repo::reserve(&mut tx, o.workspace_id, sid, &input).await?;
    tx.commit().await?;
    Ok(data)
}

/// Serialize late provider completion with cleanup so remote refs cannot be lost.
pub async fn complete_upload(
    db: &PgPool,
    o: Owner,
    sid: Uuid,
    id: Uuid,
    input: &CompleteInput,
) -> Result<Value, AppError> {
    let mut tx = db.begin().await?;
    let session = repo::lock_session(&mut tx, o, sid)
        .await?
        .ok_or(AppError::NotFound)?;
    if session.state == "cleaned" {
        return Err(AppError::Conflict(
            "This assistant session has been cleaned.",
        ));
    }
    if session.conversation_id.is_none() {
        return Err(AppError::Conflict(
            "Attach a provider conversation before completing an upload.",
        ));
    }
    let value = repo::complete_upload(&mut *tx, o.workspace_id, sid, id, input, !session.usable)
        .await?
        .ok_or(AppError::Conflict("This upload cannot be completed."))?;
    tx.commit().await?;
    Ok(value)
}

/// Refuse reviving expired or abandoned sessions.
pub fn require_active(s: &Session) -> Result<(), AppError> {
    if s.usable {
        Ok(())
    } else {
        Err(AppError::Conflict("This assistant session has ended."))
    }
}

/// Same canonical JSON hash for review and actual dispatch; query bytes remain exact.
pub fn request_hash(
    method: &str,
    path: &str,
    query: Option<&str>,
    body: Option<&Value>,
) -> [u8; 32] {
    let mut h = Sha256::new();
    for part in [method, path, query.unwrap_or("")] {
        h.update(part.as_bytes());
        h.update(b"\n");
    }
    if let Some(body) = body {
        h.update(body.to_string().as_bytes());
    }
    h.finalize().into()
}

/// Reviewed GET previews can create PDF/file effects; ordinary reads need no claim.
pub fn reviewable_method(method: &str, operation: &str) -> bool {
    if gateway_financial_operation(operation) || operation.starts_with("product_bulk_") {
        return false;
    }
    matches!(method, "POST" | "PUT" | "PATCH" | "DELETE")
        || method == "GET"
            && matches!(
                operation,
                "invoices_preview" | "pdf_templates_preview" | "messages_profile_test"
            )
}

/// Gateway money, reconciliation and account administration require the native Office UI.
fn gateway_financial_operation(operation: &str) -> bool {
    matches!(
        operation,
        "invoices_gateway_create"
            | "invoices_gateway_refund"
            | "payments_gateway_refresh"
            | "payments_gateway_resolve"
            | "payments_globadyme_callback_retire"
            | "payments_globadyme_webhook"
    )
}

/// Bind exact raw multipart bytes and MIME boundary, domain-separated from JSON drafts.
pub fn request_hash_raw(
    method: &str,
    path: &str,
    query: Option<&str>,
    content_type: &str,
    digest: &str,
) -> [u8; 32] {
    let mut h = Sha256::new();
    for part in [
        method,
        path,
        query.unwrap_or(""),
        "raw-multipart-sha256",
        content_type,
        digest,
    ] {
        h.update(part.as_bytes());
        h.update(b"\n");
    }
    h.finalize().into()
}

/// One documented ordinary-bearer operation.
struct Operation {
    method: String,
    path: String,
    id: String,
}
static OPS: OnceLock<Result<Vec<Operation>, String>> = OnceLock::new();

/// Inventory from the OpenAPI spec, built once per process.
fn operations() -> &'static Result<Vec<Operation>, String> {
    OPS.get_or_init(|| {
        let spec = crate::openapi::spec()
            .map_err(|_| "assistant operation inventory failed".to_owned())?;
        let mut ops = Vec::new();
        if let Some(paths) = spec["paths"].as_object() {
            for (path, item) in paths {
                if let Some(item) = item.as_object() {
                    for (method, op) in item {
                        if op["security"]
                            .as_array()
                            .is_some_and(|ss| ss.iter().any(|s| s.get("bearer").is_some()))
                            && let Some(id) = op["operationId"].as_str()
                            && !gateway_financial_operation(id)
                        {
                            ops.push(Operation {
                                method: method.to_ascii_uppercase(),
                                path: format!("/v1{path}"),
                                id: id.to_owned(),
                            });
                        }
                    }
                }
            }
        }
        Ok(ops)
    })
}

/// Build the inventory at boot; false when the spec could not be read.
pub fn warm_operations() -> bool {
    operations().is_ok()
}

/// True once the inventory is built.
pub fn operations_ready() -> bool {
    OPS.get().is_some()
}

/// Design routes outside voice for good: whole-document save, previews, preview links
/// (they mint and resolve bearer secrets), public reads.
fn closed_design_route(method: &str, path: &str) -> bool {
    let home = path
        .split_once("/storefront/pages/home")
        .map(|(_, rest)| rest);
    path == "/v1/storefront/pages/home"
        || path.starts_with("/v1/storefront/preview")
        || method == "POST" && home == Some("/revisions")
        || matches!(home, Some("/preview" | "/edits/preview"))
        || home.is_some_and(|rest| rest.starts_with("/preview-links"))
}

/// Allow documented ordinary-bearer business routes only; never assistant controls/secrets.
#[tracing::instrument(level = "debug", skip_all)]
pub fn resolve_operation(method: &str, path: &str) -> Result<String, AppError> {
    let ops = operations()
        .as_ref()
        .map_err(|_| AppError::Internal("assistant operation inventory failed".into()))?;
    if path.contains("/assistant/")
        || path.contains("/product-bulk-runs")
        || closed_design_route(method, path)
        || path.contains("/app-keys") && method != "DELETE"
        || path.contains("/credentials")
        || path.contains("/password")
        || path.ends_with("/logout")
    {
        return Err(AppError::field(
            "operation_id",
            "Use the secure Office form for this action.",
        ));
    }
    let actual: Vec<_> = path.split('/').collect();
    ops.iter()
        .filter(|op| op.method == method)
        .filter_map(|op| {
            let template: Vec<_> = op.path.split('/').collect();
            if template.len() != actual.len() {
                return None;
            }
            let mut literal = 0;
            for (t, a) in template.iter().zip(&actual) {
                if t.starts_with('{') {
                    if a.is_empty() {
                        return None;
                    }
                } else if t == a {
                    literal += 1;
                } else {
                    return None;
                }
            }
            Some((literal, &op.id))
        })
        .max_by_key(|(n, _)| *n)
        .map(|(_, id)| id.clone())
        .ok_or(AppError::NotFound)
}

fn normalize_path(path: &str) -> Result<String, AppError> {
    let path = if path.starts_with("/v1/") {
        path.to_owned()
    } else {
        format!("/v1{path}")
    };
    if !path.starts_with("/v1/")
        || path.contains(['?', '#', '%'])
        || path.contains("//")
        || path.split('/').any(|p| p == "." || p == "..")
    {
        return Err(AppError::field(
            "path",
            "Use an API path without a query or fragment.",
        ));
    }
    Ok(path)
}

/// Validate bounded native generation receipt before storing only status and counts.
pub fn generation_evidence(
    status: &str,
    requested: usize,
    created: usize,
    skipped: usize,
) -> Value {
    if requested > 500
        || requested.checked_sub(created) != Some(skipped)
        || !matches!((status, skipped), ("applied", 0) | ("partial", 1..))
    {
        return json!({"status":"unknown"});
    }
    json!({"status":status,"requested":requested,"created":created,"skipped":skipped})
}

/// Status-only audit projection never stores response bodies or raw errors.
pub fn evidence(operation: &str, status: u16) -> Value {
    let mut evidence = json!({"http_status":status,"meaning":if !(200..300).contains(&status){"outcome_unknown"}else if operation=="messages_send"||operation.contains("invoice")&&operation.contains("send"){"processed_delivery_not_proven"}else{"endpoint_completed"}});
    match operation {
        "teams_create" | "teams_set_permissions" => {
            evidence["permission_changes"] = json!({"status":"unknown"});
        }
        "orders_refund" => {
            evidence["refund"] = json!({"status":"unknown"});
        }
        "product_variants_generate" => {
            evidence["generation"] = json!({"status":"unknown"});
        }
        _ => {}
    }
    evidence
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn storefront_design_facts_need_workspace_admin() {
        use crate::modules::permissions::model::Granted;
        let row =
            json!({"review_facts":{"version":1,"facts":{"kind":"storefront_design","changes":[]}}});
        let member = Granted::new(false, ["setting.get".to_owned()]);
        assert_eq!(
            public_review(row.clone(), &member)["review_facts"],
            json!({"factsHiddenByPermission":true})
        );
        let admin = Granted::new(false, ["workspace_admin".to_owned()]);
        for granted in [admin, Granted::new(true, [])] {
            let shown = public_review(row.clone(), &granted);
            assert_eq!(shown["review_facts"]["facts"]["kind"], "storefront_design");
            assert!(
                shown["review_facts"]
                    .get("factsHiddenByPermission")
                    .is_none()
            );
        }
    }
    #[test]
    fn hash_binds_every_argument() {
        let base = request_hash("POST", "/v1/x", None, Some(&json!({"a":1,"b":2})));
        assert_eq!(
            base,
            request_hash("POST", "/v1/x", None, Some(&json!({"b":2,"a":1})))
        );
        assert_ne!(
            base,
            request_hash("POST", "/v1/x", Some("q=x"), Some(&json!({"a":1,"b":2})))
        );
        assert_ne!(
            base,
            request_hash("DELETE", "/v1/x", None, Some(&json!({"a":1,"b":2})))
        );
    }
    #[test]
    fn secret_controls_never_become_business_operations() {
        let home = "/v1/workspace/w/storefront/pages/home";
        for (method, path) in [
            ("POST", format!("{home}/revisions")),
            ("POST", format!("{home}/edits/preview")),
            ("GET", format!("{home}/preview")),
            ("GET", "/v1/storefront/pages/home".to_owned()),
            ("POST", format!("{home}/preview-links")),
            ("GET", format!("{home}/preview-links")),
            ("GET", format!("{home}/preview-links?revision_id=x")),
            ("DELETE", format!("{home}/preview-links/l")),
            ("POST", "/v1/storefront/preview".to_owned()),
        ] {
            assert!(resolve_operation(method, &path).is_err(), "{method} {path}");
        }
        for (method, path, id) in [
            ("POST", format!("{home}/edits"), "storefront_design_edit"),
            (
                "POST",
                format!("{home}/restore"),
                "storefront_design_restore",
            ),
            (
                "POST",
                format!("{home}/publish"),
                "storefront_design_publish",
            ),
            ("GET", home.to_owned(), "storefront_design_get"),
            (
                "GET",
                format!("{home}/revisions"),
                "storefront_design_revisions",
            ),
            (
                "GET",
                "/v1/workspace/w/storefront/design-capabilities".to_owned(),
                "storefront_design_capabilities",
            ),
        ] {
            assert_eq!(resolve_operation(method, &path).unwrap(), id, "{path}");
        }
        assert!(resolve_operation("POST", "/v1/workspace/w/assistant/actions").is_err());
        assert!(resolve_operation("POST", "/v1/workspace/w/app-keys").is_err());
    }
    #[test]
    fn gateway_money_and_administration_never_enter_assistant_inventory() {
        for (operation, path) in [
            (
                "invoices_gateway_create",
                "/v1/invoices/fetch-from-hashed/token/payment",
            ),
            (
                "invoices_gateway_refund",
                "/v1/workspace/w/invoices/i/refunds",
            ),
            (
                "payments_gateway_refresh",
                "/v1/workspace/w/payments/p/refresh",
            ),
            (
                "payments_gateway_resolve",
                "/v1/workspace/w/payments/p/resolve",
            ),
            (
                "payments_globadyme_callback_retire",
                "/v1/workspace/w/payments/globadyme/sandbox/callback-secret/retire",
            ),
            (
                "payments_globadyme_webhook",
                "/v1/payments/webhooks/globadyme/w/sandbox",
            ),
        ] {
            assert!(!reviewable_method("POST", operation), "{operation}");
            assert!(resolve_operation("POST", path).is_err(), "{operation}");
        }
        assert_eq!(
            resolve_operation("GET", "/v1/workspace/w/payments").unwrap(),
            "payments_gateway_list"
        );
        assert_eq!(
            resolve_operation("GET", "/v1/workspace/w/payments/p").unwrap(),
            "payments_gateway_get"
        );
        assert_eq!(
            resolve_operation("GET", "/v1/workspace/w/invoices/i/gateway").unwrap(),
            "invoices_gateway_inspect"
        );
        assert!(
            reviewable_method("POST", "orders_refund"),
            "legacy refunds retain their approved native review"
        );
    }
}

#[cfg(test)]
mod invoice_private_digest_tests {
    use super::*;
    #[test]
    fn bulk_grants_cannot_be_created_through_the_legacy_action_bridge() {
        for suffix in ["approve", "reviews", "settings", "stage"] {
            assert!(
                resolve_operation(
                    "POST",
                    &format!("/v1/workspace/w/product-bulk-runs/r/{suffix}")
                )
                .is_err()
            );
        }
        assert!(!reviewable_method("POST", "product_bulk_runs_approve"));
    }
    #[test]
    fn invoice_digest_stays_private_and_read_permission_controls_summary() {
        let row = json!({"review_facts":{"version":1,"operation_id":"invoices_send","facts":{"kind":"invoice","locked":{"id":"invoice","line_count":1000,"lines_omitted":995,"material_sha256":"private-digest"}}}});
        let view =
            crate::modules::permissions::model::Granted::new(false, ["invoice.view".to_owned()]);
        let public = public_review(row.clone(), &view);
        assert!(
            public["review_facts"]["facts"]["locked"]
                .get("material_sha256")
                .is_none()
        );
        assert_eq!(
            public["review_facts"]["facts"]["locked"]["line_count"],
            1000
        );
        let write =
            crate::modules::permissions::model::Granted::new(false, ["invoice.send".to_owned()]);
        let hidden = public_review(row, &write);
        assert_eq!(
            hidden["review_facts"],
            json!({"factsHiddenByPermission":true})
        );
    }
}

#[cfg(test)]
mod native_money_review_tests {
    use super::*;
    #[test]
    fn product_and_variant_quotes_keep_decimal_text_but_hide_private_digest() {
        for kind in ["product", "product_variant"] {
            let row = json!({"review_facts":{"version":1,"operation_id":"product_variants_generate","facts":{"kind":kind,"priced":{"price":"743.0578","variant_count":500,"variants_omitted":495,"material_sha256":"private"}}}});
            let read = crate::modules::permissions::model::Granted::new(
                false,
                ["product.view".to_owned()],
            );
            let public = public_review(row.clone(), &read);
            assert_eq!(
                public["review_facts"]["facts"]["priced"]["price"],
                "743.0578"
            );
            assert_eq!(
                public["review_facts"]["facts"]["priced"]["variant_count"],
                500
            );
            assert!(
                public["review_facts"]["facts"]["priced"]
                    .get("material_sha256")
                    .is_none()
            );
            let write = crate::modules::permissions::model::Granted::new(
                false,
                ["product.create".to_owned()],
            );
            assert_eq!(
                public_review(row, &write)["review_facts"],
                json!({"factsHiddenByPermission":true})
            );
        }
    }
    #[test]
    fn created_invoice_priced_digest_is_private_too() {
        let row = json!({"review_facts":{"version":1,"operation_id":"invoices_create","facts":{"kind":"invoice","priced":{"line_count":1000,"material_sha256":"private"}}}});
        let read =
            crate::modules::permissions::model::Granted::new(false, ["invoice.view".to_owned()]);
        let public = public_review(row, &read);
        assert_eq!(
            public["review_facts"]["facts"]["priced"]["line_count"],
            1000
        );
        assert!(
            public["review_facts"]["facts"]["priced"]
                .get("material_sha256")
                .is_none()
        );
    }
}

#[cfg(test)]
mod generation_evidence_tests {
    use super::*;
    #[test]
    fn generation_receipt_requires_bounded_consistent_counts_and_status() {
        assert_eq!(generation_evidence("partial", 3, 2, 1)["status"], "partial");
        for (status, requested, created, skipped) in [
            ("applied", 3, 2, 0),
            ("partial", 2, 2, 0),
            ("applied", 501, 501, 0),
            ("unexpected", 1, 1, 0),
        ] {
            assert_eq!(
                generation_evidence(status, requested, created, skipped),
                json!({"status":"unknown"})
            );
        }
    }
    #[test]
    fn known_semantic_effects_need_typed_receipts_even_after_http_success() {
        assert_eq!(
            evidence("product_variants_generate", 201)["generation"]["status"],
            "unknown"
        );
        assert_eq!(
            evidence("teams_set_permissions", 200)["permission_changes"]["status"],
            "unknown"
        );
    }
}

#[cfg(test)]
mod order_invoice_privacy_tests {
    use super::*;
    #[test]
    fn order_read_alone_does_not_expose_new_invoice_billing_facts() {
        let row = json!({"review_facts":{"version":1,"operation_id":"orders_invoice","facts":{"kind":"order","locked":{"number":"ORD-1"},"priced":{"billing_address":{"street":"Private invoice address"},"material_sha256":"private"}}}});
        let order = crate::modules::permissions::model::Granted::new(
            false,
            ["order.view".to_owned(), "order.update".to_owned()],
        );
        assert_eq!(
            public_review(row.clone(), &order)["review_facts"],
            json!({"factsHiddenByPermission":true})
        );
        let both = crate::modules::permissions::model::Granted::new(
            false,
            ["order.view".to_owned(), "invoice.view".to_owned()],
        );
        let visible = public_review(row, &both);
        assert_eq!(
            visible["review_facts"]["facts"]["priced"]["billing_address"]["street"],
            "Private invoice address"
        );
        assert!(
            visible["review_facts"]["facts"]["priced"]
                .get("material_sha256")
                .is_none()
        );
    }
}
