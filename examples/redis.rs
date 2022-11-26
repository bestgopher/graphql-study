use redis::AsyncCommands;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let client = redis::Client::open("redis://127.0.0.1")?;
    let mut conn = client.get_async_connection().await?;
    redis::cmd("set")
        .arg("my_key")
        .arg(42)
        .query_async(&mut conn)
        .await?;

    let res: i32 = conn.get("my_key").await?;
    println!("{:?}", res);

    Ok(())
}
