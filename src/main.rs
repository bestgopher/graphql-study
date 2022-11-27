#![allow(dead_code)]

use async_graphql::{
    http::GraphiQLSource, ComplexObject, Context, Enum, InputObject, InputValueError,
    InputValueResult, Object, Result, Scalar, ScalarType, Schema, SimpleObject, Value,
};
use axum::{
    response::{self, IntoResponse},
    routing::get,
    Extension, Router, Server,
};

use async_graphql_axum::{GraphQLRequest, GraphQLResponse, GraphQLSubscription};
use chrono::{DateTime, NaiveDateTime, Utc};
use futures::{Stream, StreamExt, TryStreamExt};
use serde::{Deserialize, Serialize};
use sqlx::{encode::IsNull, mysql::MySqlRow, Database, FromRow, MySql, MySqlPool, Pool, Row};
use std::{sync::Mutex, time::Duration};
use tokio::sync::broadcast::{self, Sender};
// use tokio_stream::TryStreamExt as _;

type MySchema = Schema<Query, Mutation, Subscription>;
type MysqlConn = Pool<MySql>;

#[allow(dead_code)]
const CLIENT_ID: &str = "e2cd9d43448e8d540557";
#[allow(dead_code)]
const CLIENT_SECRETS: &str = "da4a28f91e1b4025c7344a8ce6ea72c5109ce12f";

async fn graphiql() -> impl IntoResponse {
    response::Html(
        GraphiQLSource::build()
            .endpoint("http://localhost:8000/graphql")
            .subscription_endpoint("ws://localhost:8000/ws")
            .finish(),
    )
}

async fn graphql_handler(schema: Extension<MySchema>, req: GraphQLRequest) -> GraphQLResponse {
    schema.execute(req.into_inner()).await.into()
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let pool = MySqlPool::connect("mysql://root:123456@localhost/graphql").await?;
    let (tx, rx) = broadcast::channel::<Photo>(16);
    let schema = Schema::build(Query, Mutation, Subscription)
        .data(pool)
        .data(tx)
        .data(rx)
        .finish();

    println!("{}", schema.sdl());

    let app = Router::new()
        .route("/ws", GraphQLSubscription::new(schema.clone()))
        .route("/graphql", get(graphiql).post(graphql_handler))
        .layer(Extension(schema));

    println!("GraphiQL IDE: http://localhost:8000/graphql");

    Server::bind(&"127.0.0.1:8000".parse()?)
        .serve(app.into_make_service())
        .await?;

    Ok(())
}

struct Query;

#[Object]
impl Query {
    /// 获取所有的照片
    async fn total_photos(&self, ctx: &Context<'_>) -> async_graphql::Result<i32> {
        let (count,) = sqlx::query_as(r#"SELECT COUNT(*) as count FROM photo;"#)
            .fetch_one(ctx.data::<MysqlConn>()?)
            .await?;

        Ok(count)
    }

    async fn all_photos<'a>(
        &self,
        ctx: &Context<'a>,
        #[graphql(default)] after: Option<MyDatetime>,
    ) -> Result<Vec<Photo>> {
        let mut all = if after.is_none() {
            sqlx::query("SELECT * FROM photo;")
        } else {
            sqlx::query("SELECT * FROM photo WHERE created > ?;").bind(after.unwrap().0)
        }
        .map(|row: MySqlRow| -> sqlx::Result<_> { Photo::from_row(&row) })
        .fetch(ctx.data::<MysqlConn>()?);

        let mut v = vec![];
        while let Some(r) = all.try_next().await? {
            v.push(r?);
        }

        Ok(v)
    }

    async fn all_users(&self, ctx: &Context<'_>) -> Result<Vec<User>> {
        let all = sqlx::query("SELECT * FROM user;")
            .fetch_all(ctx.data::<MysqlConn>()?)
            .await?;

        let mut data = Vec::with_capacity(all.len());
        for ref row in all {
            data.push(User::from_row(row)?);
        }

        Ok(data)
    }
}

struct Mutation;

#[Object]
impl Mutation {
    async fn post_photo(&self, ctx: &Context<'_>, photo: PostPhotoInput) -> Result<Photo> {
        let p = Photo::new(photo);

        sqlx::query("INSERT INTO photo(name, description, category, github_user, created) VALUES(?, ?, ?, ?, ?);")
        .bind(&p.name)
        .bind(p.description.as_ref().unwrap_or(&"".into()))
        .bind(p.category)
        .bind(&p.github_user)
        .bind(p.created.0)
        .execute(ctx.data::<MysqlConn>()?)
        .await?;

        let sender = ctx.data::<Sender<Photo>>()?.clone();
        let p1 = p.clone();

        tokio::spawn(async move { sender.send(p1) });

        Ok(p)
    }

