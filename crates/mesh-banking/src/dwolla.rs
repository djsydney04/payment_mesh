use crate::*;
use reqwest::{Method, Response, StatusCode, Url};
use serde_json::{json, Value};
use std::{
    sync::Arc,
    time::{Duration, Instant},
};
use tokio::sync::Mutex;
use zeroize::Zeroizing;

const HAL: &str = "application/vnd.dwolla.v1.hal+json";
struct Token {
    value: Zeroizing<String>,
    expires: Instant,
}

#[derive(Clone)]
pub struct Dwolla {
    client: reqwest::Client,
    key: String,
    secret: Zeroizing<String>,
    origin: Url,
    token: Arc<Mutex<Option<Token>>>,
}

impl Dwolla {
    pub fn sandbox(key: String, secret: String) -> Result<Self> {
        if key.is_empty() || secret.is_empty() {
            return Err(Error::Invalid("Set both Dwolla Sandbox credentials"));
        }
        Ok(Self {
            client: reqwest::Client::builder()
                .timeout(Duration::from_secs(20))
                .redirect(reqwest::redirect::Policy::none())
                .build()
                .map_err(|_| Error::Protocol)?,
            key,
            secret: Zeroizing::new(secret),
            origin: Url::parse(DWOLLA_SANDBOX).map_err(|_| Error::Protocol)?,
            token: Arc::new(Mutex::new(None)),
        })
    }
    pub fn from_env() -> Result<Option<Self>> {
        let key = std::env::var("DWOLLA_KEY").unwrap_or_default();
        let secret = std::env::var("DWOLLA_SECRET").unwrap_or_default();
        if let Ok(mode) = std::env::var("DWOLLA_ENV") {
            if mode != "sandbox" {
                return Err(Error::Invalid("This build supports Dwolla Sandbox only"));
            }
        }
        if key.is_empty() && secret.is_empty() {
            return Ok(None);
        }
        Self::sandbox(key, secret).map(Some)
    }
    pub async fn verify_connection(&self) -> Result<()> {
        let value = read_json(self.call(Method::GET, "", None, None).await?).await?;
        if !value["_links"].is_object() {
            return Err(Error::Protocol);
        }
        Ok(())
    }
    async fn token(&self) -> Result<Zeroizing<String>> {
        // Coalesce concurrent refreshes without holding a synchronous/database mutex.
        let mut slot = self.token.lock().await;
        if let Some(token) = slot
            .as_ref()
            .filter(|t| t.expires > Instant::now() + Duration::from_secs(60))
        {
            return Ok(token.value.clone());
        }
        let response = self
            .client
            .post(self.url("token")?)
            .basic_auth(&self.key, Some(self.secret.as_str()))
            .form(&[("grant_type", "client_credentials")])
            .send()
            .await
            .map_err(|_| Error::Retryable)?;
        let status = response.status();
        if !status.is_success() {
            return Err(
                if status.is_server_error() || status == StatusCode::TOO_MANY_REQUESTS {
                    Error::Retryable
                } else {
                    Error::Authentication
                },
            );
        }
        let value = read_json(response).await?;
        let token = value["access_token"]
            .as_str()
            .filter(|v| !v.is_empty() && v.len() <= 4096)
            .ok_or(Error::Protocol)?;
        if !value["token_type"]
            .as_str()
            .is_some_and(|v| v.eq_ignore_ascii_case("bearer"))
        {
            return Err(Error::Protocol);
        }
        let lifetime = value["expires_in"]
            .as_u64()
            .filter(|n| (61..=86400).contains(n))
            .ok_or(Error::Protocol)?;
        let token = Zeroizing::new(token.to_owned());
        *slot = Some(Token {
            value: token.clone(),
            expires: Instant::now() + Duration::from_secs(lifetime),
        });
        Ok(token)
    }
    fn url(&self, path: &str) -> Result<Url> {
        if path.starts_with('/') || path.contains("://") || path.contains("..") {
            return Err(Error::Protocol);
        }
        self.origin.join(path).map_err(|_| Error::Protocol)
    }
    pub fn resource_id(&self, href: &str, collection: &str) -> Result<Uuid> {
        let url = Url::parse(href).map_err(|_| Error::Protocol)?;
        if url.origin() != self.origin.origin()
            || url.query().is_some()
            || url.fragment().is_some()
            || !url.username().is_empty()
            || url.password().is_some()
        {
            return Err(Error::Protocol);
        }
        let mut segments = url.path_segments().ok_or(Error::Protocol)?;
        if segments.next() != Some(collection) {
            return Err(Error::Protocol);
        }
        let id = segments.next().ok_or(Error::Protocol)?;
        if segments.next().is_some() {
            return Err(Error::Protocol);
        }
        Uuid::parse_str(id)
            .ok()
            .filter(|id| !id.is_nil())
            .ok_or(Error::Protocol)
    }
    async fn call(
        &self,
        method: Method,
        path: &str,
        body: Option<&Value>,
        key: Option<Uuid>,
    ) -> Result<Response> {
        let token = self.token().await?;
        let mut request = self
            .client
            .request(method, self.url(path)?)
            .bearer_auth(token.as_str())
            .header("Accept", HAL);
        if let Some(body) = body {
            request = request.header("Content-Type", HAL).json(body);
        }
        if let Some(key) = key {
            request = request.header("Idempotency-Key", key.to_string());
        }
        let response = request.send().await.map_err(|_| Error::Retryable)?;
        let status = response.status();
        if status.is_success() {
            return Ok(response);
        }
        if status == StatusCode::UNAUTHORIZED {
            *self.token.lock().await = None;
            return Err(Error::Authentication);
        }
        if status == StatusCode::FORBIDDEN {
            return Err(Error::Authentication);
        }
        if status.is_server_error()
            || status == StatusCode::TOO_MANY_REQUESTS
            || status == StatusCode::CONFLICT
            || status == StatusCode::REQUEST_TIMEOUT
        {
            return Err(Error::Retryable);
        }
        if status.is_client_error() {
            return Err(Error::Rejected);
        }
        // Redirects are never followed with credentials or treated as a declined transfer.
        Err(Error::Protocol)
    }
    async fn create(&self, path: &str, body: &Value, id: Uuid, collection: &str) -> Result<Uuid> {
        let response = self.call(Method::POST, path, Some(body), Some(id)).await?;
        if response.status() != StatusCode::CREATED {
            return Err(Error::Protocol);
        }
        self.resource_id(
            response
                .headers()
                .get("Location")
                .and_then(|v| v.to_str().ok())
                .ok_or(Error::Protocol)?,
            collection,
        )
    }
    pub async fn customer(&self, id: Uuid) -> Result<CustomerState> {
        let value = read_json(
            self.call(Method::GET, &format!("customers/{id}"), None, None)
                .await?,
        )
        .await?;
        if Uuid::parse_str(value["id"].as_str().ok_or(Error::Protocol)?)
            .map_err(|_| Error::Protocol)?
            != id
        {
            return Err(Error::Protocol);
        }
        let status = value["status"].as_str().ok_or(Error::Protocol)?.to_owned();
        if ![
            "verified",
            "unverified",
            "retry",
            "document",
            "kba",
            "suspended",
            "deactivated",
        ]
        .contains(&status.as_str())
        {
            return Err(Error::Protocol);
        }
        Ok(CustomerState { id, status })
    }
    pub async fn create_customer(
        &self,
        profile: &PersonalCustomer,
        correlation: Uuid,
    ) -> Result<Uuid> {
        profile.validate()?;
        let mut body = serde_json::to_value(profile)?;
        body["type"] = json!("personal");
        body["correlationId"] = json!(correlation.to_string());
        self.create("customers", &body, correlation, "customers")
            .await
    }
    pub async fn balance_source(&self, customer: Uuid) -> Result<Uuid> {
        let body = read_json(
            self.call(
                Method::GET,
                &format!("customers/{customer}/funding-sources?removed=false"),
                None,
                None,
            )
            .await?,
        )
        .await?;
        let balances: Vec<_> = body["_embedded"]["funding-sources"]
            .as_array()
            .ok_or(Error::Protocol)?
            .iter()
            .filter(|v| v["type"] == "balance" && v["removed"] != true)
            .collect();
        if balances.len() != 1 {
            return Err(Error::Protocol);
        }
        self.resource_id(
            balances[0]["_links"]["self"]["href"]
                .as_str()
                .ok_or(Error::Protocol)?,
            "funding-sources",
        )
    }
    pub async fn available_balance(&self, source: Uuid) -> Result<i64> {
        let body = read_json(
            self.call(
                Method::GET,
                &format!("funding-sources/{source}/balance"),
                None,
                None,
            )
            .await?,
        )
        .await?;
        if !is_usd(&body["balance"]["currency"]) {
            return Err(Error::Protocol);
        }
        provider_balance_cents(body["balance"]["value"].as_str().ok_or(Error::Protocol)?)
    }
    /// Membership is checked on the customer's resource, rather than trusting a supplied URL.
    pub async fn verified_bank(&self, customer: Uuid, source: Uuid) -> Result<bool> {
        let body = read_json(
            self.call(
                Method::GET,
                &format!("customers/{customer}/funding-sources?removed=false"),
                None,
                None,
            )
            .await?,
        )
        .await?;
        let sources = body["_embedded"]["funding-sources"]
            .as_array()
            .ok_or(Error::Protocol)?;
        for item in sources {
            let id = self.resource_id(
                item["_links"]["self"]["href"]
                    .as_str()
                    .ok_or(Error::Protocol)?,
                "funding-sources",
            )?;
            if id == source {
                return Ok(item["type"] == "bank"
                    && item["status"] == "verified"
                    && item["removed"] == false);
            }
        }
        Ok(false)
    }
    pub async fn plaid_partner(&self) -> Result<Uuid> {
        let body = read_json(
            self.call(Method::GET, "exchange-partners", None, None)
                .await?,
        )
        .await?;
        let partner = body["_embedded"]["exchange-partners"]
            .as_array()
            .and_then(|list| list.iter().find(|v| v["name"] == "Plaid"))
            .ok_or(Error::Protocol)?;
        self.resource_id(
            partner["_links"]["self"]["href"]
                .as_str()
                .ok_or(Error::Protocol)?,
            "exchange-partners",
        )
    }
    pub async fn create_exchange(
        &self,
        customer: Uuid,
        partner: Uuid,
        processor_token: &str,
        id: Uuid,
    ) -> Result<Uuid> {
        if !processor_token.starts_with("processor-sandbox-") || processor_token.len() > 2048 {
            return Err(Error::Invalid("Use a Plaid Sandbox processor token"));
        }
        let body = json!({"_links":{"exchange-partner":{"href":self.url(&format!("exchange-partners/{partner}"))?.as_str()}},"token":processor_token});
        self.create(
            &format!("customers/{customer}/exchanges"),
            &body,
            id,
            "exchanges",
        )
        .await
    }
    pub async fn create_bank(
        &self,
        customer: Uuid,
        exchange: Uuid,
        label: &str,
        subtype: &str,
        id: Uuid,
    ) -> Result<Uuid> {
        if !["checking", "savings"].contains(&subtype)
            || label.is_empty()
            || label.chars().count() > 80
            || label.chars().any(char::is_control)
        {
            return Err(Error::Invalid("Invalid bank account"));
        }
        let body = json!({"_links":{"exchange":{"href":self.url(&format!("exchanges/{exchange}"))?.as_str()}},"bankAccountType":subtype,"name":label});
        self.create(
            &format!("customers/{customer}/funding-sources"),
            &body,
            id,
            "funding-sources",
        )
        .await
    }
}

