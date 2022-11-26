use std::{
    num::ParseIntError,
    time::{Duration, SystemTime},
};

use async_graphql::{futures_util::StreamExt, CustomValidator, Guard, InputType};
use async_graphql::{
    ComplexObject, Context, Enum, ErrorExtensions, InputObject, Object, OutputType, Result, Schema,
    SimpleObject, Subscription,
};
use futures_core::stream::Stream;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let schema = Schema::build(Query, Mutation, Subscription)
        .data(String::from("aaaaa"))
        .finish();

    println!(
        "{}",
        serde_json::to_string(&schema.execute("query {add(a: 1, b: 2)}").await)?
    );

    println!(
        "{}",
        serde_json::to_string(&schema.execute("query {borrowFromContextData}").await)?
    );

    println!(
        "{}",
        serde_json::to_string(
            &schema
                .execute(r#"query {parseWithExtensions(input:"123")}"#)
                .await
        )?
    );

    tokio::join!(
        async {
            let now = SystemTime::now();
            let _ = &schema.execute(r#"mutation { signup signup }"#).await;
            println!("1: {:?}", now.elapsed());
        },
        async {
            let now = SystemTime::now();
            let _ = &schema.execute(r#"mutation { signup signup}"#).await;
            println!("1: {:?}", now.elapsed());
        },
        // async {
        //     let mut response = schema.execute_stream(r#"subscription { integers(step:100) }"#);

        //     // tokio::pin!(response);

        //     while let Some(i) = response.next().await {
        //         println!("{:?}", i);
        //     }
        // }
    );

    println!("{}", schema.sdl());

    Ok(())
}

struct Query;

#[Object]
impl Query {
    /// Returns the sum of a and b
    async fn add(&self, a: i32, b: i32) -> i32 {
        a + b
    }

    async fn borrow_from_context_data<'a>(&self, ctx: &Context<'a>) -> Result<&'a String> {
        ctx.data::<String>()
    }

    async fn parse_with_extensions(&self, input: String) -> Result<i32> {
        Ok(input
            .parse()
            .map_err(|err: ParseIntError| err.extend_with(|_, e| e.set("code", 400)))?)
    }
}

#[derive(SimpleObject)]
struct MyOject {
    /// Value a
    #[graphql(guard = "RoleGuard(Role::Admin)")]
    a: i32,

    /// Value b
    #[graphql(validator(custom = "MyValidator::new(3i32)"))]
    b: i32,

    #[graphql(skip)]
    c: i32,
}

#[derive(SimpleObject, InputObject)]
#[graphql(input_name = "MyObjInput")]
#[graphql(complex)]
struct MyObj {
    a: i32,
    b: i32,
}

#[ComplexObject]
impl MyObj {
    async fn c(&self) -> i32 {
        self.a + self.b
    }
}

#[derive(SimpleObject)]
#[graphql(concrete(name = "SomeName", params(i32)))]
#[graphql(concrete(name = "SomeNameOther", params(String)))]
pub struct SomeGenericObject<T: OutputType> {
    field1: Option<T>,
    field2: String,
}

#[derive(SimpleObject)]
pub struct YetAnotherObject {
    a: SomeGenericObject<i32>,
    b: SomeGenericObject<i32>,
}

/// One of the films in the Star Wars Trilogy
#[derive(Enum, Copy, Clone, Eq, PartialEq)]
pub enum Episode {
    /// Released in 1977.
    NewHope,

    /// Released in 1980.
    Empire,

    /// Released in 1983.
    #[graphql(name = "AAA")]
    Jedi,
}

struct Mutation;

#[Object]
impl Mutation {
    async fn signup(
        &self,
        #[graphql(default)] _username: String,
        #[graphql(default)] _password: String,
    ) -> Result<bool> {
        tokio::time::sleep(Duration::from_secs(3)).await;
        Ok(true)
    }

    async fn login(
        &self,
        #[graphql(default)] _username: String,
        #[graphql(default)] _password: String,
    ) -> Result<String> {
        Ok(Default::default())
    }
}

struct Subscription;

#[Subscription]
impl Subscription {
    async fn integers(&self, #[graphql(default = 1)] step: i32) -> impl Stream<Item = i32> {
        let mut value = 0;
        tokio_stream::wrappers::IntervalStream::new(tokio::time::interval(Duration::from_secs(1)))
            .map(move |_| {
                println!("step: {value}");
                value += step;
                value
            })
    }
}

#[derive(PartialEq, Eq)]
enum Role {
    Admin,
    Guest,
}

struct RoleGuard(Role);

#[async_trait::async_trait]
impl Guard for RoleGuard {
    async fn check(&self, ctx: &Context<'_>) -> Result<()> {
        match ctx.data::<RoleGuard>() {
            Ok(Self(i)) if i == &self.0 => Ok(()),
            _ => Err("a".into()),
        }
    }
}

struct MyValidator<T> {
    value: T,
}

impl<T> MyValidator<T> {
    pub fn new(value: T) -> Self {
        Self { value }
    }
}

impl<T> CustomValidator<T> for MyValidator<T>
where
    T: PartialEq + Eq + InputType,
{
    fn check(&self, value: &T) -> Result<(), String> {
        if value.eq(&self.value) {
            Ok(())
        } else {
            Err(Default::default())
        }
    }
}
