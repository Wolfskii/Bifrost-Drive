use bifrost_db::Database;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let database_url = std::env::var("BIFROST_DATABASE_URL")
        .unwrap_or_else(|_| "sqlite://bifrost-drive.db".to_owned());
    let database = Database::connect(&database_url).await?;
    database.migrate().await?;
    println!("Database migrations applied");
    Ok(())
}