    // https://github.com/login/oauth/authorize?client_id=e2cd9d43448e8d540557&scope=user
    async fn github_auth(&self, ctx: &Context<'_>, code: String) -> Result<AuthPayload> {
        let access = request_github_token(code).await?;
        let access_token = match access {
            AccessTokenResp::Fail { error, .. } => return Err(error.into()),
            AccessTokenResp::Success { access_token, .. } => access_token,
        };

        let user = request_github_user_account(&access_token).await?;

        sqlx::query(
            r#"
            INSERT INTO user(name, github_login, github_token, avatar) VALUES (?, ?, ?, ?) ON DUPLICATE KEY UPDATE
            name = VALUES(name), github_login=VALUES(github_login), github_token=VALUES(github_token), avatar=VALUES(avatar);
            "#,
        )
        .bind(&user.name)
        .bind(&user.github_login.to_string())
        .bind(&access_token)
        .bind(&user.avatar)
        .execute(ctx.data::<MysqlConn>()?)
        .await?;

        Ok(AuthPayload {
            user,
            token: access_token,
        })
    }
}

struct Subscription;

#[async_graphql::Subscription]
impl Subscription {
    async fn integers(&self, #[graphql(default = 1)] step: i32) -> Result<impl Stream<Item = i32>> {
        let mut value = 0;
        Ok(
            tokio_stream::wrappers::IntervalStream::new(tokio::time::interval(
                Duration::from_secs(3),
            ))
            .map(move |_| {
                println!("step: {value}");
                value += step;
                value
            }),
        )
    }

    async fn new_photo(&self, ctx: &Context<'_>) -> Result<impl Stream<Item = Photo>> {
        let stream = ctx.data::<Sender<Photo>>()?.subscribe();
        Ok(tokio_stream::wrappers::BroadcastStream::new(stream)
            .filter_map(|x| async move { x.ok() }))
    }
}

#[derive(SimpleObject, Clone, Deserialize, Serialize)]
#[graphql(complex)]
struct Photo {
    id: async_graphql::ID,
    name: String,
    description: Option<String>,
    category: PhotoCategory,
    #[graphql(skip)]
    github_user: String,
    created: MyDatetime,
}

impl FromRow<'_, MySqlRow> for Photo {
    fn from_row(row: &MySqlRow) -> Result<Self, sqlx::Error> {
        Ok(Self {
            id: row.try_get::<'_, i32, _>("id")?.to_string().into(),
            name: row.try_get("name")?,
            description: row.try_get("description").ok(),
            category: PhotoCategory::from_row(row)?,
            github_user: row.try_get("github_user")?,
            created: MyDatetime(row.try_get("created")?),
        })
    }
}

#[ComplexObject]
impl Photo {
    #[graphql(skip)]
    fn new(photo: PostPhotoInput) -> Self {
        Self {
            name: photo.name,
            description: if photo.description.is_empty() {
                None
            } else {
                Some(photo.description)
            },
            category: photo.category.unwrap_or_default(),
            id: uuid::Uuid::new_v4().into(),
            github_user: "".into(),
            created: MyDatetime(Utc::now().timestamp()),
        }
    }

    async fn url(&self) -> String {
        format!("http://localhost:8000/img/{}", self.id.as_str())
    }

    async fn posted_by(&self, ctx: &Context<'_>) -> Option<User> {
        ctx.data::<Vec<User>>()
            .map(|x| {
                x.iter()
                    .cloned()
                    .find(|y| y.name.as_ref().map_or_else(|| false, |_| true))
                    .unwrap()
            })
            .ok()
    }

    async fn tagged_users(&self, ctx: &Context<'_>) -> Vec<User> {
        let users = ctx
            .data::<Vec<Tag>>()
            .unwrap()
            .iter()
            .filter_map(|x| {
                if self.id == x.photo_id {
                    Some(x.user_id.as_str())
                } else {
                    None
                }
            })
            .collect::<Vec<&str>>();

        ctx.data::<Vec<User>>()
            .unwrap()
            .iter()
            .cloned()
            .filter(|u| users.contains(&u.github_login.as_str()))
            .collect()
    }
}

#[derive(Enum, Clone, Copy, Eq, PartialEq, Serialize, Deserialize)]
enum PhotoCategory {
    #[graphql(name = "SELFIE")]
    Selfie,
    #[graphql(name = "PORTRAIT")]
    Portrait,
    #[graphql(name = "ACTION")]
    Action,
    #[graphql(name = "LANDSCAPE")]
    Landscape,
    #[graphql(name = "GRAPHIC")]
    Graphic,
}

impl Default for PhotoCategory {
    fn default() -> Self {
        Self::Portrait
    }
}