impl PaymentRail for Dwolla {
    async fn submit(&self, operation: &BankOperation) -> Result<Uuid> {
        operation.validate()?;
        if operation
            .submit_before
            .is_some_and(|deadline| deadline <= mesh_core::now())
        {
            return Err(Error::Expired);
        }
        let _ = self.token().await?;
        if operation
            .submit_before
            .is_some_and(|deadline| deadline <= mesh_core::now())
        {
            return Err(Error::Expired);
        }
        let body = json!({"_links":{"source":{"href":self.url(&format!("funding-sources/{}",operation.source))?.as_str()},"destination":{"href":self.url(&format!("funding-sources/{}",operation.destination))?.as_str()}},"amount":{"currency":"USD","value":decimal_amount(operation.amount_cents)?},"correlationId":operation.id.to_string()});
        self.create("transfers", &body, operation.id, "transfers")
            .await
    }
    async fn observe(&self, transfer: Uuid) -> Result<TransferObservation> {
        tokio::time::timeout(Duration::from_secs(30), self.observe_chain(transfer))
            .await
            .map_err(|_| Error::Retryable)?
    }
}

fn is_usd(value: &Value) -> bool {
    value
        .as_str()
        .is_some_and(|s| s.eq_ignore_ascii_case("USD"))
}

