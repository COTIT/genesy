use super::{
    dto::{DefinitionRequest, FieldDefinition, ListQuery},
    service,
};
use crate::{
    AppError, AppState,
    error::ValidationErrorBody,
    http::{
        response::{Data, Envelope, MessageBody},
        workspace::{Can, WorkspaceAccess, permissions},
    },
};
use axum::extract::{Path, Query, State};
use uuid::Uuid;
permissions! { FieldAdmin => "workspace_admin" }
fn read(a: &WorkspaceAccess) -> Result<(), AppError> {
    if a.allows("product.view") || a.allows("product.list") {
        Ok(())
    } else {
        Err(AppError::Forbidden("This action is unauthorized."))
    }
}
#[utoipa::path(get,path="/workspace/{workspace}/product-fields",tag="product_fields",operation_id="product_fields_list",params(("workspace"=Uuid,Path),ListQuery),responses((status=404,body=MessageBody,description="Unknown workspace"),(status=422,body=ValidationErrorBody,description="Invalid filter"),(status=200,body=Envelope<Vec<FieldDefinition>>,description="Field definitions"),(status=403,body=MessageBody,description="Missing permission")),security(("bearer"=[])))]
pub async fn list(
    State(s): State<AppState>,
    a: WorkspaceAccess,
    Query(q): Query<ListQuery>,
) -> Result<Data<Vec<FieldDefinition>>, AppError> {
    read(&a)?;
    Ok(Data(
        service::list(&s.db, a.workspace_id, q.status.as_deref()).await?,
    ))
}
#[utoipa::path(get,path="/workspace/{workspace}/product-fields/{field}",tag="product_fields",operation_id="product_fields_get",params(("workspace"=Uuid,Path),("field"=Uuid,Path)),responses((status=403,body=MessageBody,description="Missing permission"),(status=200,body=Envelope<FieldDefinition>,description="Field definition"),(status=404,body=MessageBody,description="Unknown definition")),security(("bearer"=[])))]
pub async fn get(
    State(s): State<AppState>,
    a: WorkspaceAccess,
    Path((_, id)): Path<(Uuid, Uuid)>,
) -> Result<Data<FieldDefinition>, AppError> {
    read(&a)?;
    Ok(Data(service::get(&s.db, a.workspace_id, id).await?))
}
#[utoipa::path(post,path="/workspace/{workspace}/product-fields",tag="product_fields",operation_id="product_fields_create",params(("workspace"=Uuid,Path)),request_body=DefinitionRequest,responses((status=404,body=MessageBody,description="Unknown workspace or definition"),(status=422,body=ValidationErrorBody,description="Invalid definition"),(status=201,body=Envelope<FieldDefinition>,description="Created definition"),(status=403,body=MessageBody,description="Missing permission")),security(("bearer"=[])))]
pub async fn create(
    State(s): State<AppState>,
    a: Can<FieldAdmin>,
    crate::http::json::ValidatedJson(r): crate::http::json::ValidatedJson<DefinitionRequest>,
) -> Result<(axum::http::StatusCode, Data<FieldDefinition>), AppError> {
    Ok((
        axum::http::StatusCode::CREATED,
        Data(service::save(&s.db, a.workspace_id, None, r).await?),
    ))
}
#[utoipa::path(patch,path="/workspace/{workspace}/product-fields/{field}",tag="product_fields",operation_id="product_fields_update",params(("workspace"=Uuid,Path),("field"=Uuid,Path)),request_body=DefinitionRequest,responses((status=422,body=ValidationErrorBody,description="Invalid definition"),(status=404,body=MessageBody,description="Unknown definition"),(status=200,body=Envelope<FieldDefinition>,description="Field definition"),(status=403,body=MessageBody,description="Missing permission")),security(("bearer"=[])))]
pub async fn update(
    State(s): State<AppState>,
    a: Can<FieldAdmin>,
    Path((_, id)): Path<(Uuid, Uuid)>,
    crate::http::json::ValidatedJson(r): crate::http::json::ValidatedJson<DefinitionRequest>,
) -> Result<Data<FieldDefinition>, AppError> {
    Ok(Data(
        service::save(&s.db, a.workspace_id, Some(id), r).await?,
    ))
}
#[utoipa::path(delete,path="/workspace/{workspace}/product-fields/{field}",tag="product_fields",operation_id="product_fields_archive",params(("workspace"=Uuid,Path),("field"=Uuid,Path)),responses((status=404,body=MessageBody,description="Unknown workspace or definition"),(status=200,body=Envelope<FieldDefinition>,description="Field definition"),(status=403,body=MessageBody,description="Missing permission")),security(("bearer"=[])))]
pub async fn archive(
    State(s): State<AppState>,
    a: Can<FieldAdmin>,
    Path((_, id)): Path<(Uuid, Uuid)>,
) -> Result<Data<FieldDefinition>, AppError> {
    let prior = service::get(&s.db, a.workspace_id, id).await?;
    if prior.status == "archived" {
        return Ok(Data(prior));
    }
    let r = serde_json::from_value(serde_json::json!({"status":"archived"}))
        .map_err(|e| AppError::Internal(e.to_string()))?;
    Ok(Data(
        service::save(&s.db, a.workspace_id, Some(id), r).await?,
    ))
}
