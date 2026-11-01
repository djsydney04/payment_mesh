use crate::{AppError, AppResult};
use reqwest::{StatusCode, Url};
use serde_json::{json, Value};
use std::time::Duration;
use zeroize::Zeroizing;

const RESPONSE_LIMIT: usize = 1_048_576;
#[derive(Clone)]
pub struct Plaid {
    client: reqwest::Client,
    client_id: String,
    secret: Zeroizing<String>,
    redirect_uri: String,
    origin: Url,
}
impl Plaid {
    pub fn from_env() -> AppResult<Option<Self>> {
        let client_id = std::env::var("PLAID_CLIENT_ID").unwrap_or_default();
        let secret = std::env::var("PLAID_SECRET").unwrap_or_default();
        let redirect_uri = std::env::var("PLAID_REDIRECT_URI").unwrap_or_default();
        if client_id.is_empty() && secret.is_empty() {
            return Ok(None);
        }
        Self::sandbox(client_id, secret, redirect_uri).map(Some)
    }
    fn sandbox(client_id: String, secret: String, redirect_uri: String) -> AppResult<Self> {
        let redirect = Url::parse(&redirect_uri)
            .map_err(|_| AppError::internal("Invalid Plaid redirect URI"))?;
        if client_id.is_empty()
            || secret.is_empty()
            || redirect.scheme() != "https"
            || redirect.host_str().is_none()
            || !redirect.username().is_empty()
            || redirect.password().is_some()
            || redirect.query().is_some()
            || redirect.fragment().is_some()
        {
            return Err(AppError::internal("Set PLAID_CLIENT_ID, PLAID_SECRET, and a registered HTTPS PLAID_REDIRECT_URI together"));
        }
        Ok(Self {
            client: reqwest::Client::builder()
                .timeout(Duration::from_secs(20))
                .redirect(reqwest::redirect::Policy::none())
                .build()
                .map_err(|_| AppError::internal("Could not initialize Plaid client"))?,
            client_id,
            secret: Zeroizing::new(secret),
            redirect_uri,
            origin: Url::parse("https://sandbox.plaid.com/")
                .map_err(|_| AppError::internal("Invalid Plaid origin"))?,
        })
    }
    #[cfg(test)]
    pub(crate) fn test_client(origin: Url) -> Self {
        let mut client = Self::sandbox(
            "test-client".into(),
            "private-client-secret".into(),
            "https://mesh.example/plaid/".into(),
        )
        .unwrap();
        client.origin = origin;
        client
    }
    async fn call(&self, path: &str, mut body: Value) -> AppResult<Value> {
        if !path.starts_with('/') || path.contains("://") || path.contains("..") {
            return Err(AppError::internal("Invalid Plaid path"));
        }
        body["client_id"] = json!(self.client_id);
        body["secret"] = json!(self.secret.as_str());
        // The only runtime constructor pins Sandbox. Redirects cannot carry credentials elsewhere.
        let mut response = self
            .client
            .post(
                self.origin
                    .join(path)
                    .map_err(|_| AppError::internal("Invalid Plaid path"))?,
            )
            .json(&body)
            .send()
            .await
            .map_err(|_| AppError::upstream("Bank connection timed out; it will retry safely"))?;
        let status = response.status();
        if response
            .content_length()
            .is_some_and(|size| size > RESPONSE_LIMIT as u64)
        {
            return Err(AppError::upstream("Plaid returned an unreadable response"));
        }
        let mut bytes = Zeroizing::new(Vec::new());
        while let Some(chunk) = response.chunk().await.map_err(|_| {
            AppError::upstream("Bank response was interrupted; it will retry safely")
        })? {
            if bytes.len().saturating_add(chunk.len()) > RESPONSE_LIMIT {
                return Err(AppError::upstream("Plaid returned an unreadable response"));
            }
            bytes.extend_from_slice(&chunk);
        }
        let value: Value = serde_json::from_slice(&bytes)
            .map_err(|_| AppError::upstream("Plaid returned an unreadable response"))?;
        if !status.is_success() {
            // These errors require a fresh user Link session. API credentials and rate limits do not.
            if status.is_client_error()
                && status != StatusCode::TOO_MANY_REQUESTS
                && matches!(
                    value["error_code"].as_str(),
                    Some(
                        "INVALID_PUBLIC_TOKEN"
                            | "INVALID_ACCESS_TOKEN"
                            | "ITEM_LOGIN_REQUIRED"
                            | "ITEM_NOT_FOUND"
                    )
                )
            {
                return Err(AppError::bad(
                    "plaid_relink_required",
                    "This bank sign-in expired. Connect with Plaid again",
                ));
            }
            return Err(AppError::upstream(
                "Plaid could not complete this bank connection",
            ));
        }
        Ok(value)
    }
    pub async fn link_token(&self, wallet: &str) -> AppResult<Value> {
        let value=self.call("/link/token/create",json!({"client_name":"Mesh","user":{"client_user_id":wallet},"products":["auth"],"country_codes":["US"],"language":"en","redirect_uri":self.redirect_uri,"account_filters":{"depository":{"account_subtypes":["checking","savings"]}}})).await?;
        let token = value["link_token"]
            .as_str()
            .filter(|token| token.starts_with("link-sandbox-") && token.len() <= 512)
            .ok_or_else(|| AppError::upstream("Plaid did not return a Sandbox Link token"))?;
        Ok(json!({"link_token":token,"mode":"plaid_sandbox"}))
    }
    /// This exchange consumes a one-time public token. Persist the result before any further call.
    pub async fn exchange_public_token(&self, public_token: &str) -> AppResult<Zeroizing<String>> {
        validate_public_token(public_token)?;
        let token = self
            .call(
                "/item/public_token/exchange",
                json!({"public_token":public_token}),
            )
            .await?;
        let access = token["access_token"]
            .as_str()
            .filter(|s| s.starts_with("access-sandbox-") && s.len() <= 2048)
            .ok_or_else(|| AppError::upstream("Plaid did not return a Sandbox access token"))?;
        let item = token["item_id"]
            .as_str()
            .filter(|s| !s.is_empty() && s.len() <= 256)
            .ok_or_else(|| AppError::upstream("Plaid did not return an Item ID"))?;
        Ok(Zeroizing::new(
            json!({"access_token":access,"item_id":item}).to_string(),
        ))
    }
    pub async fn account(&self, credentials: &str, account_id: &str) -> AppResult<LinkedBank> {
        validate_account_id(account_id)?;
        let mut credentials: Value = serde_json::from_str(credentials)?;
        let access = credentials["access_token"]
            .as_str()
            .filter(|s| s.starts_with("access-sandbox-"))
            .ok_or_else(|| AppError::internal("Bank access token missing"))?;
        // Account ownership within this Link Item and its type are fetched server-side.
        let accounts = self
            .call("/accounts/get", json!({"access_token":access}))
            .await?;
        let account = accounts["accounts"]
            .as_array()
            .and_then(|list| {
                list.iter().find(|a| {
                    a["account_id"] == account_id
                        && a["type"] == "depository"
                        && (a["subtype"] == "checking" || a["subtype"] == "savings")
                })
            })
            .ok_or_else(|| {
                AppError::bad(
                    "plaid_relink_required",
                    "Choose a checking or savings account from this Link session",
                )
            })?;
        credentials["account_id"] = json!(account_id);
        Ok(LinkedBank {
            label: account["name"]
                .as_str()
                .unwrap_or("Linked bank")
                .chars()
                .filter(|c| !c.is_control())
                .take(80)
                .collect(),
            mask: account["mask"]
                .as_str()
                .unwrap_or("••••")
                .chars()
                .filter(|c| !c.is_control())
                .take(4)
                .collect(),
            subtype: account["subtype"]
                .as_str()
                .ok_or_else(|| AppError::upstream("Plaid did not return the account type"))?
                .into(),
            credentials: Zeroizing::new(credentials.to_string()),
        })
    }
    pub async fn exchange(&self, public_token: &str, account_id: &str) -> AppResult<LinkedBank> {
        validate_account_id(account_id)?;
        let credentials = self.exchange_public_token(public_token).await?;
        self.account(&credentials, account_id).await
    }
    /// Processor tokens stay server-to-server and are discarded once Dwolla records the exchange.
    pub async fn dwolla_processor_token(&self, bank: &LinkedBank) -> AppResult<Zeroizing<String>> {
        let credentials: Value = serde_json::from_str(&bank.credentials)?;
        let access = credentials["access_token"]
            .as_str()
            .ok_or_else(|| AppError::internal("Bank access token missing"))?;
        let account = credentials["account_id"]
            .as_str()
            .ok_or_else(|| AppError::internal("Bank account missing"))?;
        let response = self
            .call(
                "/processor/token/create",
                json!({"access_token":access,"account_id":account,"processor":"dwolla"}),
            )
            .await?;
        let token = response["processor_token"]
            .as_str()
            .filter(|s| s.starts_with("processor-sandbox-") && s.len() <= 2048)
            .ok_or_else(|| AppError::upstream("Plaid did not return a Sandbox processor token"))?;
        Ok(Zeroizing::new(token.into()))
    }
}
pub fn validate_public_token(token: &str) -> AppResult<()> {
    if !token.starts_with("public-sandbox-")
        || token.len() > 512
        || token.chars().any(char::is_control)
    {
        return Err(AppError::bad(
            "invalid_bank_token",
            "Use a Plaid Sandbox account",
        ));
    }
    Ok(())
}
pub fn validate_account_id(account: &str) -> AppResult<()> {
    if account.is_empty() || account.len() > 256 || account.chars().any(char::is_control) {
        return Err(AppError::bad(
            "invalid_bank_account",
            "Choose a checking or savings account from this Link session",
        ));
    }
    Ok(())
}
pub struct LinkedBank {
    pub label: String,
    pub mask: String,
    pub subtype: String,
    pub credentials: Zeroizing<String>,
}