async fn read_json(mut response: Response) -> Result<Value> {
    if response.content_length().is_some_and(|n| n > 1_048_576) {
        return Err(Error::Protocol);
    }
    let mut bytes = Vec::new();
    while let Some(chunk) = response.chunk().await.map_err(|_| Error::Retryable)? {
        if bytes.len().saturating_add(chunk.len()) > 1_048_576 {
            return Err(Error::Protocol);
        }
        bytes.extend_from_slice(&chunk);
    }
    serde_json::from_slice(&bytes).map_err(|_| Error::Protocol)
}

pub struct CustomerState {
    pub id: Uuid,
    pub status: String,
}

impl Dwolla {
    fn funding_link(&self, value: &Value, side: &str) -> Result<Uuid> {
        let specific = format!("{side}-funding-source");
        let link = value["_links"]
            .get(&specific)
            .unwrap_or(&value["_links"][side]);
        self.resource_id(
            link["href"].as_str().ok_or(Error::Protocol)?,
            "funding-sources",
        )
    }
    async fn observe_chain(&self, transfer: Uuid) -> Result<TransferObservation> {
        use std::collections::{HashSet, VecDeque};
        let mut queue = VecDeque::from([transfer]);
        let mut seen = HashSet::new();
        let mut root: Option<TransferObservation> = None;
        let mut statuses = vec![];
        let mut related = vec![];
        while let Some(id) = queue.pop_front() {
            if !seen.insert(id) {
                continue;
            }
            if seen.len() > 8 {
                return Err(Error::Protocol);
            }
            let value = read_json(
                self.call(Method::GET, &format!("transfers/{id}"), None, None)
                    .await?,
            )
            .await?;
            if Uuid::parse_str(value["id"].as_str().ok_or(Error::Protocol)?)
                .map_err(|_| Error::Protocol)?
                != id
                || !is_usd(&value["amount"]["currency"])
            {
                return Err(Error::Protocol);
            }
            let amount_cents =
                provider_cents(value["amount"]["value"].as_str().ok_or(Error::Protocol)?)?;
            let correlation_id =
                Uuid::parse_str(value["correlationId"].as_str().ok_or(Error::Protocol)?)
                    .map_err(|_| Error::Protocol)?;
            let status = match value["status"].as_str() {
                Some("pending") => TransferStatus::Pending,
                Some("processed") => TransferStatus::Processed,
                Some("failed") => TransferStatus::Failed,
                Some("cancelled") => TransferStatus::Cancelled,
                Some("returned") => TransferStatus::Returned,
                _ => return Err(Error::Protocol),
            };
            if let Some(root) = root.as_ref() {
                if amount_cents != root.amount_cents || correlation_id != root.correlation_id {
                    return Err(Error::Protocol);
                }
                related.push(id);
            } else {
                root = Some(TransferObservation {
                    id,
                    source: self.funding_link(&value, "source")?,
                    destination: self.funding_link(&value, "destination")?,
                    amount_cents,
                    correlation_id,
                    status,
                    related_transfers: vec![],
                });
            }
            statuses.push(status);
            for relation in ["funded-transfer", "funding-transfer"] {
                if let Some(link) = value["_links"].get(relation) {
                    queue.push_back(
                        self.resource_id(
                            link["href"].as_str().ok_or(Error::Protocol)?,
                            "transfers",
                        )?,
                    );
                }
            }
        }
        let mut root = root.ok_or(Error::Protocol)?;
        root.status = [
            TransferStatus::Returned,
            TransferStatus::Failed,
            TransferStatus::Cancelled,
            TransferStatus::Pending,
        ]
        .into_iter()
        .find(|s| statuses.contains(s))
        .unwrap_or(TransferStatus::Processed);
        root.related_transfers = related;
        Ok(root)
    }
}

