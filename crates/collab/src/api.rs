use crate::{
    auth,
    db::{InviteCode, User, UserId},
    AppState, Error, Result,
};
use anyhow::anyhow;
use axum::{
    body::Body,
    extract::{Path, Query},
    http::{self, Request, StatusCode},
    middleware::{self, Next},
    response::IntoResponse,
    routing::{get, post, put},
    Extension, Json, Router,
};
use nanoid::nanoid;
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use tower::ServiceBuilder;
use tracing::instrument;

pub fn routes(state: Arc<AppState>) -> Router<Body> {
    Router::new()
        .route("/users", get(get_users).post(create_user))
        .route(
            "/users/:login",
            get(get_user).put(update_user).delete(destroy_user),
        )
        .route("/users/:login/access_tokens", post(create_access_token))
        .route(
            "/users/:id/invite_codes",
            get(get_invite_codes).post(create_invite_code),
        )
        .route("/invite_codes/:code", put(update_invite_code))
        .route("/panic", post(trace_panic))
        .layer(
            ServiceBuilder::new()
                .layer(Extension(state))
                .layer(middleware::from_fn(validate_api_token)),
        )
}

pub async fn validate_api_token<B>(req: Request<B>, next: Next<B>) -> impl IntoResponse {
    let token = req
        .headers()
        .get(http::header::AUTHORIZATION)
        .and_then(|header| header.to_str().ok())
        .ok_or_else(|| {
            Error::Http(
                StatusCode::BAD_REQUEST,
                "missing authorization header".to_string(),
            )
        })?
        .strip_prefix("token ")
        .ok_or_else(|| {
            Error::Http(
                StatusCode::BAD_REQUEST,
                "invalid authorization header".to_string(),
            )
        })?;

    let state = req.extensions().get::<Arc<AppState>>().unwrap();

    if token != state.api_token {
        Err(Error::Http(
            StatusCode::UNAUTHORIZED,
            "invalid authorization token".to_string(),
        ))?
    }

    Ok::<_, Error>(next.run(req).await)
}

async fn get_users(Extension(app): Extension<Arc<AppState>>) -> Result<Json<Vec<User>>> {
    let users = app.db.get_all_users().await?;
    Ok(Json(users))
}

#[derive(Deserialize)]
struct CreateUserParams {
    github_login: String,
    admin: bool,
}

async fn create_user(
    Json(params): Json<CreateUserParams>,
    Extension(app): Extension<Arc<AppState>>,
) -> Result<Json<User>> {
    let user_id = app
        .db
        .create_user(&params.github_login, params.admin)
        .await?;

    let user = app
        .db
        .get_user_by_id(user_id)
        .await?
        .ok_or_else(|| anyhow!("couldn't find the user we just created"))?;

    Ok(Json(user))
}

#[derive(Deserialize)]
struct UpdateUserParams {
    admin: bool,
}

async fn update_user(
    Path(user_id): Path<i32>,
    Json(params): Json<UpdateUserParams>,
    Extension(app): Extension<Arc<AppState>>,
) -> Result<()> {
    app.db
        .set_user_is_admin(UserId(user_id), params.admin)
        .await?;
    Ok(())
}

async fn destroy_user(
    Path(user_id): Path<i32>,
    Extension(app): Extension<Arc<AppState>>,
) -> Result<()> {
    app.db.destroy_user(UserId(user_id)).await?;
    Ok(())
}

async fn get_user(
    Path(login): Path<String>,
    Extension(app): Extension<Arc<AppState>>,
) -> Result<Json<User>> {
    let user = app
        .db
        .get_user_by_github_login(&login)
        .await?
        .ok_or_else(|| anyhow!("user not found"))?;
    Ok(Json(user))
}

#[derive(Serialize)]
struct UserWithInviteCodes {
    #[serde(flatten)]
    user: User,
    invite_codes: Vec<InviteCode>,
}

#[derive(Deserialize)]
struct CreateInviteCodeParams {
    allowed_usage_count: u32,
}

async fn get_invite_codes(
    Path(user_id): Path<i32>,
    Extension(app): Extension<Arc<AppState>>,
) -> Result<Json<Vec<InviteCode>>> {
    Ok(Json(app.db.get_invite_codes(UserId(user_id)).await?))
}

async fn create_invite_code(
    Path(user_id): Path<i32>,
    Json(params): Json<CreateInviteCodeParams>,
    Extension(app): Extension<Arc<AppState>>,
) -> Result<()> {
    app.db
        .create_invite_code(UserId(user_id), &nanoid!(16), params.allowed_usage_count)
        .await?;
    Ok(())
}

#[derive(Deserialize)]
struct UpdateInviteCodeParams {
    remaining_count: u32,
}

async fn update_invite_code(
    Path(code): Path<String>,
    Json(params): Json<UpdateInviteCodeParams>,
    Extension(app): Extension<Arc<AppState>>,
) -> Result<()> {
    app.db
        .update_invite_code(&code, params.remaining_count)
        .await?;
    Ok(())
}

#[derive(Debug, Deserialize)]
struct Panic {
    version: String,
    text: String,
}

#[instrument(skip(panic))]
async fn trace_panic(panic: Json<Panic>) -> Result<()> {
    tracing::error!(version = %panic.version, text = %panic.text, "panic report");
    Ok(())
}

#[derive(Deserialize)]
struct CreateAccessTokenQueryParams {
    public_key: String,
    impersonate: Option<String>,
}

#[derive(Serialize)]
struct CreateAccessTokenResponse {
    user_id: UserId,
    encrypted_access_token: String,
}

async fn create_access_token(
    Path(login): Path<String>,
    Query(params): Query<CreateAccessTokenQueryParams>,
    Extension(app): Extension<Arc<AppState>>,
) -> Result<Json<CreateAccessTokenResponse>> {
    //     request.require_token().await?;

    let user = app
        .db
        .get_user_by_github_login(&login)
        .await?
        .ok_or_else(|| anyhow!("user not found"))?;

    let mut user_id = user.id;
    if let Some(impersonate) = params.impersonate {
        if user.admin {
            if let Some(impersonated_user) = app.db.get_user_by_github_login(&impersonate).await? {
                user_id = impersonated_user.id;
            } else {
                return Err(Error::Http(
                    StatusCode::UNPROCESSABLE_ENTITY,
                    format!("user {impersonate} does not exist"),
                ));
            }
        } else {
            return Err(Error::Http(
                StatusCode::UNAUTHORIZED,
                format!("you do not have permission to impersonate other users"),
            ));
        }
    }

    let access_token = auth::create_access_token(app.db.as_ref(), user_id).await?;
    let encrypted_access_token =
        auth::encrypt_access_token(&access_token, params.public_key.clone())?;

    Ok(Json(CreateAccessTokenResponse {
        user_id,
        encrypted_access_token,
    }))
}
