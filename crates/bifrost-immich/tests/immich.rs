use bifrost_common::RemotePath;
use bifrost_immich::{
    ImmichConfig, ImmichCredentials, ImmichProvider, ALBUMS_DIRECTORY, PHOTOS_DIRECTORY,
};
use bifrost_storage::StorageProvider;
use reqwest::{Client, StatusCode};
use serde::Deserialize;
use std::env;
use tokio::time::{sleep, Duration, Instant};
use url::Url;

const ADMIN_EMAIL: &str = "bifrost-test@immich.local";
const ADMIN_PASSWORD: &str = "bifrost-test-password";
const ADMIN_NAME: &str = "Bifrost Test";

#[derive(Debug, Deserialize)]
struct LoginResponse {
    #[serde(rename = "accessToken")]
    access_token: String,
}

#[derive(Debug, Deserialize)]
struct ApiKeyResponse {
    secret: String,
}

fn integration_endpoint() -> Option<String> {
    env::var_os("BIFROST_IMMICH_INTEGRATION")?;
    Some(env::var("BIFROST_IMMICH_ENDPOINT").unwrap_or_else(|_| "http://127.0.0.1:2283".to_owned()))
}

async fn wait_for_server(client: &Client, endpoint: &str) {
    let deadline = Instant::now() + Duration::from_secs(120);
    let url = format!("{endpoint}/api/server/config");
    loop {
        if let Ok(response) = client.get(&url).send().await {
            if response.status().is_success() {
                return;
            }
        }
        assert!(
            Instant::now() < deadline,
            "Immich server did not become ready"
        );
        sleep(Duration::from_secs(2)).await;
    }
}

async fn bootstrap_api_key(client: &Client, endpoint: &str) -> String {
    let signup = client
        .post(format!("{endpoint}/api/auth/admin-sign-up"))
        .json(&serde_json::json!({
            "email": ADMIN_EMAIL,
            "password": ADMIN_PASSWORD,
            "name": ADMIN_NAME,
        }))
        .send()
        .await
        .unwrap();
    assert_eq!(
        signup.status(),
        StatusCode::CREATED,
        "Immich admin signup failed"
    );

    let login = client
        .post(format!("{endpoint}/api/auth/login"))
        .json(&serde_json::json!({
            "email": ADMIN_EMAIL,
            "password": ADMIN_PASSWORD,
        }))
        .send()
        .await
        .unwrap();
    assert_eq!(login.status(), StatusCode::CREATED, "Immich login failed");
    let login = login.json::<LoginResponse>().await.unwrap();

    let api_key = client
        .post(format!("{endpoint}/api/api-keys"))
        .bearer_auth(login.access_token)
        .json(&serde_json::json!({
            "name": "Bifrost integration",
            "permissions": ["all"],
        }))
        .send()
        .await
        .unwrap();
    assert_eq!(
        api_key.status(),
        StatusCode::CREATED,
        "Immich API key creation failed"
    );
    api_key.json::<ApiKeyResponse>().await.unwrap().secret
}

#[tokio::test]
#[ignore = "requires task test:immich and Docker"]
async fn immich_docker_provider_contract() {
    let Some(endpoint) = integration_endpoint() else {
        return;
    };
    let endpoint_url = Url::parse(&endpoint).unwrap();
    let client = Client::new();
    wait_for_server(&client, endpoint.trim_end_matches('/')).await;
    let api_key = bootstrap_api_key(&client, endpoint.trim_end_matches('/')).await;
    let provider = ImmichProvider::connect_with_credentials(
        ImmichConfig {
            endpoint: endpoint_url,
        },
        ImmichCredentials::ApiKey(api_key),
    )
    .await
    .unwrap();

    provider.test_connection().await.unwrap();
    let root = provider.list(&RemotePath::root(), None).await.unwrap();
    assert_eq!(root.entries.len(), 2);
    assert!(root
        .entries
        .iter()
        .any(|entry| entry.metadata.path == RemotePath::parse(PHOTOS_DIRECTORY).unwrap()));
    assert!(root
        .entries
        .iter()
        .any(|entry| entry.metadata.path == RemotePath::parse(ALBUMS_DIRECTORY).unwrap()));
    provider
        .list(&RemotePath::parse(PHOTOS_DIRECTORY).unwrap(), None)
        .await
        .unwrap();
    provider
        .list(&RemotePath::parse(ALBUMS_DIRECTORY).unwrap(), None)
        .await
        .unwrap();
    assert!(provider.capacity().await.unwrap().is_some());
}