/// Do not log, persist unencrypted, or send these fields over the mesh.
#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PersonalCustomer {
    pub first_name: String,
    pub last_name: String,
    pub email: String,
    pub address1: String,
    pub city: String,
    pub state: String,
    pub postal_code: String,
    pub date_of_birth: String,
    pub ssn: Zeroizing<String>,
}
impl PersonalCustomer {
    pub fn validate(&self) -> Result<()> {
        for field in [
            &self.first_name,
            &self.last_name,
            &self.address1,
            &self.city,
        ] {
            if field.is_empty()
                || field.chars().count() > 50
                || field
                    .chars()
                    .any(|c| c.is_control() || "<>==\"`!?%~${}[]\\".contains(c))
            {
                return Err(Error::Invalid("Invalid customer details"));
            }
        }
        if self.email.len() > 254
            || !self.email.contains('@')
            || self.email.chars().any(char::is_control)
            || self.state.len() != 2
            || !self.state.bytes().all(|b| b.is_ascii_uppercase())
            || self.postal_code.len() != 5
            || !self.postal_code.bytes().all(|b| b.is_ascii_digit())
            || self.ssn.len() != 4
            || !self.ssn.bytes().all(|b| b.is_ascii_digit())
        {
            return Err(Error::Invalid("Invalid customer details"));
        }
        if self.date_of_birth.len() != 10
            || self.date_of_birth.as_bytes()[4] != b'-'
            || self.date_of_birth.as_bytes()[7] != b'-'
            || !self
                .date_of_birth
                .bytes()
                .enumerate()
                .all(|(i, b)| i == 4 || i == 7 || b.is_ascii_digit())
        {
            return Err(Error::Invalid("Use a birth date in YYYY-MM-DD format"));
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::{
        body::{to_bytes, Body},
        extract::{Request, State},
        http::HeaderMap,
        response::Response,
        routing::any,
        Router,
    };
    use std::collections::VecDeque;

    struct Reply {
        status: StatusCode,
        body: Value,
        location: Option<String>,
    }
    struct Captured {
        method: Method,
        path: String,
        headers: HeaderMap,
        body: Vec<u8>,
    }
    #[derive(Default)]
    struct Script {
        replies: VecDeque<Reply>,
        requests: Vec<Captured>,
    }
    struct Mock {
        client: Dwolla,
        script: Arc<Mutex<Script>>,
        task: tokio::task::JoinHandle<()>,
    }
    impl Drop for Mock {
        fn drop(&mut self) {
            self.task.abort();
        }
    }
    async fn respond(State(script): State<Arc<Mutex<Script>>>, request: Request) -> Response {
        let (parts, body) = request.into_parts();
        let bytes = to_bytes(body, 32_768).await.unwrap();
        let mut state = script.lock().await;
        state.requests.push(Captured {
            method: parts.method,
            path: parts.uri.to_string(),
            headers: parts.headers,
            body: bytes.to_vec(),
        });
        let reply = state
            .replies
            .pop_front()
            .expect("unexpected provider request");
        let mut response = Response::builder()
            .status(reply.status)
            .header("Content-Type", HAL);
        if let Some(location) = reply.location {
            response = response.header("Location", location);
        }
        response.body(Body::from(reply.body.to_string())).unwrap()
    }
    async fn mock() -> Mock {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let origin = Url::parse(&format!("http://{}/", listener.local_addr().unwrap())).unwrap();
        let script = Arc::new(Mutex::new(Script::default()));
        let app = Router::new()
            .fallback(any(respond))
            .with_state(script.clone());
        let task = tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });
        let mut client =
            Dwolla::sandbox("test-client-id".into(), "test-client-secret".into()).unwrap();
        client.origin = origin;
        Mock {
            client,
            script,
            task,
        }
    }
    fn token() -> Reply {
        Reply {
            status: StatusCode::OK,
            body: json!({"access_token":"test-oauth-token","token_type":"bearer","expires_in":3600}),
            location: None,
        }
    }
    fn ok(body: Value) -> Reply {
        Reply {
            status: StatusCode::OK,
            body,
            location: None,
        }
    }
    fn op() -> BankOperation {
        BankOperation {
            id: Uuid::new_v4(),
            ledger_id: Uuid::new_v4(),
            wallet: "01".repeat(32),
            kind: OperationKind::CashOut,
            source: Uuid::new_v4(),
            destination: Uuid::new_v4(),
            amount_cents: 1250,
            submit_before: None,
        }
    }

