use crate::{
    config::Config,
    domain::{Delivery, EventType, post_text, reply_key},
    http::{Http, Response},
    media::{self, PostImage},
};
use anyhow::{Context, Result, ensure};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct PostRef {
    pub uri: String,
    pub cid: String,
}
pub struct Publisher<'a> {
    config: &'a Config,
    http: Http,
    session: Option<Session>,
}
struct Session {
    token: String,
    did: String,
    service: String,
}
impl<'a> Publisher<'a> {
    pub fn new(config: &'a Config) -> Self {
        Self {
            config,
            http: Http::new(30_000),
            session: None,
        }
    }
    fn authenticate(&mut self) -> Result<()> {
        if self.session.is_some() {
            return Ok(());
        }
        let response=self.http.post_json(&format!("{}/xrpc/com.atproto.server.createSession",self.config.bluesky_url.trim_end_matches('/')),None,&json!({"identifier":self.config.identifier.as_deref().context("Missing Bluesky identifier")?,"password":self.config.password.as_deref().context("Missing Bluesky password")?}))?;
        let data = success_json(response, "Bluesky login")?;
        let token = data["accessJwt"]
            .as_str()
            .context("Login response missing access token")?
            .to_owned();
        let did = data["did"]
            .as_str()
            .context("Login response missing DID")?
            .to_owned();
        let endpoint = data["didDoc"]["service"]
            .as_array()
            .and_then(|services| {
                services.iter().find(|s| {
                    s["id"]
                        .as_str()
                        .is_some_and(|id| id.ends_with("#atproto_pds"))
                })
            })
            .and_then(|s| s["serviceEndpoint"].as_str())
            .unwrap_or(&self.config.bluesky_url);
        let url = url::Url::parse(endpoint).context("Invalid PDS URL")?;
        ensure!(
            url.scheme() == "https"
                || (url.scheme() == "http"
                    && matches!(url.host_str(), Some("localhost" | "127.0.0.1"))),
            "Insecure PDS endpoint rejected"
        );
        self.session = Some(Session {
            token,
            did,
            service: endpoint.trim_end_matches('/').to_owned(),
        });
        Ok(())
    }
    fn session(&self) -> Result<&Session> {
        self.session
            .as_ref()
            .context("Publisher is not authenticated")
    }
    fn existing(&self, key: &str) -> Result<Option<PostRef>> {
        let session = self.session()?;
        let mut url = url::Url::parse(&format!(
            "{}/xrpc/com.atproto.repo.getRecord",
            session.service
        ))?;
        url.query_pairs_mut()
            .append_pair("repo", &session.did)
            .append_pair("collection", "app.bsky.feed.post")
            .append_pair("rkey", key);
        let response = self
            .http
            .get(url.as_str(), Some(&session.token), 2_000_000)?;
        if response.status == 200 {
            return Ok(Some(serde_json::from_slice(&response.bytes)?));
        }
        let error: Value = serde_json::from_slice(&response.bytes).unwrap_or(Value::Null);
        if matches!(response.status, 400 | 404) && error["error"] == "RecordNotFound" {
            return Ok(None);
        }
        anyhow::bail!("Bluesky record lookup HTTP {}", response.status)
    }
    pub fn publish(&mut self, delivery: &Delivery) -> Result<PostRef> {
        let config = self.config;
        self.publish_with_images(
            delivery,
            || {
                media::render(
                    config,
                    &delivery.payload.station,
                    delivery.style,
                    delivery.payload.kind == EventType::Electrified,
                )
            },
            || media::streetview(config, &delivery.payload.station),
        )
    }
    fn publish_with_images(
        &mut self,
        delivery: &Delivery,
        render: impl FnOnce() -> Result<PostImage>,
        streetview: impl FnOnce() -> Result<PostImage>,
    ) -> Result<PostRef> {
        self.authenticate()?;
        let station = &delivery.payload.station;
        let root = match self.existing(&delivery.record_key)? {
            Some(root) => root,
            None => {
                let image = render()?;
                self.create(
                    &delivery.record_key,
                    &post_text(&delivery.payload),
                    &image,
                    None,
                    &crate::domain::now(),
                )?
            }
        };
        if self.config.streetview {
            let key = reply_key(&delivery.record_key)?;
            if self.existing(&key)?.is_some() {
                return Ok(root);
            }
            let image = match streetview() {
                Ok(image) => image,
                Err(error) => {
                    eprintln!(
                        "Street View unavailable; announcement delivered: {}",
                        self.config.redact(&error.to_string())
                    );
                    return Ok(root);
                }
            };
            self.create(
                &key,
                &format!("📸 Street view of {}", station.display_name()),
                &image,
                Some(&root),
                &crate::domain::now(),
            )?;
        }
        Ok(root)
    }
    fn create(
        &self,
        key: &str,
        text: &str,
        image: &PostImage,
        reply: Option<&PostRef>,
        created_at: &str,
    ) -> Result<PostRef> {
        ensure!(
            image.bytes.len() <= media::MAX_IMAGE_BYTES,
            "Image too large"
        );
        let session = self.session()?;
        let uploaded = success_json(
            self.http.post(
                &format!("{}/xrpc/com.atproto.repo.uploadBlob", session.service),
                Some(&session.token),
                &image.bytes,
                "image/jpeg",
            )?,
            "Bluesky upload",
        )?;
        ensure!(uploaded["blob"].is_object(), "Invalid blob response");
        let mut record = json!({"$type":"app.bsky.feed.post","text":text,"langs":["en"],"createdAt":created_at,"embed":{"$type":"app.bsky.embed.images","images":[{"alt":image.alt,"image":uploaded["blob"],"aspectRatio":{"width":image.width,"height":image.height}}]}});
        if let Some(root) = reply {
            record["reply"] = json!({"root":root,"parent":root});
        }
        let result = (|| -> Result<PostRef> {
            let response=self.http.post_json(&format!("{}/xrpc/com.atproto.repo.createRecord",session.service),Some(&session.token),&json!({"repo":session.did,"collection":"app.bsky.feed.post","rkey":key,"record":record}))?;
            Ok(serde_json::from_value(success_json(
                response,
                "Bluesky create",
            )?)?)
        })();
        match result {
            Ok(post) => Ok(post),
            Err(error) => match self.existing(key) {
                Ok(Some(post)) => Ok(post),
                _ => Err(error),
            },
        }
    }
}
fn success_json(response: Response, operation: &str) -> Result<Value> {
    ensure!(
        (200..300).contains(&response.status),
        "{operation} HTTP {}",
        response.status
    );
    serde_json::from_slice(&response.bytes).with_context(|| format!("Invalid {operation} response"))
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;
    use crate::{
        domain::{Payload, Style, encode_tid},
        tests::{mock_server, station},
    };
    use std::collections::HashMap;
    fn config(url: String, street: bool) -> Config {
        Config::from_map(&HashMap::from([
            ("BLUESKY_SERVICE_URL".into(), url),
            ("BLUESKY_IDENTIFIER".into(), "test".into()),
            ("BLUESKY_APP_PASSWORD".into(), "test-password".into()),
            ("STREETVIEW_ENABLED".into(), street.to_string()),
            ("GOOGLE_MAPS_API_KEY".into(), "test-only".into()),
        ]))
        .unwrap()
    }
    fn login() -> String {
        json!({"did":"did:plc:test","accessJwt":"test-token"}).to_string()
    }
    #[test]
    fn retry_of_existing_root_and_reply_does_no_rendering_or_upload() {
        let root = json!({"uri":"at://root","cid":"root-cid"}).to_string();
        let reply = json!({"uri":"at://reply","cid":"reply-cid"}).to_string();
        let (url, server) = mock_server(vec![(200, login()), (200, root), (200, reply)]);
        let config = config(url, true);
        let mut publisher = Publisher::new(&config);
        let delivery = Delivery {
            id: "delivery".into(),
            record_key: encode_tid(1700000000000000 << 10),
            style: Style::Nightline,
            payload: Payload {
                kind: EventType::Discovered,
                station: station("1"),
                observed_at: "2026-01-01T00:00:00.000Z".into(),
            },
        };
        assert_eq!(publisher.publish(&delivery).unwrap().cid, "root-cid");
        let requests = server.join().unwrap();
        assert_eq!(requests.len(), 3);
        assert!(requests[1].starts_with("GET /xrpc/com.atproto.repo.getRecord"));
    }
    #[test]
    fn uncertain_create_is_reconciled_with_same_record_key() {
        let(url,server)=mock_server(vec![(200,login()),(200,json!({"blob":{"$type":"blob","ref":{"$link":"cid"},"mimeType":"image/jpeg","size":3}}).to_string()),(500,"{}".into()),(200,json!({"uri":"at://created","cid":"cid-created"}).to_string())]);
        let config = config(url, false);
        let mut publisher = Publisher::new(&config);
        publisher.authenticate().unwrap();
        let image = PostImage {
            bytes: vec![1, 2, 3],
            alt: "test".into(),
            width: 1,
            height: 1,
        };
        let result = publisher
            .create("test-key", "Text", &image, None, "2026-01-01T00:00:00.000Z")
            .unwrap();
        assert_eq!(result.cid, "cid-created");
        let requests = server.join().unwrap();
        assert!(requests[2].contains("\"rkey\":\"test-key\""));
        assert!(requests[3].contains("rkey=test-key"));
    }
    #[test]
    fn auth_or_rate_limit_lookup_error_does_not_trigger_post() {
        let (url, server) = mock_server(vec![(200, login()), (429, "{}".into())]);
        let config = config(url, false);
        let mut publisher = Publisher::new(&config);
        publisher.authenticate().unwrap();
        assert!(publisher.existing("key").is_err());
        assert_eq!(server.join().unwrap().len(), 2);
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod delivery_tests {
    use super::*;
    use crate::{
        domain::{Payload, Style, encode_tid},
        tests::{mock_server, station},
    };
    use std::collections::HashMap;
    fn image() -> Result<PostImage> {
        Ok(PostImage {
            bytes: vec![1, 2, 3],
            alt: "Station map alt".into(),
            width: 1080,
            height: 1350,
        })
    }
    fn config(url: String) -> Config {
        Config::from_map(&HashMap::from([
            ("BLUESKY_SERVICE_URL".into(), url),
            ("BLUESKY_IDENTIFIER".into(), "test".into()),
            ("BLUESKY_APP_PASSWORD".into(), "password".into()),
            ("STREETVIEW_ENABLED".into(), "true".into()),
            ("GOOGLE_MAPS_API_KEY".into(), "test-only".into()),
        ]))
        .unwrap()
    }
    fn delivery() -> Delivery {
        Delivery {
            id: "event".into(),
            record_key: encode_tid(1700000000000000 << 10),
            style: Style::Civic,
            payload: Payload {
                kind: EventType::Discovered,
                station: station("1"),
                observed_at: "2020-01-01T00:00:00.000Z".into(),
            },
        }
    }
    fn login() -> (u16, String) {
        (
            200,
            json!({"did":"did:plc:test","accessJwt":"token"}).to_string(),
        )
    }
    fn missing() -> (u16, String) {
        (400, json!({"error":"RecordNotFound"}).to_string())
    }
    fn blob() -> (u16, String) {
        (200,json!({"blob":{"$type":"blob","ref":{"$link":"image-cid"},"mimeType":"image/jpeg","size":3}}).to_string())
    }
    fn root() -> (u16, String) {
        (200, json!({"uri":"at://root","cid":"root-cid"}).to_string())
    }
    fn reply() -> (u16, String) {
        (
            200,
            json!({"uri":"at://reply","cid":"reply-cid"}).to_string(),
        )
    }
    fn body(request: &str) -> Value {
        serde_json::from_str(request.split_once("\r\n\r\n").unwrap().1).unwrap()
    }
    #[test]
    fn creates_new_root_and_threaded_reply_with_publication_time() {
        let (url, server) = mock_server(vec![
            login(),
            missing(),
            blob(),
            root(),
            missing(),
            blob(),
            reply(),
        ]);
        let config = config(url);
        let mut publisher = Publisher::new(&config);
        let d = delivery();
        let before = crate::domain::now();
        publisher.publish_with_images(&d, image, image).unwrap();
        let requests = server.join().unwrap();
        let root = body(&requests[3]);
        let reply = body(&requests[6]);
        assert_eq!(root["rkey"], d.record_key);
        assert_eq!(root["record"]["text"], post_text(&d.payload));
        assert_eq!(
            root["record"]["embed"]["images"][0]["aspectRatio"],
            json!({"width":1080,"height":1350})
        );
        assert_eq!(
            root["record"]["embed"]["images"][0]["alt"],
            "Station map alt"
        );
        assert!(root["record"]["createdAt"].as_str().unwrap() >= before.as_str());
        assert_eq!(reply["rkey"], reply_key(&d.record_key).unwrap());
        assert_eq!(
            reply["record"]["reply"],
            json!({"root":{"uri":"at://root","cid":"root-cid"},"parent":{"uri":"at://root","cid":"root-cid"}})
        );
    }
    #[test]
    fn failed_reply_retries_without_recreating_root() {
        let (url, server) = mock_server(vec![
            login(),
            missing(),
            blob(),
            root(),
            missing(),
            blob(),
            (500, "{}".into()),
            missing(),
            root(),
            missing(),
            blob(),
            reply(),
        ]);
        let config = config(url);
        let mut publisher = Publisher::new(&config);
        let d = delivery();
        assert!(publisher.publish_with_images(&d, image, image).is_err());
        publisher
            .publish_with_images(&d, || panic!("Existing root must not render again"), image)
            .unwrap();
        let requests = server.join().unwrap();
        assert_eq!(
            requests
                .iter()
                .filter(
                    |r| r.starts_with("POST /xrpc/com.atproto.repo.createRecord")
                        && body(r)["rkey"] == d.record_key
                )
                .count(),
            1
        );
    }
    #[test]
    fn unavailable_streetview_leaves_successful_root_delivered() {
        let (url, server) = mock_server(vec![login(), root(), missing()]);
        let config = config(url);
        let mut publisher = Publisher::new(&config);
        let d = delivery();
        assert_eq!(
            publisher
                .publish_with_images(
                    &d,
                    || panic!("Already posted"),
                    || anyhow::bail!("Street View HTTP 404")
                )
                .unwrap()
                .cid,
            "root-cid"
        );
        assert_eq!(server.join().unwrap().len(), 3);
    }
}
