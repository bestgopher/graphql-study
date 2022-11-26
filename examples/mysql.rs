use sqlx::MySqlPool;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let _pool = MySqlPool::connect("mysql://root:123456@localhost").await?;

    Ok(())
}