    #[tokio::test]
    async fn actual_http_uses_oauth_and_exact_idempotent_usd_transfers() {
        let mock = mock().await;
        let op = op();
        let transfer = Uuid::new_v4();
        let origin = mock.client.origin.as_str().trim_end_matches('/');
        mock.script.lock().await.replies.extend([token(),Reply {status:StatusCode::CREATED,body:json!({}),location:Some(format!("{origin}/transfers/{transfer}"))},ok(json!({"id":transfer,"status":"processed","amount":{"currency":"USD","value":"12.50"},"correlationId":op.id,"_links":{"source":{"href":format!("{origin}/funding-sources/{}",op.source)},"destination":{"href":format!("{origin}/funding-sources/{}",op.destination)}}}))]);
        assert_eq!(mock.client.submit(&op).await.unwrap(), transfer);
        let observation = mock.client.observe(transfer).await.unwrap();
        assert!(observation.matches(&op));
        assert_eq!(observation.status, TransferStatus::Processed);
        let script = mock.script.lock().await;
        assert_eq!(script.requests.len(), 3);
        assert_eq!(script.requests[0].path, "/token");
        assert_eq!(
            script.requests[0].headers["content-type"],
            "application/x-www-form-urlencoded"
        );
        assert_eq!(script.requests[0].body, b"grant_type=client_credentials");
        assert_eq!(script.requests[1].method, Method::POST);
        assert_eq!(script.requests[1].path, "/transfers");
        assert_eq!(
            script.requests[1].headers["idempotency-key"],
            op.id.to_string()
        );
        assert_eq!(
            script.requests[1].headers["authorization"],
            "Bearer test-oauth-token"
        );
        let body: Value = serde_json::from_slice(&script.requests[1].body).unwrap();
        assert_eq!(body["amount"], json!({"currency":"USD","value":"12.50"}));
        assert_eq!(body["correlationId"], op.id.to_string());
    }