impl sqlx::FromRow<'_, MySqlRow> for PhotoCategory {
    fn from_row(row: &MySqlRow) -> Result<Self, sqlx::Error> {
        let value: i8 = row.try_get("category")?;
        Ok(match value {
            1 => Self::Selfie,
            2 => Self::Portrait,
            3 => Self::Action,
            4 => Self::Landscape,
            5 => Self::Graphic,
            _ => Self::Selfie,
        })
    }
}

impl sqlx::Encode<'_, MySql> for PhotoCategory {
    fn encode_by_ref(&self, buf: &mut Vec<u8>) -> IsNull {
        buf.extend((*self as i8).to_le_bytes());

        IsNull::No
    }

    fn size_hint(&self) -> usize {
        1
    }
}

impl sqlx::Type<MySql> for PhotoCategory {
    fn type_info() -> <MySql as Database>::TypeInfo {
        i8::type_info()
    }
}

#[derive(InputObject)]
struct PostPhotoInput {
    name: String,
    #[graphql(default_with = "Some(PhotoCategory::Portrait)")]
    category: Option<PhotoCategory>,
    description: String,
}

#[derive(SimpleObject, Clone, Deserialize, Serialize, Debug)]
#[graphql(complex)]
struct User {
    #[serde(rename = "login")]
    github_login: async_graphql::ID,
    name: Option<String>,
    #[serde(rename = "avatar_url")]
    avatar: Option<String>,
    #[serde(default)]
    github_token: String,
}

impl FromRow<'_, MySqlRow> for User {
    fn from_row(row: &MySqlRow) -> Result<Self, sqlx::Error> {
        Ok(Self {
            github_login: row.try_get::<'_, String, _>("github_login")?.into(),
            name: row.try_get("name").ok(),
            avatar: row.try_get("avatar").ok(),
            github_token: row.try_get("github_token")?,
        })
    }
}

#[ComplexObject]
impl User {
    async fn in_photo(&self, ctx: &Context<'_>) -> Vec<Photo> {
        let photo_ids = ctx
            .data::<Vec<Tag>>()
            .unwrap()
            .iter()
            .filter_map(|x| {
                if x.user_id == self.github_login.as_str() {
                    Some(x.photo_id.as_str())
                } else {
                    None
                }
            })
            .collect::<Vec<&str>>();

        ctx.data::<Mutex<Vec<Photo>>>()
            .unwrap()
            .lock()
            .unwrap()
            .iter()
            .cloned()
            .filter(|x| photo_ids.contains(&x.id.as_str()))
            .collect()
    }

    async fn posted_photos(&self, _ctx: &Context<'_>) -> Result<Vec<Photo>> {
        todo!()
    }
}

struct Tag {
    photo_id: async_graphql::ID,
    user_id: String,
}

#[derive(Serialize, Deserialize, Clone)]
struct MyDatetime(i64);

#[Scalar]
impl ScalarType for MyDatetime {
    fn parse(value: Value) -> InputValueResult<Self> {
        if let Value::String(s) = &value {
            let d = NaiveDateTime::parse_from_str(s, "%Y-%m-%d %H:%M:%S")?;
            Ok(Self(d.timestamp()))
        } else {
            Err(InputValueError::expected_type(value))
        }
    }

    fn is_valid(value: &Value) -> bool {
        matches!(value, Value::String(_))
    }

    fn to_value(&self) -> Value {
        Value::String(
            DateTime::<Utc>::from_utc(NaiveDateTime::from_timestamp_opt(self.0, 0).unwrap(), Utc)
                .format("%Y-%m-%d %H:%M:%S")
                .to_string(),
        )
    }
}

#[derive(SimpleObject)]
struct AuthPayload {
    token: String,
    user: User,
}

#[derive(Deserialize, Debug)]
#[serde(untagged)]
enum AccessTokenResp {
    Success {
        access_token: String,
        scope: String,
        token_type: String,
    },

    Fail {
        error: String,
        error_description: String,
        error_uri: String,
    },
}

async fn request_github_token(code: String) -> Result<AccessTokenResp> {
    Ok(reqwest::Client::new()
        .post("https://github.com/login/oauth/access_token")
        .header("Accept", "application/json")
        .json(&serde_json::json!({"client_id": CLIENT_ID, "client_secret": CLIENT_SECRETS, "code": code}))
        .send()
        .await?
        .json::<AccessTokenResp>()
        .await?
    )
}

async fn request_github_user_account(token: &str) -> Result<User> {
    Ok(reqwest::Client::new()
        .get("https://api.github.com/user")
        .header("Accept", "application/json")
        .header("User-Agent", "Awesome-Octocat-App")
        .header("Authorization", format!("Bearer {token}"))
        .send()
        .await?
        .json::<User>()
        .await?)
}

#[cfg(test)]
mod tests {
    use crate::request_github_token;

    #[tokio::test]
    async fn test_request_github_token() {
        request_github_token("aaa".into()).await.unwrap();
    }
}