#[cfg(test)]
pub(crate) mod test_support {
    use super::*;
    use axum::{
        body::{to_bytes, Body},
        extract::{Request, State},
        response::Response,
        routing::post,
        Router,
    };
    use std::{
        collections::VecDeque,
        sync::{Arc, Mutex},
    };
    pub(crate) struct Reply {
        pub status: StatusCode,
        pub body: String,
        pub location: Option<String>,
    }
    #[derive(Default)]
    struct Script {
        replies: VecDeque<Reply>,
        requests: Vec<(String, Value)>,
    }
    pub(crate) struct Mock {
        pub client: Plaid,
        script: Arc<Mutex<Script>>,
        task: tokio::task::JoinHandle<()>,
    }
    impl Mock {
        pub fn reply(&self, status: StatusCode, body: Value) {
            self.raw_reply(status, body.to_string(), None);
        }
        pub fn raw_reply(&self, status: StatusCode, body: String, location: Option<String>) {
            self.script.lock().unwrap().replies.push_back(Reply {
                status,
                body,
                location,
            });
        }
        pub fn requests(&self) -> Vec<(String, Value)> {
            self.script.lock().unwrap().requests.clone()
        }
    }
    impl Drop for Mock {
        fn drop(&mut self) {
            self.task.abort();
        }
    }
    async fn respond(State(script): State<Arc<Mutex<Script>>>, request: Request) -> Response {
        let path = request.uri().path().to_owned();
        let bytes = to_bytes(request.into_body(), 16_384).await.unwrap();
        let body = serde_json::from_slice(&bytes).unwrap();
        let mut script = script.lock().unwrap();
        script.requests.push((path, body));
        let reply = script
            .replies
            .pop_front()
            .expect("unexpected Plaid request");
        let mut response = Response::builder().status(reply.status);
        if let Some(location) = reply.location {
            response = response.header("Location", location);
        }
        response.body(Body::from(reply.body)).unwrap()
    }
    pub async fn mock() -> Mock {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let origin = Url::parse(&format!("http://{}/", listener.local_addr().unwrap())).unwrap();
        let script = Arc::new(Mutex::new(Script::default()));
        let app = Router::new()
            .fallback(post(respond))
            .with_state(script.clone());
        let task = tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });
        Mock {
            client: Plaid::test_client(origin),
            script,
            task,
        }
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    use test_support::mock;
    #[tokio::test]
    async fn token_account_and_processor_requests_use_exact_selected_account() {
        let mock = mock().await;
        mock.reply(
            StatusCode::OK,
            json!({"access_token":"access-sandbox-private","item_id":"item-123"}),
        );
        mock.reply(StatusCode::OK,json!({"accounts":[{"account_id":"selected","type":"depository","subtype":"checking","name":"Test checking","mask":"1234"},{"account_id":"other","type":"credit","subtype":"credit card"}]}));
        mock.reply(
            StatusCode::OK,
            json!({"processor_token":"processor-sandbox-private"}),
        );
        let bank = mock
            .client
            .exchange("public-sandbox-private", "selected")
            .await
            .unwrap();
        assert_eq!(bank.label, "Test checking");
        assert_eq!(bank.mask, "1234");
        assert_eq!(
            mock.client
                .dwolla_processor_token(&bank)
                .await
                .unwrap()
                .as_str(),
            "processor-sandbox-private"
        );
        let requests = mock.requests();
        assert_eq!(
            requests.iter().map(|r| r.0.as_str()).collect::<Vec<_>>(),
            [
                "/item/public_token/exchange",
                "/accounts/get",
                "/processor/token/create"
            ]
        );
        assert_eq!(requests[2].1["account_id"], "selected");
        assert_eq!(requests[2].1["processor"], "dwolla");
        assert_eq!(requests[0].1["client_id"], "test-client");
    }
    #[tokio::test]
    async fn selected_account_cannot_escape_the_link_item_or_be_a_credit_account() {
        let mock = mock().await;
        for selected in ["absent", "credit"] {
            mock.reply(StatusCode::OK,json!({"accounts":[{"account_id":"credit","type":"credit","subtype":"credit card"}]}));
            assert_eq!(
                mock.client
                    .account("{\"access_token\":\"access-sandbox-private\"}", selected)
                    .await
                    .unwrap_err_code(),
                "plaid_relink_required"
            );
        }
    }
    #[tokio::test]
    async fn only_expired_user_sessions_request_relinking_and_provider_errors_are_private() {
        let mock = mock().await;
        for (status, code, expected) in [
            (
                StatusCode::BAD_REQUEST,
                "INVALID_PUBLIC_TOKEN",
                "plaid_relink_required",
            ),
            (
                StatusCode::BAD_REQUEST,
                "INVALID_API_KEYS",
                "bank_unavailable",
            ),
            (
                StatusCode::TOO_MANY_REQUESTS,
                "INVALID_PUBLIC_TOKEN",
                "bank_unavailable",
            ),
        ] {
            mock.reply(
                status,
                json!({"error_code":code,"error_message":"private customer details"}),
            );
            let error = mock
                .client
                .exchange_public_token("public-sandbox-private")
                .await
                .unwrap_err();
            assert_eq!(error.code, expected);
            assert!(!error.message.contains("private customer details"));
        }
    }
    #[tokio::test]
    async fn redirects_are_not_followed_and_response_size_is_bounded() {
        let mock = mock().await;
        mock.raw_reply(
            StatusCode::TEMPORARY_REDIRECT,
            "{}".into(),
            Some("https://example.com/collect".into()),
        );
        assert!(mock
            .client
            .exchange_public_token("public-sandbox-private")
            .await
            .is_err());
        assert_eq!(mock.requests().len(), 1);
        mock.raw_reply(StatusCode::OK, "x".repeat(RESPONSE_LIMIT + 1), None);
        assert!(mock
            .client
            .exchange_public_token("public-sandbox-private")
            .await
            .is_err());
        assert_eq!(mock.requests().len(), 2);
    }
    #[tokio::test]
    async fn link_token_uses_auth_us_and_registered_redirect() {
        let mock = mock().await;
        mock.reply(StatusCode::OK, json!({"link_token":"link-sandbox-test"}));
        assert_eq!(
            mock.client.link_token("wallet-public-key").await.unwrap()["link_token"],
            "link-sandbox-test"
        );
        let request = &mock.requests()[0].1;
        assert_eq!(request["products"], json!(["auth"]));
        assert_eq!(request["country_codes"], json!(["US"]));
        assert_eq!(request["redirect_uri"], "https://mesh.example/plaid/");
        assert_eq!(
            request["account_filters"]["depository"]["account_subtypes"],
            json!(["checking", "savings"])
        );
    }
    #[test]
    fn configuration_and_tokens_cannot_target_production_or_invalid_redirects() {
        for redirect in [
            "http://example.com/plaid/",
            "https://user:pass@example.com/plaid/",
            "https://example.com/plaid/?token=x",
            "https://example.com/plaid/#fragment",
        ] {
            assert!(Plaid::sandbox("id".into(), "secret".into(), redirect.into()).is_err());
        }
        assert!(validate_public_token("public-production-token").is_err());
        assert!(validate_account_id("").is_err());
    }
    trait ErrorCode {
        fn unwrap_err_code(self) -> &'static str;
    }
    impl<T> ErrorCode for AppResult<T> {
        fn unwrap_err_code(self) -> &'static str {
            match self {
                Ok(_) => panic!("expected failure"),
                Err(e) => e.code,
            }
        }
    }
}