    #[tokio::test]
    async fn processed_summary_waits_for_its_bank_leg_and_uses_funding_source_relations() {
        let mock = mock().await;
        let operation = op();
        let root = Uuid::new_v4();
        let bank_leg = Uuid::new_v4();
        let origin = mock.client.origin.as_str().trim_end_matches('/');
        let parent = json!({"id":root,"status":"processed","amount":{"currency":"usd","value":"12.50"},"correlationId":operation.id,"_links":{"source":{"href":format!("{origin}/customers/{}",Uuid::new_v4())},"destination":{"href":format!("{origin}/customers/{}",Uuid::new_v4())},"source-funding-source":{"href":format!("{origin}/funding-sources/{}",operation.source)},"destination-funding-source":{"href":format!("{origin}/funding-sources/{}",operation.destination)},"funded-transfer":{"href":format!("{origin}/transfers/{bank_leg}")}}});
        let child = json!({"id":bank_leg,"status":"pending","amount":{"currency":"USD","value":"12.50"},"correlationId":operation.id,"_links":{"funding-transfer":{"href":format!("{origin}/transfers/{root}")}}});
        mock.script
            .lock()
            .await
            .replies
            .extend([token(), ok(parent), ok(child)]);
        let observation = mock.client.observe(root).await.unwrap();
        assert!(observation.matches(&operation));
        assert_eq!(observation.status, TransferStatus::Pending);
        assert_eq!(observation.related_transfers, vec![bank_leg]);
        assert_eq!(mock.script.lock().await.requests.len(), 3);
    }

