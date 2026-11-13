//! Category rules: parent must be a live category of the workspace and never the category
//! itself or one of its descendants (Laravel had no cycle guard); a live name is unique per
//! workspace, trimmed and case-insensitive (Laravel allowed duplicates).

use sqlx::PgConnection;
use uuid::Uuid;

use crate::{
    AppState,
    error::AppError,
    modules::product_categories::{
        dto::{BAD_PARENT, CategoryInput, TAKEN},
        repo::{self, CategoryRow, ListFilter},
    },
};

/// One page plus total.
pub async fn list(
    state: &AppState,
    workspace_id: Uuid,
    f: &ListFilter<'_>,
) -> Result<(Vec<CategoryRow>, i64), AppError> {
    let (rows, mut total) = repo::list(&state.db, workspace_id, f).await?;
    if rows.is_empty() && f.offset > 0 {
        total = repo::count(&state.db, workspace_id, f).await?;
    }
    Ok((rows, total))
}

/// One live category with its parent and live children, else 404.
pub async fn get(
    state: &AppState,
    workspace_id: Uuid,
    id: &str,
) -> Result<(CategoryRow, Option<CategoryRow>, Vec<CategoryRow>), AppError> {
    let c = repo::find(&state.db, workspace_id, parse(id)?)
        .await?
        .ok_or(AppError::NotFound)?;
    let parent = match c.parent_id {
        Some(p) => repo::find(&state.db, workspace_id, p).await?,
        None => None,
    };
    let children = repo::children(&state.db, workspace_id, c.id).await?;
    Ok((c, parent, children))
}

/// Insert; returns the new id.
pub async fn create(
    state: &AppState,
    workspace_id: Uuid,
    c: CategoryInput,
) -> Result<Uuid, AppError> {
    let mut tx = state.db.begin().await?;
    // Two creates of one name could each find it free.
    repo::lock_tree(&mut tx, workspace_id).await?;
    check_name(&mut tx, workspace_id, c.name.as_deref(), None).await?;
    check_parent(&mut tx, workspace_id, c.parent_id.flatten(), None).await?;
    let id = repo::insert(&mut *tx, workspace_id, &c).await?;
    tx.commit().await?;
    Ok(id)
}

/// Patch, else 404.
pub async fn update(
    state: &AppState,
    workspace_id: Uuid,
    id: &str,
    c: CategoryInput,
) -> Result<(), AppError> {
    let id = parse(id)?;
    let mut tx = state.db.begin().await?;
    if c.parent_id.flatten().is_some() || c.name.is_some() {
        // Two re-parents checked at once could each pass and close a loop; two renames could
        // each take one free name.
        repo::lock_tree(&mut tx, workspace_id).await?;
        if repo::find(&mut *tx, workspace_id, id).await?.is_none() {
            return Err(AppError::NotFound);
        }
        check_name(&mut tx, workspace_id, c.name.as_deref(), Some(id)).await?;
        check_parent(&mut tx, workspace_id, c.parent_id.flatten(), Some(id)).await?;
    }
    if !repo::update(&mut *tx, workspace_id, id, &c).await? {
        return Err(AppError::NotFound);
    }
    tx.commit().await?;
    Ok(())
}

/// Soft delete, else 404.
pub async fn delete(state: &AppState, workspace_id: Uuid, id: &str) -> Result<(), AppError> {
    let id = parse(id)?;
    let mut tx = state.db.begin().await?;
    crate::modules::products::service::lock_identity(&mut tx, workspace_id).await?;
    if !repo::soft_delete(&mut *tx, workspace_id, id).await? {
        return Err(AppError::NotFound);
    }
    tx.commit().await?;
    Ok(())
}

async fn check_parent(
    conn: &mut PgConnection,
    workspace_id: Uuid,
    parent: Option<Uuid>,
    id: Option<Uuid>,
) -> Result<(), AppError> {
    match parent {
        Some(p) if !repo::parent_ok(conn, workspace_id, p, id).await? => {
            Err(AppError::field("parent_id", BAD_PARENT))
        }
        _ => Ok(()),
    }
}

async fn check_name(
    conn: &mut PgConnection,
    workspace_id: Uuid,
    name: Option<&str>,
    id: Option<Uuid>,
) -> Result<(), AppError> {
    match name {
        Some(n) if repo::name_taken(conn, workspace_id, n, id).await? => {
            Err(AppError::field("name", TAKEN))
        }
        _ => Ok(()),
    }
}

/// Malformed id = unknown id = 404.
fn parse(id: &str) -> Result<Uuid, AppError> {
    Uuid::parse_str(id).map_err(|_| AppError::NotFound)
}