    #[tokio::test]
    async fn a_related_transfer_with_a_different_correlation_is_not_accepted() {
        let mock = mock().await;
        let operation = op();
        let root = Uuid::new_v4();
        let leg = Uuid::new_v4();
        let origin = mock.client.origin.as_str().trim_end_matches('/');
        let parent = json!({"id":root,"status":"processed","amount":{"currency":"USD","value":"12.50"},"correlationId":operation.id,"_links":{"source":{"href":format!("{origin}/funding-sources/{}",operation.source)},"destination":{"href":format!("{origin}/funding-sources/{}",operation.destination)},"funded-transfer":{"href":format!("{origin}/transfers/{leg}")}}});
        let child = json!({"id":leg,"status":"processed","amount":{"currency":"USD","value":"12.50"},"correlationId":Uuid::new_v4(),"_links":{}});
        mock.script
            .lock()
            .await
            .replies
            .extend([token(), ok(parent), ok(child)]);
        assert!(matches!(
            mock.client.observe(root).await,
            Err(Error::Protocol)
        ));
    }
    #[test]
    fn location_headers_never_allow_cross_origin_credentials_or_unexpected_resource_types() {
        let client = Dwolla::sandbox("key".into(), "secret".into()).unwrap();
        let id = Uuid::new_v4();
        assert_eq!(
            client
                .resource_id(&format!("{DWOLLA_SANDBOX}/transfers/{id}"), "transfers")
                .unwrap(),
            id
        );
        for value in [
            format!("https://api.dwolla.com/transfers/{id}"),
            format!("http://api-sandbox.dwolla.com/transfers/{id}"),
            format!("https://api-sandbox.dwolla.com.evil.invalid/transfers/{id}"),
            format!("{DWOLLA_SANDBOX}/customers/{id}"),
            format!("{DWOLLA_SANDBOX}/transfers/{id}?x=1"),
            format!("{DWOLLA_SANDBOX}/transfers/{id}/extra"),
        ] {
            assert!(client.resource_id(&value, "transfers").is_err());
        }
    }
    #[tokio::test]
    async fn redirect_responses_do_not_leak_bearer_credentials_or_finish_transfers() {
        let mock = mock().await;
        mock.script.lock().await.replies.extend([
            token(),
            Reply {
                status: StatusCode::TEMPORARY_REDIRECT,
                body: json!({}),
                location: Some("https://example.invalid/capture".into()),
            },
        ]);
        assert!(matches!(
            mock.client.submit(&op()).await,
            Err(Error::Protocol)
        ));
        assert_eq!(mock.script.lock().await.requests.len(), 2);
    }
    #[tokio::test]
    async fn conflict_rate_limit_and_server_error_remain_retryable() {
        let mock = mock().await;
        mock.script.lock().await.replies.push_back(token());
        for status in [
            StatusCode::CONFLICT,
            StatusCode::TOO_MANY_REQUESTS,
            StatusCode::INTERNAL_SERVER_ERROR,
        ] {
            mock.script.lock().await.replies.push_back(Reply {
                status,
                body: json!({"message":"private provider detail"}),
                location: None,
            });
            assert!(matches!(
                mock.client.submit(&op()).await,
                Err(Error::Retryable)
            ));
        }
    }
    #[tokio::test]
    async fn concurrent_calls_share_one_token_refresh() {
        let mock = mock().await;
        let source = Uuid::new_v4();
        {
            let mut script = mock.script.lock().await;
            script.replies.push_back(token());
            for _ in 0..8 {
                script
                    .replies
                    .push_back(ok(json!({"balance":{"currency":"USD","value":"20.00"}})));
            }
        }
        let mut tasks = vec![];
        for _ in 0..8 {
            let client = mock.client.clone();
            tasks.push(tokio::spawn(async move {
                client.available_balance(source).await
            }));
        }
        for task in tasks {
            assert_eq!(task.await.unwrap().unwrap(), 2000);
        }
        assert_eq!(
            mock.script
                .lock()
                .await
                .requests
                .iter()
                .filter(|r| r.path == "/token")
                .count(),
            1
        );
    }
    #[tokio::test]
    async fn token_expiry_invalidates_cache_without_automatically_repeating_the_post() {
        let mock = mock().await;
        let operation = op();
        let transfer = Uuid::new_v4();
        let location = format!("{}transfers/{transfer}", mock.client.origin);
        mock.script.lock().await.replies.extend([
            token(),
            Reply {
                status: StatusCode::UNAUTHORIZED,
                body: json!({}),
                location: None,
            },
            token(),
            Reply {
                status: StatusCode::CREATED,
                body: json!({}),
                location: Some(location),
            },
        ]);
        assert!(matches!(
            mock.client.submit(&operation).await,
            Err(Error::Authentication)
        ));
        assert_eq!(mock.client.submit(&operation).await.unwrap(), transfer);
        assert_eq!(mock.script.lock().await.requests.len(), 4);
    }
    #[tokio::test]
    async fn plaid_secure_exchange_has_separate_persistable_keys_for_each_creation_step() {
        let mock = mock().await;
        let customer = Uuid::new_v4();
        let partner = Uuid::new_v4();
        let exchange = Uuid::new_v4();
        let bank = Uuid::new_v4();
        let exchange_key = Uuid::new_v4();
        let bank_key = Uuid::new_v4();
        let origin = mock.client.origin.as_str().trim_end_matches('/');
        mock.script.lock().await.replies.extend([token(),ok(json!({"_embedded":{"exchange-partners":[{"name":"Plaid","_links":{"self":{"href":format!("{origin}/exchange-partners/{partner}")}}}]}})),Reply {status:StatusCode::CREATED,body:json!({}),location:Some(format!("{origin}/exchanges/{exchange}"))},Reply {status:StatusCode::CREATED,body:json!({}),location:Some(format!("{origin}/funding-sources/{bank}"))}]);
        let observed_partner = mock.client.plaid_partner().await.unwrap();
        assert_eq!(observed_partner, partner);
        assert_eq!(
            mock.client
                .create_exchange(customer, partner, "processor-sandbox-test", exchange_key)
                .await
                .unwrap(),
            exchange
        );
        assert_eq!(
            mock.client
                .create_bank(customer, exchange, "Checking", "checking", bank_key)
                .await
                .unwrap(),
            bank
        );
        let script = mock.script.lock().await;
        assert_eq!(
            script.requests[2].headers["idempotency-key"],
            exchange_key.to_string()
        );
        assert_eq!(
            script.requests[3].headers["idempotency-key"],
            bank_key.to_string()
        );
        let body: Value = serde_json::from_slice(&script.requests[3].body).unwrap();
        assert_eq!(body["bankAccountType"], "checking");
    }
    #[tokio::test]
    async fn bank_binding_requires_verified_membership_and_balance_can_be_negative() {
        let mock = mock().await;
        let customer = Uuid::new_v4();
        let bank = Uuid::new_v4();
        let origin = mock.client.origin.as_str().trim_end_matches('/');
        let item = json!({"type":"bank","status":"verified","removed":false,"_links":{"self":{"href":format!("{origin}/funding-sources/{bank}")}}});
        mock.script.lock().await.replies.extend([
            token(),
            ok(json!({"_embedded":{"funding-sources":[item.clone()]}})),
            ok(json!({"_embedded":{"funding-sources":[]}})),
            ok(json!({"balance":{"currency":"USD","value":"-12.50"}})),
        ]);
        assert!(mock.client.verified_bank(customer, bank).await.unwrap());
        assert!(!mock
            .client
            .verified_bank(Uuid::new_v4(), bank)
            .await
            .unwrap());
        assert_eq!(mock.client.available_balance(bank).await.unwrap(), -1250);
        assert_eq!(
            mock.script.lock().await.requests[1].path,
            format!("/customers/{customer}/funding-sources?removed=false")
        );
    }
    #[test]
    fn pii_and_tokens_do_not_appear_in_provider_errors() {
        for error in [
            Error::Retryable,
            Error::Rejected,
            Error::Authentication,
            Error::Protocol,
        ] {
            assert!(!error.to_string().contains("test-client-secret"));
        }
        assert!(Dwolla::sandbox("".into(), "secret".into()).is_err());
    }
}
